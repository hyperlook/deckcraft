//! Optional native Pi bridge. All process and pipe I/O is off the UI thread.
use deckcraft_ai::request::{DEADLINE_SECS, Event, Failure, Host, OUTPUT_LIMIT, QUEUE_LIMIT, Request};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const BRIDGE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/ai/pi-bridge");

#[derive(Default)]
pub struct PiHost {
    active: Option<Active>,
}
struct Active {
    stop: Arc<AtomicBool>,
    events: Receiver<Event>,
    worker: JoinHandle<()>,
}
impl Host for PiHost {
    fn start(&mut self, request: Request) -> Result<(), Failure> {
        if self.active.as_ref().is_some_and(|a| !a.worker.is_finished()) {
            return Err(Failure::Busy);
        }
        if let Some(active) = self.active.take() {
            let _ = active.worker.join();
        }
        let bytes = request.encode()?;
        let id = request.id();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&stop);
        let (tx, events) = mpsc::sync_channel(QUEUE_LIMIT);
        let worker = thread::Builder::new()
            .name("deckcraft-pi-bridge".into())
            .spawn(move || supervise(id, bytes, signal, tx))
            .map_err(|_| Failure::Unavailable)?;
        self.active = Some(Active { stop, events, worker });
        Ok(())
    }
    fn poll(&mut self) -> Option<Event> {
        self.active.as_ref()?.events.try_recv().ok()
    }
    fn cancel(&mut self) {
        if let Some(active) = &self.active {
            active.stop.store(true, Ordering::Release);
        }
    }
}
impl Drop for PiHost {
    fn drop(&mut self) {
        self.cancel(); // Supervisor aborts, kills if needed, closes pipes and reaps off-thread.
    }
}

fn error(id: u64, code: Failure) -> Event {
    Event::Error { id, code }
}
fn supervise(id: u64, bytes: Vec<u8>, stop: Arc<AtomicBool>, events: SyncSender<Event>) {
    let event = run_bridge(id, bytes, &stop).unwrap_or_else(|code| error(id, code));
    let _ = events.try_send(event); // Never block shutdown on a dropped/full UI queue.
}
fn read_protocol(reader: impl Read) -> Result<Event, Failure> {
    let mut bytes = Vec::new();
    reader.take((OUTPUT_LIMIT as u64) + 1).read_to_end(&mut bytes).map_err(|_| Failure::Exited)?;
    if bytes.len() > OUTPUT_LIMIT {
        return Err(Failure::Limit);
    }
    if bytes.last() != Some(&b'\n') {
        return Err(Failure::Protocol);
    }
    // Exactly one terminal JSONL frame; duplicate frames / diagnostics on stdout are invalid.
    Event::decode(&bytes)
}
fn run_bridge(id: u64, bytes: Vec<u8>, stop: &AtomicBool) -> Result<Event, Failure> {
    let mut command = Command::new("bun");
    command.args(["--no-env-file", "--no-install", "--config=bunfig.toml", "main.ts"]).current_dir(BRIDGE_DIR);
    run_child(id, bytes, stop, command, Duration::from_secs(DEADLINE_SECS))
}

