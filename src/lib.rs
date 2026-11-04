//! cpa-pools-and-aliases — CLIProxyAPI plugin (C ABI v1, JSON envelopes).
//!
//! 1. Serves the "Pools and Aliases" management page (a menu resource). The page edits CPA's
//!    native model aliases through the regular /v8/management API; the plugin never routes.
//! 2. Cleans pool entries in model-list responses (/v1/models for OpenAI, Claude, Gemini and
//!    Codex-style clients): a pool is one name over many accounts, so its display name must not
//!    carry whichever member's provider/harness label CPA happened to pick, e.g.
//!    "Claude Opus 5.5 (Devin)". Pool entries get the pool's own label instead.
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

/// lower-cased pool name -> label shown to clients
fn pools() -> &'static RwLock<HashMap<String, String>> {
    static POOLS: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    POOLS.get_or_init(|| RwLock::new(HashMap::new()))
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

/// Accepts both `pools: [zeus, ...]` and `pools: [{name: zeus, label: Zeus}, ...]`.
fn parse_pools(yaml: &[u8]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let doc: serde_yaml::Value = match serde_yaml::from_slice(yaml) {
        Ok(v) => v,
        Err(_) => return out,
    };
    let list = match doc.get("pools").and_then(|p| p.as_sequence()) {
        Some(l) => l,
        None => return out,
    };
    for item in list {
        let (name, label) = match item {
            serde_yaml::Value::String(s) => (s.trim().to_string(), String::new()),
            serde_yaml::Value::Mapping(_) => (
                item.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
                item.get("label").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
            ),
            _ => continue,
        };
        if name.is_empty() {
            continue;
        }
        let label = if label.is_empty() { name.clone() } else { label };
        out.insert(name.to_lowercase(), label);
    }
    out
}

fn apply_config(request: &[u8]) {
    let req: LifecycleRequest = serde_json::from_slice(request).unwrap_or_default();
    let yaml = req.config_yaml.and_then(|s| B64.decode(s.as_bytes()).ok()).unwrap_or_default();
    let parsed = parse_pools(&yaml);
    if let Ok(mut guard) = pools().write() {
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
    let table = pools().read().ok()?.clone();
    if table.is_empty() {
        return None;
    }
    let req: InterceptRequest = serde_json::from_slice(request).ok()?;
    // Model lists are the only responses CPA sends here without a model.
    if !req.Model.is_empty() || !req.RequestedModel.is_empty() {
        return None;
    }
    let raw = B64.decode(req.Body?.as_bytes()).ok()?;
    let mut body: Value = serde_json::from_slice(&raw).ok()?;
    if !clean_model_list(&mut body, &table) {
        return None;
    }
    let out = serde_json::to_vec(&body).ok()?;
    Some(json!({ "Body": B64.encode(out) }))
}

/// Rewrites pool entries in any of the model-list shapes CPA serves:
///   OpenAI/Claude  {"data":[{"id","display_name"?}]}
///   Codex clients  {"models":[{"slug","display_name"}]}
///   Gemini         {"models":[{"name":"models/<id>","displayName"}]}
fn clean_model_list(body: &mut Value, table: &HashMap<String, String>) -> bool {
    let mut changed = false;
    for key in ["data", "models"] {
        let Some(Value::Array(items)) = body.get_mut(key) else { continue };
        for item in items.iter_mut() {
            let Value::Object(obj) = item else { continue };
            let Some(id) = model_id(obj) else { continue };
            let Some(label) = table.get(&id.to_lowercase()) else { continue };
            changed |= set_label(obj, label);
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

    fn table() -> HashMap<String, String> {
        parse_pools(b"enabled: true\npools:\n  - zeus\n  - name: argon\n    label: Argon Pool\n")
    }

    #[test]
    fn parses_strings_and_objects() {
        let t = table();
        assert_eq!(t.get("zeus").map(String::as_str), Some("zeus"));
        assert_eq!(t.get("argon").map(String::as_str), Some("Argon Pool"));
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
        assert_eq!(claude["data"][0]["display_name"], "Argon Pool");

        let mut gemini = json!({"models":[{"name":"models/zeus","displayName":"Claude Opus 5.5"}]});
        assert!(clean_model_list(&mut gemini, &t));
        assert_eq!(gemini["models"][0]["displayName"], "zeus");

        let mut openai = json!({"object":"list","data":[{"id":"zeus","object":"model"}]});
        assert!(!clean_model_list(&mut openai, &t));
    }

    #[test]
    fn leaves_model_responses_alone() {
        pools().write().unwrap().insert("zeus".into(), "zeus".into());
        let body = B64.encode(br#"{"models":[{"slug":"zeus","display_name":"x (Devin)"}]}"#);
        let req = json!({"Model":"zeus","RequestedModel":"zeus","Body":body}).to_string();
        assert!(intercept_response(req.as_bytes()).is_none());
        let req = json!({"Model":"","RequestedModel":"","Body":body}).to_string();
        let out = intercept_response(req.as_bytes()).expect("rewritten");
        let new_body: Value = serde_json::from_slice(&B64.decode(out["Body"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(new_body["models"][0]["display_name"], "zeus");
    }
}
