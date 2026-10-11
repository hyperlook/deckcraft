# Pi model bridge — #15 A developer pilot

Optional native adapter, **not** a complete agent or standalone Rust distribution.
Requires the already-installed Bun and globally installed Pi SDK **1.1.0**. Other SDK versions fail closed until the transport is reviewed again. Nothing installs dependencies automatically.

SDK discovery is fixed to `$BUN_INSTALL/install/global/node_modules/@earendil-works/pi-coding-agent` (default `~/.bun/install/global/node_modules/...`). `BUN_INSTALL` is trusted launch configuration, not an IPC/document field. SDK authentication uses its normal agent directory, including `PI_CODING_AGENT_DIR`; Rust never opens credential files. No project resource discovery, `models.json`, session history, tools, MCP or extensions are loaded.

Only the reviewed built-in **openai-codex** provider is exposed in this pilot. Use Pi `/login openai-codex` when authentication is unavailable. Custom/extension providers and compatible endpoints are deliberately unavailable, not silently loaded or substituted.

## Desktop path

```sh
cargo run -p deckcraft -- --sample
```

Open top-right **AI** → load Pi models → explicitly select a model → review endpoint and material → check consent → send. Default material is fictional/public. No current slide text is sent. The result is an in-memory title/points candidate, not an outline and not a deck mutation. Cancellation, failed calls and candidate review do not write the deck or history. Closing the originating document cancels/discards its candidate; switching tabs does not rebind the snapshot. Web supplies no local adapter and remains editable.

The old extraction checkpoint is collapsed and labelled historical/unaccepted. It is not a prerequisite for the new model entry.

## Protocol

A fresh fixed bridge process per catalog/request; bounded JSONL over stdin/stdout. No shell and no text/credentials on the command line. Stderr is discarded by the host. One request followed only by an optional cancellation:

```json
{"id":1,"op":"models"}
{"id":2,"op":"generate","provider":"openai-codex","model":"gpt-6.1-sol","input":"Non-sensitive selected material"}
{"id":2,"op":"cancel"}
```

Exactly one terminal stdout frame: `models` (provider/model/endpoint + `sdkVersion`), `result` (`{title,points}`), or `error` (allowlisted code only). Rust revalidates identity, fields, collection/string limits and request kind. The model never chooses an ID, command, path or document. `generate` needs no confirmed Outline / `Round::from_confirmed`, so #20 can reuse `deckcraft_ai::request::{Request,Event,Lifecycle,Host}`.

UI actions are `ai.loadModels`, `ai.sendMaterial`, `ai.cancelRequest`; the send action consumes the UI consent and selected material, not remote parameters. This is not #18's headless AI service API.

## Limits and reviewed transport

- Serialized IPC input and actual model payload: **512 KiB**. No silent truncation.
- Aggregate decoded upstream HTTP bytes: **1 MiB**, including SSE envelopes/reasoning/error bodies **and OAuth refresh responses**, checked before SDK parsing/buffering. Normalized deltas and stdout have independent 1 MiB limits.
- Deadline **120 seconds**, event queue **64**. All pipe writes/reads and process supervision are off-thread. Cancel propagates SDK AbortSignal, then the native supervisor kills an unresponsive bridge after 250 ms and reaps it. App exit drops/signals the host, not the UI thread's network connection.
- Only `https://chatgpt.com/backend-api/codex/responses` for model requests and `https://auth.openai.com/oauth/token` for SDK refresh. Redirects are errors. One model HTTP request, **zero retries**, forced **SSE**, no WebSocket fallback.
- SDK `openai-codex-responses.js` 1.1.0 was inspected: custom `fetch` and `maxRetries` are forwarded through `streamSimple`; SSE decoding and non-2xx body parsing happen after this wrapper. OAuth refresh uses global `fetch`, also wrapped. Credential refresh/write/locking remain SDK-owned (`auth-storage.js` with `proper-lockfile`). No token is copied/migrated.
- Codex's adapter does **not** put `maxTokens` into its request body. The passed 4096 option is not claimed as a server-enforced limit; upstream byte cap, abort and host deadline enforce the client bound. Cancellation cannot promise a provider refunds work already performed.
- Trusted local credential files/credential commands and SDK module initialization remain trusted local code, not an OS sandbox or a total process-memory quota. This is why providers/transports/SDK versions are deliberately narrow.

## Offline verification

```sh
bun test scripts/ai/pi-bridge
cargo test -p deckcraft-ai --lib
cargo test -p deckcraft-ui-egui --lib
cargo test -p deckcraft ai_host --bin deckcraft -- --ignored
```

The last command explicitly opts into a **Bun-only fake process**, without Pi import/auth/network. It covers blocked stdin, cancel, timeout, malformed/oversized stdout, wrong identity and exit. It is ignored in the default Rust suite so optional Bun is not a build/test prerequisite for ordinary Rust-only CI. The default suite still checks IPC parsing and pure lifecycle/UI behavior.

## Explicit live smoke (uses provider quota)

Never run automatically in CI:

```sh
bun --no-env-file --no-install scripts/ai/pi-bridge/smoke.ts --live gpt-6.1-sol
```

Sends only the built-in fictional garden fixture. Prints SDK version, provider/model and validation status; neither result body nor credentials are logged. This verifies connectivity/schema, **not** full-deck understanding, rewriting quality or human acceptance. Evidence: [AI A review](../../../docs/ai-a-review.md).
