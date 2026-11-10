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
use std::io::Write;
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
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

unsafe extern "C" fn plugin_shutdown() {
    let _ = catch_unwind(|| save_stats(now_secs(), true));
}

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
    /// where the cache-hit counts are kept across restarts; unset = memory only
    stats_file: Option<PathBuf>,
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
        stats_file: parse_stats_file(&doc),
    }
}

/// `stats-file: "~/.cpa-pools/cache-stats.json"`. A leading `~/` is the home directory of the user CPA runs as.
fn parse_stats_file(doc: &serde_yaml::Value) -> Option<PathBuf> {
    let raw = doc.get("stats-file")?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(expand_home(raw, std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))))
}

fn expand_home(raw: &str, home: Option<std::ffi::OsString>) -> PathBuf {
    let rest = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\"));
    match (rest, home.filter(|h| !h.is_empty())) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(raw),
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
    let stats_file = parsed.stats_file.clone();
    if let Ok(mut guard) = settings().write() {
        *guard = parsed;
    }
    attach_stats_file(stats_file, now_secs());
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
            save_stats(now_secs(), false);
            ok_envelope(json!({}))
        }
        "response.intercept_after" => ok_envelope(intercept_response(request).unwrap_or_else(|| json!({}))),
        "plugin.quiesce" | "plugin.shutdown" => {
            save_stats(now_secs(), true);
            ok_envelope(json!({}))
        }
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

#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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

#[derive(Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct MemberStats {
    auth_id: String,
    provider: String,
    model: String,
    last_seen: u64,
    slots: VecDeque<(u64, Counter)>,
    total: Counter,
}

type PoolStats = HashMap<String, HashMap<String, MemberStats>>;

#[derive(Default)]
struct Stats {
    since: u64,
    /// lower-cased pool id -> member key (auth index, else auth id) -> counters
    pools: PoolStats,
    /// the configured `stats-file`
    file: Option<PathBuf>,
    /// counts changed since the last save
    dirty: bool,
    /// last save attempt and last save that worked (0 = none)
    save_tried: u64,
    saved: u64,
    file_error: Option<String>,
}

fn stats() -> &'static Mutex<Stats> {
    static STATS: OnceLock<Mutex<Stats>> = OnceLock::new();
    STATS.get_or_init(|| Mutex::new(Stats { since: now_secs(), ..Stats::default() }))
}

// ------------------------------------------------------------------ keeping the counts across restarts

/// The counts are written at most this often while requests arrive, and once more when the plugin stops.
const SAVE_EVERY_SECS: u64 = 60;
const STATS_FILE_VERSION: u32 = 1;
const RELATIVE_PATH: &str = "stats-file must be an absolute path or start with ~/";

#[derive(Serialize)]
struct SavedStatsRef<'a> {
    version: u32,
    since: u64,
    saved: u64,
    pools: &'a PoolStats,
}

#[derive(Deserialize)]
struct SavedStats {
    version: u32,
    since: u64,
    #[serde(default)]
    pools: PoolStats,
}

fn encode_stats(since: u64, pools: &PoolStats, now: u64) -> Vec<u8> {
    serde_json::to_vec(&SavedStatsRef { version: STATS_FILE_VERSION, since, saved: now, pools }).unwrap_or_default()
}

/// Reads a saved file: when counting started and the counters, without the slots older than a day.
fn decode_stats(bytes: &[u8], now: u64) -> Option<(u64, PoolStats)> {
    let mut saved: SavedStats = serde_json::from_slice(bytes).ok()?;
    if saved.version != STATS_FILE_VERSION {
        return None;
    }
    let slot = now / SLOT_SECS;
    for members in saved.pools.values_mut() {
        for m in members.values_mut() {
            while m.slots.front().is_some_and(|(s, _)| *s + KEEP_SLOTS <= slot) {
                m.slots.pop_front();
            }
        }
    }
    let since = if saved.since == 0 || saved.since > now { now } else { saved.since };
    Some((since, saved.pools))
}

/// Writes next to the target and renames, so a crash never leaves half a file. Owner-only on Unix.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Called on every (re)configure. A newly set file is read once, when nothing has been counted yet; counts already
/// in memory win over the file and are written to it on the next save.
fn attach_stats_file(path: Option<PathBuf>, now: u64) {
    let Ok(mut st) = stats().lock() else { return };
    attach_stats_file_to(&mut st, path, now);
}

