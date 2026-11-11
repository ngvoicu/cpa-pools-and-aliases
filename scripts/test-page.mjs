#!/usr/bin/env node
// Tests the measured-cache logic of web/index.html without a browser: the block between the
// ">>> measured-cache" and "<<< measured-cache" markers is evaluated with the few helpers it uses.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const html = readFileSync(new URL("../web/index.html", import.meta.url), "utf8");
const pick = (re, what) => { const m = html.match(re); assert.ok(m, `not found in index.html: ${what}`); return m[0]; };
const source = [
  pick(/^const lc = .*$/m, "lc"),
  pick(/^const mkey = .*$/m, "mkey"),
  pick(/^const CACHE_LEVELS = .*$/m, "CACHE_LEVELS"),
  pick(/^const cacheLabel = .*$/m, "cacheLabel"),
  pick(/^const isOpenRouter = .*$/m, "isOpenRouter"),
  pick(/\/\/ >>> measured-cache[\s\S]*?\/\/ <<< measured-cache/, "measured-cache block"),
  "return { observedExtract, observedRoute, measuredCache, metaRetention, setState: (v) => { S = v; } };",
].join("\n");
const page = new Function("let S = {};\n" + source)();

const run = (id, route, client, model, checks, extra = {}) => ({
  id, route, client, model, date: extra.date || "2026-11-08", latest: extra.latest !== false,
  options: extra.options || {}, control: extra.control || { warm: 1, total: 1 },
  checks: checks.map(([age, warm, total]) => ({ age, warm, total })),
  all_warm_through_s: extra.safe ?? 0, ...(extra.lacks ? { lacks: extra.lacks } : {}),
});
const doc = { cache: [
  run("d/D5p", "devin", "pi+cliproxyapi", "gpt-6-astra", [["5m", 2, 2], ["15m", 1, 2], ["30m", 2, 2], ["1h", 0, 2]], { safe: 300 }),
  run("d/D5c", "devin", "devin-cli", "gpt-6-astra", [["5m", 2, 2], ["2h", 2, 2]], { safe: 7200 }),
  run("h/H2", "codex-subscription", "pi+cliproxyapi", "gpt-6.1-sol", [["5m", 3, 3], ["30m", 3, 3], ["1h", 2, 3]], { safe: 1800 }),
  run("x/X1", "codex-subscription", "cliproxyapi", "gpt-6.1-sol", [["5m", 3, 3], ["15m", 2, 3]], { safe: 300, date: "2026-11-06" }),
  run("e/E1", "meta-subscription", "pi+cliproxyapi", "muse-ember-1.0", [["5m", 2, 2], ["30m", 1, 2]], { safe: 300 }),
  run("e/E2", "meta-subscription", "pi+cliproxyapi", "muse-ember-1.0", [["5m", 3, 3], ["4h", 3, 3]], { safe: 14400, options: { prompt_cache_retention: "24h" } }),
  run("o/O1", "openrouter", "cliproxyapi", "anthropic/claude-opus-5.5", [], { control: { warm: 0, total: 1 }, lacks: "router-for-me/CLIProxyAPI#7284" }),
  run("r/R1h", "openrouter", "direct", "anthropic/claude-opus-5.5", [["30m", 3, 3]], { safe: 1800, options: { ttl: "1h" } }),
  run("r/R6", "openrouter", "direct", "x-ai/grok-4.7", [["2.5m", 3, 3], ["5m", 2, 3], ["15m", 0, 3]], { safe: 150 }),
  run("d/D12p", "devin", "pi+cliproxyapi", "swe-2", [["5m", 0, 2], ["15m", 0, 2]], { safe: 0 }),
  run("d/D3", "devin", "cliproxyapi", "swe-2", [], { control: { warm: 0, total: 1 }, date: "2026-11-04" }),
  run("old/C1", "codex-subscription", "pi+cliproxyapi", "gpt-6.1-terra", [["5m", 3, 3]], { safe: 300, latest: false }),
] };
const obs = page.observedExtract(doc);
const state = (config = {}) => page.setState({ ext: { obs }, config });
const devin = { kind: "auth", provider: "devin" }, codex = { kind: "auth", provider: "codex" }, meta = { kind: "auth", provider: "meta" };
const openrouter = { kind: "apikey", provider: "openai-compatibility", baseUrl: "https://openrouter.ai/api/v1" };
const openaiKey = { kind: "apikey", provider: "codex", baseUrl: "https://api.openai.com/v1" };
state();

