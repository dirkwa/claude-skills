// Minimal Signal K WASM plugin in AssemblyScript: config, a poll-driven
// delta, and one HTML endpoint. Build: `npm run build`.

import {
  Delta,
  JSON,
  PathValue,
  Update,
  debug,
  emit,
  setError,
  setStatus
} from '@signalk/assemblyscript-plugin-sdk/assembly'

const DEFAULT_GREETING = 'Hello'

let greeting = DEFAULT_GREETING
let ticks: i64 = 0

export function plugin_name(): string {
  return 'My AssemblyScript WASM Plugin'
}

export function plugin_schema(): string {
  return `{"type":"object","properties":{"greeting":{"type":"string","title":"Greeting","default":"${DEFAULT_GREETING}"}}}`
}

// The config arrives as UTF-8 bytes, not as an AssemblyScript string.
// Called again on every config save, on the same instance: reset state here.
export function plugin_start(configPtr: usize, configLen: usize): i32 {
  greeting = DEFAULT_GREETING
  ticks = 0
  const parsed = JSON.parse(String.UTF8.decodeUnsafe(configPtr, configLen))
  if (!parsed.isObj) {
    setError('Invalid configuration')
    return 1
  }
  const value = (<JSON.Obj>parsed).getString('greeting')
  if (value != null) greeting = value.valueOf()
  setStatus('Running')
  return 0
}

export function plugin_stop(): i32 {
  debug('stopped')
  return 0
}

// Called every second while running.
export function poll(): i32 {
  ticks++
  const value = new PathValue('environment.myPlugin.ticks', ticks.toString())
  emit(new Delta('vessels.self', [new Update([value])]))
  return 0
}

export function http_endpoints(): string {
  return '[{"method":"GET","path":"/hello","handler":"handle_hello"}]'
}

// HTTP handler: the request context arrives as an AssemblyScript string
// (the second argument is its length in UTF-16 units); return the envelope.
export function handle_hello(_request: string, _length: i32): string {
  const html = `<!doctype html><h1>${escapeHtml(greeting)}</h1><p>ticks: ${ticks}</p>`
  return `{"statusCode":200,"headers":{"Content-Type":"text/html; charset=utf-8"},"body":${jsonString(html)}}`
}

// The SDK's bundled JSON encoder escapes control characters as a bare
// backslash, which is invalid JSON, so the envelope's body is escaped here.
function jsonString(s: string): string {
  const parts = new Array<string>()
  parts.push('"')
  let start = 0
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i)
    if (c != 0x22 && c != 0x5c && c >= 0x20) continue
    parts.push(s.substring(start, i))
    parts.push(c < 0x20 ? '\\u' + c.toString(16).padStart(4, '0') : '\\' + String.fromCharCode(c))
    start = i + 1
  }
  parts.push(s.substring(start))
  parts.push('"')
  return parts.join('')
}

// Config values and delta strings (AIS names, for example) are untrusted.
function escapeHtml(s: string): string {
  return s
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;')
}