fn attach_stats_file_to(st: &mut Stats, path: Option<PathBuf>, now: u64) {
    if st.file == path {
        return;
    }
    st.file = path.clone();
    st.file_error = None;
    st.saved = 0;
    st.save_tried = 0;
    let Some(path) = path else { return };
    if !path.is_absolute() {
        st.file_error = Some(RELATIVE_PATH.to_string());
        return;
    }
    if !st.pools.is_empty() {
        st.dirty = true;
        return;
    }
    match std::fs::read(&path) {
        Ok(bytes) => match decode_stats(&bytes, now) {
            Some((since, pools)) => {
                st.since = since;
                st.pools = pools;
            }
            None => {
                // Not ours or damaged: keep it aside instead of overwriting it.
                let mut aside = path.as_os_str().to_owned();
                aside.push(".unreadable");
                let _ = std::fs::rename(&path, PathBuf::from(aside));
                st.file_error = Some("the saved counts could not be read; counting starts again".to_string());
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => st.file_error = Some(format!("could not read the saved counts: {e}")),
    }
}

/// Saves the counts when they changed and the last save is old enough (`force`: now, whatever its age).
fn save_stats(now: u64, force: bool) {
    let due = |st: &Stats| {
        st.dirty && st.file.as_ref().is_some_and(|p| p.is_absolute()) && (force || now.saturating_sub(st.save_tried) >= SAVE_EVERY_SECS)
    };
    match stats().lock() {
        Ok(st) if due(&st) => {}
        _ => return,
    }
    // One writer at a time, and the bytes are taken after waiting for it, so an older snapshot never lands last.
    static WRITER: Mutex<()> = Mutex::new(());
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    let (path, bytes) = {
        let Ok(mut st) = stats().lock() else { return };
        if !due(&st) {
            return;
        }
        let Some(path) = st.file.clone() else { return };
        st.save_tried = now;
        st.dirty = false;
        (path, encode_stats(st.since, &st.pools, now))
    };
    let result = write_atomic(&path, &bytes);
    let Ok(mut st) = stats().lock() else { return };
    if st.file.as_deref() != Some(path.as_path()) {
        return; // the file was changed meanwhile
    }
    match result {
        Ok(()) => {
            st.saved = now;
            st.file_error = None;
        }
        Err(e) => {
            st.dirty = true;
            st.file_error = Some(format!("could not save the counts: {e}"));
        }
    }
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
    let Ok(mut guard) = stats().lock() else { return };
    let st = &mut *guard;
    let m = st.pools.entry(pool).or_default().entry(key).or_default();
    m.auth_id = r.AuthID.trim().to_string();
    m.provider = r.Provider.trim().to_string();
    m.model = r.Model.trim().to_string();
    m.last_seen = now;
    m.total.add(&c);
    st.dirty = true;
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

/// Counters per pool and member for the last hour, the last day and since counting started.
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
    // `file`: null when the counts live in memory only; else where they are kept, the last save and any problem.
    let file = st.file.as_ref().map(|p| {
        json!({ "path": p.to_string_lossy(), "saved": if st.saved > 0 { json!(st.saved) } else { Value::Null }, "error": st.file_error })
    });
    json!({ "since": st.since, "now": now, "slot_seconds": SLOT_SECS, "file": file, "pools": pools })
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
    fn reads_the_stats_file_setting() {
        assert_eq!(parse_settings(b"pools: [zeus]\n").stats_file, None);
        assert_eq!(parse_settings(b"stats-file: '  '\n").stats_file, None);
        assert_eq!(parse_settings(b"stats-file: /var/lib/cpa/hits.json\n").stats_file, Some(PathBuf::from("/var/lib/cpa/hits.json")));
        assert_eq!(expand_home("~/x/hits.json", Some("/home/u".into())), PathBuf::from("/home/u/x/hits.json"));
        assert_eq!(expand_home("~/x/hits.json", None), PathBuf::from("~/x/hits.json"));
        assert_eq!(expand_home("/abs/~/hits.json", Some("/home/u".into())), PathBuf::from("/abs/~/hits.json"));
    }

    fn member(slots: &[(u64, u64)]) -> MemberStats {
        let mut m = MemberStats { auth_id: "id-a1".into(), provider: "claude".into(), model: "m".into(), ..MemberStats::default() };
        for (slot, prompt) in slots {
            let c = Counter { requests: 1, prompt: *prompt, cache_read: prompt / 2, ..Counter::default() };
            m.slots.push_back((*slot, c));
            m.total.add(&c);
            m.last_seen = slot * SLOT_SECS;
        }
        m
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cpa-pools-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn saved_counts_survive_a_restart() {
        let now = 1_800_000_000;
        let slot = now / SLOT_SECS;
        let mut pools: PoolStats = HashMap::new();
        pools.entry("zeus".into()).or_default().insert("a1".into(), member(&[(slot - KEEP_SLOTS - 5, 1000), (slot - 3, 4000), (slot, 6000)]));
        let bytes = encode_stats(now - 9000, &pools, now);
        assert!(!String::from_utf8_lossy(&bytes).contains("sk-"));

        // read back at once: everything is there
        let (since, back) = decode_stats(&bytes, now).expect("decodes");
        assert_eq!(since, now - 9000);
        let m = &back["zeus"]["a1"];
        assert_eq!((m.slots.len(), m.total.requests, m.total.prompt, m.total.cache_read), (2, 3, 11000, 5500));
        assert_eq!((m.auth_id.as_str(), m.provider.as_str(), m.last_seen), ("id-a1", "claude", slot * SLOT_SECS));

        // read back two days later: the slots are gone, the totals and the start stay
        let (since, back) = decode_stats(&bytes, now + 2 * 24 * 3600).expect("decodes");
        assert_eq!((since, back["zeus"]["a1"].slots.len(), back["zeus"]["a1"].total.requests), (now - 9000, 0, 3));

        // a start in the future (clock moved back) falls back to now; other formats are refused
        assert_eq!(decode_stats(&encode_stats(now + 50, &pools, now), now).map(|x| x.0), Some(now));
        assert!(decode_stats(br#"{"version":2,"since":1,"pools":{}}"#, now).is_none());
        assert!(decode_stats(b"not json", now).is_none());
    }

    #[test]
    fn restores_from_the_file_only_when_nothing_was_counted() {
        let now = 1_800_000_000;
        let dir = temp_dir("restore");
        let path = dir.join("nested").join("hits.json");
        let mut pools: PoolStats = HashMap::new();
        pools.entry("zeus".into()).or_default().insert("a1".into(), member(&[(now / SLOT_SECS, 4000)]));
        write_atomic(&path, &encode_stats(now - 500, &pools, now)).expect("writes, creating the folders");
        assert!(!dir.join("nested").join("hits.json.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }

        // an empty plugin takes the file's counts and its start
        let mut st = Stats { since: now, ..Stats::default() };
        attach_stats_file_to(&mut st, Some(path.clone()), now);
        assert_eq!((st.since, st.pools["zeus"]["a1"].total.prompt, st.dirty, st.file_error.clone()), (now - 500, 4000, false, None));

        // counts already in memory win, and are due for saving
        let mut busy = Stats { since: now, ..Stats::default() };
        busy.pools.entry("argon".into()).or_default().insert("b2".into(), member(&[(now / SLOT_SECS, 10)]));
        attach_stats_file_to(&mut busy, Some(path.clone()), now);
        assert!(busy.pools.contains_key("argon") && !busy.pools.contains_key("zeus") && busy.dirty);

        // a relative path is refused with a message; a missing file is fine; a damaged one is set aside
        let mut rel = Stats::default();
        attach_stats_file_to(&mut rel, Some(PathBuf::from("hits.json")), now);
        assert_eq!(rel.file_error.as_deref(), Some(RELATIVE_PATH));
        let mut fresh = Stats::default();
        attach_stats_file_to(&mut fresh, Some(dir.join("none.json")), now);
        assert_eq!((fresh.file_error.clone(), fresh.pools.len()), (None, 0));
        let bad = dir.join("bad.json");
        std::fs::write(&bad, b"{ half a file").unwrap();
        let mut damaged = Stats::default();
        attach_stats_file_to(&mut damaged, Some(bad.clone()), now);
        assert!(damaged.file_error.is_some() && !bad.exists() && dir.join("bad.json.unreadable").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_through_the_plugin_calls() {
        let dir = temp_dir("calls");
        let path = dir.join("hits.json");
        // "zeus" stays a pool throughout: other tests share the plugin's settings
        let cfg = |file: Option<&Path>| {
            let mut yaml = "pools: [zeus, saved/zeus]\n".to_string();
            if let Some(f) = file {
                yaml.push_str(&format!("stats-file: {}\n", f.display()));
            }
            json!({ "config_yaml": B64.encode(yaml) }).to_string()
        };
        handle("plugin.register", cfg(Some(&path)).as_bytes());
        handle("usage.handle", &usage("saved/zeus", "s1", "claude", detail(100, 3000, 900, 20, 4020), false, true));
        handle("plugin.shutdown", b"{}");
        let (_, pools) = decode_stats(&std::fs::read(&path).expect("saved when the plugin stops"), now_secs()).expect("decodes");
        assert_eq!(pools["saved/zeus"]["s1"].total, Counter { requests: 1, failed: 0, prompt: 4000, cache_read: 3000, cache_write: 900 });
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("sk-"), "never keeps keys");
        let snap = stats_snapshot(now_secs());
        assert_eq!(snap["file"]["path"], path.to_string_lossy().as_ref());
        assert!(snap["file"]["saved"].as_u64().is_some() && snap["file"]["error"].is_null());
        // without the setting the counts stay in memory and nothing more is written
        handle("plugin.reconfigure", cfg(None).as_bytes());
        assert!(stats_snapshot(now_secs())["file"].is_null());
        std::fs::remove_file(&path).unwrap();
        handle("usage.handle", &usage("saved/zeus", "s1", "claude", detail(100, 3000, 900, 20, 4020), false, true));
        handle("plugin.shutdown", b"{}");
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
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
        assert!(body.get("file").is_some());
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
