# Tauriless

Tauriless embeds the native Tauri 2 runtime in a Rust library and exposes a
small C ABI. The embedding process keeps ownership of its main thread and calls
one non-blocking Tauri event-loop iteration approximately every 16 ms.

This is a first C-ABI prototype. It deliberately has no N-API layer and no
second window, webview, callback, or resource abstraction.

## Architecture

The exported surface is intentionally small:

```c
tauriless_create(&runtime);
tauriless_send(runtime, json, json_len);
tauriless_drain(runtime, &batch);
tauriless_destroy(runtime);
```

`tauriless_send` forwards a native Tauri command and payload to Tauri's own IPC
dispatcher through a hidden, ordinary Tauri webview named `__tauriless`. Tauri
therefore remains the only owner of its windows, webviews, tray icons, menus,
resources, callback resolution, and ACL checks.

`tauriless_drain` performs exactly one `App::run_iteration`, collects completed
IPC responses, Tauri events, and Tauri channel messages, and returns one UTF-8
JSON batch. It never invokes a foreign callback from Rust and it never starts a
GUI thread.

Every operation on an instance, including destruction, must happen on the OS
thread that called `tauriless_create`; for GUI hosts this must be the main
thread.

## Build on Windows

The toolchain executables and this README live in the workspace root; the crate
lives in `tauriless/`. Install the requested toolchain from the workspace root:

```powershell
.\msvcup.exe install msvc msvc-14.44.17.14 sdk-10.0.22621.7 --manifest-update-always
```

Build the crate from the workspace root inside the MSVC environment:

```powershell
cmd /d /s /c "call msvc\vcvars-x64.bat >nul && cargo build --manifest-path tauriless\Cargo.toml"
```

The debug DLL and import library are written to `tauriless/target/debug`. The C
header is [`tauriless/include/tauriless.h`](tauriless/include/tauriless.h), and
[`examples/smoke.c`](examples/smoke.c) is a complete native host.

## Request and drain protocol

A request uses upstream Tauri command names and upstream payload shapes:

```json
{
  "id": 1,
  "cmd": "plugin:app|name",
  "payload": {},
  "webview": "__tauriless"
}
```

`webview` is optional and defaults to `__tauriless`; `method`, `params`, and
`target` are accepted as aliases for `cmd`, `payload`, and `webview`.

A later drain contains the correlated Tauri response without an additional
result model:

```json
{
  "messages": [
    { "kind": "result", "id": 1, "ok": true, "value": "Tauriless" }
  ]
}
```

Persistent callbacks use Tauri's normal `Channel<T>` payload. When a command
sent through the hidden IPC context contains a channel such as
`"__CHANNEL__:9001"`, the bridge intercepts Tauri's already serialized channel
delivery before it reaches JavaScript:

```json
{
  "kind": "channel",
  "webview": "__tauriless",
  "id": 9001,
  "index": 0,
  "message": "tray-show"
}
```

The interceptor applies only to the hidden bridge webview. Channels created by
JavaScript in real webviews retain Tauri's standard behavior.

Native window and webview events also reuse Tauri's Rust serialization and keep
their upstream event names and payloads:

```json
{
  "kind": "event",
  "source": "webview-window",
  "window": "main",
  "event": "tauri://drag-drop",
  "payload": {
    "paths": ["C:\\example.txt"],
    "position": { "x": 120.0, "y": 80.0 }
  }
}
```

For example, the native Tauri webview command can create a window:

```json
{
  "id": 2,
  "cmd": "plugin:webview|create_webview_window",
  "payload": {
    "options": {
      "label": "main",
      "title": "Main",
      "url": "index.html",
      "visible": true
    }
  }
}
```

There is no dependency scheduler in the bridge. Before sending another command
that addresses `main`, the host must observe the successful result for `id: 2`
in `tauriless_drain`. The same rule applies to Tauri resource IDs.

For binary Tauri responses, `value` is represented as `{ "bytes": [...] }`.
Buffers returned by `tauriless_drain` or `tauriless_last_error` belong to the
caller and must be released with `tauriless_buffer_free`.

