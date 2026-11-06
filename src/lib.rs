//! cpa-pools-and-aliases — CLIProxyAPI plugin (C ABI v1, JSON envelopes).
//!
//! 1. Serves the "Pools and Aliases" management page (a menu resource). The page edits CPA's
//!    native model aliases through the regular /v8/management API; the plugin never routes.
//! 2. Cleans pool entries in model-list responses (/v1/models for OpenAI, Claude, Gemini and
//!    Codex-style clients): a pool is one name over many accounts, so its display name must not
//!    carry whichever member's provider/harness label CPA happened to pick, e.g.
//!    "Claude Opus 5.5 (Devin)". Pool entries show their plain model id instead.
//!
//! 3. Reports each pool's limits in those lists: the context window, output limit and prompt-cache
//!    lifetime the page stored (plugins.configs.cpa-pools-and-aliases.limits). CPA itself keeps the
//!    metadata of whichever member it registered last, so without this a pool's limits change with
//!    registration order.
//!
//! 4. Measures each pool's prompt-cache hit rate: CPA hands every finished request to the plugin (usage.handle);
//!    requests for a pool are counted per member, in memory, and served to the page at
//!    /v0/management/cpa-pools-and-aliases/cache-stats (management key required).
//!
//! Pools come from the plugin's own config (plugins.configs.cpa-pools-and-aliases.pools), which
//! CPA hands to the plugin on plugin.register / plugin.reconfigure.

use std::collections::{HashMap, VecDeque};
use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Map, Value};

const ABI_VERSION: u32 = 1;
const PLUGIN_ID: &str = "cpa-pools-and-aliases";
const PLUGIN_NAME: &str = "Pools and Aliases";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = "https://github.com/ngvoicu/cpa-pools-and-aliases";
const LOGO: &str = "https://raw.githubusercontent.com/ngvoicu/cpa-pools-and-aliases/main/assets/logo.svg";
const PAGE_HTML: &str = include_str!("../web/index.html");
/// Management API route for the hit rates (under /v0/management/, so it needs the management key).
const STATS_PATH: &str = "/v0/management/cpa-pools-and-aliases/cache-stats";

// ------------------------------------------------------------------ C ABI

#[repr(C)]
pub struct CliproxyBuffer {
    ptr: *mut u8,
    len: usize,
}

type HostCall = unsafe extern "C" fn(*mut c_void, *const c_char, *const u8, usize, *mut CliproxyBuffer) -> i32;
type HostFree = unsafe extern "C" fn(*mut c_void, usize);
type PluginCall = unsafe extern "C" fn(*const c_char, *const u8, usize, *mut CliproxyBuffer) -> i32;
type PluginFree = unsafe extern "C" fn(*mut c_void, usize);
type PluginShutdown = unsafe extern "C" fn();

#[repr(C)]
pub struct CliproxyHostApi {
    abi_version: u32,
    host_ctx: *mut c_void,
    call: Option<HostCall>,
    free_buffer: Option<HostFree>,
}

#[repr(C)]
pub struct CliproxyPluginApi {
    abi_version: u32,
    call: Option<PluginCall>,
    free_buffer: Option<PluginFree>,
    shutdown: Option<PluginShutdown>,
}

/// # Safety
/// Called by CPA with valid pointers for the lifetime of the process.
#[no_mangle]
pub unsafe extern "C" fn cliproxy_plugin_init(_host: *const CliproxyHostApi, plugin: *mut CliproxyPluginApi) -> i32 {
    if plugin.is_null() {
        return 1;
    }
    (*plugin).abi_version = ABI_VERSION;
    (*plugin).call = Some(plugin_call);
    (*plugin).free_buffer = Some(plugin_free);
    (*plugin).shutdown = Some(plugin_shutdown);
    0
}

unsafe extern "C" fn plugin_call(method: *const c_char, request: *const u8, request_len: usize, response: *mut CliproxyBuffer) -> i32 {
    if !response.is_null() {
        (*response).ptr = ptr::null_mut();
        (*response).len = 0;
    }
    if method.is_null() {
        write_response(response, error_envelope("invalid_method", "method is required"));
        return 1;
    }
    let method = CStr::from_ptr(method).to_string_lossy().into_owned();
    let body: &[u8] = if request.is_null() || request_len == 0 { &[] } else { std::slice::from_raw_parts(request, request_len) };
    // Never let a panic cross the FFI boundary into the host.
    let out = catch_unwind(AssertUnwindSafe(|| handle(&method, body)))
        .unwrap_or_else(|_| error_envelope("plugin_panic", "internal plugin error"));
    write_response(response, out);
    0
}

