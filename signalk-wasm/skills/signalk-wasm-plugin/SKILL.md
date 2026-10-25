---
name: signalk-wasm-plugin
description: Use when writing, debugging or publishing a WebAssembly plugin for SignalK server (Rust wasm32-wasip1 or AssemblyScript, wasmManifest/wasmCapabilities in package.json; runs in Node's WASI with only the sk_* host imports — isolated, not a security boundary), or when deciding between a WASM plugin, a Node plugin and a native process (async Rust such as tokio or bluer cannot run inside one) — the buffer ABI and required exports, which capabilities are actually enforced, HTTP endpoints and serving a UI (admin-only routes, 64 KB Rust responses, the signalk-webapp mount, a Datastar/htmx recipe), deltas, providers, sockets, Rust traps on wasm32-wasip1 (panics, state, size, which crates work), Rust and AssemblyScript starters, and a version matrix of the runtime limits and fixes the official docs don't mention.
---

# Write a SignalK WASM plugin

A SignalK **WASM plugin** is an npm package whose plugin logic is a `.wasm` binary, run by the
server in Node's built-in WASI (`node:wasi`, preview1) instead of being `require()`d. It talks
to the server only through `env.sk_*` host imports, so it gets no `app` object, no npm
dependencies at runtime and no install scripts. That makes it a good fit for self-contained
logic: protocol decoders, calculations, device drivers over UDP/TCP, and providers.

That is **isolation, not a sandbox**. Node documents `node:wasi` as unsuitable for running
untrusted code, and the `network`/`rawSockets` capabilities grant host access by design; the
server docs dropped their sandbox claim in 2.35.0 (server PR #3207). Treat a WASM plugin with
the same trust as a Node plugin.

It is **not** the only way to use WebAssembly in SignalK. If you need the full
`@signalk/server-api` (`app.*`, Express routes with read-only access, serial ports, the NMEA
event bus) and only want WASM for speed or for reusing Rust/C code, write a normal Node plugin
and load a wasm-pack module inside it — the sibling skill **`signalk-wasm-library`** covers
that. Decide first:

| You need… | Use |
|---|---|
| Self-contained synchronous logic in Rust/AssemblyScript, one binary for every arch, no `app` | WASM plugin (this skill) |
| `app.*` APIs — BLE via `app.bleApi` (server ≥ 2.31.0), serial, CAN, `nmea0183`/`nmea2000JsonOut` events — read-only plugin routes, streaming responses | Node plugin, optionally + a WASM library |
| An async Rust stack you already have (tokio, bluer, reqwest) | Native process sending deltas over the SignalK WebSocket |
| A UI for a cockpit display | A static webapp (`signalk-webapp`) reading the server's own APIs — see §6 |

**Async Rust does not port into a WASM plugin.** `wasm32-wasip1` has no threads and no Unix
sockets (so no D-Bus/BlueZ), tokio's `net` feature doesn't compile for it, and a `block_on`
inside an export blocks the server's event loop — measured details in
[rust-patterns.md](rust-patterns.md). For a BLE sensor the least code is a small Node plugin
on `app.bleApi.subscribeGATT()`: the server owns the adapter (local BlueZ or ESP32 gateways)
and calls you back per notification. To keep an existing tokio/bluer program, run it as a
separate process that sends deltas to `/signalk/v1/stream` (under security it needs a token
with write access) — and don't let the server manage the same adapter.

Two starters next to this file are complete plugins (config, `poll`-driven delta, one HTML
endpoint), both verified on a live 2.35.1 server: `starter/` (Rust, 92 KB, with host-side
`cargo test`) and `starter-assemblyscript/` (25 KB). Copy one, rename, `npm install && npm run build`.

## 1. package.json: what makes it a WASM plugin

```json
{
  "name": "my-wasm-plugin",
  "keywords": ["signalk-wasm-plugin"],
  "wasmManifest": "plugin.wasm",
  "wasmCapabilities": { "dataRead": true, "dataWrite": true },
  "files": ["plugin.wasm", "public/"]
}
```

- **`wasmManifest`** is what routes the package to the WASM loader (not the keyword). The
  keyword `signalk-wasm-plugin` is what gets it *discovered* in `node_modules`.
- **`wasmCapabilities`** is optional; `dataRead`/`dataWrite` default to `true`, everything else
  to `false` (`storage` defaults to `"vfs-only"`).
- **The plugin ID comes from the package name**, never from your `plugin_id` export:
  `@` and `/` become `_`, so `@me/foo` → `_me_foo`. Config lives at
  `<configdir>/plugin-config-data/<id>.json`.
- **Scoped package names need server ≥ 2.34.0.** Before 2.34.0 the host bindings were keyed
  by the package name while the server uses the derived ID, so with `@scope/x`
  `sk_set_status` never reached the admin UI, `sk_read_config` returned `{}`,
  `sk_save_config` failed and sockets weren't closed on stop (status and config loss
  reproduced on 2.33.0). Fixed in
  2.34.0 (server PR #3172; status verified live on 2.35.1). What clients see keeps the
  package name: `$source`, PUT source, provider and binary-stream ids are `@scope/x`.
  Publishing for older servers too? Stay unscoped.
- **App Store visibility.** The App Store searches npm for `signalk-node-server-plugin`
  (and the webapp keywords) only — a package with just `signalk-wasm-plugin` never shows up
  there, though `npm install` into the server dir still works. Adding both keywords makes it
  listed; the loader de-duplicates and still loads it as WASM because `wasmManifest` wins.
  The cost: a server too old to know WASM would try to `require()` it. The official docs say
  not to add the Node keyword — make the trade-off consciously.
- The plugin is **disabled until enabled** in Plugin Config; there is no enabled-by-default.

## 2. Language and toolchain

- **Rust** (recommended): `crate-type = ["cdylib"]`, `cargo build --release --target
  wasm32-wasip1` (not `wasm32-wasi`, not `wasm32-unknown-unknown`, not a component).
  `lto = true` + `strip = true` + `opt-level = "s"` keep the starter at 92 KB with
  serde_json, half of cargo's release default (`panic = "abort"` is already the target's
  default, so the starter doesn't set it). Sizes, `wasm-opt`, panics, state and which crates work:
  [rust-patterns.md](rust-patterns.md).
- **AssemblyScript**: `@signalk/assemblyscript-plugin-sdk` (0.3.0) + `assemblyscript`
  (0.28.20 works, though the SDK was built against 0.27); `asconfig.json` must set
  `"exportRuntime": true` (the host reads strings with `__getString`). Pin the SDK as
  `^0.3.0` — the repo examples still say `^0.2.0` (2.35.1 and master), which excludes 0.3
  under caret-on-0.x rules. Both are build-time only, so they belong in `devDependencies`.
  `plugin_start` gets the config as UTF-8 bytes (`String.UTF8.decodeUnsafe(ptr, len)`), HTTP
  and PUT handlers get managed strings (declare the parameter as `string`) — see
  `starter-assemblyscript/`. For JSON output write your own string escaping: the SDK's
  bundled `assemblyscript-json` escapes control characters as a bare backslash, which is
  invalid JSON for a body containing a newline.
- **Go/TinyGo**: a docs page exists, but there is no TinyGo example and no loader branch for
  it; treat as unsupported. **Component Model / .NET** was removed (server PR #2258);
  component binaries are rejected at load.

## 3. The Rust ABI (exports the host calls)

The host decides the flavour from exports: `plugin_id` **and** `allocate` ⇒ Rust buffer ABI.

| Export | Signature | Notes |
|---|---|---|
| `allocate` / `deallocate` | `(size) -> ptr` / `(ptr, size)` | **Required.** The host allocates every request/response buffer in your memory. |
| `plugin_id`, `plugin_name`, `plugin_schema` | `(out_ptr, out_max) -> written` | Host buffer is **8 KB** — a bigger JSON schema is silently truncated. |
| `plugin_start` | `(cfg_ptr, cfg_len) -> i32` | Config JSON (UTF-8). Non-zero ⇒ status `error`. |
| `plugin_stop` | `() -> i32` | |
| `poll` *(opt)* | `() -> i32` | Every 1 s while running. Your event loop for sockets/timers. |
| `delta_handler` *(opt)* | `(ptr, len)` | **Every** delta of **every** context, unfiltered, `JSON.stringify`'d per call — costly on a Pi; filter with a cheap substring check first. |
| `http_endpoints` *(opt)* | `(out_ptr, out_max) -> written` | See §6. |
| `handle_put_<ctx>_<path>` *(opt)* | `(val_ptr, val_len, resp_ptr, resp_max) -> written` | Dots → `_`; registered via `sk_register_put_handler`. **Server ≥ 2.35.0**: before it, every PUT to a WASM-handled path answered HTTP 500 and AssemblyScript handlers got the wrong string types (server PR #3223). |

Two behaviours to design for:

- **Config save = `plugin_stop` + `plugin_start` on the same instance.** Globals survive, so
  `plugin_start` must fully reset state (the starter replaces its whole `State`).
- **Reloading a rebuilt `.wasm`.** Up to and including 2.35.1 there is none: disable +
  enable reuses the loaded instance (verified live), so restart the server — only the first
  enable of a plugin that was disabled at boot loads from disk. Server PR #3222, merged to
  master after 2.35.1 and **unreleased at the time of writing**, makes every enable load
  `plugin.wasm` from disk (keeping the old instance if the new one fails to load) and
  deregisters PUT handlers on stop. The "Reload button" and buffered-delta replay once
  described in the docs never existed. Crash auto-restart exists only as dead code:
  `handleWasmPluginCrash` (restart with backoff, give up after 3) is exported from
  `src/wasm/loader/plugin-lifecycle.ts` but nothing calls it (2.35.1), so a trap never
  restarts a plugin.

## 4. Host imports and what capabilities really do

Everything is imported from module `env`; the full table with signatures is in
[reference.md](reference.md). Strings into the host are always `(ptr, len)` UTF-8.

- Every import is always linked. A missing capability makes the call **return 0/-1 and log a
  debug line** — no error in the admin UI. Check return values.
- **Enforced**: `putHandlers`, `resourceProvider`, `weatherProvider`, `radarProvider`,
  `rawSockets`; `network` controls whether the AssemblyScript `as-fetch` imports exist.
- **Declared but not enforced** (2.35.1 source): `dataRead`, `dataWrite`, `httpEndpoints`,
  `storage`, `serialPorts`. Declare them anyway for intent; don't rely on them for safety.
- `sk_has_capability` only answers for `network` and `rawSockets`.

## 5. Data in and out

- **Emit**: `sk_handle_message(ptr, len, version)` with `1` (v1 data model) or `2` (v2 event
  only, e.g. course paths). Omit `source`/`timestamp`; context defaults to `vessels.self`;
  `$source` becomes the package name.
- **Notifications**: `sk_publish_notification(path, value)` validates the state
  (`normal|alert|warn|alarm|emergency`).
- **Read own vessel**: `sk_get_self_path` **always returns 0** (reproduced on 2.33.0,
  binding unchanged through 2.35.1) — it looks for `getSelfPath` on the shared `app`, which
  only per-plugin Node `appCopy`s have (server issue #3080; fix proposed in server PR
  #3173). Deltas reach `delta_handler` with the resolved context
  (`vessels.urn:mrn:…`), not `vessels.self`, so you can't simply match on "self"; the
  workaround in use is to treat the context whose position comes from a non-AIS source as
  your own vessel. Plain path reads (`sk_get_path`) don't exist either, although the
  AssemblyScript SDK declares one — calling `getPath()` fails to link.
- **No server event bus**: WASM can't subscribe to `nmea0183`, `nmea2000JsonOut` or emit
  `nmea2000out` (the proposal, server PR #2295, was closed unmerged). Converters that need the
  raw NMEA streams must be Node plugins.

## 6. HTTP endpoints, UIs and the "can I serve a web app?" question

`http_endpoints` returns `[{"method","path","handler"}]` (GET/POST/PUT/DELETE only), mounted
at `/plugins/<id><path>`. The handler gets the request context JSON
`{method, path, query, params, body, headers}` and writes the envelope
`{"statusCode", "headers", "body"}`; set `Content-Type: text/html` and the body string is
sent as is. Verified limits:

- **Admin-only when security is on.** A `readonly` *and* a `readwrite` user both get 401 on
  WASM plugin routes; only admins pass (live on 2.33.0, unchanged in 2.35.1). Node plugins
  can open routes with `router.access('readonly')`; for WASM a per-endpoint `access` field is
  proposed in server PR #3169. Never put consumer-facing data behind these routes.
- **Rust (buffer ABI): 64 KB response buffer, envelope included.** One byte over and the
  truncated JSON fails to parse ⇒ 500 (`Unterminated string in JSON at position 65536`);
  check the length and return a small error envelope instead (the starter does). The
  AssemblyScript loader reads the returned string with `__getString`, so it has no such cap.
  Body is text only in both — no images or fonts. (Larger Rust responses: server PR #3175;
  binary bodies: #3176.)
- **Synchronous on the server's event loop.** A slow handler stalls every client; the
  10-second "timeout" in the route code cannot fire while WASM is running. No long-lived
  SSE either: a `text/event-stream` body is sent once, when the handler returns (plugin SSE
  channels: server PR #3177).
- **Push goes through binary streams, not routes.** `sk_emit_binary_stream(id, bytes)` with
  `id = "plugins/<packageName>/<name>"` fans the bytes out to every WebSocket client of
  `/signalk/v2/api/streams/plugins/<packageName>/<name>` (one-way, binary frames). Verified:
  this endpoint uses the normal stream auth, so an anonymous client under `allow_readonly`
  receives it while the same plugin's HTTP routes answer 401.

So for a dashboard or display (KIP/Freeboard-style), don't render HTML in WASM. Ship a
**static webapp** in the same package — add the keyword `signalk-webapp` and put files in
`public/` — and let the page read the server's own APIs (`/signalk/v1/stream` WebSocket,
REST, any resources your plugin provides, and its binary streams for live push). Verified:
it is mounted at **`/<packageName>/`** and only with that keyword (re-verified on 2.35.1);
`/plugins/<id>/index.html` is a 404. Use WASM HTTP handlers for small admin-only pages and
actions — or, where admin-only is acceptable, as the backend of a server-rendered
hypermedia UI: [hypermedia-ui.md](hypermedia-ui.md) (Datastar/htmx, fragments, per-display
state, polling).

Data that clients should read belongs behind a **provider**:
`sk_register_resource_provider(type)` + `resources_{list_resources,get_resource,set_resource,delete_resource}`
exports serves `/signalk/v2/api/resources/<type>` under normal (read-only-capable) auth.
Weather (`weather_get_*`) and radar (`radar_*`, plus `sk_radar_emit_spokes` for binary
streams) work the same way. Providers stay registered after `plugin_stop` — the server keeps
calling the stopped instance, so return sensible empty results when not running.

## 7. Storage

The plugin sees a WASI root `/` mapped to
`<configdir>/plugin-config-data/<id>/vfs/` with `data/`, `config/`, `tmp/`.

**Verified bug (2.33.0, still in 2.35.1):** for a plugin that is enabled when the server
boots, the live instance's `/` points at a temporary directory the loader deletes right after
loading, so every `std::fs` call fails with `No such file or directory (os error 44)`. The
same plugin enabled at runtime from the admin UI writes fine — until the next restart. A fix
is proposed in server PR #3171; until it ships, don't depend on WASI file I/O. For small
persistent state use `sk_save_config`/`sk_read_config` (scoped names need ≥ 2.34.0, §1), or
keep state in the data model / a resource provider.

## 8. Network and sockets

- **Rust has no HTTP client.** `rawSockets` gives non-blocking UDP (`sk_udp_*`, multicast
  supported) and TCP (`sk_tcp_*`, line or raw mode) that you drive from `poll`. Each socket
  buffers at most 1000 datagrams/lines (oldest dropped); `recv` truncates to your buffer and
  **drops the rest of that datagram** — size buffers for the largest packet. TCP sockets are
  not closed on stop: close them in `plugin_stop` (server PR #3174 proposes doing it for you).
- **AssemblyScript** gets `fetchSync` via `as-fetch` with `"network": true`,
  `"transform": ["as-fetch/transform"]` and `exportRuntime`. It only works where the host
  drives Asyncify: `plugin_start` and the weather/radar provider handlers — not in `poll`,
  `delta_handler`, HTTP, PUT or resource handlers. The `httpGet`/`httpPost` helpers listed in
  the capabilities doc don't exist in the SDK.
- No serial ports, BLE or CAN from WASM (a BLE binding is proposed in server PR #3182). No
  Rust HTTP client either: `reqwest` and `ureq` compile for `wasm32-wasip1` but fail at
  runtime — see [rust-patterns.md](rust-patterns.md).

## 9. Develop, debug, publish

- **Dev loop**: symlink the package into `~/.signalk/node_modules/<name>` **and** add it to
  `~/.signalk/package.json` `dependencies`, or the next App Store install prunes it as
  extraneous. Rebuild ⇒ restart the server (≤ 2.35.1; see §3).
- **Logs**: put `signalk:wasm:*` in `<configdir>/debug` (or `DEBUG=signalk:wasm:*`).
  `sk_debug` lines appear as `signalk:wasm:bindings [<package>] …`. There's no settings toggle.
- **A trap doesn't stop the plugin**: a panic in `poll`/`delta_handler` is logged at debug
  level and the instance keeps being called; a panic in an HTTP handler is a 500. Because
  nothing unwinds, a panic inside a `RefCell` borrow poisons the plugin until restart — see
  [rust-patterns.md](rust-patterns.md). An infinite loop freezes the whole server.
  `--no-plugins` starts the server without plugins (WASM included) for recovery.
- **Publish**: ship the built `plugin.wasm` through `files` (it's gitignored, so without
  `files` npm ships nothing) and build in CI before `npm publish` — the user's server never
  runs a toolchain. Use the same OIDC trusted-publishing flow as any npm package.

## 10. Limits by version

Re-check this table against the server release you target; each row says where the
statement comes from. "Proposed" PRs were open and unreleased at the time of writing.

| Limit | Status | Since / where |
|---|---|---|
| Scoped package names lose status, config, socket cleanup | Fixed | 2.34.0 (#3172); status verified 2.35.1 |
| PUT to a WASM-handled path answers 500 | Fixed | 2.35.0 (#3223); Rust and AssemblyScript verified 2.35.1 |
| Rebuilt `.wasm` needs a server restart | Fixed on master, unreleased | after 2.35.1 (#3222); restart still needed on 2.35.1 (verified) |
| Routes admin-only under security | Open | proposed #3169 |
| WASI file writes fail for plugins enabled at boot | Open | verified 2.35.1; proposed #3171 |
| `sk_get_self_path` always 0 | Open | verified 2.33.0, unchanged 2.35.1; proposed #3173 |
| TCP sockets and providers not cleaned up on stop | Open | proposed #3174 (PUT handlers: #3222, unreleased) |
| Rust HTTP responses capped at 64 KB | Open | 2.35.1 source; proposed #3175 |
| Text-only HTTP bodies | Open | proposed #3176 |
| No long-lived SSE from routes | Open | one-shot body verified 2.35.1; proposed #3177 |
| No BLE from WASM | Open | proposed #3182 |
| Not listed in the App Store with only `signalk-wasm-plugin` | Open | 2.35.1 source; proposed #3211 |

## Testing

Keep the logic in plain Rust modules with `cargo test` on the host target. Raw `extern "C"`
imports don't link on the host, so keep every `sk_*` import behind the starter's `cfg`-gated
`host` module — [rust-patterns.md](rust-patterns.md) §6. Then gate on a live server: install the published
`signalk-server` into a scratch dir, start it with `-c <scratch-config>`, symlink the plugin,
enable it via its config file, and `curl` the endpoints and `/signalk/v1/api/vessels/self/...`
paths you emit.

---

*Verified October 2026. Live on the npm release of signalk-server 2.35.1 (Node 22, rustc
1.97, AssemblyScript 0.28.20 with SDK 0.3.0): both starters (config and its default, poll
delta, HTML endpoint with an escaped hostile greeting), a `wasm-opt`'d Rust build, scoped-name status, Rust and AssemblyScript PUT
handler replies, the VFS boot bug, the `/<packageName>/` mount, no reload on disable +
enable, state across requests and config saves, panic behaviour, and the request/response
details in hypermedia-ui.md. Live on 2.33.0, with the code involved unchanged through
2.35.1: route auth for readonly/readwrite/admin users, the 64 KB overflow, binary-stream
push to an anonymous `allow_readonly` client and `sk_get_self_path` returning 0. Unreleased
changes are from the server's master branch after 2.35.1; other statements are from
`src/wasm/**` at 2.35.1.*
