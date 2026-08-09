# Tauriless

Tauriless embeds the native Tauri 2 runtime in a Rust library and exposes a
small C ABI. The embedding process keeps ownership of its main thread and calls
one non-blocking Tauri event-loop iteration approximately every 16 ms.

This is a first C-ABI prototype. It deliberately has no N-API layer and no
second callback or resource abstraction.

Node-API is not required. The npm package selects the built-in FFI provided by
Node, Deno, or Bun and binds exactly the five functions in
`tauriless/include/tauriless.h` behind one small class.

## Architecture

The exported surface is intentionally small:

```c
tauriless_create(&runtime);
tauriless_send(runtime, json);
batch = tauriless_drain(runtime);
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
webviews are intentionally ignored. If omitted, the only existing webview window
is used; with several, `main` is preferred. If several exist and none is named
`main`, the request fails and the caller must provide a label explicitly.
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
delivered to JavaScript. This experimental rule also affects channels created by
code inside real webviews; ordinary invoke promise results are separate and
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
`tauriless_drain` returns a borrowed NUL-terminated UTF-8 JSON pointer, following
the TDLib receive pattern. It remains valid until the next `tauriless_drain` or
`tauriless_destroy` call. Hosts must copy or decode it before that point; the
JavaScript adapter converts it to a JavaScript string and calls `JSON.parse()`
synchronously, so Rust can safely replace the backing buffer on the next drain.
`tauriless_last_error` similarly borrows thread-local storage until the next ABI
error on that thread.

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
described in
[Slint and the Node.js Event Loop](https://slint.dev/blog/slint-and-the-nodejs-event-loop).

## Deno FFI end-to-end demo

[`examples/deno_ffi_demo.js`](examples/deno_ffi_demo.js) is a single-file,
repeatable manual test. It contains the complete HTML document and passes it in
the fragment of the built-in app URL to the generic
`tauriless/assets/index.html` loader; it uses no web server, separate demo page,
or external JavaScript dependencies. Keeping a real app URL is important on
WebView2 because WRY 0.55.1 cannot parse the empty IPC source reported for a
top-level `data:` document. The demo creates the real webview through
`tauriless_send`, then creates its menu and tray through the same generic path.

From the workspace root, build the DLL, copy the supplied Deno executable beside
the debug DLL, and run the demo:

```powershell
cmd /d /s /c "call msvc\vcvars-x64.bat >nul && cargo build --manifest-path tauriless\Cargo.toml"
Copy-Item -LiteralPath .\deno.exe -Destination .\tauriless\target\debug\deno.exe -Force
.\tauriless\target\debug\deno.exe run --allow-ffi .\examples\deno_ffi_demo.js
```

The debug-directory copy is intentional: the upstream notification plugin
recognizes executables under `target/debug` as development applications and does
not require an installed Windows AppUserModelID. Use **Esci** in the tray menu
or `Ctrl+C` to shut the runtime down.

## npm package and cross-platform release

The single `npm/index.js` adapter selects Node `node:ffi`, Deno FFI, or
`bun:ffi`; it contains no native addon. It is published through GitHub Packages as
`@mefistofelix/tauriless`. A release contains these x86-64 dynamic libraries in
one tarball:

- `native/win32-x64/tauriless.dll`
- `native/darwin-x64/libtauriless.dylib`
- `native/linux-x64/libtauriless.so`

Configure the GitHub registry and install it:

```console
npm login --scope=@mefistofelix --auth-type=legacy --registry=https://npm.pkg.github.com
npm install @mefistofelix/tauriless
```

The login uses your GitHub username and a classic GitHub personal access token
with `read:packages`; it is not an npmjs.com account.

Node, Deno, and Bun use the same class:

```js
import { Tauriless } from "npm:@mefistofelix/tauriless";

const tauriless = new Tauriless();
tauriless.send({
  id: 1,
  cmd: "plugin:webview|create_webview_window",
  payload: { options: { label: "main", title: "Tauriless" } },
});

const timer = setInterval(() => {
  for (const message of tauriless.drain().messages) console.log(message);
}, 16);
```

The class adds only UTF-8 JSON encoding and immediate copying/parsing of the
borrowed drain string; it has no callbacks, resource layer, or internal timer.
Run Node 26.1+ with `--experimental-ffi`, Deno with `--allow-ffi`, or Bun
normally. Deno reads the consuming project's `.npmrc`, including the GitHub
scope and token.

## PHP FFI example

PHP loads the same release binary directly. Set `TAURILESS_LIBRARY_PATH` to the
downloaded `.dll`, `.so`, or `.dylib` and enable the PHP FFI extension:

```php
<?php
$library = getenv('TAURILESS_LIBRARY_PATH');
if ($library === false) throw new RuntimeException('Set TAURILESS_LIBRARY_PATH');