unsafe extern "C" fn plugin_free(ptr: *mut c_void, len: usize) {
    if !ptr.is_null() && len > 0 {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr as *mut u8, len)));
    }
}

unsafe extern "C" fn plugin_shutdown() {}

unsafe fn write_response(response: *mut CliproxyBuffer, out: Vec<u8>) {
    if response.is_null() || out.is_empty() {
        return;
    }
    let boxed = out.into_boxed_slice();
    let len = boxed.len();
    (*response).ptr = Box::into_raw(boxed) as *mut u8;
    (*response).len = len;
}

fn ok_envelope(result: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "ok": true, "result": result })).unwrap_or_default()
}

fn error_envelope(code: &str, message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "ok": false, "error": { "code": code, "message": message } })).unwrap_or_default()
}

// ------------------------------------------------------------------ state

#[derive(Clone, Default, Debug)]
struct Settings {
    /// lower-cased pool id -> the id as configured, shown to clients as the model's display name
    pools: HashMap<String, String>,
    /// model lists show only the pools (requests for other models still work)
    pools_only: bool,
    /// lower-cased pool id -> limits reported for the pool
    limits: HashMap<String, Limits>,
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Limits {
    context: Option<u64>,
    output: Option<u64>,
    cache_ttl: Option<u64>,
}

fn settings() -> &'static RwLock<Settings> {
    static SETTINGS: OnceLock<RwLock<Settings>> = OnceLock::new();
    SETTINGS.get_or_init(|| RwLock::new(Settings::default()))
}

fn page_b64() -> &'static str {
    static PAGE: OnceLock<String> = OnceLock::new();
    PAGE.get_or_init(|| B64.encode(PAGE_HTML.as_bytes()))
}

#[derive(Deserialize, Default)]
struct LifecycleRequest {
    #[serde(default)]
    config_yaml: Option<String>,
}

/// Plugin config: `pools: [claude-opus-5-5, ...]` plus `pools-only: bool`. Older `{name, label}` entries
/// are accepted; the label is ignored, since a pool always shows as its model id.
fn parse_settings(yaml: &[u8]) -> Settings {
    let doc: serde_yaml::Value = match serde_yaml::from_slice(yaml) {
        Ok(v) => v,
        Err(_) => return Settings::default(),
    };
    Settings {
        pools: parse_pools(&doc),
        pools_only: doc.get("pools-only").and_then(|v| v.as_bool()).unwrap_or(false),
        limits: parse_limits(&doc),
    }
}

/// `limits: {<pool>: {context: 922000, output: 64000, cache-ttl: 3600}}`; missing or invalid values are skipped.
fn parse_limits(doc: &serde_yaml::Value) -> HashMap<String, Limits> {
    let mut out = HashMap::new();
    let Some(map) = doc.get("limits").and_then(|v| v.as_mapping()) else { return out };
    let num = |v: &serde_yaml::Value, k: &str| v.get(k).and_then(|x| x.as_u64()).filter(|n| *n > 0);
    for (k, v) in map {
        let Some(name) = k.as_str().map(str::trim).filter(|s| !s.is_empty()) else { continue };
        let l = Limits { context: num(v, "context"), output: num(v, "output"), cache_ttl: num(v, "cache-ttl") };
        if l != Limits::default() {
            out.insert(name.to_lowercase(), l);
        }
    }
    out
}

fn parse_pools(doc: &serde_yaml::Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let list = match doc.get("pools").and_then(|p| p.as_sequence()) {
        Some(l) => l,
        None => return out,
    };
    for item in list {
        let name = match item {
            serde_yaml::Value::String(s) => s.trim().to_string(),
            serde_yaml::Value::Mapping(_) => item.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
            _ => continue,
        };
        if name.is_empty() {
            continue;
        }
        out.insert(name.to_lowercase(), name);
    }
    out
}

fn apply_config(request: &[u8]) {
    let req: LifecycleRequest = serde_json::from_slice(request).unwrap_or_default();
    let yaml = req.config_yaml.and_then(|s| B64.decode(s.as_bytes()).ok()).unwrap_or_default();
    let parsed = parse_settings(&yaml);
    if let Ok(mut guard) = settings().write() {
        *guard = parsed;
    }
}

