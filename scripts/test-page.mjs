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
  pick(/^const CACHE_RANK = .*$/m, "CACHE_RANK"),
  pick(/^const cacheLabel = .*$/m, "cacheLabel"),
  pick(/^const isOpenRouter = .*$/m, "isOpenRouter"),
  pick(/\/\/ >>> measured-cache[\s\S]*?\/\/ <<< measured-cache/, "measured-cache block"),
  "return { observedExtract, observedRoute, measuredCache, metaRetention, knownExtract, knownCacheFor, limitsExtract, measuredLimits, parseTok, levelForSeconds, cacheAdvice, cacheSummaryParts, setState: (v) => { S = v; } };",
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
doc.known = [
  { route: "claude-subscription", client: "cliproxyapi", model: "*", ttl_s: 3600, basis: "CPA asks for the 1-hour cache on Claude logins.", source: "x" },
  { route: "anthropic-api", client: "cliproxyapi", model: "*", ttl_s: 300, basis: "A Claude key gets the 5-minute cache.", source: "x" },
  { route: "anthropic-api", client: "cliproxyapi", model: "*", when: { "fingerprint-profile": "claude-code-cli" }, ttl_s: 3600, basis: "A key with the claude-code-cli profile gets 1 hour.", source: "x" },
  { route: "anthropic-api", client: "some-other-proxy", model: "*", ttl_s: 7200, basis: "not about CPA", source: "x" },
];
doc.limits = [
  { route: "codex-subscription", model: "gpt-6.1-terra", input_accepted: 920870, input_refused: 921875 },
  { route: "devin", model: "gpt-6-1-sol", input_accepted: 915390, input_refused: 925000 },
  { route: "claude-subscription", model: "claude-opus-5-5", context: 1000000, output: 128000 },
];
const obs = page.observedExtract(doc), known = page.knownExtract(doc), lim = page.limitsExtract(doc);
const state = (config = {}) => page.setState({ ext: { obs, known, lim }, config });
const devin = { kind: "auth", provider: "devin" }, codex = { kind: "auth", provider: "codex" }, meta = { kind: "auth", provider: "meta" };
const openrouter = { kind: "apikey", provider: "openai-compatibility", baseUrl: "https://openrouter.ai/api/v1" };
const openaiKey = { kind: "apikey", provider: "codex", baseUrl: "https://api.openai.com/v1" };
state();

// routes
const claudeLogin = { kind: "auth", provider: "claude" }, claudeKey = { kind: "apikey", provider: "claude", baseUrl: "", profile: "" };
assert.equal(page.observedRoute(claudeLogin), "claude-subscription");
assert.equal(page.observedRoute(claudeKey), "anthropic-api");
assert.equal(page.observedRoute({ kind: "apikey", provider: "claude", baseUrl: "https://relay.example.com" }), "", "a Claude key on someone else's address is not Anthropic's API");
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

// known lifetimes come from the file, not from the page: logins 1 hour, keys 5 minutes, the profile exception
state();
assert.equal(known.length, 3, "rows about another proxy are dropped");
let k = page.knownCacheFor(claudeLogin, "claude-opus-5-5");
assert.equal(k.level, "1h"); assert.equal(k.src, "known"); assert.match(k.why, /Not measured/);
assert.equal(page.knownCacheFor(claudeKey, "claude-opus-5-5").level, "5m");
assert.equal(page.knownCacheFor({ ...claudeKey, profile: "Claude-Code-CLI" }, "claude-opus-5-5").level, "1h", "the more specific row wins");
assert.equal(page.knownCacheFor(devin, "devin/claude-opus-5-5"), null);
page.setState({ ext: { obs, known: [], lim: [] }, config: {} });
assert.equal(page.knownCacheFor(claudeLogin, "claude-opus-5-5"), null, "without the file the page knows nothing about Claude");
state();
assert.equal(page.levelForSeconds(0), "none"); assert.equal(page.levelForSeconds(299), "none"); assert.equal(page.levelForSeconds(14400), "2h");

// measured limits: a catalog value the measurement agrees with stays, one it contradicts is replaced
let l = page.measuredLimits(codex, "gpt-6.1-terra", 922000, 128000);
assert.equal(l.ctx, 920870, "922,000 was refused (921,875 is the smallest refused size)");
assert.equal(l.changed, true);
l = page.measuredLimits(codex, "gpt-6.1-terra", 921000, 128000);
assert.equal(l.ctx, 921000); assert.equal(l.changed, false, "between accepted and refused: consistent");
l = page.measuredLimits(devin, "devin/gpt-6-1-sol", 1000000, 128000);
assert.equal(l.ctx, 915390, "the list says 1M, 925k was refused"); assert.equal(l.refused, 925000);
l = page.measuredLimits(codex, "gpt-6.1-terra", 872000, 128000);
assert.equal(l.ctx, 920870, "a catalog value below what was accepted is raised to it");
l = page.measuredLimits(claudeLogin, "claude-opus-5-5", 0, 0);
assert.equal(l.ctx, 1000000); assert.equal(l.out, 128000); assert.equal(l.refused, 1000001);
assert.equal(page.measuredLimits(claudeLogin, "claude-opus-5-5", 1000000, 128000).changed, false);
assert.equal(page.measuredLimits(devin, "devin/gpt-6-astra", 1000000, 128000), null, "nothing measured: no opinion");

