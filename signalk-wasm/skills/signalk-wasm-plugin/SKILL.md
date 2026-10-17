---
name: signalk-wasm-plugin
description: Use when writing, debugging or publishing a sandboxed WebAssembly plugin for SignalK server (Rust wasm32-wasip1 or AssemblyScript, wasmManifest/wasmCapabilities in package.json) — the buffer ABI and required exports, which capabilities are actually enforced, HTTP endpoints and serving a UI (admin-only routes, 64 KB Rust responses, the signalk-webapp mount), deltas, providers, sockets, and the verified runtime bugs (scoped package names, VFS writes after restart) that the official docs don't mention.
---

# Write a SignalK WASM plugin

A SignalK **WASM plugin** is an npm package whose plugin logic is a `.wasm` binary, run by the
server in Node's built-in WASI (`node:wasi`, preview1) instead of being `require()`d. It talks
to the server only through `env.sk_*` host imports, so it gets no `app` object, no npm
dependencies at runtime and no install scripts. That makes it a good fit for self-contained
logic: protocol decoders, calculations, device drivers over UDP/TCP, and providers.

It is **not** the only way to use WebAssembly in SignalK. If you need the full
`@signalk/server-api` (`app.*`, Express routes with read-only access, serial ports, the NMEA
event bus) and only want WASM for speed or for reusing Rust/C code, write a normal Node plugin
and load a wasm-pack module inside it — the sibling skill **`signalk-wasm-library`** covers
that. Decide first:

| You need… | Use |
|---|---|
| Sandbox, no `app` access, Rust/AssemblyScript end to end | WASM plugin (this skill) |
| Serial/BLE/CAN, `nmea0183`/`nmea2000JsonOut` events, read-only plugin routes, streaming responses | Node plugin, optionally + a WASM library |
| A UI for a cockpit display | A static webapp (`signalk-webapp`) reading the server's own APIs — see §6 |

`starter/` next to this file is a complete Rust plugin (config, `poll`-driven delta, one HTML
endpoint) verified on a live server. Copy it, rename, `npm run build`.

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
- **Use an unscoped package name.** Verified on 2.33.0: with a scoped name (`@scope/x`) the
  host bindings are keyed by the *package name* while everything else uses the derived ID, so
  `sk_set_status` never reaches the admin UI (status stays empty), `sk_read_config` returns
  `{}`, and deltas carry `$source: "@scope/x"`. Unscoped names make all three agree.
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
  `panic = "abort"` and `opt-level = "s"` keep it ~90 KB with serde_json.
- **AssemblyScript**: `@signalk/assemblyscript-plugin-sdk` (0.3.0) + `assemblyscript`;
  `asconfig.json` must set `"exportRuntime": true` (the host reads strings with
  `__getString`). Pin the SDK as `^0.3.0` — the repo examples still say `^0.2.0`, which
  excludes 0.3 under caret-on-0.x rules.
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
| `handle_put_<ctx>_<path>` *(opt)* | `(val_ptr, val_len, resp_ptr, resp_max) -> written` | Dots → `_`; registered via `sk_register_put_handler`. |

Two behaviours to design for:

- **Config save = `plugin_stop` + `plugin_start` on the same instance.** Globals survive, so
  `plugin_start` must fully reset state (the starter replaces its whole `State`).
- **There is no hot reload.** A rebuilt `.wasm` is picked up on server restart (or on the
  first enable of a plugin that was disabled at boot). The "Reload button", buffered-delta
  replay and crash auto-restart described in the docs do not exist in the code.

## 4. Host imports and what capabilities really do

Everything is imported from module `env`; the full table with signatures is in
[reference.md](reference.md). Strings into the host are always `(ptr, len)` UTF-8.

- Every import is always linked. A missing capability makes the call **return 0/-1 and log a
  debug line** — no error in the admin UI. Check return values.
- **Enforced**: `putHandlers`, `resourceProvider`, `weatherProvider`, `radarProvider`,
  `rawSockets`; `network` controls whether the AssemblyScript `as-fetch` imports exist.
- **Declared but not enforced** (2.33.0): `dataRead`, `dataWrite`, `httpEndpoints`,
  `storage`, `serialPorts`. Declare them anyway for intent; don't rely on them for safety.
- `sk_has_capability` only answers for `network` and `rawSockets`.

## 5. Data in and out

- **Emit**: `sk_handle_message(ptr, len, version)` with `1` (v1 data model) or `2` (v2 event
  only, e.g. course paths). Omit `source`/`timestamp`; context defaults to `vessels.self`;
  `$source` becomes the package name.
- **Notifications**: `sk_publish_notification(path, value)` validates the state
  (`normal|alert|warn|alarm|emergency`).