// ------------------------------------------------------------------ dispatch

fn handle(method: &str, request: &[u8]) -> Vec<u8> {
    match method {
        "plugin.register" | "plugin.reconfigure" => {
            apply_config(request);
            ok_envelope(registration())
        }
        "management.register" => ok_envelope(json!({
            "routes": [{ "Method": "GET", "Path": STATS_PATH }],
            "resources": [{
                "Path": "/ui",
                "Menu": PLUGIN_NAME,
                "Description": "Create model pools from any subscription or API key."
            }]
        })),
        "management.handle" => {
            if management_path(request).is_some_and(|p| p.trim_end_matches('/') == STATS_PATH) {
                let body = serde_json::to_vec(&stats_snapshot(now_secs())).unwrap_or_default();
                return ok_envelope(json!({
                    "StatusCode": 200,
                    "Headers": { "content-type": ["application/json"], "cache-control": ["no-store"] },
                    "Body": B64.encode(body)
                }));
            }
            // Every other call is the page (/ui), the only resource.
            let prefix = r#"{"ok":true,"result":{"StatusCode":200,"Headers":{"content-type":["text/html; charset=utf-8"],"cache-control":["no-store"],"x-content-type-options":["nosniff"]},"Body":""#;
            let mut out = Vec::with_capacity(prefix.len() + page_b64().len() + 3);
            out.extend_from_slice(prefix.as_bytes());
            out.extend_from_slice(page_b64().as_bytes());
            out.extend_from_slice(br#""}}"#);
            out
        }
        "usage.handle" => {
            if let Ok(cfg) = settings().read() {
                record_usage(request, now_secs(), &cfg.pools);
            }
            ok_envelope(json!({}))
        }
        "response.intercept_after" => ok_envelope(intercept_response(request).unwrap_or_else(|| json!({}))),
        "plugin.quiesce" | "plugin.shutdown" => ok_envelope(json!({})),
        _ => error_envelope("unknown_method", &format!("{PLUGIN_ID}: unknown method {method}")),
    }
}

fn registration() -> Value {
    json!({
        "schema_version": 1,
        "metadata": {
            "Name": PLUGIN_NAME, "Version": VERSION, "Author": "ngvoicu",
            "GitHubRepository": REPO, "Logo": LOGO, "ConfigFields": []
        },
        "capabilities": { "management_api": true, "response_interceptor": true, "usage_plugin": true }
    })
}

fn management_path(request: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    #[allow(non_snake_case)]
    struct Req {
        #[serde(default)]
        Path: String,
    }
    serde_json::from_slice::<Req>(request).ok().map(|r| r.Path)
}

// ------------------------------------------------------------------ cache hit rates

/// Requests are counted in 5-minute slots, kept for a day.
const SLOT_SECS: u64 = 300;
const KEEP_SLOTS: u64 = 24 * 3600 / SLOT_SECS;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Counter {
    requests: u64,
    failed: u64,
    /// prompt tokens, cached or not
    prompt: u64,
    cache_read: u64,
    cache_write: u64,
}

impl Counter {
    fn add(&mut self, o: &Counter) {
        self.requests += o.requests;
        self.failed += o.failed;
        self.prompt += o.prompt;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }
    fn json(&self) -> Value {
        json!({ "requests": self.requests, "failed": self.failed, "prompt_tokens": self.prompt,
                "cache_read_tokens": self.cache_read, "cache_write_tokens": self.cache_write })
    }
}

#[derive(Default)]
struct MemberStats {
    auth_id: String,
    provider: String,
    model: String,
    last_seen: u64,
    slots: VecDeque<(u64, Counter)>,
    total: Counter,
}

struct Stats {
    since: u64,
    /// lower-cased pool id -> member key (auth index, else auth id) -> counters
    pools: HashMap<String, HashMap<String, MemberStats>>,
}

fn stats() -> &'static Mutex<Stats> {
    static STATS: OnceLock<Mutex<Stats>> = OnceLock::new();
    STATS.get_or_init(|| Mutex::new(Stats { since: now_secs(), pools: HashMap::new() }))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The fields of a usage record the plugin reads. Never the client key or the credential secret
/// (APIKey, Source): they are not deserialized at all.
#[derive(Deserialize, Default)]
#[allow(non_snake_case)]
struct UsageRecord {
    #[serde(default)]
    Alias: String,
    #[serde(default)]
    Model: String,
    #[serde(default)]
    Provider: String,
    #[serde(default)]
    ExecutorType: String,
    #[serde(default)]
    AuthID: String,
    #[serde(default)]
    AuthIndex: String,
    #[serde(default)]
    Failed: bool,
    #[serde(default = "yes")]
    Generate: bool,
    #[serde(default)]
    Detail: UsageDetail,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, Default, Clone, Copy)]