$ffi = FFI::cdef(<<<'C'
typedef struct Tauriless Tauriless;
int tauriless_create(Tauriless **out);
int tauriless_send(Tauriless *runtime, const char *json);
const char *tauriless_drain(Tauriless *runtime);
int tauriless_destroy(Tauriless *runtime);
const char *tauriless_last_error(void);
C, $library);

function checkStatus(FFI $ffi, int $status, string $operation): void {
    if ($status === 0) return;
    $error = $ffi->tauriless_last_error();
    $message = FFI::isNull($error) ? '' : FFI::string($error);
    throw new RuntimeException("$operation: $message");
}

$runtime = $ffi->new('Tauriless *');
checkStatus($ffi, $ffi->tauriless_create(FFI::addr($runtime)), 'create');

$request = json_encode([
    'id' => 1,
    'cmd' => 'plugin:webview|create_webview_window',
    'payload' => ['options' => ['label' => 'main', 'title' => 'Tauriless']],
], JSON_THROW_ON_ERROR);
$bytes = $ffi->new('char[' . (strlen($request) + 1) . ']', false);
FFI::memcpy($bytes, $request, strlen($request));
checkStatus(
    $ffi,
    $ffi->tauriless_send($runtime, $bytes),
    'send',
);

try {
    while (true) {
        $batch = $ffi->tauriless_drain($runtime);
        if (FFI::isNull($batch)) checkStatus($ffi, 2, 'drain');
        $messages = json_decode(
            FFI::string($batch),
            true,
            flags: JSON_THROW_ON_ERROR,
        );
        foreach ($messages['messages'] ?? [] as $message) {
            echo json_encode($message, JSON_THROW_ON_ERROR), PHP_EOL;
        }
        usleep(16_000);
    }
} finally {
    checkStatus($ffi, $ffi->tauriless_destroy($runtime), 'destroy');
}
```

`.github/workflows/release-native.yml` builds each binary on its matching native
GitHub-hosted x86-64 runner: Windows Server, macOS Intel, and Ubuntu. There is
no cross-compilation, Zig, custom SDK, or Docker involved. Pushing a `vX.Y.Z`
tag creates a GitHub Release containing the three dynamic libraries, C header,
and npm tarball. The Release is created first, and each native runner uploads
its binary as soon as that build finishes.

After all three native builds succeed, the native workflow dispatches the separate
`.github/workflows/publish-npm.yml` workflow. It downloads those exact release
binaries, assembles the precompiled module, publishes it to GitHub Packages
through the repository `GITHUB_TOKEN`, and attaches the npm tarball back to the
Release.

Both workflows are independently runnable from GitHub Actions. A manual native
run accepts a release tag and one component: `prepare`, `windows`, `macos`,
`linux`, or `all`. The npm workflow accepts an existing complete release tag,
so a failed publication can be retried without rebuilding native libraries.
Package versions are immutable once successfully published. The tag must match
the Cargo version. ARM and musl are not release targets, and Linux consumers
still need Tauri's GTK/WebKitGTK runtime libraries. No npmjs account, npm token,
or repository secret is required for publishing.

## Code running in a webview

Tauriless injects no JavaScript. Code in a webview uses the standard Tauri IPC
surface already installed by Tauri, directly or through the upstream JS API:

```js
await window.__TAURI_INTERNALS__.invoke("plugin:app|name", {});
```

There is intentionally no Tauriless implementation or compatibility promise for
`@tauri-apps/api`. Standard Tauri APIs can be used inside a webview only to the
extent that the selected upstream Tauri version and plugins already provide
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
`tauriless/capabilities/default.json` before using Tauriless outside a trusted
embedding environment.

## Versioning note

The bridge uses Tauri's public `Webview::on_message`/`InvokeRequest` path to
avoid recreating its IPC and resource machinery. Tauri documents this surface as
not yet stable, so the workspace pins the unmodified crates.io Tauri 2.11.5
release; upgrades must be explicit and the end-to-end demo must be retested.