fn run_child(id: u64, bytes: Vec<u8>, stop: &AtomicBool, mut command: Command, deadline: Duration) -> Result<Event, Failure> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // Never inherit SDK errors that may contain credentials/bodies.
        .spawn()
        .map_err(|_| Failure::Unavailable)?;
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Failure::Unavailable);
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Failure::Unavailable);
    };
    let (cancel_tx, cancel_rx) = mpsc::sync_channel::<()>(1);
    // Writing large input can block too. The supervisor must still enforce cancel/deadline.
    let writer = match thread::Builder::new().name("pi-input".into()).spawn(move || {
        if stdin.write_all(&bytes).is_ok() && stdin.flush().is_ok() && cancel_rx.recv().is_ok() {
            let _ = writeln!(stdin, "{{\"id\":{id},\"op\":\"cancel\"}}");
            let _ = stdin.flush();
        }
    }) {
        Ok(writer) => writer,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::Unavailable);
        }
    };
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    let reader = match thread::Builder::new().name("pi-output".into()).spawn(move || {
        let _ = output_tx.try_send(read_protocol(stdout));
    }) {
        Ok(reader) => reader,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            drop(cancel_tx);
            let _ = writer.join();
            return Err(Failure::Unavailable);
        }
    };
    let start = Instant::now();
    let mut stopping: Option<(Instant, Failure)> = None;
    let mut output = None;
    let mut exit_ok = false;
    loop {
        if stopping.is_none() {
            let failure = if stop.load(Ordering::Acquire) {
                Some(Failure::Cancelled)
            } else if start.elapsed() >= deadline {
                Some(Failure::Timeout)
            } else {
                None
            };
            if let Some(failure) = failure {
                let _ = cancel_tx.try_send(());
                stopping = Some((Instant::now(), failure));
            }
        }
        if let Ok(result) = output_rx.try_recv() {
            // Overflow is detected before process exit. Kill immediately instead of waiting 120s.
            if result.is_err() {
                let _ = child.kill();
            }
            output = Some(result);
        }
        if stopping.is_some_and(|(at, _)| at.elapsed() >= Duration::from_millis(250)) {
            let _ = child.kill();
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_ok = status.success();
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                break;
            }
        }
    }
    let _ = child.wait();
    drop(cancel_tx);
    let _ = writer.join();
    let _ = reader.join();
    if let Some((_, code)) = stopping {
        return Err(code);
    }
    if let Some(Err(code)) = &output {
        return Err(*code);
    }
    if !exit_ok {
        return Err(Failure::Exited);
    }
    let event = output.or_else(|| output_rx.try_recv().ok()).ok_or(Failure::Exited)??;
    if event.id() != id {
        return Err(Failure::Protocol);
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(mode: &str, stop: bool, deadline: Duration) -> Result<Event, Failure> {
        let mut command = Command::new("bun");
        command.args(["--no-env-file", "--no-install", "--config=bunfig.toml", "fixtures/worker.ts", mode]).current_dir(BRIDGE_DIR);
        run_child(7, vec![b'x'; 512 * 1024], &AtomicBool::new(stop), command, deadline)
    }
    #[test]
    #[ignore = "requires optional local Bun; offline subprocess integration, no SDK/auth/network"]
    fn offline_process_exit_identity_overflow_cancel_and_deadline() {
        let deadline = Duration::from_secs(10);
        assert!(matches!(fixture("ok", false, deadline), Ok(Event::Result { id: 7, .. })));
        assert!(matches!(fixture("wrong", false, deadline), Err(Failure::Protocol)));
        assert!(matches!(fixture("malformed", false, deadline), Err(Failure::Protocol)));
        assert!(fixture("exit", false, deadline).is_err());
        assert!(matches!(fixture("oversize", false, deadline), Err(Failure::Limit)));
        let start = Instant::now();
        assert!(matches!(fixture("blocked", true, deadline), Err(Failure::Cancelled)));
        assert!(matches!(fixture("blocked", false, Duration::from_millis(50)), Err(Failure::Timeout)));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn ipc_is_bounded_and_single_terminal() {
        assert!(matches!(read_protocol(&b"{\"id\":1,\"type\":\"error\",\"code\":\"auth\"}\n"[..]), Ok(Event::Error { code: Failure::Auth, .. })));
        assert!(read_protocol(&b"{\"id\":1,\"type\":\"error\",\"code\":\"auth\"}"[..]).is_err());
        assert!(read_protocol(&b"{}\n{}\n"[..]).is_err());
        assert!(matches!(read_protocol(&vec![b'x'; OUTPUT_LIMIT + 1][..]), Err(Failure::Limit)));
    }
}
