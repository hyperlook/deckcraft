# Pi model bridge — M1 shared workspace (#20 + #12)

Optional native adapter, **not** a complete agent or standalone Rust distribution.
Requires the already-installed Bun and globally installed Pi SDK **1.1.0**. Other SDK versions fail closed until the transport is reviewed again. Nothing installs dependencies automatically.

SDK discovery is fixed to `$BUN_INSTALL/install/global/node_modules/@earendil-works/pi-coding-agent` (default `~/.bun/install/global/node_modules/...`). `BUN_INSTALL` is trusted launch configuration, not an IPC/document field. SDK authentication uses its normal agent directory, including `PI_CODING_AGENT_DIR`; Rust never opens credential files. No project resource discovery, `models.json`, session history, tools, MCP or extensions are loaded.

Only the reviewed built-in **openai-codex** provider is exposed in this pilot. Use Pi `/login openai-codex` when authentication is unavailable. Custom/extension providers and compatible endpoints are deliberately unavailable, not silently loaded or substituted.

## Desktop path

```sh
cargo run -p deckcraft -- --sample
```

Create/open a project → top-right **AI**. Models load automatically without uploading document text. Choose the model and enable **允许此文稿使用所选模型** once for this document/model in the running session. Switching models revokes consent; native reopen reuses valid context without calling a model.

Empty starter: type a topic → answer purpose/audience/size choices (or other text) → the coordinator requests a strict plan → planned slots appear in the **existing** thumbnail/sorter lists of the same project. Imported slides: originals remain visible → consent enables background source extraction and whole-deck understanding → original thumbnails acquire inferred intents. Extraction overflow blocks the entire request; image/chart/table/notes/OCR understanding is not claimed. No source-snapshot refresh, staging or outline-confirmation console is required.

Right-click a page to adjust its intent; planned slots also show the intent editor at their canvas position. Existing move/delete/undo commands preserve slot identity. Save `.deckcraft` to persist requirements, evidence, mainline and explicit planned/existing state. Unexpanded planned slots cannot be exported/rendered as blank finished slides or presented. PPTX export of existing slides with AI metadata does not mark the project saved. Cancellation, failure, target edits and document closure cannot install late results on another tab. Web remains editable without the local bridge. Product specification: [AI Assistant RFC](../../../docs/rfcs/ai-assistant.md).

## Protocol

A fresh fixed bridge process per catalog/request; bounded JSONL over stdin/stdout. No shell and no text/credentials on the command line. Stderr is discarded by the host. One request followed only by an optional cancellation:

```json
{"id":1,"op":"models"}
{"id":2,"op":"generate","provider":"openai-codex","model":"gpt-6.1-sol","input":"Non-sensitive selected material"}
{"id":3,"op":"summarize","provider":"openai-codex","model":"gpt-6.1-sol","input":"Serialized ordered page sources and coverage"}
{"id":4,"op":"plan","provider":"openai-codex","model":"gpt-6.1-sol","input":"Serialized clarified requirements"}
{"id":4,"op":"cancel"}
```

Exactly one terminal stdout frame: `models`, legacy smoke `result` (`{title,points}`), `summary` (`{mainline,pages:[{pageId,role,intent,unknown}]}`), `plan` (`{mainline,pages:[{title,intent}]}`), or `error` (allowlisted code only). Rust checks request identity/kind/bounds, summary source IDs/order and plan count against clarified requirements. Plan IDs/state are engine-assigned; model content cannot choose another document, command, path, version or evidence. Neither start requires a confirmed legacy outline.

Shared coordinator: `crates/ai/src/workspace.rs`. Host/UI actions: `ai.chat`, `ai.reply`, `ai.authorize`, `ai.retry`, `ai.cancelRequest`; `ai.loadModels` is a connection retry, not a normal prerequisite. Engine writes: `ai.installWorkspace`, `ai.adjustPageIntent` (legacy `ai.setSummary` remains available for compatibility). These are not #18's future headless task/authorization API.

## Limits and reviewed transport

- Serialized IPC input and actual model payload: **512 KiB**. No silent truncation.
- Aggregate decoded upstream HTTP bytes: **1 MiB**, including SSE envelopes/reasoning/error bodies **and OAuth refresh responses**, checked before SDK parsing/buffering. Normalized deltas and stdout have independent 1 MiB limits.
- Deadline **120 seconds**, event queue **64**. All pipe writes/reads and process supervision are off-thread. Cancel propagates SDK AbortSignal, then the native supervisor kills an unresponsive bridge after 250 ms and reaps it. App exit drops/signals the host, not the UI thread's network connection.
- Only `https://chatgpt.com/backend-api/codex/responses` for model requests and `https://auth.openai.com/oauth/token` for SDK refresh. Redirects are errors. One model HTTP request, **zero retries**, forced **SSE**, no WebSocket fallback.
- SDK `openai-codex-responses.js` 1.1.0 was inspected: custom `fetch` and `maxRetries` are forwarded through `streamSimple`; SSE decoding and non-2xx body parsing happen after this wrapper. OAuth refresh uses global `fetch`, also wrapped. Credential refresh/write/locking remain SDK-owned (`auth-storage.js` with `proper-lockfile`). No token is copied/migrated.
- Codex's adapter does **not** put `maxTokens` into its request body. The passed 4096 (connection) / 16384 (whole-deck) option is not claimed as a server-enforced limit; upstream byte cap, abort and host deadline enforce the client bound. Cancellation cannot promise a provider refunds work already performed.
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

Sends only the built-in fictional garden fixture. Add `--whole-deck` to send a four-page fictional problem/solution/action/image-only fixture and check the strict summary schema, ordered coverage and unknown image page. Both modes print SDK version, provider/model and validation status; neither result body nor credentials are logged. These are opt-in quota-consuming checks, **not** human acceptance of synthesis quality. Product specification: [AI Assistant RFC](../../../docs/rfcs/ai-assistant.md).