## Host timer

The language adapter owns the timer and callback map. In JavaScript-like
pseudocode:

```js
const timer = setInterval(() => {
  const { messages } = JSON.parse(native.tauriless_drain(runtime));
  for (const message of messages) dispatchByIdOrEvent(message);
}, 16);
```

The native call itself does not wait for the next GUI event. The 16 ms interval
is a generic interoperability trade-off: bounded GUI latency in exchange for a
small amount of periodic work while idle. This is the generic timer approach
described in [Slint and the Node.js Event Loop](https://slint.dev/blog/slint-and-the-nodejs-event-loop).

## Deno FFI end-to-end demo

[`examples/deno_ffi_demo.js`](examples/deno_ffi_demo.js) is a single-file,
repeatable manual test. It contains the complete HTML document and passes it in
the fragment of the built-in app URL to the generic
`tauriless/assets/index.html` loader;
it uses no web server, separate demo page, or external JavaScript dependencies.
Keeping a real app URL is important on WebView2 because WRY 0.55.1 cannot parse
the empty IPC source reported for a top-level `data:` document. The demo creates
a webview and a tray menu, reports native drag/drop paths to Deno, sends OS
notifications, and receives Tauri's serialized native events and plugin
channels through `tauriless_drain`.

From the workspace root, build the DLL, copy the supplied Deno executable beside
the debug DLL, and run the demo:

```powershell
cmd /d /s /c "call msvc\vcvars-x64.bat >nul && cargo build --manifest-path tauriless\Cargo.toml"
Copy-Item -LiteralPath .\deno.exe -Destination .\tauriless\target\debug\deno.exe -Force
.\tauriless\target\debug\deno.exe run --allow-ffi .\examples\deno_ffi_demo.js
```

The debug-directory copy is intentional: the upstream notification plugin
recognizes executables under `target/debug` as development applications and
does not require an installed Windows AppUserModelID. Use **Esci** in the tray
menu or `Ctrl+C` to shut the runtime down.

## Code running in a webview

Tauriless injects only this convenience entry point:

```js
await window.tauriless_send({
  cmd: "plugin:app|name",
  payload: {}
});
```

It is a direct JavaScript call to Tauri's existing
`window.__TAURI_INTERNALS__.invoke`; it has no extra Rust dispatcher.

There is intentionally no Tauriless implementation or compatibility promise
for `@tauri-apps/api`. Standard Tauri APIs can be used inside a webview only to
the extent that the selected upstream Tauri version and plugins already provide
them. Tauriless neither replaces `__TAURI_INTERNALS__` nor mirrors those APIs.

`window.__TAURILESS__.emit(event, payload)` is the only custom application IPC
command. It forwards an application event to the external host's next drain.

## Included Tauri functionality

The build keeps Tauri's built-in app, event, image, menu, path, resource, tray,
webview, and window plugins. It additionally initializes the official
notification, opener, and OS plugins. Native window and webview events are
forwarded from Tauri's Rust event bus; menu, tray, and other persistent plugin
callbacks use Tauri's existing channel serialization.

## Security model

The prototype intentionally grants every generated Tauri core command plus the
enabled plugins to every window, webview, and URL. Tauri's parser rejects bare
`*`, so the capability uses `*://*` for hierarchical URLs and `data:*` for
opaque inline pages. Tauri 2 still performs its normal ACL evaluation; these
patterns make the result effectively unrestricted.

This is unsafe for untrusted or remotely controlled web content: such content
can reach operating-system functionality. Tighten
`tauriless/capabilities/default.json` before using Tauriless outside a
trusted embedding environment.

## Versioning note

The bridge uses Tauri's public `Webview::on_message`/`InvokeRequest` path to
avoid recreating its IPC and resource machinery. Tauri documents this surface
as not yet stable, so the workspace pins Tauri exactly to `2.11.5`; upgrades
must be explicit and retested.