- **Read own vessel**: `sk_get_self_path` currently **always returns 0** — the binding looks
  for `getSelfPath` on the shared `app`, which only per-plugin Node `appCopy`s have (server
  issue #3080). Deltas reach `delta_handler` with the resolved context
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
  WASM plugin routes; only admins pass. Node plugins can open routes with
  `router.access('readonly')` — WASM has no equivalent yet. Never put consumer-facing data
  behind these routes.
- **Rust (buffer ABI): 64 KB response buffer, envelope included.** One byte over and the
  truncated JSON fails to parse ⇒ 500 (`Unterminated string in JSON at position 65536`);
  check the length and return a small error envelope instead (the starter does). The
  AssemblyScript loader reads the returned string with `__getString`, so it has no such cap.
  Body is text only in both — no images or fonts.
- **Synchronous on the server's event loop.** A slow handler stalls every client; the
  10-second "timeout" in the route code cannot fire while WASM is running.
- **Push goes through binary streams, not routes.** `sk_emit_binary_stream(id, bytes)` with
  `id = "plugins/<packageName>/<name>"` fans the bytes out to every WebSocket client of
  `/signalk/v2/api/streams/plugins/<packageName>/<name>` (one-way, binary frames). Verified:
  this endpoint uses the normal stream auth, so an anonymous client under `allow_readonly`
  receives it while the same plugin's HTTP routes answer 401.

So for a dashboard or display (KIP/Freeboard-style), don't render HTML in WASM. Ship a
**static webapp** in the same package — add the keyword `signalk-webapp` and put files in
`public/` — and let the page read the server's own APIs (`/signalk/v1/stream` WebSocket,
REST, any resources your plugin provides, and its binary streams for live push). Verified:
it is mounted at **`/<packageName>/`** and only with that keyword; the docs' claim that
`public/` is served at `/plugins/<id>/` is wrong. Use WASM HTTP handlers for small
admin-only pages and actions.

Data that clients should read belongs behind a **provider**:
`sk_register_resource_provider(type)` + `resources_{list_resources,get_resource,set_resource,delete_resource}`
exports serves `/signalk/v2/api/resources/<type>` under normal (read-only-capable) auth.
Weather (`weather_get_*`) and radar (`radar_*`, plus `sk_radar_emit_spokes` for binary
streams) work the same way. Providers stay registered after `plugin_stop` — the server keeps
calling the stopped instance, so return sensible empty results when not running.

## 7. Storage

The plugin sees a WASI root `/` mapped to
`<configdir>/plugin-config-data/<id>/vfs/` with `data/`, `config/`, `tmp/`.

**Verified bug (2.33.0):** for a plugin that is enabled when the server boots, the live
instance's `/` points at a temporary directory the loader deletes right after loading, so
every `std::fs` call fails with `No such file or directory (os error 44)`. The same plugin
enabled at runtime from the admin UI writes fine — until the next restart. Until that is
fixed, don't depend on WASI file I/O. For small persistent state use
`sk_save_config`/`sk_read_config` (works with unscoped names), or keep state in the data
model / a resource provider.

## 8. Network and sockets

- **Rust has no HTTP client.** `rawSockets` gives non-blocking UDP (`sk_udp_*`, multicast
  supported) and TCP (`sk_tcp_*`, line or raw mode) that you drive from `poll`. Each socket
  buffers at most 1000 datagrams/lines (oldest dropped); `recv` truncates to your buffer and
  **drops the rest of that datagram** — size buffers for the largest packet. TCP sockets are
  not closed on stop: close them in `plugin_stop`.
- **AssemblyScript** gets `fetchSync` via `as-fetch` with `"network": true`,
  `"transform": ["as-fetch/transform"]` and `exportRuntime`. It only works where the host
  drives Asyncify: `plugin_start` and the weather/radar provider handlers — not in `poll`,
  `delta_handler`, HTTP, PUT or resource handlers. The `httpGet`/`httpPost` helpers listed in
  the capabilities doc don't exist in the SDK.
- No serial ports, BLE or CAN from WASM.

## 9. Develop, debug, publish

- **Dev loop**: symlink the package into `~/.signalk/node_modules/<name>` **and** add it to
  `~/.signalk/package.json` `dependencies`, or the next App Store install prunes it as
  extraneous. Rebuild ⇒ restart the server.
- **Logs**: put `signalk:wasm:*` in `<configdir>/debug` (or `DEBUG=signalk:wasm:*`).
  `sk_debug` lines appear as `signalk:wasm:bindings [<package>] …`. There's no settings toggle.
- **A trap doesn't stop the plugin**: a panic in `poll`/`delta_handler` is logged and the
  instance keeps being called; an infinite loop freezes the whole server. `--no-plugins`
  starts the server without plugins (WASM included) for recovery.
- **Publish**: ship the built `plugin.wasm` through `files` (it's gitignored, so without
  `files` npm ships nothing) and build in CI before `npm publish` — the user's server never
  compiles anything. Use the same OIDC trusted-publishing flow as any npm package.

## Testing

Keep the logic in plain Rust modules with `cargo test` on the host target; keep the `extern
"C"` surface thin. Then gate on a live server: install the published `signalk-server` into a
scratch dir, start it with `-c <scratch-config>`, symlink the plugin, enable it via its
config file, and `curl` the endpoints and `/signalk/v1/api/vessels/self/...` paths you emit.

---

*Verified October 2026 against the npm release of signalk-server 2.33.0 on Node 22, with a
Rust `wasm32-wasip1` probe plugin and the bundled `starter/`: route auth for
readonly/readwrite/admin users, the 64 KB overflow, HTML responses, `/<packageName>/` static
mount, plugin binary-stream push to an anonymous `allow_readonly` client, scoped-name
status/config loss, the VFS boot bug and `sk_get_self_path` returning 0 were all reproduced
live. Other
statements are from reading `src/wasm/**` at the same version.*
