// Fixed, tool-free local SDK adapter. No AgentSession, resource loader or extensions.
import { BridgeError, DEADLINE_MS, INPUT_LIMIT, OUTPUT_LIMIT, ENDPOINT, SYSTEM, SUMMARY_SYSTEM, PLAN_SYSTEM, parsePlan, parseSummary, parseResult, boundedFetch, consumeEvents, parseRequest, safeError } from "./protocol.ts";
import { createReadStream } from "node:fs";

console.log = console.warn = console.error = console.info = console.debug = () => {};
const controller = new AbortController();
let requestId = 0;
let terminal = false;
let written = 0;
let timedOut = false;
const timer = setTimeout(() => {
  timedOut = true;
  controller.abort();
  setTimeout(() => finish({id: requestId, type: "error", code: "timeout"}), 250);
}, DEADLINE_MS);
let transportFailure: string | undefined;
const rawFetch = globalThis.fetch;
const budget = {total: 0};
const fault = (code: string) => { transportFailure = code; };
// SDK OAuth refresh also buffers response.json()/text(): bound it before SDK parsing.
// This does not implement OAuth, inspect tokens, or write credentials.
globalThis.fetch = boundedFetch(controller.signal, rawFetch, ["https://auth.openai.com/oauth/token"], 2, fault, budget);
const emit = (event: object) => {
  const line = JSON.stringify(event) + "\n";
  written += Buffer.byteLength(line);
  if (written > OUTPUT_LIMIT) throw new BridgeError("limit");
  process.stdout.write(line);
};
function finish(event: object) {
  if (terminal) return;
  terminal = true;
  emit(event);
  clearTimeout(timer);
  // Flush protocol before exiting; no credential/provider handles survive the request.
  process.stdout.write("", () => process.exit(0));
}
async function run(line: string) {
  try {
    const request = parseRequest(line);
    requestId = request.id;
    let sdk: typeof import("@earendil-works/pi-coding-agent");
    const { homedir } = await import("node:os");
    const { join } = await import("node:path");
    // A local developer installation, never a document-supplied module or Bun auto-install.
    const sdkRoot = join(process.env.BUN_INSTALL ?? join(homedir(), ".bun"), "install/global/node_modules/@earendil-works/pi-coding-agent");
    let sdkVersion: string;
    try {
      const pkg = await Bun.file(join(sdkRoot, "package.json")).json();
      sdkVersion = String(pkg.version);
      // Security review is version-bound: do not silently trust a different transport implementation.
      if (sdkVersion !== "1.1.0") throw new BridgeError("unavailable");
      sdk = await import(join(sdkRoot, "dist/index.js"));
    } catch { throw new BridgeError("unavailable"); }
    // Only built-in models: no models.json endpoint overrides or extension loading.
    // authPath defaults to SDK getAgentDir(), respecting PI_CODING_AGENT_DIR.
    const runtime = await sdk.ModelRuntime.create({modelsPath: null, refreshOnCreate: false, allowModelNetwork: false, signal: controller.signal});
    if (request.op === "models") {
      const models = (await runtime.getAvailable("openai-codex", {signal: controller.signal})).slice(0, 256).map(m => ({provider: m.provider, model: m.id, endpoint: ENDPOINT}));
      finish({id: requestId, type: "models", models, sdkVersion});
      return;
    }
    const model = runtime.getModel(request.provider, request.model);
    if (!model || model.api !== "openai-codex-responses") throw new BridgeError("unsupported");
    // Preflight refresh in the SDK, without retaining or exposing the returned credential.
    try {
      if (!(await runtime.getAvailable(request.provider, {signal: controller.signal})).some(m => m.id === request.model) ||
          !(await runtime.getAuth(model, {signal: controller.signal}))) throw new BridgeError("auth");
    } catch { throw new BridgeError(transportFailure ?? "auth"); }
    const stream = runtime.streamSimple(model, {
      systemPrompt: request.op === "summarize" ? SUMMARY_SYSTEM : request.op === "plan" ? PLAN_SYSTEM : SYSTEM,
      messages: [{role: "user", content: request.input, timestamp: Date.now()}],
    }, {
      signal: controller.signal, transport: "sse", maxRetries: 0,
      timeoutMs: DEADLINE_MS, maxTokens: request.op === "generate" ? 4096 : 16384, reasoning: "minimal", cacheRetention: "none",
      fetch: boundedFetch(controller.signal, rawFetch, [ENDPOINT], 1, fault, budget),
      toolChoice: "none",
      onPayload(payload) {
        if (Buffer.byteLength(JSON.stringify(payload)) > INPUT_LIMIT) {
          transportFailure = "limit";
          throw new BridgeError("limit");
        }
      },
    });
    if (request.op === "plan") {
      const result = await consumeEvents(stream, controller, parsePlan);
      finish({id: requestId, type: "plan", result});
    } else if (request.op === "summarize") {
      const result = await consumeEvents(stream, controller, parseSummary);
      finish({id: requestId, type: "summary", result});
    } else {
      const result = await consumeEvents(stream, controller, parseResult);
      finish({id: requestId, type: "result", result});
    }
  } catch (e) {
    finish({id: requestId, type: "error", code: timedOut ? "timeout" : controller.signal.aborted ? safeError(e, true) : transportFailure ?? safeError(e, false)});
  }
}
// stdin framing is capped before JSON parsing. Cancel is the only subsequent command.
let pending = Buffer.alloc(0);
let started = false;
const input = createReadStream("", {fd: 0, autoClose: false, highWaterMark: 8192});
input.on("data", chunk => {
  pending = Buffer.concat([pending, Buffer.from(chunk)]);
  if (pending.byteLength > INPUT_LIMIT) { controller.abort(); finish({id: requestId, type: "error", code: "limit"}); return; }
  let newline: number;
  while ((newline = pending.indexOf(10)) >= 0) {
    const line = pending.subarray(0, newline).toString("utf8");
    pending = pending.subarray(newline + 1);
    if (!started) { started = true; void run(line); }
    else {
      try {
        const value = JSON.parse(line);
        if (value.op !== "cancel" || value.id !== requestId || Object.keys(value).length !== 2) throw new Error();
        controller.abort();
        // Let SDK credential locks unwind before the supervisor's hard-kill grace expires.
        setTimeout(() => finish({id: requestId, type: "error", code: "cancelled"}), 200);
      } catch { controller.abort(); finish({id: requestId, type: "error", code: "protocol"}); }
    }
  }
});
input.on("end", () => { if (!started || pending.length) finish({id: requestId, type: "error", code: "protocol"}); });
input.on("error", () => finish({id: requestId, type: "error", code: "protocol"}));
