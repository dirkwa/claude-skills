//! Minimal Signal K WASM plugin: buffer ABI, config, a poll-driven delta,
//! and one HTML endpoint. Build: `npm run build` (target wasm32-wasip1).

use serde::Deserialize;
use std::cell::RefCell;

// The sk_* imports exist only inside the server. On the host target they are
// replaced by a fake that records emitted deltas, so `cargo test` links.
#[cfg(target_arch = "wasm32")]
mod host {
    #[link(wasm_import_module = "env")]
    extern "C" {
        fn sk_debug(ptr: *const u8, len: usize);
        fn sk_set_status(ptr: *const u8, len: usize);
        fn sk_set_error(ptr: *const u8, len: usize);
        // version: 1 = v1 data model, 2 = v2 event only
        fn sk_handle_message(ptr: *const u8, len: usize, version: i32);
    }

    pub fn debug(s: &str) {
        unsafe { sk_debug(s.as_ptr(), s.len()) }
    }
    pub fn set_status(s: &str) {
        unsafe { sk_set_status(s.as_ptr(), s.len()) }
    }
    pub fn set_error(s: &str) {
        unsafe { sk_set_error(s.as_ptr(), s.len()) }
    }
    pub fn emit_v1(delta: &str) {
        unsafe { sk_handle_message(delta.as_ptr(), delta.len(), 1) }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod host {
    use std::cell::RefCell;

    thread_local! {
        pub static EMITTED: RefCell<Vec<String>> = RefCell::new(Vec::new());
    }

    pub fn debug(_: &str) {}
    pub fn set_status(_: &str) {}
    pub fn set_error(_: &str) {}
    pub fn emit_v1(delta: &str) {
        EMITTED.with(|e| e.borrow_mut().push(delta.to_owned()))
    }
}

use host::{debug, emit_v1, set_error, set_status};

// The host allocates request/response buffers inside the module through
// these two exports, so they must exist and must agree with each other.
#[no_mangle]
pub extern "C" fn allocate(size: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(size);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

#[no_mangle]
pub extern "C" fn deallocate(ptr: *mut u8, size: usize) {
    unsafe { drop(Vec::from_raw_parts(ptr, 0, size)) }
}

fn write_out(s: &str, ptr: *mut u8, max: usize) -> i32 {
    let n = s.len().min(max);
    unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), ptr, n) }
    n as i32
}

fn read_in(ptr: *const u8, len: usize) -> String {
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

const DEFAULT_GREETING: &str = "Hello";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Config {
    greeting: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            greeting: DEFAULT_GREETING.to_owned(),
        }
    }
}

#[derive(Default)]
struct State {
    config: Config,
    ticks: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

// Metadata exports: the host reads each into an 8 KB buffer.
#[no_mangle]
pub extern "C" fn plugin_id(out: *mut u8, max: usize) -> i32 {
    write_out("my-wasm-plugin", out, max)
}

#[no_mangle]
pub extern "C" fn plugin_name(out: *mut u8, max: usize) -> i32 {
    write_out("My WASM Plugin", out, max)
}

#[no_mangle]
pub extern "C" fn plugin_schema(out: *mut u8, max: usize) -> i32 {
    let schema = format!(
        r#"{{"type":"object","properties":{{"greeting":{{"type":"string","title":"Greeting","default":"{DEFAULT_GREETING}"}}}}}}"#
    );
    write_out(&schema, out, max)
}

// Called again on every config save, on the same instance: reset state here.
#[no_mangle]
pub extern "C" fn plugin_start(cfg_ptr: *const u8, cfg_len: usize) -> i32 {
    let config: Config = match serde_json::from_str(&read_in(cfg_ptr, cfg_len)) {
        Ok(c) => c,
        Err(e) => {
            set_error(&format!("Invalid configuration: {e}"));
            return 1;
        }
    };
    STATE.with(|s| *s.borrow_mut() = State { config, ticks: 0 });
    set_status("Running");
    0
}

#[no_mangle]
pub extern "C" fn plugin_stop() -> i32 {
    debug("stopped");
    0
}

// Called every second while running.
#[no_mangle]
pub extern "C" fn poll() -> i32 {
    let ticks = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.ticks += 1;
        s.ticks
    });
    emit_v1(&format!(
        r#"{{"updates":[{{"values":[{{"path":"environment.myPlugin.ticks","value":{ticks}}}]}}]}}"#
    ));
    0
}

#[no_mangle]
pub extern "C" fn http_endpoints(out: *mut u8, max: usize) -> i32 {
    write_out(
        r#"[{"method":"GET","path":"/hello","handler":"handle_hello"}]"#,
        out,
        max,
    )
}

// HTTP handler: request context JSON in, response envelope JSON out.
// The response buffer is 64 KB including the envelope; a clipped envelope is
// invalid JSON, so oversized output is replaced by a complete error envelope.
#[no_mangle]
pub extern "C" fn handle_hello(
    req_ptr: *const u8,
    req_len: usize,
    resp_ptr: *mut u8,
    resp_max: usize,
) -> i32 {
    let _request = read_in(req_ptr, req_len);
    let (greeting, ticks) = STATE.with(|s| {
        let s = s.borrow();
        (s.config.greeting.clone(), s.ticks)
    });
    let html = format!(
        "<!doctype html><h1>{}</h1><p>ticks: {ticks}</p>",
        escape_html(&greeting)
    );
    let envelope = serde_json::json!({
        "statusCode": 200,
        "headers": { "Content-Type": "text/html; charset=utf-8" },
        "body": html
    });
    let out = envelope.to_string();
    if out.len() > resp_max {
        return write_out(RESPONSE_TOO_LARGE, resp_ptr, resp_max);
    }
    write_out(&out, resp_ptr, resp_max)
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const RESPONSE_TOO_LARGE: &str =
    r#"{"statusCode":500,"headers":{"Content-Type":"text/plain"},"body":"response exceeds the 64 KB buffer"}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_greeting_uses_default() {
        let cfg = b"{}";
        assert_eq!(plugin_start(cfg.as_ptr(), cfg.len()), 0);
        STATE.with(|s| assert_eq!(s.borrow().config.greeting, DEFAULT_GREETING));
    }

    #[test]
    fn poll_emits_ticks() {
        let cfg = br#"{"greeting":"Hi"}"#;
        assert_eq!(plugin_start(cfg.as_ptr(), cfg.len()), 0);
        poll();
        host::EMITTED.with(|e| assert!(e.borrow()[0].contains(r#""value":1"#)));
    }

    #[test]
    fn greeting_is_html_escaped() {
        assert_eq!(escape_html("<b>&</b>"), "&lt;b&gt;&amp;&lt;/b&gt;");
    }
}
