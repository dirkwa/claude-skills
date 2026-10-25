# Rust traps specific to SignalK WASM plugins

Not a Rust tutorial: only what is different because the plugin is a `wasm32-wasip1` cdylib
called synchronously from the server's event loop through `node:wasi`. Compile results and
sizes below were measured with rustc/cargo 1.97 (October 2026); runtime behaviour was checked
under `node:wasi` (Node 22, configured as the server configures it) and, where marked, on a
live signalk-server 2.35.1.

## 1. No async runtime, no blocking

The host calls your exports one at a time on the server's only JavaScript thread. Anything
that waits inside an export stalls every client of the server.

- **tokio**: `rt` (current-thread) compiles and `block_on` of a ready future returns, but
  `net` and `rt-multi-thread` refuse to build: `Only features sync,macros,io-util,rt,time are
  supported on wasm.` A `block_on` that actually waits blocks the server, so there is no
  useful way to run tokio-based code (bluer, most async clients) inside a plugin.
- **Threads**: `std::thread::spawn` compiles, then panics at runtime with
  `failed to spawn thread: … Unsupported`.
- **`std::net`**: `TcpStream::connect` and `UdpSocket::bind` compile and return
  `operation not supported on this platform`. Use the host's `sk_udp_*` / `sk_tcp_*`
  imports (`rawSockets` capability) instead.

The pattern that works is **poll-driven I/O**: open sockets in `plugin_start`, then drain
them without blocking from `poll` (every second) or `delta_handler`, and keep any protocol
state machine in your own state:

```rust
#[no_mangle]
pub extern "C" fn poll() -> i32 {
    let mut buf = [0u8; 2048]; // size for the largest datagram: the rest is dropped
    let mut addr = [0u8; 64];
    let mut port = [0u8; 2];
    let id = STATE.with(|s| s.borrow().socket);
    // recv returns 0 both for "nothing queued" and for an empty datagram, and -1 on
    // error, so loop on the queue length instead of on recv's result.
    while unsafe { sk_udp_pending(id) } > 0 {
        let n = unsafe { sk_udp_recv(id, buf.as_mut_ptr(), buf.len(), addr.as_mut_ptr(), port.as_mut_ptr()) };
        if n < 0 { break; }
        handle_datagram(&buf[..n as usize]);
    }
    0
}
```

## 2. State: `thread_local!` + `RefCell`, reset in `plugin_start`

There is one instance per plugin and it lives until the server stops. Keep state in a
`thread_local!` `RefCell` (or a `static` with interior mutability). Verified live on 2.35.1
with a probe plugin:

- State **survives** between HTTP requests, `poll` calls and PUTs.
- State **survives a config save** — that is `plugin_stop` + `plugin_start` on the same
  instance — so `plugin_start` must replace the whole state, not patch it.
- Disable + enable on 2.35.1 also reuses the instance. Only a server restart starts fresh.
  (Server master after 2.35.1, unreleased at the time of writing, loads a new instance on
  enable — see SKILL.md §3.)
- Nothing survives a restart. Persist small state with `sk_save_config` and read it back
  in `plugin_start`; WASI file writes are unreliable (SKILL.md §7).

## 3. Panics and traps

`wasm32-wasip1` is `panic = "abort"` by default (`rustc --print cfg` shows it; setting it in
the profile changes nothing — the binary is byte-for-byte the same size). A panic prints
the message to the server's stderr (`thread '<unnamed>' panicked at src/lib.rs:…`) and
then traps with `RuntimeError: unreachable`. Observed on 2.35.1:

| Where | What the server does |
|---|---|
| HTTP handler | `500 {"error":"unreachable"}`; the next request is served by the same instance |
| `poll` / `delta_handler` | Logged at debug level only (`poll() error: RuntimeError: unreachable`); called again next time; admin UI still shows the plugin as running |
| `plugin_start` | Plugin state becomes `error` |

**No unwinding means no destructors.** A panic while a `RefCell` is mutably borrowed leaves
the borrow flag set forever: every later `borrow_mut()` panics with
`RefCell already borrowed`, every second from `poll`, and neither a config save nor
disable + enable (2.35.1) recovers — only a restart. So: never `unwrap()` on input inside
a borrow, parse and validate *before* taking the borrow, keep borrows short, and return
an error status instead of panicking. The same goes for half-updated state: a trap in the
middle of a multi-step update leaves it half done.