#[allow(non_snake_case)]
struct UsageDetail {
    #[serde(default)]
    InputTokens: i64,
    #[serde(default)]
    OutputTokens: i64,
    #[serde(default)]
    ReasoningTokens: i64,
    #[serde(default)]
    CachedTokens: i64,
    #[serde(default)]
    CacheReadTokens: i64,
    #[serde(default)]
    CacheCreationTokens: i64,
    #[serde(default)]
    TotalTokens: i64,
}

/// Returns (prompt tokens, cache read, cache write). Providers count input two ways: Anthropic's input
/// excludes the cache (prompt = input + read + write), OpenAI-style input includes it (prompt = input).
/// Known providers decide; otherwise the record's own numbers do (a reported total that adds the cache on top
/// of the input, or a cache larger than the input, means it was excluded).
fn prompt_tokens(d: &UsageDetail, provider: &str, executor: &str) -> (u64, u64, u64) {
    let pos = |v: i64| v.max(0) as u64;
    let (input, read, write) = (pos(d.InputTokens), pos(d.CacheReadTokens.max(d.CachedTokens)), pos(d.CacheCreationTokens));
    let (output, reasoning, total) = (pos(d.OutputTokens), pos(d.ReasoningTokens), pos(d.TotalTokens));
    let p = provider.to_lowercase();
    let e = executor.to_lowercase();
    let both = format!("{p} {e}");
    let excluded = if e == "openaicompatexecutor" || p == "openai-compatibility" || p.starts_with("openai-compatible-") {
        false
    } else if both.contains("claude") || both.contains("anthropic") {
        true
    } else if ["gemini", "aistudio", "antigravity", "vertex", "openai", "codex", "xai", "grok"].iter().any(|m| both.contains(m)) {
        false
    } else {
        let cache = read + write;
        cache > input || (cache > 0 && total > 0 && (total == input + cache + output || total == input + cache + output + reasoning))
    };
    let prompt = if excluded { input + read + write } else { input.max(read + write) };
    (prompt, read, write)
}

/// Adds one finished request to its pool member's counters (requests for anything but a pool are ignored).
fn record_usage(request: &[u8], now: u64, pools: &HashMap<String, String>) {
    let Ok(r) = serde_json::from_slice::<UsageRecord>(request) else { return };
    if !r.Generate {
        return; // token counting, not a model call
    }
    let pool = r.Alias.trim().to_lowercase();
    if !pools.contains_key(&pool) {
        return;
    }
    let key = if r.AuthIndex.trim().is_empty() { r.AuthID.trim().to_string() } else { r.AuthIndex.trim().to_string() };
    if key.is_empty() {
        return;
    }
    let mut c = Counter { requests: 1, ..Counter::default() };
    if r.Failed {
        c.failed = 1;
    } else {
        let (prompt, read, write) = prompt_tokens(&r.Detail, &r.Provider, &r.ExecutorType);
        c.prompt = prompt;
        c.cache_read = read;
        c.cache_write = write;
    }
    let Ok(mut st) = stats().lock() else { return };
    let m = st.pools.entry(pool).or_default().entry(key).or_default();
    m.auth_id = r.AuthID.trim().to_string();
    m.provider = r.Provider.trim().to_string();
    m.model = r.Model.trim().to_string();
    m.last_seen = now;
    m.total.add(&c);
    let slot = now / SLOT_SECS;
    match m.slots.back_mut() {
        Some((s, counter)) if *s == slot => counter.add(&c),
        _ => m.slots.push_back((slot, c)),
    }
    while m.slots.front().is_some_and(|(s, _)| *s + KEEP_SLOTS <= slot) {
        m.slots.pop_front();
    }
}

