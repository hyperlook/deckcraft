// Explicit opt-in only. Never called by bun test/CI; sends a fixed non-sensitive fixture.
import { DEADLINE_MS, INPUT_LIMIT, OUTPUT_LIMIT, parseResult } from "./protocol.ts";
if (process.argv[2] !== "--live" || !process.argv[3] || process.argv.length !== 4) {
  console.error("Usage: bun --no-env-file --no-install scripts/ai/pi-bridge/smoke.ts --live MODEL_ID (uses Pi login / provider quota)");
  process.exit(2);
}
const model = process.argv[3];
async function exchange(request: object) {
  const input = JSON.stringify(request) + "\n";
  if (Buffer.byteLength(input) > INPUT_LIMIT) throw new Error("limit");
  const child = Bun.spawn(["bun", "--no-env-file", "--no-install", "--config=bunfig.toml", "main.ts"], {
    cwd: import.meta.dir, stdin: "pipe", stdout: "pipe", stderr: "ignore",
  });
  child.stdin.write(input);
  child.stdin.end();
  const timer = setTimeout(() => child.kill(), DEADLINE_MS + 500);
  try {
    const chunks: Uint8Array[] = [];
    let bytes = 0;
    for await (const chunk of child.stdout) {
      bytes += chunk.byteLength;
      if (bytes > OUTPUT_LIMIT) { child.kill(); throw new Error("limit"); }
      chunks.push(chunk);
    }
    if (await child.exited !== 0) throw new Error("exited");
    const event = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    if (event.id !== (request as {id: number}).id) throw new Error("protocol");
    // Only allowlisted local codes can escape this smoke, never raw SDK errors.
    if (event.type === "error") {
      const codes = ["auth", "rate_limit", "invalid_output", "limit", "cancelled", "timeout", "network", "unavailable", "unsupported", "protocol", "transport"];
      throw new Error(codes.includes(event.code) ? event.code : "protocol");
    }
    return event;
  } finally { clearTimeout(timer); child.kill(); await child.exited; }
}
try {
  const catalog = await exchange({id: 1, op: "models"});
  if (catalog.type !== "models" || !catalog.models.some((m: any) => m.provider === "openai-codex" && m.model === model)) throw new Error("unsupported");
  const event = await exchange({id: 2, op: "generate", provider: "openai-codex", model,
    input: "Non-sensitive fixture: A fictional community garden holds an open day on Saturday. Visitors can learn about composting and plant care. Admission is free."});
  if (event.type !== "result") throw new Error("protocol");
  parseResult(JSON.stringify(event.result));
  console.log(JSON.stringify({sdkVersion: catalog.sdkVersion, provider: "openai-codex", model, validation: "passed"}));
} catch (error) {
  const codes = ["auth", "rate_limit", "invalid_output", "limit", "cancelled", "timeout", "network", "unavailable", "unsupported", "protocol", "transport", "exited"];
  const code = error instanceof Error && codes.includes(error.message) ? error.message : "unavailable";
  console.error(JSON.stringify({provider: "openai-codex", model, validation: "failed", code}));
  process.exitCode = 1;
}