// the limit fields
assert.equal(page.parseTok("922k"), 922000); assert.equal(page.parseTok("1.05M"), 1050000); assert.equal(page.parseTok("128,000"), 128000);
assert.equal(page.parseTok(" 1048576 "), 1048576); assert.equal(page.parseTok(""), 0);
assert.ok(Number.isNaN(page.parseTok("lots"))); assert.ok(Number.isNaN(page.parseTok("-5k"))); assert.ok(Number.isNaN(page.parseTok("0")));

// a run that continues an earlier one: the file's safe value decides, even if a reader sees only cold checks
page.setState({ ext: { obs: page.observedExtract({ cache: [run("m/M1", "meta-subscription", "pi+cliproxyapi", "muse-ember-1.0", [["8h", 0, 3], ["24h", 0, 3]], { safe: 14400 })] }), known: [], lim: [] }, config: {} });
m = page.measuredCache(meta, "muse-ember-1.0");
assert.equal(m.level, "2h", "4 hours safe is the longest level, not 'no cache'"); assert.match(m.why, /up to 4 h/);
state();

// the line under every member: shown whether or not a level is set
let a = page.cacheAdvice("", { level: "30m", src: "measured" });
assert.deepEqual(a, { tone: "warn", use: "30m", text: "suggested cache: 30 min (measured)" }, "not set: suggest, with Use");
a = page.cacheAdvice("30m", { level: "30m", src: "measured" });
assert.equal(a.tone, "quiet"); assert.equal(a.use, "", "nothing to change"); assert.match(a.text, /30 min \(measured\) · same as set/);
a = page.cacheAdvice("30m", { level: "5m", src: "measured" });
assert.deepEqual(a, { tone: "warn", use: "5m", text: "set to 30 min; measured: 5 min" }, "set above a measurement: flagged");
a = page.cacheAdvice("1h", { level: "none", src: "known" });
assert.equal(a.tone, "warn"); assert.equal(a.use, "none");
a = page.cacheAdvice("5m", { level: "2h", src: "measured" });
assert.equal(a.tone, "quiet", "set below a measurement is the owner's cautious choice"); assert.equal(a.use, "2h");
assert.match(a.text, /suggested cache: 2 hours \(measured\) · set to 5 min/);
a = page.cacheAdvice("1h", { level: "5m", src: "models.dev" });
assert.equal(a.tone, "quiet", "a documented value is not firm enough to flag"); assert.equal(a.use, "5m");
for (const set of ["", "none", "30m"]) {
  a = page.cacheAdvice(set, null);
  assert.equal(a.tone, "quiet"); assert.equal(a.use, ""); assert.match(a.text, /^no suggestion/);
}

// the pool's cache line: what the pool gets, red when the members don't agree
const line = (levels) => page.cacheSummaryParts(levels).map((x) => (x.cls === "bad" ? "RED " : "") + x.text);
assert.deepEqual(line(["1h", "1h"]), ["cache: 1 hour"]);
assert.deepEqual(line(["2h", "30m", "30m"]), ["RED cache: 30 min"], "mixed: the shortest, in red");
assert.deepEqual(line(["5m", "none"]), ["RED cache: none"], "a member that doesn't cache means the pool gets no lifetime");
assert.deepEqual(line(["5m", ""]), ["RED cache: none"]);
assert.deepEqual(line(["none"]), ["RED cache: none"]);
assert.deepEqual(line(["", ""]), ["RED cache: none"]);
assert.match(page.cacheSummaryParts(["2h", "30m"])[0].title, /from 30 min to 2 hours/);
assert.match(page.cacheSummaryParts(["2h", "30m", "none", "none", ""])[0].title, /^2 members don't cache; 1 member has no cache level set; 2 members cache 30 min – 2 hours\./);
assert.match(page.cacheSummaryParts(["5m", "none"])[0].title, /^1 member doesn't cache; 1 member caches 5 min\./);

// cache hits: every member gets a line of the same shape, and a rate only when it can say something
const hits = new Function([pick(/^const HITS_LOW = .*$/m, "HITS_LOW"), pick(/\/\/ >>> hits[\s\S]*?\/\/ <<< hits/, "hits block"), "return { hitsRate, hitsLabel };"].join("\n"))();
const counts = (requests, failed, prompt, read) => ({ requests, failed, prompt, read, write: 0 });
assert.deepEqual(hits.hitsLabel(counts(166, 0, 7600000, 7450000), "24 h"), { text: "hits 98% · 24 h", low: false, why: "" });
assert.equal(hits.hitsLabel(counts(181, 0, 5000000, 4950000), "1 h").text, "hits 99% · 1 h", "the window is always shown");
assert.deepEqual(hits.hitsLabel(counts(40, 0, 900000, 90000), "24 h"), { text: "hits 10% · 24 h", low: true, why: "" });
assert.deepEqual(hits.hitsLabel(counts(6, 0, 858, 0), "24 h"), { text: "hits – · tiny prompts · 24 h", low: false, why: "small" },
  "six test pings must not read as a 0% cache");
assert.deepEqual(hits.hitsLabel(counts(4, 4, 0, 0), "24 h"), { text: "hits – · 4 failed · 24 h", low: false, why: "failed" });
assert.deepEqual(hits.hitsLabel(counts(0, 0, 0, 0), "24 h"), { text: "hits – · no requests · 24 h", low: false, why: "none" });
assert.equal(hits.hitsLabel(counts(0, 0, 0, 0), "since 23:12").text, "hits – · no requests · since 23:12");
assert.equal(hits.hitsRate(counts(10, 2, 400000, 200000)).rate, 50, "failed requests don't count toward the average size");
assert.equal(hits.hitsRate(counts(3, 0, 40000, 0)).rate, 0, "a real 0% on cacheable prompts is still shown");

console.log("page tests passed");
