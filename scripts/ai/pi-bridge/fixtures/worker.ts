// Offline subprocess fixture: never import the SDK, resolve auth, or call the network.
import { createReadStream } from "node:fs";
const mode = process.argv[2];
if (mode === "blocked") setInterval(() => {}, 1000); // Never read stdin.
else createReadStream("", {fd: 0}).once("data", () => {
  if (mode === "exit") process.exit(1);
  if (mode === "oversize") { process.stdout.write("x".repeat(1024 * 1024 + 1)); setInterval(() => {}, 1000); return; }
  if (mode === "malformed") process.stdout.write("bad JSON\n");
  else process.stdout.write(JSON.stringify({id: mode === "wrong" ? 999 : 7, type: "result", result: {title: "offline", points: []}}) + "\n");
  process.stdout.write("", () => process.exit(0));
});
