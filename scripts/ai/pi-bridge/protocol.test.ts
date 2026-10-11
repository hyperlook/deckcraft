import { describe, test, expect } from "bun:test";
import { BridgeError, ENDPOINT, INPUT_LIMIT, OUTPUT_LIMIT, boundedFetch, consumeEvents, parseRequest, parseResult, parseSummary, parsePlan, safeError } from "./protocol.ts";
const generate = {id: 1, op: "generate", provider: "openai-codex", model: "fixture", input: "public data"};
const result = {title: "Public", points: ["data"]};
async function* events(values: any[]) { for (const v of values) yield v; }
const done = (value = result) => ({type: "done", reason: "stop", message: {content: [{type: "text", text: JSON.stringify(value)}]}});
describe("strict tool-free contract (offline, no personal auth)", () => {
  test("bounded requests and exact fields", () => {
    expect(parseRequest(JSON.stringify(generate))).toEqual(generate);
    for (const value of [{...generate, endpoint: "https://evil.test"}, {...generate, authPath: "/tmp/secret"}, {...generate, id: 2**53}, {...generate, provider: "extension"}, {...generate, input: ""}]) {
      expect(() => parseRequest(JSON.stringify(value))).toThrow();
    }
    expect(() => parseRequest("x".repeat(INPUT_LIMIT + 1))).toThrow();
  });
  test("whole-deck summary is a separate, concise, strict contract", async () => {
    expect(parseRequest(JSON.stringify({...generate, op: "summarize"})).op).toBe("summarize");
    const summary = {mainline: "从问题到方案再到行动", pages: [
      {pageId: 11, role: "建立问题", intent: "为后续行动方案铺垫", unknown: false},
      {pageId: 12, role: "未知", intent: "图片未读，无法判断", unknown: true},
    ]};
    expect(parseSummary(JSON.stringify(summary))).toEqual(summary);
    const bad = [
      {...summary, command: "file.save"}, {...summary, pages: []},
      {...summary, mainline: "x".repeat(2001)},
      {...summary, pages: [summary.pages[0], summary.pages[0]]},
      {...summary, pages: [{...summary.pages[0], pageId: -1}]},
      {...summary, pages: [{...summary.pages[0], pageId: 2**32}]},
      {...summary, pages: [{...summary.pages[0], role: "x".repeat(201)}]},
      {...summary, pages: [{...summary.pages[0], intent: "x".repeat(501)}]},
      {...summary, pages: [{...summary.pages[0], unknown: "false"}]},
      {...summary, pages: [{...summary.pages[0], intent: "line\nline"}]},
      {...summary, pages: [{...summary.pages[0], sources: []}]},
      result,
    ];
    for (const value of bad) expect(() => parseSummary(JSON.stringify(value))).toThrow();
    const event = {type: "done", reason: "stop", message: {content: [{type: "text", text: JSON.stringify(summary)}]}};
    expect(await consumeEvents(events([event]), new AbortController(), parseSummary)).toEqual(summary);
    await expect(consumeEvents(events([{...event, reason: "length"}]), new AbortController(), parseSummary)).rejects.toThrow();
    await expect(consumeEvents(events([{type: "toolcall_start"}]), new AbortController(), parseSummary)).rejects.toThrow();
  });
  test("new-project plan has no model-assigned identities or commands", async () => {
    const plan = {mainline: "问题、方案、行动", pages: Array.from({length:3}, (_, i) => ({title:`页 ${i+1}`, intent:"表达意图；事实待补"}))};
    expect(parseRequest(JSON.stringify({...generate, op:"plan"})).op).toBe("plan");
    expect(parsePlan(JSON.stringify(plan))).toEqual(plan);
    for (const v of [{...plan, pages:[]}, {...plan, pages:Array(13).fill(plan.pages[0])}, {...plan, mainline:"x".repeat(2001)},
      {...plan, pages:plan.pages.map(p => ({...p, pageId:123}))}, {...plan, command:"shape.insert"},
      {...plan, pages:plan.pages.map(p => ({...p, intent:"line\nline"}))},
      {...plan, pages:plan.pages.map(p => ({...p, title:""}))}, result]) {
      expect(() => parsePlan(JSON.stringify(v))).toThrow();
    }
    const terminal = {type:"done",reason:"stop",message:{content:[{type:"text",text:JSON.stringify(plan)}]}};
    expect(await consumeEvents(events([terminal]),new AbortController(),parsePlan)).toEqual(plan);
    await expect(consumeEvents(events([{type:"toolcall_start"}]),new AbortController(),parsePlan)).rejects.toThrow();
  });
  test("limited result and field whitelist", () => {
    expect(parseResult(JSON.stringify(result))).toEqual(result);
    for (const value of [{...result, command: "file.save"}, {...result, points: Array(33).fill("x")}, {...result, title: "x".repeat(501)}, {...result, points: [null]}, {...result, points: ["x".repeat(2001)]}]) {
      expect(() => parseResult(JSON.stringify(value))).toThrow();
    }
    expect(() => parseResult("```json\n{}\n```" )).toThrow();
  });
  test("only terminal complete output; tools, reasoning overflow, truncation and cancelled late results rejected", async () => {
    expect(await consumeEvents(events([done()]), new AbortController())).toEqual(result);
    for (const values of [[{type:"toolcall_start"}], [{type:"thinking_delta", delta:"x".repeat(OUTPUT_LIMIT + 1)}], [done(), done()], [{...done(), reason:"length"}], [{type:"error", error:{errorMessage:"Bearer secret"}}], [{type:"text_delta", delta:"{}"}]]) {
      await expect(consumeEvents(events(values), new AbortController())).rejects.toBeInstanceOf(BridgeError);
    }
    const controller = new AbortController(); controller.abort();
    await expect(consumeEvents(events([done()]), controller)).rejects.toThrow("cancelled");
  });
  test("upstream body bound before SDK buffering, no redirect or retry, status code only", async () => {
    const controller = new AbortController();
    let calls = 0;
    let fault = "";
    const fake = (async (_: any, init: any) => {
      calls++;
      expect(init.redirect).toBe("error");
      return new Response(new Uint8Array(OUTPUT_LIMIT + 1));
    }) as typeof fetch;
    const transport = boundedFetch(controller.signal, fake, [ENDPOINT], 1, code => fault = code);
    await expect(transport("https://evil.test" )).rejects.toThrow("transport");
    expect(calls).toBe(0);
    // A disallowed endpoint cannot consume a network call, but that transport is now unusable.
    const bounded = boundedFetch(controller.signal, fake, [ENDPOINT], 1, code => fault = code);
    const response = await bounded(ENDPOINT);
    await expect(response.text()).rejects.toThrow("limit");
    expect(fault).toBe("limit");
    await expect(bounded(ENDPOINT)).rejects.toThrow("transport");
    expect(calls).toBe(1);
    for (const [status, code] of [[401,"auth"], [403,"auth"], [429,"rate_limit"]] as const) {
      const fetcher = boundedFetch(controller.signal, (async () => new Response("token secret", {status})) as typeof fetch);
      await expect(fetcher(ENDPOINT)).rejects.toThrow(code);
    }
  });
  test("abort reaches transport; auth and model responses share aggregate budget", async () => {
    const controller = new AbortController();
    let reached: AbortSignal | undefined;
    const fake = (async (_: any, init: any) => { reached = init.signal; return new Response(new Uint8Array(OUTPUT_LIMIT / 2 + 1)); }) as typeof fetch;
    const budget = {total: 0};
    const auth = boundedFetch(controller.signal, fake, ["https://auth.openai.com/oauth/token"], 1, () => {}, budget);
    const model = boundedFetch(controller.signal, fake, [ENDPOINT], 1, () => {}, budget);
    await (await auth("https://auth.openai.com/oauth/token")).arrayBuffer();
    await expect((await model(ENDPOINT)).arrayBuffer()).rejects.toThrow("limit");
    controller.abort();
    expect(reached?.aborted).toBe(true);
  });
  test("errors never forward credential, response, headers or paths", () => {
    expect(safeError(new Error("Bearer secret /private/auth.json HTTP sensitive body"), false)).toBe("network");
    expect(safeError({code:"auth", token:"secret"}, false)).toBe("auth");
    expect(safeError(new Error("secret"), true)).toBe("cancelled");
    expect(safeError(new BridgeError("limit"), true)).toBe("limit");
  });
});