fn window(m: &MemberStats, now: u64, secs: u64) -> Counter {
    let first = (now.saturating_sub(secs) / SLOT_SECS) + 1;
    let mut c = Counter::default();
    for (s, counter) in &m.slots {
        if *s >= first {
            c.add(counter);
        }
    }
    c
}

/// Counters per pool and member for the last hour, the last day and since the plugin started.
fn stats_snapshot(now: u64) -> Value {
    let Ok(st) = stats().lock() else { return json!({}) };
    let mut pools = Map::new();
    for (pool, members) in &st.pools {
        let mut list: Vec<Value> = members
            .iter()
            .map(|(key, m)| {
                json!({
                    "auth_index": key, "auth_id": m.auth_id, "provider": m.provider, "model": m.model,
                    "last_seen": m.last_seen,
                    "hour": window(m, now, 3600).json(), "day": window(m, now, 24 * 3600).json(), "all": m.total.json(),
                })
            })
            .collect();
        list.sort_by(|a, b| a["auth_index"].as_str().cmp(&b["auth_index"].as_str()));
        pools.insert(pool.clone(), json!({ "members": list }));
    }
    json!({ "since": st.since, "now": now, "slot_seconds": SLOT_SECS, "pools": pools })
}

// ------------------------------------------------------------------ model lists

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct InterceptRequest {
    #[serde(default)]
    Model: String,
    #[serde(default)]
    RequestedModel: String,
    #[serde(default)]
    Body: Option<String>,
}

/// Returns a ResponseInterceptResponse with a new Body, or None to leave the response untouched.
fn intercept_response(request: &[u8]) -> Option<Value> {
    let cfg = settings().read().ok()?.clone();
    // With no pools there is nothing to rename, and "pools only" would leave clients an empty list.
    if cfg.pools.is_empty() {
        return None;
    }
    let req: InterceptRequest = serde_json::from_slice(request).ok()?;
    // Model lists are the only responses CPA sends here without a model.
    if !req.Model.is_empty() || !req.RequestedModel.is_empty() {
        return None;
    }
    let raw = B64.decode(req.Body?.as_bytes()).ok()?;
    let mut body: Value = serde_json::from_slice(&raw).ok()?;
    if !clean_model_list(&mut body, &cfg) {
        return None;
    }
    let out = serde_json::to_vec(&body).ok()?;
    Some(json!({ "Body": B64.encode(out) }))
}

/// Rewrites pool entries in any of the model-list shapes CPA serves:
///   OpenAI/Claude  {"data":[{"id","display_name"?}]}
///   Codex clients  {"models":[{"slug","display_name"}]}
///   Gemini         {"models":[{"name":"models/<id>","displayName"}]}
/// With `pools_only`, every entry that isn't a pool is dropped.
fn clean_model_list(body: &mut Value, cfg: &Settings) -> bool {
    let mut changed = false;
    for key in ["data", "models"] {
        let Some(Value::Array(items)) = body.get_mut(key) else { continue };
        let before = items.len();
        items.retain(|item| {
            !cfg.pools_only || item.as_object().and_then(model_id).is_some_and(|id| cfg.pools.contains_key(&id.to_lowercase()))
        });
        changed |= items.len() != before;
        for item in items.iter_mut() {
            let Value::Object(obj) = item else { continue };
            let Some(id) = model_id(obj) else { continue };
            let Some(label) = cfg.pools.get(&id.to_lowercase()) else { continue };
            changed |= set_label(obj, label);
            if let Some(l) = cfg.limits.get(&id.to_lowercase()) {
                changed |= set_limits(obj, l);
            }
        }
    }
    changed
}

/// Writes a pool's limits into one model-list entry, in that list's own field names:
///   Codex clients  context_window, max_context_window, max_tokens, prompt_cache_ttl_seconds
///   Gemini         inputTokenLimit, outputTokenLimit
///   others         only limit fields already present (plain OpenAI lists carry none)
fn set_limits(obj: &mut Map<String, Value>, l: &Limits) -> bool {
    let mut changed = false;
    let mut put = |obj: &mut Map<String, Value>, key: &str, v: Option<u64>, add: bool| {
        let Some(v) = v else { return };
        if !add && !obj.contains_key(key) {
            return;
        }
        if obj.get(key).and_then(|x| x.as_u64()) != Some(v) {
            obj.insert(key.into(), json!(v));
            changed = true;
        }
    };
    if obj.contains_key("slug") {
        put(obj, "context_window", l.context, true);
        put(obj, "max_context_window", l.context, true);
        put(obj, "max_tokens", l.output, true);
        put(obj, "prompt_cache_ttl_seconds", l.cache_ttl, true);
    } else if obj.get("name").and_then(|v| v.as_str()).is_some_and(|n| n.starts_with("models/")) {
        put(obj, "inputTokenLimit", l.context, true);
        put(obj, "outputTokenLimit", l.output, true);
    } else {
        for key in ["context_length", "max_context_length", "max_input_tokens", "context_window"] {
            put(obj, key, l.context, false);
        }
        for key in ["max_completion_tokens", "max_tokens", "max_output_tokens"] {
            put(obj, key, l.output, false);
        }
    }
    changed
}

