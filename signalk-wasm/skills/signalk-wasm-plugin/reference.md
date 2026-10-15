# SignalK WASM host imports (`env` module)

Signatures as the host defines them in `src/wasm/bindings/env-imports.ts` (signalk-server
2.33.0). `ptr/len` pairs are UTF-8 in your linear memory. "Gate" is the `wasmCapabilities`
flag the host checks; a failed gate returns `0`/`-1` and only logs a debug line.

Rust declaration form:

```rust
#[link(wasm_import_module = "env")]
extern "C" {
    fn sk_debug(ptr: *const u8, len: usize);
}
```

## Core

| Import | Signature | Gate | Returns / behaviour |
|---|---|---|---|
| `sk_debug` | `(ptr, len)` | — | Logged as `signalk:wasm:bindings [<package>] …` |
| `sk_set_status` | `(ptr, len)` | — | Admin UI status (lost for scoped package names) |
| `sk_set_error` | `(ptr, len)` | — | Admin UI error |
| `sk_handle_message` | `(ptr, len, version)` | (`dataWrite`, not enforced) | `version` 1 = v1 data model, 2 = v2 event only |
| `sk_publish_notification` | `(path_ptr, path_len, value_ptr, value_len) -> i32` | — | 0 / -1; value needs `state` ∈ normal, alert, warn, alarm, emergency |
| `sk_get_self_path` | `(path_ptr, path_len, buf_ptr, buf_max) -> i32` | (`dataRead`, not enforced) | **Always 0 in 2.33.0** (server issue #3080) |
| `sk_read_config` | `(buf_ptr, buf_max) -> i32` | — | Bytes written of the saved `configuration`; 0 if missing *or* buffer too small |
| `sk_save_config` | `(ptr, len) -> i32` | — | 0 / -1; atomic write, keeps `enabled` |
| `sk_has_capability` | `(ptr, len) -> i32` | — | 1/0, but only knows `network` and `rawSockets` |

## Providers and handlers

| Import | Signature | Gate | Exports you must provide |
|---|---|---|---|
| `sk_register_put_handler` | `(ctx_ptr, ctx_len, path_ptr, path_len) -> i32` | `putHandlers` | `handle_put_<ctx>_<path>` (dots → `_`): Rust `(val_ptr, val_len, resp_ptr, resp_max=8192) -> written`, returns `{"state","statusCode","message"}` |
| `sk_register_resource_provider` | `(type_ptr, type_len) -> i32` | `resourceProvider` | `resources_list_resources`, `resources_get_resource`, `resources_set_resource`, `resources_delete_resource`: Rust `(req_ptr, req_len, resp_ptr, resp_max=65536) -> written` |
| `sk_register_weather_provider` | `(name_ptr, name_len) -> i32` | `weatherProvider` | `weather_get_observations`, `weather_get_forecasts`, `weather_get_warnings` |
| `sk_register_radar_provider` | `(name_ptr, name_len) -> i32` | `radarProvider` | `radar_get_radars`, `radar_get_radar_info`, `radar_get_state`, `radar_get_capabilities`, `radar_set_*`, … |
| `sk_radar_emit_spokes` | `(id_ptr, id_len, data_ptr, data_len) -> i32` | `radarProvider` | Binary stream to `radars/<id>` |
| `sk_emit_binary_stream` | `(id_ptr, id_len, data_ptr, data_len) -> i32` | — | Stream id must be `radars/<id>` or `plugins/<package>/…`; clients read it over WebSocket at `/signalk/v2/api/streams/<id>` with normal (read-only capable) auth |

## Raw sockets (`rawSockets`)

All non-blocking; poll them from your `poll` export.

| Import | Signature | Notes |
|---|---|---|
| `sk_udp_create` | `(type) -> id` | 0 = udp4, 1 = udp6; -1 on failure |
| `sk_udp_bind` | `(id, port) -> i32` | Binds asynchronously; returns 0 before the bind has finished |
| `sk_udp_join_multicast` / `sk_udp_leave_multicast` | `(id, addr_ptr, addr_len, iface_ptr, iface_len) -> i32` | Empty iface = default |
| `sk_udp_set_multicast_ttl` | `(id, ttl) -> i32` | |
| `sk_udp_set_multicast_loopback` / `sk_udp_set_broadcast` | `(id, enabled) -> i32` | |
| `sk_udp_send` | `(id, addr_ptr, addr_len, port, data_ptr, data_len) -> i32` | Returns `data_len` before the send completes |
| `sk_udp_recv` | `(id, buf_ptr, buf_max, addr_out_ptr, port_out_ptr) -> i32` | Bytes, 0 if empty; source address written NUL-terminated, port as u16 LE; the rest of an oversized datagram is dropped |
| `sk_udp_pending` | `(id) -> i32` | Queued datagrams (max 1000, oldest dropped) |
| `sk_udp_close` | `(id)` | UDP sockets are also closed on `plugin_stop` |
| `sk_tcp_create` | `() -> id` | |
| `sk_tcp_connect` | `(id, addr_ptr, addr_len, port) -> i32` | Starts connecting; poll `sk_tcp_connected` |
| `sk_tcp_connected` | `(id) -> i32` | 1 connected, 0 not (yet) connected — a refused or dropped connection also reads 0, so add your own timeout; -1 unknown socket or no capability |
| `sk_tcp_set_line_buffering` | `(id, enabled) -> i32` | 1 = split on `\n` / `\r\n` (default), 0 = raw |
| `sk_tcp_send` | `(id, ptr, len) -> i32` | |
| `sk_tcp_recv_line` / `sk_tcp_recv_raw` | `(id, buf_ptr, buf_max) -> i32` | 0 if nothing buffered |
| `sk_tcp_pending` | `(id) -> i32` | |
| `sk_tcp_close` | `(id)` | **Not** closed for you on stop — do it in `plugin_stop` |

## HTTP endpoint handler (any name listed by `http_endpoints`)

Rust: `(req_ptr, req_len, resp_ptr, resp_max=65536) -> written`.

Request context:

```json
{ "method": "GET", "path": "/hello", "query": {}, "params": {}, "body": {}, "headers": {} }
```

Response envelope (write it with a JSON serializer so the body is escaped correctly):

```json
{ "statusCode": 200, "headers": { "Content-Type": "text/html; charset=utf-8" }, "body": "<h1>…</h1>" }
```

A string body with a JSON content type is parsed and re-sent as JSON; any other body is sent
as is.

## AssemblyScript differences

AssemblyScript exports return managed strings (`plugin_name(): string`,
`resources_get_resource(json: string): string`, …) that the host reads with `__getString`,
so `exportRuntime: true` is mandatory. HTTP handlers receive `(ptr, len)` where `ptr` is a
**managed UTF-16 string** created with `__newString` and `len` is its length in UTF-16 code
units — decode it as such, not as UTF-8 bytes as some docs show.
