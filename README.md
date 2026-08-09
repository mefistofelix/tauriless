# Tauriless

Tauriless embeds the native Tauri 2 runtime in a Rust library and exposes a
small C ABI. The embedding process keeps ownership of its main thread and calls
one non-blocking Tauri event-loop iteration approximately every 16 ms.

This is a first C-ABI prototype. It deliberately has no N-API layer and no
second callback or resource abstraction.

Node-API is not required. Node.js 26.1 added the experimental built-in
`node:ffi` module, so the npm package loads this same C ABI directly. Node must
currently be started with `--experimental-ffi`.

## Architecture

The exported surface is intentionally small:

```c
tauriless_create(&runtime);
tauriless_send(runtime, json, json_len);
tauriless_drain(runtime, &batch);
tauriless_destroy(runtime);
```

Tauriless starts with no window or webview and uses the unmodified Tauri crate.
The first request must be `plugin:webview|create_webview_window`; the bridge
deserializes its upstream `WindowConfig` payload and calls
`WebviewWindowBuilder::from_config(...).build()`. Once a real webview exists,
all requests reuse Tauri's own `Webview::on_message` dispatcher, ACL, plugin
commands, resource table, invoke responses, and channels.

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

A request uses upstream Tauri command names and payload shapes. For example:

```json
{
  "id": 1,
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

With no existing webview, the command above is the only accepted request. Its
options are passed to Tauri's standard `WebviewWindowBuilder`, and its result is
returned by a later drain.

After creation, `webview` selects a stable `WebviewWindow` source context; child
webviews are intentionally ignored. If omitted, the only existing webview
window is used; with several, `main` is preferred. If several exist and none is
named `main`, the request fails and the caller must provide a label explicitly.
`method`, `params`, and `target` are accepted as aliases for `cmd`, `payload`,
and `webview`.

Persistent callbacks use Tauri's normal `Channel<T>` payload. The bridge
intercepts every already serialized channel delivery before it reaches
JavaScript:

```json
{
  "kind": "channel",
  "webview": "main",
  "id": 9001,
  "index": 0,
  "message": "tray-show"
}
```

The interceptor returns `true`, so intercepted channels are consumed and never
delivered to JavaScript. This experimental rule also affects channels created
by code inside real webviews; ordinary invoke promise results are separate and
continue to resolve normally.

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

There is no dependency scheduler or alternate plugin dispatcher in the bridge.

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
the real webview through `tauriless_send`, then creates its menu and tray through
the same generic path.

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

## npm package and cross-platform release

The `npm/` package contains a small ESM loader for Node's built-in FFI and no
native addon. A release contains these x86-64 dynamic libraries in one tarball:

- `native/win32-x64/tauriless.dll`
- `native/darwin-x64/libtauriless.dylib`
- `native/linux-x64/libtauriless.so`

Use it with Node 26.1 or newer:

```console
node --experimental-ffi app.mjs
```

`.github/workflows/release-native.yml` builds each binary on its matching native
GitHub-hosted x86-64 runner: Windows Server, macOS Intel, and Ubuntu. There is no
cross-compilation, Zig, custom SDK, or Docker involved. Pushing a `vX.Y.Z` tag
creates a GitHub Release containing the three dynamic libraries, C header, and
checksums.

After publishing the GitHub Release, the native workflow dispatches the
separate `.github/workflows/publish-npm.yml` workflow. It downloads those exact
release binaries, assembles the precompiled module, publishes it to npm, and
attaches the npm tarball back to the Release. It can also be rerun manually for
an existing tag. The tag must match the Cargo version. ARM and musl are not
release targets, and Linux consumers still need Tauri's GTK/WebKitGTK runtime
libraries. The first npm publication needs an `NPM_TOKEN` repository secret;
afterward npm trusted publishing can use `publish-npm.yml` and its GitHub OIDC
identity.

## Code running in a webview

Tauriless injects no JavaScript. Code in a webview uses the standard Tauri IPC
surface already installed by Tauri, directly or through the upstream JS API:

```js
await window.__TAURI_INTERNALS__.invoke("plugin:app|name", {});
```

There is intentionally no Tauriless implementation or compatibility promise
for `@tauri-apps/api`. Standard Tauri APIs can be used inside a webview only to
the extent that the selected upstream Tauri version and plugins already provide
them. Tauriless neither replaces `__TAURI_INTERNALS__` nor mirrors those APIs.

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
as not yet stable, so the workspace pins the unmodified crates.io Tauri 2.11.5
release; upgrades must be explicit and the end-to-end demo must be retested.