// routes
assert.equal(page.observedRoute(devin), "devin");
assert.equal(page.observedRoute(codex), "codex-subscription");
assert.equal(page.observedRoute(openrouter), "openrouter");
assert.equal(page.observedRoute(openaiKey), "openai-api");
assert.equal(page.observedRoute({ kind: "apikey", provider: "codex", baseUrl: "" }), "", "a Codex key without the OpenAI address has no route");
assert.equal(page.observedRoute({ kind: "apikey", provider: "openai-compatibility", baseUrl: "https://example.com/v1" }), "");

// the case that started this: Devin on GPT-6 Astra is 5 minutes, not the vendor's 30
let m = page.measuredCache(devin, "devin/gpt-6-astra");
assert.equal(m.level, "5m");
assert.equal(m.src, "measured");
assert.match(m.why, /5m 2\/2, 15m 1\/2/);
assert.match(m.why, /d\/D5p/, "the vendor CLI's run must not be used");

// through pi wins over plain requests, whatever the dates
assert.equal(page.measuredCache(codex, "gpt-6.1-sol").level, "30m");
assert.equal(page.measuredCache(codex, "gpt-6.1-sol").id, "h/H2");

// runs that are no longer the latest are ignored; unknown models and routes give nothing
assert.equal(page.measuredCache(codex, "gpt-6.1-terra"), null);
assert.equal(page.measuredCache(devin, "devin/some-new-model"), null);
assert.equal(page.measuredCache({ kind: "apikey", provider: "claude", baseUrl: "" }, "claude-opus-5-5"), null);
assert.equal(page.measuredCache(openaiKey, "gpt-6.1-sol"), null);

// Meta: the run must match what the config asks Meta for
assert.equal(page.measuredCache(meta, "muse-ember-1.0").id, "e/E1");
assert.equal(page.measuredCache(meta, "muse-ember-1.0").level, "5m");
state({ requests: { payload: { override: [{ models: [{ name: "muse-*", protocol: "meta" }], params: { prompt_cache_retention: "24h" } }] } } });
assert.equal(page.metaRetention(), "24h");
assert.equal(page.measuredCache(meta, "muse-ember-1.0").id, "e/E2");
assert.equal(page.measuredCache(meta, "muse-ember-1.0").level, "2h", "4 hours measured, 2 hours is the longest level");
state({ requests: { payload: { default: [{ models: [{ name: "gpt-*", protocol: "codex" }], params: { prompt_cache_retention: "24h" } }] } } });
assert.equal(page.metaRetention(), "", "a rule for another protocol is not Meta's");
state();

// OpenRouter: a run made before CPA marked requests for caching is skipped, and so is one that asked for 1 hour
assert.equal(page.measuredCache(openrouter, "anthropic/claude-opus-5.5"), null);
// ... and a run straight to OpenRouter counts; not reliable at 5 minutes means no level worth warming for
m = page.measuredCache(openrouter, "x-ai/grok-4.7");
assert.equal(m.level, "none");
assert.match(m.why, /not reliable even at 5 minutes/);

// never cached
m = page.measuredCache(devin, "devin/swe-2");
assert.equal(m.level, "none");
assert.match(m.why, /no check found the cache/);

// a broken or missing file suggests nothing
assert.deepEqual(page.observedExtract(null), []);
assert.deepEqual(page.observedExtract({ cache: "x" }), []);
page.setState({ ext: null, config: {} });
assert.equal(page.measuredCache(devin, "devin/gpt-6-astra"), null);

console.log("page tests passed");
