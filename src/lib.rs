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
//! Pools come from the plugin's own config (plugins.configs.cpa-pools-and-aliases.pools), which
//! CPA hands to the plugin on plugin.register / plugin.reconfigure.

use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::{OnceLock, RwLock};

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
            ok_envelope(json!({
                "schema_version": 1,
                "metadata": {
                    "Name": PLUGIN_NAME, "Version": VERSION, "Author": "ngvoicu",
                    "GitHubRepository": REPO, "Logo": LOGO, "ConfigFields": []
                },
                "capabilities": { "management_api": true, "response_interceptor": true }
            }))
        }
        "management.register" => ok_envelope(json!({
            "resources": [{
                "Path": "/ui",
                "Menu": PLUGIN_NAME,
                "Description": "Create model pools from any subscription or API key."
            }]
        })),
        "management.handle" => {
            // Only one resource (/ui) is registered, so every call serves the page.
            let prefix = r#"{"ok":true,"result":{"StatusCode":200,"Headers":{"content-type":["text/html; charset=utf-8"],"cache-control":["no-store"],"x-content-type-options":["nosniff"]},"Body":""#;
            let mut out = Vec::with_capacity(prefix.len() + page_b64().len() + 3);
            out.extend_from_slice(prefix.as_bytes());
            out.extend_from_slice(page_b64().as_bytes());
            out.extend_from_slice(br#""}}"#);
            out
        }
        "response.intercept_after" => ok_envelope(intercept_response(request).unwrap_or_else(|| json!({}))),
        "plugin.quiesce" | "plugin.shutdown" => ok_envelope(json!({})),
        _ => error_envelope("unknown_method", &format!("{PLUGIN_ID}: unknown method {method}")),
    }
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