## 4. Release profile and size

The binary ships in the npm tarball, and the server compiles the module (`WebAssembly.compile`)
on every load — on a Pi.
Measured on `starter/` (serde + serde_json):

| Profile | `plugin.wasm` |
|---|---|
| cargo's release default (`opt-level=3`, no LTO, `strip="debuginfo"`) | 185,212 B |
| `opt-level="s"`, `lto=false`, `strip=true` | 137,177 B |
| `opt-level="s"`, `lto=true`, `strip=false` | 343,218 B |
| **`opt-level="s"`, `lto=true`, `strip=true`** (the starter) | **91,849 B** |
| `opt-level="z"`, `lto=true`, `strip=true` | 90,866 B |
| … plus `codegen-units=1` | 90,115 B |
| starter + `wasm-opt -Oz` (binaryen 132) | 80,394 B |

`lto` and `strip` matter; `s` vs `z` barely does. `wasm-opt` saves another ~12 %, but it
rejects rustc's output unless you enable the features the target uses (`memory.copy
operations require bulk memory operations`):

```sh
wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int \
  --enable-sign-ext --enable-mutable-globals plugin.wasm -o plugin.wasm
```

The optimized starter loaded and ran unchanged on 2.35.1.

## 5. Which crates work

Each probe was a cdylib calling the crate for real, built with the starter's profile;
sizes are the whole module.

| Crate | Builds | Runs | Size | Notes |
|---|---|---|---|---|
| `serde_json` | yes | yes (live) | starter 92 KB | |
| `chrono` 0.4 | yes | yes, `Utc::now()` via WASI clock | 52 KB | `default-features = false, features = ["alloc"]` (no clock): 30 KB |
| `regex` 1 | yes | yes | **932 KB** | `default-features = false, features = ["std"]`: 308 KB; `regex-lite`: 99 KB |
| `tokio` `rt` | yes | ready futures only | 82 KB | `net`, `rt-multi-thread`: compile error |
| `reqwest` 0.12 | yes | **no** | 252 KB | Picks its browser backend (wasm-bindgen); first request panics `cannot call wasm-bindgen imported functions on non-wasm targets`. `blocking` doesn't exist on wasm32 |
| `ureq` 2 | yes | **no** | 1 MB | Every request: `operation not supported on this platform` |
| `bluer` 0.17 | **no** | — | — | `libdbus-sys` build script fails; there is no D-Bus or Unix socket in WASI |

"Builds" is not "works": the dangerous ones compile and fail at runtime. A crate is safe
when it is pure computation (parsers, math, serde) or uses only WASI clock, random and
stdio. Anything that opens sockets, spawns threads or binds to JavaScript (wasm-bindgen,
`js-sys`, `web-sys`) does not work. There is no HTTP client for
Rust plugins; use `sk_tcp_*` or move the HTTP part to a Node plugin.

## 6. Host-side tests

Declared as plain `extern "C"`, the `sk_*` imports make `cargo test` on the host target fail
to link — `undefined symbol: sk_handle_message` — even for a test that never calls them,
because the `#[no_mangle]` exports keep every import referenced. `starter/` therefore puts
all of its imports in a `cfg`-gated `host` module, with a recording fake on the host:

```rust
#[cfg(target_arch = "wasm32")]
mod host {
    #[link(wasm_import_module = "env")]
    extern "C" {
        fn sk_handle_message(ptr: *const u8, len: usize, version: i32);
        // … every other sk_* import the plugin uses
    }
    pub fn emit_v1(d: &str) { unsafe { sk_handle_message(d.as_ptr(), d.len(), 1) } }
}

#[cfg(not(target_arch = "wasm32"))]
mod host {
    use std::cell::RefCell;
    thread_local! { pub static EMITTED: RefCell<Vec<String>> = RefCell::new(Vec::new()); }
    pub fn emit_v1(d: &str) { EMITTED.with(|e| e.borrow_mut().push(d.to_owned())) }
    // … a no-op or recording fake for every other import
}
```

Every import needs both halves; one left outside the `cfg` breaks the host link again.
Verified on the starter: `cargo test` runs its three tests (default config, emitted delta,
HTML escaping), and the `wasm32-wasip1` build imports exactly the four `sk_*` functions it
uses. Keep protocol logic in ordinary modules with ordinary tests; the exports then only
parse input, call the logic and serialize output.
