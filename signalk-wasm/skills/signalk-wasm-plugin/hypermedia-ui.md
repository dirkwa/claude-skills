# Hypermedia UI recipe (Datastar, htmx)

A WASM plugin is one long-lived instance with its own memory, so it can be the backend of a
server-rendered UI: routes return HTML fragments, the page swaps them in, and all state
stays in the plugin. The server-side behaviour below was verified on signalk-server 2.35.1
with a Rust probe plugin; the attribute syntax is Datastar 1.0's (`data-on:click`, colon for
keyed attributes), taken from its docs and not run here.

**Check the access limit first.** With security on, WASM plugin routes answer only admins
(SKILL.md §6). A cockpit display logged in as a read-only user, or an anonymous one, gets
401. This recipe therefore fits a server without security, or an admin's own browser
(same origin, so the admin session cookie is sent). Per-endpoint `access: "readonly"` is
proposed in server PR #3169, unreleased.

## Layout

```
my-plugin/
  package.json      # keywords: signalk-wasm-plugin + signalk-webapp
  plugin.wasm       # routes: /panel, /action …
  public/
    index.html      # the shell page
    datastar.js     # vendored: a boat is often offline, so no CDN
```

- **The shell** lives in `public/` and is served at `/<packageName>/` once the package has
  the `signalk-webapp` keyword (verified; not at `/plugins/<id>/`). Keep it out of the
  routes: Rust responses are capped at 64 KB, envelope included.
- **The routes** are the backend at `/plugins/<id>/<path>`. Return fragments whose
  top-level elements carry an `id`; Datastar morphs them into the page.

```html
<div id="panel" data-on-interval__duration.1s="@get('/plugins/my-plugin/panel')">…</div>
<button data-on:click="@post('/plugins/my-plugin/ack')">Ack</button>
```

**Escape what you render.** Config values and strings from deltas (AIS vessel names,
notification messages) are untrusted; HTML-escape them before putting them in a fragment —
both starters show an `escape_html`/`escapeHtml` helper.

## What the handler sees and can return (verified)

- **GET**: Datastar sends its signals as the `datastar` query parameter. It arrives as a
  *string*: `"query":{"datastar":"{\"cid\":\"d1\"}"}` — parse it a second time.
- **POST with a JSON body** arrives already parsed: `"body":{"cid":"d1","tab":2}`.
- **Headers** arrive lowercased, cookies included (`"cookie":"sid=xyz"`). The handler does
  not get the logged-in SignalK user, only the raw headers.
- **Response headers pass through**, so `Set-Cookie` works.
- **`text/html`** bodies are sent as is. A string body with an `application/json` content
  type is parsed and re-sent as JSON — that is the "patch signals" response.
- **A one-shot `text/event-stream` body** is delivered complete (a few
  `event: datastar-patch-elements` blocks, then the response ends). There is no long-lived
  stream: the handler is synchronous and the response is sent when it returns. Plugin SSE
  channels are proposed in server PR #3177, unreleased. Until then, poll with
  `data-on-interval`.

## Per-display state

One instance serves every client, so plugin globals are shared state — right for boat data,
wrong for "which tab is open on the helm display".

- **Cookie**: every plugin and webapp shares the server's origin, so give the cookie a
  plugin-specific name and path: on a request without it, set
  `Set-Cookie: myplugin_sid=<random>; Path=/plugins/<id>; HttpOnly` and keep a
  `HashMap<sid, DisplayState>`. Expire old entries yourself; the map lives until
  the server restarts.
- **Signal**: `data-signals="{cid: '…'}"` and read `cid` from the query or body.
- Pure view state can stay in Datastar signals on the client.

## Keep it cheap

Every poll from every display runs synchronously on the server's event loop. Render small
fragments from state you already hold (fed by `delta_handler` or `poll`), don't recompute on
each request, and poll no faster than the data changes. The same pattern works with htmx
(`hx-get` + `hx-trigger="every 1s"`): only the attribute syntax differs.
