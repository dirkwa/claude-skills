---
name: signalk-wasm-library
description: Use when shipping Rust (or other) code compiled with wasm-pack inside an ordinary Node/TypeScript SignalK plugin or npm library — instead of a native addon or a child-process binary — covering why prebuilt wasm survives the app store's --ignore-scripts installs, the wasm-pack pkg/.gitignore trap that silently ships a tarball without the .wasm, keeping the glue out of bundlers, the dual ESM/CJS TypeScript wrapper, freeing wasm-bindgen objects, and parity testing against the native build.
---

# Ship a wasm-pack module inside a SignalK Node plugin

The other way to use WebAssembly in SignalK: keep a normal Node plugin (full
`@signalk/server-api`, read-only routes, serial ports, the NMEA event bus) and run the heavy
or reused code as a **wasm-bindgen module loaded in-process**. The reference implementation is
[`@canboat/wasm`](https://github.com/canboat/canboat-wasm) — canboat's Rust NMEA 2000
decoder/encoder compiled for `wasm32-unknown-unknown` and published as an npm package; read it
as the worked example. (For a fully sandboxed plugin written in Rust, see the sibling skill
**`signalk-wasm-plugin`**.)

## 1. Why this pattern exists

- **Install scripts never run** on app-store installs (`npm --ignore-scripts`), and npm 12
  skips dependency scripts by default. A native addon built in `postinstall` silently
  disappears; a `.wasm` file that is *already in the tarball* needs no build step, no
  toolchain on the boat and no per-arch prebuilds. One artifact runs on x86, arm64 and armv7.
- **In-process beats a child process.** `@canboat/wasm` decodes a line in about 5 µs —
  roughly 2.8× faster than the pure-JS `canboatjs` and faster than the native binary behind
  a pipe. Cost: a bigger package (1.55 MB wasm, ~530 KB gzipped).
- **One codebase, two targets.** The same Rust crate builds the native CLI and the wasm
  module, so behaviour can't drift — if you keep the wasm crate free of protocol logic and
  depend on the upstream crate with only its sans-I/O features.

## 2. Design rule: sans-I/O

`wasm32-unknown-unknown` has no sockets, files or clock. Put parsing/encoding/state machines
in Rust and leave **all I/O in TypeScript** (`node:net` for TCP gateways, `serialport`,
WebSocket). For byte-stream protocols expose a feed/poll shape — `decodeBytes(chunk)`,
`takePendingTx()`, `tick()` called about once a second so timers advance on a quiet link —
rather than callbacks into JS.

## 3. Build

```toml
[lib]
crate-type = ["cdylib", "rlib"]   # rlib keeps `cargo test` on the host target

[dependencies]
wasm-bindgen = "0.2"

[profile.release]
opt-level = "z"
lto = true
strip = true
```

```json
"type": "module",
"scripts": {
  "build:wasm": "wasm-pack build --release --target nodejs && rm -f pkg/.gitignore",
  "build:ts": "tsup",
  "build": "npm run build:wasm && npm run build:ts",
  "prepack": "npm run build"
},
"files": ["dist/", "pkg/"]
```

- **`--target nodejs`** emits CommonJS glue that loads the `.wasm` synchronously with
  `readFileSync(`${__dirname}/<name>_bg.wasm`)`. No async init, and both `require` and ESM
  `import` work (the ESM default import exposes the exports).
- **The `pkg/.gitignore` trap (verified):** wasm-pack writes `pkg/.gitignore` containing
  `*`, and npm honours nested ignore files *even inside a directory listed in `files`*.
  Without the `rm`, `npm pack` ships `package.json` and nothing else — no glue, no wasm —
  and the install "succeeds". Assert it in CI:
  ```sh
  npm pack --dry-run 2>&1 | tee pack.txt
  grep -q "pkg/<name>_bg.wasm" pack.txt
  ```
- **wasm-opt.** wasm-pack downloads binaryen from GitHub on first build; behind a proxy or
  offline that fails (`failed to download … binaryen`) — `wasm-opt = false` under
  `[package.metadata.wasm-pack.profile.release]` unblocks a build. canboat-wasm also had to
  pass `wasm-opt = ["-O", "--enable-bulk-memory", "--enable-nontrapping-float-to-int"]`
  because recent rustc emits those features and wasm-pack's bundled wasm-opt rejects them
  by default.

## 4. The TypeScript wrapper (strict, dual ESM/CJS)

```ts
// ts/index.ts — public entry; the wasm-pack glue stays a runtime module.
export { Decoder, encodeData, version } from '../pkg/my_crate.js'
export { FromPgn } from './compat.js'
```

```ts
// tsup.config.ts
import { defineConfig } from 'tsup'

export default defineConfig({
  entry: ['ts/index.ts'],
  format: ['esm', 'cjs'],
  dts: true,
  target: 'node18',
  // The glue loads the .wasm from its own directory: bundling it breaks __dirname.
  external: [/\.\.\/pkg\/my_crate\.js$/]
})
```

`exports` maps `types`/`import`/`require` to `dist/index.{d.ts,js,cjs}`. `"type": "module"` is what makes tsup name them `index.js` (ESM) and `index.cjs`; the glue
stays CommonJS because wasm-pack writes its own `pkg/package.json`. Typecheck with
`strict` and `noUncheckedIndexedAccess`; wasm-pack's generated `.d.ts` types the raw API, and
a hand-written compatibility layer (`compat.ts`) can mimic an existing JS API so consumers
swap packages without code changes. Smoke-test **both** entries (`test/smoke.mjs` and
`test/smoke.cjs`) against the built `dist/`.

## 5. Using it from the plugin

- Create stateful objects (decoders) **once** in `start()` and reuse them; don't construct
  per message on the delta path.
- **Free what you create.** wasm-bindgen classes own Rust memory; the glue registers a
  `FinalizationRegistry`, but GC timing is not a memory budget on a Pi. Call `.free()` in
  `stop()` (a freed object throws `null pointer passed to rust` if used again — drop your
  reference too).
- Strings cross the boundary by copy (UTF-8 encode/decode each call). For hot paths prefer
  APIs that take/return bytes (`Uint8Array`) or batch work per call.
- Calls are synchronous on the event loop, like any JS: keep each one short.

## 6. Parity is the contract

When the wasm build mirrors a native tool, test that the outputs are **byte-identical**, not
"similar": pin representative wire vectors in the smoke tests, and run the deep comparison
(a recorded live capture through both builds, `diff` the output) in CI or a harness. A vector
that changes is a finding to explain, never a fixture to update. Version the npm package
after the upstream release it compiles (`@canboat/wasm@8.3.0` = canboat `v8.3.0`) and pin
the crate exactly (`version = "=8.3.0"`), so the version number tells users what's inside.

---

*Verified October 2026 with rustc 1.97 and wasm-pack 0.15 on a minimal wasm-bindgen crate:
the `pkg/.gitignore` tarball drop (1 file packed vs 5 after removing it), synchronous
CJS/ESM loading of `--target nodejs` glue, `FinalizationRegistry` in the glue, and use after
`.free()`. Packaging, performance and parity figures are from `@canboat/wasm` 8.3.0 and its
CI.*