fn model_id(obj: &Map<String, Value>) -> Option<String> {
    for key in ["slug", "id", "name"] {
        if let Some(Value::String(s)) = obj.get(key) {
            let s = s.strip_prefix("models/").unwrap_or(s);
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn set_label(obj: &mut Map<String, Value>, label: &str) -> bool {
    let mut changed = false;
    let mut has_display = false;
    for key in ["display_name", "displayName"] {
        if let Some(v) = obj.get_mut(key) {
            has_display = true;
            if v.as_str() != Some(label) {
                *v = Value::String(label.to_string());
                changed = true;
            }
        }
    }
    // Codex-style entries always carry a display name; add one if CPA left it out.
    if !has_display && obj.contains_key("slug") {
        obj.insert("display_name".into(), Value::String(label.to_string()));
        changed = true;
    }
    changed
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Settings {
        parse_settings(b"enabled: true\npools:\n  - zeus\n  - name: argon\n    label: Argon Pool\n")
    }

    #[test]
    fn parses_strings_and_objects() {
        let t = table();
        assert_eq!(t.pools.get("zeus").map(String::as_str), Some("zeus"));
        assert_eq!(t.pools.get("argon").map(String::as_str), Some("argon"));
        assert!(!t.pools_only);
        assert!(parse_settings(b"pools-only: true\npools: [zeus]\n").pools_only);
    }

    #[test]
    fn pools_only_hides_everything_else() {
        let mut t = table();
        t.pools_only = true;
        let mut codex = json!({"models":[{"slug":"zeus","display_name":"x (Devin)"},{"slug":"gpt-6"}]});
        assert!(clean_model_list(&mut codex, &t));
        assert_eq!(codex["models"].as_array().unwrap().len(), 1);
        assert_eq!(codex["models"][0]["display_name"], "zeus");
        let mut gemini = json!({"models":[{"name":"models/gemini-4"},{"name":"models/ARGON","displayName":"y"}],"nextPageToken":""});
        assert!(clean_model_list(&mut gemini, &t));
        assert_eq!(gemini["models"].as_array().unwrap().len(), 1);
        assert_eq!(gemini["models"][0]["displayName"], "argon");
        let mut openai = json!({"object":"list","data":[{"id":"zeus"},{"id":"claude-opus-5-5"},{"bogus":1}]});
        assert!(clean_model_list(&mut openai, &t));
        assert_eq!(openai["data"], json!([{"id":"zeus"}]));
    }

    #[test]
    fn cleans_codex_claude_and_gemini_lists() {
        let t = table();
        let mut codex = json!({"models":[{"slug":"zeus","display_name":"Claude Opus 5.5 (Devin)"},{"slug":"gpt-6","display_name":"GPT 6"}]});
        assert!(clean_model_list(&mut codex, &t));
        assert_eq!(codex["models"][0]["display_name"], "zeus");
        assert_eq!(codex["models"][1]["display_name"], "GPT 6");

        let mut claude = json!({"data":[{"id":"argon","display_name":"Gemini 4 Argon (Devin)","type":"model"}]});
        assert!(clean_model_list(&mut claude, &t));
        assert_eq!(claude["data"][0]["display_name"], "argon");

        let mut gemini = json!({"models":[{"name":"models/zeus","displayName":"Claude Opus 5.5"}]});
        assert!(clean_model_list(&mut gemini, &t));
        assert_eq!(gemini["models"][0]["displayName"], "zeus");

        let mut openai = json!({"object":"list","data":[{"id":"zeus","object":"model"}]});
        assert!(!clean_model_list(&mut openai, &t));
    }

    #[test]
    fn writes_pool_limits_into_model_lists() {
        let t = parse_settings(
            b"pools: [gpt-6.1-sol, subs/claude-opus-5-5]\nlimits:\n  gpt-6.1-sol: {context: 922000, output: 64000}\n  subs/claude-opus-5-5: {context: 1000000, output: 128000, cache-ttl: 3600}\n  bad: {context: -1}\n",
        );
        assert_eq!(t.limits.len(), 2);
        let mut codex = json!({"models":[
            {"slug":"gpt-6.1-sol","display_name":"GPT","context_window":500000,"max_context_window":872000,"max_tokens":128000},
            {"slug":"subs/claude-opus-5-5","display_name":"x"},
            {"slug":"swe-2","context_window":262000}]});
        assert!(clean_model_list(&mut codex, &t));
        let m = &codex["models"];
        assert_eq!((m[0]["context_window"].as_u64(), m[0]["max_context_window"].as_u64(), m[0]["max_tokens"].as_u64()), (Some(922000), Some(922000), Some(64000)));
        assert!(m[0].get("prompt_cache_ttl_seconds").is_none());
        assert_eq!(m[1]["prompt_cache_ttl_seconds"], 3600);
        assert_eq!(m[1]["max_tokens"], 128000);
        assert_eq!(m[2]["context_window"], 262000);
        let mut gemini = json!({"models":[{"name":"models/gpt-6.1-sol","displayName":"gpt-6.1-sol","inputTokenLimit":1000000}]});
        assert!(clean_model_list(&mut gemini, &t));
        assert_eq!((gemini["models"][0]["inputTokenLimit"].as_u64(), gemini["models"][0]["outputTokenLimit"].as_u64()), (Some(922000), Some(64000)));
        // plain OpenAI entries have no limit fields: nothing is added
        let mut openai = json!({"data":[{"id":"gpt-6.1-sol","object":"model"}]});
        assert!(!clean_model_list(&mut openai, &t));
        assert_eq!(openai["data"][0], json!({"id":"gpt-6.1-sol","object":"model"}));
    }

    fn detail(input: i64, read: i64, write: i64, output: i64, total: i64) -> UsageDetail {
        UsageDetail { InputTokens: input, OutputTokens: output, CachedTokens: read, CacheReadTokens: read, CacheCreationTokens: write, TotalTokens: total, ..UsageDetail::default() }
    }

    #[test]
    fn counts_prompt_tokens_both_ways() {
        // Anthropic: input excludes the cache (values as CPA records them for a Claude key)
        assert_eq!(prompt_tokens(&detail(100, 3000, 900, 20, 4020), "claude", "ClaudeExecutor"), (4000, 3000, 900));
        // OpenAI-compatible (also OpenRouter Claude) and Codex: input includes it
        assert_eq!(prompt_tokens(&detail(5000, 4000, 0, 20, 5020), "openai-compatible-openrouter", "OpenAICompatExecutor"), (5000, 4000, 0));
        assert_eq!(prompt_tokens(&detail(5000, 4000, 0, 20, 5020), "codex", "CodexExecutor"), (5000, 4000, 0));
        // unknown providers: the record's own numbers decide
        assert_eq!(prompt_tokens(&detail(100, 3000, 900, 20, 4020), "devin", "DevinExecutor"), (4000, 3000, 900));
        assert_eq!(prompt_tokens(&detail(5000, 1000, 0, 20, 5020), "devin", "DevinExecutor"), (5000, 1000, 0));
        assert_eq!(prompt_tokens(&detail(10, 3000, 0, 20, 0), "devin", ""), (3010, 3000, 0));
        assert_eq!(prompt_tokens(&detail(-1, 0, 0, 0, 0), "", ""), (0, 0, 0));
    }

    fn usage(pool: &str, index: &str, provider: &str, d: UsageDetail, failed: bool, generate: bool) -> Vec<u8> {
        json!({"Alias": pool, "Model": "m", "Provider": provider, "ExecutorType": "", "AuthID": format!("id-{index}"),
               "AuthIndex": index, "Failed": failed, "Generate": generate, "Source": "sk-secret", "APIKey": "sk-client",
               "Detail": {"InputTokens": d.InputTokens, "OutputTokens": d.OutputTokens, "CachedTokens": d.CachedTokens,
                          "CacheReadTokens": d.CacheReadTokens, "CacheCreationTokens": d.CacheCreationTokens, "TotalTokens": d.TotalTokens}})
        .to_string()
        .into_bytes()
    }

    #[test]
    fn records_pool_requests_per_member_and_window() {
        let pools: HashMap<String, String> = [("hits/zeus".to_string(), "hits/Zeus".to_string())].into();
        let t0 = 1_800_000_000;
        let claude = detail(100, 3000, 900, 20, 4020);
        // two hours ago: counts for the day, not the hour
        record_usage(&usage("hits/Zeus", "a1", "claude", claude, false, true), t0 - 7200, &pools);
        record_usage(&usage("hits/zeus", "a1", "claude", detail(100, 3900, 0, 20, 4020), false, true), t0 - 60, &pools);
        record_usage(&usage("hits/zeus", "b2", "openai-compatible-x", detail(5000, 0, 0, 20, 5020), false, true), t0 - 30, &pools);
        record_usage(&usage("hits/zeus", "b2", "openai-compatible-x", detail(0, 0, 0, 0, 0), true, true), t0, &pools);
        record_usage(&usage("not-a-pool", "a1", "claude", claude, false, true), t0, &pools);
        record_usage(&usage("hits/zeus", "a1", "claude", claude, false, false), t0, &pools); // token count only

        let snap = stats_snapshot(t0);
        assert!(snap["pools"].get("not-a-pool").is_none());
        let members = snap["pools"]["hits/zeus"]["members"].as_array().unwrap();
        assert_eq!(members.len(), 2);
        let a = &members[0];
        assert_eq!(a["auth_index"], "a1");
        assert_eq!(a["hour"], json!({"requests":1,"failed":0,"prompt_tokens":4000,"cache_read_tokens":3900,"cache_write_tokens":0}));
        assert_eq!((a["day"]["requests"].as_u64(), a["day"]["prompt_tokens"].as_u64()), (Some(2), Some(8000)));
        assert_eq!(a["all"]["cache_read_tokens"], 6900);
        let b = &members[1];
        assert_eq!(b["hour"], json!({"requests":2,"failed":1,"prompt_tokens":5000,"cache_read_tokens":0,"cache_write_tokens":0}));
        assert!(!snap.to_string().contains("sk-"), "never keeps keys");
        // two days later the slots are gone; the totals stay
        let later = stats_snapshot(t0 + 2 * 24 * 3600);
        let a = &later["pools"]["hits/zeus"]["members"][0];
        assert_eq!((a["day"]["requests"].as_u64(), a["all"]["requests"].as_u64()), (Some(0), Some(2)));
    }

    #[test]
    fn serves_stats_on_its_route_and_the_page_elsewhere() {
        let reg: Value = serde_json::from_slice(&handle("management.register", b"{}")).unwrap();
        assert_eq!(reg["result"]["routes"][0]["Path"], STATS_PATH);
        assert_eq!(registration()["capabilities"]["usage_plugin"], true);
        let req = json!({"Method":"GET","Path": STATS_PATH}).to_string();
        let out: Value = serde_json::from_slice(&handle("management.handle", req.as_bytes())).unwrap();
        let body: Value = serde_json::from_slice(&B64.decode(out["result"]["Body"].as_str().unwrap()).unwrap()).unwrap();
        assert!(body["since"].as_u64().is_some() && body["pools"].is_object());
        let req = json!({"Method":"GET","Path":"/v0/resource/plugins/cpa-pools-and-aliases/ui"}).to_string();
        let out: Value = serde_json::from_slice(&handle("management.handle", req.as_bytes())).unwrap();
        assert_eq!(out["result"]["Headers"]["content-type"][0], "text/html; charset=utf-8");
    }

    #[test]
    fn leaves_model_responses_alone() {
        settings().write().unwrap().pools.insert("zeus".into(), "zeus".into());
        let body = B64.encode(br#"{"models":[{"slug":"zeus","display_name":"x (Devin)"}]}"#);
        let req = json!({"Model":"zeus","RequestedModel":"zeus","Body":body}).to_string();
        assert!(intercept_response(req.as_bytes()).is_none());
        let req = json!({"Model":"","RequestedModel":"","Body":body}).to_string();
        let out = intercept_response(req.as_bytes()).expect("rewritten");
        let new_body: Value = serde_json::from_slice(&B64.decode(out["Body"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(new_body["models"][0]["display_name"], "zeus");
    }
}
