# AGENTS.md — Core Engineering & Architecture Guide

DeckCraft is a pure-Rust presentation application targeting Microsoft PowerPoint parity — and superiority (speed, native performance, agent control). It runs natively on macOS, Windows, Linux and FreeBSD, and on the web via WASM.

---

## 1. Robustness & Stability (Never crash)

People trust DeckCraft with their presentations; a crash loses work. Stability outranks quick hacks.
- **No unhandled panics**: No `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`, `unimplemented!` in production/non-test code.
- **No unsafe**: Keep `unsafe_code = "forbid"`. Production crates carry:
  `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`.
- **Defensive error handling**: Use `Result<T, E>` + `?`. Treat input-derived values (files, commands, MCP/control params) defensively: `get()` instead of direct indexing `[i]`, checked arithmetic, no NaN casts, capped allocations, and bounded recursion.
- **Panic guard**: `Session::execute` runs commands inside a panic guard (`crates/engine/src/guard.rs`) that preserves presentation state. It is a safety net, not a license to panic.
- Accompany crash fixes with regression tests.

---

## 2. Architecture

| Layer | Crates | Description |
|---|---|---|
| L0 | `geom`, `color` | Units, transforms, preset shapes, color models |
| L1 | `model` | Presentation, masters, layouts, slides, shapes, text, themes, inheritance, text edit ops |
| L2 | `fonts`, `text` | Font handling, text layout |
| L3 | `render` | Rendering pipeline (`vello_cpu`) |
| L4 | `format`, `pptx` | `.deckcraft` file format and PPTX import/export |
| L5 | `engine` | Session, history (undo/redo), commands, tools, key bindings, sample data |
| L6 | `mcp` | Model Context Protocol interface |
| L7 | `ui-egui` | Swappable GUI (egui/eframe/winit; layers below do NOT depend on UI) |
| apps | `deckcraft`, `deckcraft-cli`, `deckcraft-web` | Desktop, CLI, and Web entrypoints |

### Architectural Rules
- **Everything is a command** (`crates/engine/src/cmd/*`):
  Each command has: id, label, ribbon/menu place, shortcut, params doc, `enabled`, `run`, and tests.
  Command IDs align with standard presentation semantics (`slide.new`, `format.bold`, `arrange.group`, `transition.set`…).
  Programmatic calls never trigger modal dialogs.
  Pointer gestures go through `Session::pointer`, keys through `Session::key`, ensuring the UI, CLI, control channel, and MCP share identical behaviors.
- **Thin UI**:
  UI panels only read engine state and dispatch actions through commands.
  Colors come from theme tokens. Built with `egui` (never Tauri).
- **Preserve WASM compatibility**:
  Core crates (L0–L5) must compile to WASM (`cargo xtask wasm`).

---

## 3. Running and Inspecting the App

- **Run Desktop App with Sample & Control Channel**:
  `cargo run --release -p deckcraft -- --sample --control 7990`
- **Drive it via Control Channel**:
  Send JSON lines over TCP to `127.0.0.1:7990`, e.g.:
  `{"id":1,"method":"engine.execute","params":{"command":"shape.insert","params":{"preset":"star5","rect":[100,100,200,200]}}}`
  `{"id":2,"method":"ui.screenshot","params":{"path":"/tmp/shot.png"}}`
  Methods reference: `crates/ui-egui/src/control.rs`, docs: `docs/control-protocol.md`.
- **Headless / CLI**:
  - Render slides to images:
    `cargo run -p deckcraft-cli -- render --sample --all --scale 1 out/`
  - Run headless command and export:
    `cargo run -p deckcraft-cli -- run --sample --cmd 'slide.new={"title":"Hi"}' --export out.png`
- **MCP Server**:
  - Headless: `cargo run -p deckcraft-cli -- mcp`
  - Connected to live app: `cargo run -p deckcraft-cli -- mcp --connect 7990`
  - See `docs/mcp.md`.
- **Parallel Builds**: Use separate `CARGO_TARGET_DIR` if running concurrent processes.

---

## 4. Quality & Testing

- Format & Lint: `cargo fmt` and `cargo clippy --all-targets`
- Run test suite: `cargo test`
- Workspace task runner: `cargo xtask <command>` (e.g. `layers`, `wasm`, `stats`)
