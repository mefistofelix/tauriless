# Tauriless

Tauriless embeds the native Tauri 2 runtime in a Rust library and exposes a
small C ABI. The embedding process keeps ownership of its main thread and enters
the native Tauri/Tao event loop for caller-bounded slices.

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
batch = tauriless_run(runtime, timeout_ms);
tauriless_destroy(runtime);
```

`tauriless_create` initially creates only the Tauriless bridge state: no
`tauri::App`, window, or webview exists yet. The first forwarded Tauri request
must be `plugin:webview|create_webview_window`; at that point Tauriless lazily
builds the pinned Tauri app, then deserializes the upstream `WindowConfig`
and calls `WebviewWindowBuilder::from_config(...).build()`. Bridge-owned
controls, including the Windows process AppUserModelID control, can run before
that lazy build. Once a real webview exists, all forwarded requests reuse Tauri's
own `Webview::on_message` dispatcher, ACL, plugin commands, resource table,
invoke responses, and channels.

Before the lazy app build, `tauriless_run` simply returns bridge-owned outbox
messages, so configuration requests can use the normal send/result protocol.
After the app exists, each run enters the normal native Tauri/Tao event loop,
drains ready work, and may wait up to `timeout_ms` for the next native wake.
`timeout_ms == 0` is non-blocking. A wake returns early after its work is drained.
The call then returns one UTF-8 JSON batch. Tauriless never invokes a foreign
callback from Rust and never starts a GUI thread.

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
[`examples/smoke.c`](examples/smoke.c) is a complete native host. Tauriless uses
the standard icon assets from the official `create-tauri-app` scaffold instead
of the former generated 1x1 placeholder. The committed `icon.png` is converted
to RGBA because Tauri's context generation requires an RGBA PNG; `build.rs`
asserts PNG color type 6 to prevent the release Actions regression where an
indexed/grayscale icon breaks the build.

## Request and run protocol

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

With no existing webview, the command above is the only forwarded Tauri request.
Bridge-owned controls remain available. Its options are passed to Tauri's
standard `WebviewWindowBuilder`, and its result is returned by a later run.

### Application identity

Before creating the first webview, every desktop host uses the same bridge-owned
identity command:

```json
{
  "id": 1,
  "cmd": "tauriless:set-app-user-model-id",
  "payload": {
    "appId": "com.example.myapp",
    "name": "My App"
  }
}
```

`appId` is cross-platform: Tauriless persists it before the lazy app build and
copies it to `generate_context!().config_mut().identifier` immediately before
`Builder::build`, so Tauri itself and `plugin:app|identifier` see the requested
identifier on Windows, macOS, and Linux. `appID` remains an accepted alias.
`name` is optional and is only operationally significant for the Windows
registration described below. The command is rejected after the lazy app build;
if it is omitted, the Tauri identifier remains exactly `Tauriless`.

Tauriless builds Tauri with `custom-protocol`, which is Tauri's production-mode
feature. This matters for packaged macOS applications because the notification
backend must use the configured application identity rather than development
Terminal identity.

### Windows process AppUserModelID extras

On Windows only, the same identity command also performs the native registration
extras needed by this embedding scenario. The bridge obtains the current executable with `GetModuleFileNameW(NULL, ...)`,
creates or updates `FOLDERID_Programs\My App.lnk` directly, without an
intermediate `Tauriless` directory, and always refreshes its `IShellLinkW`
target to that absolute executable. It writes `PKEY_AppUserModel_ID` through
`IPropertyStore`, commits it, and saves the `.lnk` through `IPersistFile`. An
existing shortcut with that name is updated. It then calls
`SetCurrentProcessExplicitAppUserModelID(appId)` directly. All of this completes
before Tauri or WebView is initialized. No PowerShell, helper executable, or
spawned process is involved. `name` is optional; when omitted Tauriless uses the filename stem of the host
JavaScript/TypeScript script from the process command line, or falls back to the
current executable filename stem.

Success is returned through `run()` with the resolved paths:

```json
{
  "appId": "com.example.myapp",
  "name": "My App",
  "executablePath": "C:\\Tools\\deno.exe",
  "shortcutPath": "C:\\Users\\me\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\My App.lnk"
}
```

Failures have a clear `operation` and `message`, plus the path when already
known, for example:

```json
{
  "operation": "save-start-menu-shortcut",
  "message": "Access is denied. (0x80070005)",
  "executablePath": "C:\\Tools\\deno.exe",
  "shortcutPath": "C:\\Users\\me\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\My App.lnk"
}
```

If the identity command is not sent, Tauri's identifier is exactly `Tauriless`.
Once the Tauri app has been built, changing the identity is rejected. On macOS
and Linux the command stops after configuring the Tauri identifier; only Windows
performs shortcut and explicit process AppUserModelID registration.

A Deno host should set it before the first webview and can then use the ordinary
Tauri notification plugin once a webview exists:

```js
const registration = await request("tauriless:set-app-user-model-id", {
  appId: "com.example.myapp",
  name: "My App",
});
if (registration.shortcutPath) console.log(registration.shortcutPath);

const created = await request("plugin:webview|create_webview_window", {
  options: {
    label: "main",
    title: "My App",
    url: "index.html",
  },
});
console.log(created.webviewDataDirectory);

console.log(await request("plugin:app|identifier", {}));

await request("plugin:notification|notify", {
  options: { title: "My App", body: "Notifica di prova" },
});
```

The identity command is bridge-owned and therefore works while `tauri::App` is
still absent; its result is queued in the bridge outbox and can be received by
`run()`. App identity and WebView2 storage are deliberately
independent. On Windows, if `dataDirectory` is omitted, every webview-window uses
`%LOCALAPPDATA%\\Tauriless\\<sha256>`, where `<sha256>` is the SHA-256 of the
exact absolute executable path returned by `GetModuleFileNameW`, encoded as
UTF-16LE. The same executable path therefore shares one WebView2 profile across
its windows; moving or copying the executable to another path gets another
profile, regardless of the AppUserModelID.

The successful create result contains `label` and the exact
`webviewDataDirectory`. A WebView creation failure contains `operation`,
`message`, and the same path once it has been resolved. The hash-based default
directory is intentionally unchanged and never uses the shortcut `name`.

If `dataDirectory` is supplied, it takes precedence and retains the upstream
Tauri relative-LocalData behavior; absolute paths and `..` components are
rejected. Tauriless reapplies that resolved directory explicitly through
`WebviewWindowBuilder::data_directory`, working around Tauri 2.11.5 dropping
`WindowConfig.data_directory` during conversion to runtime webview attributes.
The same interception is applied to every `plugin:webview|create_webview_window`
request, not only the first. `plugin:app|identifier` and
`plugin:notification|notify` remain ordinary upstream Tauri commands and require
a real webview.

After creation, `webview` selects a stable `WebviewWindow` source context; child
webviews are intentionally ignored. If omitted, the only existing webview window
is used; with several, `main` is preferred. If several exist and none is named
`main`, the request fails and the caller must provide a label explicitly.
`method`, `params`, and `target` are accepted as aliases for `cmd`, `payload`,
and `webview`.

### Host-provided local assets

Tauriless registers Tauri's public asynchronous URI protocol hook under the
standard `tauri` scheme. This replaces only the compile-time asset resolver: a
page still has Tauri's normal local origin (`tauri://localhost` on macOS/Linux,
`http://tauri.localhost` on Windows), standard IPC initialization, and ACL.
The complete pinned Tauri and Tao source trees live under `vendor/`; the bounded `run_for` delta is applied there and tracked in `patches/`. WRY itself is not patched.

When a webview requests `index.html`, CSS, JavaScript, an image, or another
local asset, `run()` returns:

```json
{
  "kind": "asset-request",
  "requestId": 12,
  "webview": "main",
  "method": "GET",
  "url": "tauri://localhost/",
  "headers": {}
}
```

The host answers through the existing `send()` function. It may provide a local
file path, which Rust reads as bytes, or inline UTF-8 content:

```json
{
  "id": 2,
  "cmd": "tauriless:asset-response",
  "payload": {
    "requestId": 12,
    "path": "C:\\my-app\\index.html"
  }
}
```

```json
{
  "id": 3,
  "cmd": "tauriless:asset-response",
  "payload": {
    "requestId": 13,
    "content": "body { color: white; }",
    "mime": "text/css; charset=utf-8"
  }
}
```

`status` defaults to 200 and `headers` defaults to empty. Content type precedence
is an explicit `Content-Type` header, then `mime`, then the local path extension,
then the requested URL extension, and finally `application/octet-stream`.
`content` takes precedence when both `content` and `path` are present. Unknown
paths can be answered with `status: 404` and a short `content` body. The response
command receives its normal result through a later run.

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

Because Tauri's listener API has no wildcard event name, Tauriless registers all
16 named core events exposed by the pinned Tauri version: resize, move, close,
destroy, focus, blur, scale, theme, window/webview creation, drag enter/over/drop/
leave, suspend, and resume. It also registers every exact event name emitted on
Tauri's Rust event bus by the current official `plugins-workspace` v2 audit:
`deep-link://new-url`, `log://log`, and `store://change`. Menu, tray, and most
plugin callbacks use dynamic `Channel<T>` IDs instead of event names and are
already covered generically by the global `Channel<T>` interceptor.
`tauri://created` and `tauri://error` are local JavaScript constructor signals,
not emissions on Tauri's Rust event bus, so they are intentionally not listed.

Hosts can add an exact event name without changing or rebuilding the Rust
library. These bridge-owned event commands also work before the first webview exists:

```json
{ "id": 20, "cmd": "tauriless:subscribe", "payload": { "event": "my-plugin://changed" } }
```

```json
{ "id": 21, "cmd": "tauriless:unsubscribe", "payload": { "event": "my-plugin://changed" } }
```

Subscribe and unsubscribe are idempotent. The list above is the initial set, not
a protected set: any default can be unsubscribed and later subscribed again.
Both commands immediately update existing targets and also determine which
listeners future windows and webviews receive. Their normal `kind: "result"`
acknowledgements are returned through `run()`. An added event uses the same
message shape and target metadata as a default event:

```json
{
  "kind": "event",
  "source": "webview-window",
  "window": "main",
  "event": "my-plugin://changed",
  "payload": { "value": 42 }
}
```

There is no special `source: "subscription"` marker. Names are exact and use
Tauri's valid event-name character set; `*` is not a wildcard. Events already
queued before unsubscribe remain available to the next run.

The audit distinguishes transport coverage from plugin availability. The core
plugins and notification, opener, OS, positioner, and store are
initialized in this binary.
The other official plugins listed below are not automatically compiled merely by
registering their event names; adding one still requires its Rust crate and
Tauri builder initialization.

The pinned asynchronous-output audit is:

| Tauri surface | Native asynchronous output handled by Tauriless |
| --- | --- |
| `window`, `webview`, `webviewWindow` | All 16 statically named `tauri://` core events above |
| `menu`, `tray` | Dynamic `Channel<T>` IDs, caught without a name list |
| `event` | Arbitrary application-defined names cannot be wildcard-listened through Tauri's public API; Tauriless explicitly registers `tauriless://webview-message` |
| `app`, `core`, `dpi`, `image`, `path` | No additional named asynchronous event output |
| `mocks` | JavaScript test helpers only; no native runtime output |
| deep-link, log, store | Exact Rust event-bus names registered: `deep-link://new-url`, `log://log`, `store://change` |
| fs, global-shortcut, shell, updater, upload, websocket | Dynamic `Channel<T>` output, caught generically if that plugin is later installed |
| notification | Desktop commands have no named Rust event. Its `notification` and `actionPerformed` plugin listeners are mobile-only and unsupported here |
| autostart, cli, clipboard-manager, dialog, http, opener, OS, positioner, process, SQL, stronghold, window-state | No additional named Rust event-bus output |
| barcode-scanner, biometric, haptics, nfc | Mobile functionality; unsupported by Tauriless |
| geolocation | Its watch API is channel-shaped, but the official desktop implementation currently returns defaults and does not produce watch updates; not installed |

This inventory covers every plugin directory in the official
[`plugins-workspace` v2](https://github.com/tauri-apps/plugins-workspace/tree/v2/plugins)
snapshot audited for this release. A plugin may expose commands or Rust callback
builders without emitting a named event; those are not missing event names.
Tauriless targets x86-64 Windows and Linux plus both Intel and Apple Silicon
macOS; Android and iOS plugin implementations and their plugin-listener events
are intentionally excluded.

There is no dependency scheduler or alternate plugin dispatcher in the bridge.

For binary Tauri responses, `value` is represented as `{ "bytes": [...] }`.
`tauriless_run` returns a borrowed NUL-terminated UTF-8 JSON pointer, following
the TDLib receive pattern. It remains valid until the next `tauriless_run` or
`tauriless_destroy` call. Hosts must copy or decode it before that point; the
JavaScript adapter converts it to a JavaScript string and calls `JSON.parse()`
synchronously, so Rust can safely replace the backing buffer on the next run.
`tauriless_last_error` similarly borrows thread-local storage until the next ABI
error on that thread.

## Event-loop slices

`tauriless_run(runtime, timeout_ms)` first drains work that is already ready. If
`timeout_ms` is non-zero and the native loop would otherwise sleep, it waits in
the platform event loop until a GUI wake or the deadline, whichever comes first,
then drains that wake before returning. A zero timeout never waits.

An asynchronous host can preserve the old timer-driven behavior by calling
`run(0)` periodically:

```js
const timer = setInterval(() => {
  const { messages } = JSON.parse(native.tauriless_run(runtime, 0));
  for (const message of messages) dispatchByIdOrEvent(message);
}, 16);
```

A host that deliberately wants the native GUI wait can instead call `run(16)`;
the call may return earlier on GUI activity. Windows uses Tao's Win32 message
loop plus `MsgWaitForMultipleObjectsEx`, macOS uses the normal AppKit run loop
with a bounded wake deadline, and Linux uses the GTK/GLib main context timeout.

## Deno FFI end-to-end demo

[`examples/deno_ffi_demo.js`](examples/deno_ffi_demo.js) is a single-file,
repeatable manual test. It contains the complete HTML document, writes it to one
temporary file, and returns that path when Rust emits the initial asset request;
it uses no web server, separate source file, fragment loader, or external
JavaScript dependencies. A second CSS request is answered with inline `content`
and an explicit `mime`, so both response forms are exercised. The demo creates
the real webview through `tauriless_send`, then creates its menu and tray through
the same generic path.

From the workspace root, build the DLL, copy the supplied Deno executable beside
the debug DLL, and run the demo:

```powershell
cmd /d /s /c "call msvc\vcvars-x64.bat >nul && cargo build --manifest-path tauriless\Cargo.toml"
Copy-Item -LiteralPath .\deno.exe -Destination .\tauriless\target\debug\deno.exe -Force
.\tauriless\target\debug\deno.exe run --allow-ffi --allow-write .\examples\deno_ffi_demo.js
```

The debug-directory copy is intentional: the upstream notification plugin
recognizes executables under `target/debug` as development applications and does
not require an installed Windows AppUserModelID. Use **Esci** in the tray menu
or `Ctrl+C` to shut the runtime down.

[`examples/deno_npm_demo.js`](examples/deno_npm_demo.js) is the equivalent
single-file example for the precompiled package. It imports
`npm:@mefistofelix/tauriless` directly. On Windows it first sends
`tauriless:set-app-user-model-id` with
`com.mefistofelix.tauriless.deno-npm-demo`, creates the real webview, and then
sends a startup toast through `plugin:notification|notify`. The same demo also
demonstrates bidirectional messaging: webview JavaScript emits
`tauriless://webview-message` through Tauri's standard event plugin and Deno
receives it through `run()`; Deno emits a targeted
`tauriless://host-message` back to the webview. The latter is also used by the
example's `setHtml()` helper to update a DOM subtree. Tauri exposes neither
`Webview::eval` nor a native `set_html` as public IPC commands, so no custom
command or Tauri patch is introduced.

```powershell
.\deno.exe run --allow-ffi --allow-write .\examples\deno_npm_demo.js
```

## npm package and cross-platform release

The single `npm/index.js` adapter selects Node `node:ffi`, Deno FFI, or
`bun:ffi`; it contains no native addon. It is published publicly on the main npm
registry as `@mefistofelix/tauriless` through Trusted Publishing. A release
contains these precompiled dynamic libraries in one tarball:

- `native/win32-x64/tauriless.dll`
- `native/darwin-x64/libtauriless.dylib`
- `native/darwin-arm64/libtauriless.dylib`
- `native/linux-x64/libtauriless.so`

Install it from the normal npm registry:

```console
npm install @mefistofelix/tauriless
```

No GitHub Packages `.npmrc` or personal access token is required.

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
  for (const message of tauriless.run(0).messages) console.log(message);
}, 16);
```

The class adds only UTF-8 JSON encoding and immediate copying/parsing of the
borrowed run string; it has no callbacks, resource layer, or internal timer.
Run Node 26.1+ with `--experimental-ffi`, Deno with `--allow-ffi`, or Bun
normally. Deno can resolve `npm:@mefistofelix/tauriless` directly from the public
npm registry.

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
const char *tauriless_run(Tauriless *runtime, unsigned int timeout_ms);
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
        $batch = $ffi->tauriless_run($runtime, 0);
        if (FFI::isNull($batch)) checkStatus($ffi, 2, 'run');
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

### PHP with TrueAsync

With [PHP TrueAsync](https://true-async.github.io/), keep the FFI declarations,
runtime creation, request encoding, and `tauriless_send` call above unchanged.
Replace only the final blocking `while`/`usleep` section with a coroutine whose
`Async\delay(16)` is backed by the TrueAsync/libuv event loop timer:

```php
$pump = Async\spawn(function () use ($ffi, $runtime): void {
    try {
        while (true) {
            $batch = $ffi->tauriless_run($runtime, 0);
            if (FFI::isNull($batch)) checkStatus($ffi, 2, 'run');

            // FFI::string copies Rust's borrowed pointer before the next run.
            $messages = json_decode(
                FFI::string($batch),
                true,
                flags: JSON_THROW_ON_ERROR,
            );
            foreach ($messages['messages'] ?? [] as $message) {
                echo json_encode($message, JSON_THROW_ON_ERROR), PHP_EOL;
            }

            Async\delay(16);
        }
    } finally {
        checkStatus($ffi, $ffi->tauriless_destroy($runtime), 'destroy');
    }
});

Async\await($pump);
```

`delay()` suspends only this coroutine, so the TrueAsync event loop can continue
servicing its other work between GUI iterations. Do not use
`Async\spawn_thread`: creation, run, send, and destruction of one Tauriless
instance must remain on the same main OS thread.

`.github/workflows/release-native.yml` builds each binary on its matching native
GitHub-hosted runner: Windows x64, macOS Intel, macOS Apple Silicon, and Linux
x64. There is no cross-compilation, Zig, custom SDK, or Docker involved. Pushing
a `vX.Y.Z` tag creates a GitHub Release containing the four dynamic libraries,
C header, and npm tarball. The Release is created first, and each native runner
uploads its binary as soon as that build finishes.

After all four native builds succeed, the native workflow dispatches the separate
`.github/workflows/publish-npm.yml` workflow. It downloads those exact release
binaries, assembles the precompiled module, publishes it to the public npm
registry through Trusted Publishing (OIDC), and attaches the npm tarball back to
the Release.

Both workflows are independently runnable from GitHub Actions. A manual native
run accepts a release tag and one component: `prepare`, `windows`, `macos`,
`linux`, or `all`. The npm workflow accepts an existing complete release tag,
so a failed publication can be retried without rebuilding native libraries.
Package versions are immutable once successfully published. The tag must match
the Cargo version. ARM is a release target only on Apple Silicon macOS; musl is
not a release target, and Linux consumers still need Tauri's GTK/WebKitGTK
runtime libraries. No npm access token or repository secret is required for
publishing.

## Python ctypes and asyncio example

Python uses the same five C functions through the standard `ctypes` module.
Set `TAURILESS_LIBRARY_PATH` to the downloaded native library. The optional
`TAURILESS_INDEX_HTML` points at a local file for the initial document; without
it, the example returns inline HTML instead.

```python
import asyncio
import ctypes
import itertools
import json
import os

lib = ctypes.CDLL(os.environ["TAURILESS_LIBRARY_PATH"])
handle = ctypes.c_void_p()

lib.tauriless_create.argtypes = [ctypes.POINTER(ctypes.c_void_p)]
lib.tauriless_create.restype = ctypes.c_int32
lib.tauriless_send.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
lib.tauriless_send.restype = ctypes.c_int32
lib.tauriless_run.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
lib.tauriless_run.restype = ctypes.c_void_p
lib.tauriless_destroy.argtypes = [ctypes.c_void_p]
lib.tauriless_destroy.restype = ctypes.c_int32
lib.tauriless_last_error.argtypes = []
lib.tauriless_last_error.restype = ctypes.c_void_p


def last_error():
    pointer = lib.tauriless_last_error()
    return ctypes.string_at(pointer).decode("utf-8") if pointer else ""


def check(status, operation):
    if status != 0:
        raise RuntimeError(f"{operation} failed ({status}): {last_error()}")


check(lib.tauriless_create(ctypes.byref(handle)), "create")
next_id = itertools.count(1)
pending = {}


def send(request):
    encoded = json.dumps(
        request, ensure_ascii=False, separators=(",", ":")
    ).encode("utf-8")
    check(lib.tauriless_send(handle, encoded), "send")


def request(command, payload=None, webview=None):
    request_id = next(next_id)
    message = {"id": request_id, "cmd": command, "payload": payload or {}}
    if webview is not None:
        message["webview"] = webview

    future = asyncio.get_running_loop().create_future()
    pending[request_id] = future
    try:
        send(message)
    except BaseException as error:
        pending.pop(request_id, None)
        future.set_exception(error)
    return future


def report_background(future):
    if not future.cancelled() and future.exception() is not None:
        print("background request failed:", future.exception())


def handle_message(message):
    if message["kind"] == "result":
        future = pending.pop(message["id"], None)
        if future is None:
            return
        if message["ok"]:
            future.set_result(message.get("value"))
        else:
            future.set_exception(RuntimeError(str(message.get("error"))))
    elif message["kind"] == "asset-request":
        index_path = os.environ.get("TAURILESS_INDEX_HTML")
        payload = {"requestId": message["requestId"]}
        is_document = message["url"].endswith(("/", "/index.html"))
        if not is_document:
            payload.update({
                "status": 404,
                "content": "not found",
                "mime": "text/plain; charset=utf-8",
            })
        elif index_path:
            payload["path"] = os.path.abspath(index_path)
        else:
            payload.update({
                "content": "<!doctype html><h1>Tauriless + asyncio</h1>",
                "mime": "text/html; charset=utf-8",
            })

        # Do not await inside the run handler: this result needs a later run.
        request("tauriless:asset-response", payload).add_done_callback(
            report_background
        )
    else:
        print(json.dumps(message, ensure_ascii=False))


def run_once(timeout_ms=0):
    pointer = lib.tauriless_run(handle, timeout_ms)
    if not pointer:
        raise RuntimeError(f"run failed: {last_error()}")
    # Copy and decode Rust's borrowed string before the next run.
    batch = json.loads(ctypes.string_at(pointer).decode("utf-8"))
    for message in batch.get("messages", []):
        handle_message(message)


async def main():
    loop = asyncio.get_running_loop()
    stopped = loop.create_future()
    timer = None

    def run_tick():
        nonlocal timer
        try:
            run_once(0)                    # non-blocking native GUI slice
        except BaseException as error:
            if not stopped.done():
                stopped.set_exception(error)
        else:
            timer = loop.call_later(0.016, run_tick)

    timer = loop.call_soon(run_tick)
    try:
        await request(
            "plugin:webview|create_webview_window",
            {"options": {
                "label": "main",
                "title": "Tauriless + Python",
                "url": "index.html",
                "visible": True,
            }},
        )
        await stopped                      # application work continues here
    finally:
        if timer is not None:
            timer.cancel()
        check(lib.tauriless_destroy(handle), "destroy")


asyncio.run(main())
```

`ctypes.string_at()` performs the required synchronous copy of the borrowed
run buffer. Do not call Tauriless through `asyncio.to_thread()` or an executor:
creation, send, run, and destruction of one runtime must all remain on the
main OS thread. `loop.call_later()` schedules one 16 ms timer callback at a time,
so the rest of the Python event loop continues to run between GUI iterations.

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

The package name `@tauri-apps/api` refers to the upstream JavaScript client and
is not copied into the Tauriless npm package. A page may bundle that client or
call `window.__TAURI_INTERNALS__` directly; the native surfaces available to it
are listed here.

| JavaScript module | Included now | Notes |
| --- | --- | --- |
| `@tauri-apps/api` | Client not bundled | Its standard IPC works for native modules present below; Tauriless does not reimplement it |
| app, core, event, image, menu, path, tray, webview, window | Yes | Built-in Tauri core plugins |
| webviewWindow | Yes, as a JS facade | Composes the built-in window and webview APIs; it is not a separate Rust plugin |
| dpi | No native plugin needed | JavaScript data classes/types used by window APIs |
| mocks | No native plugin | JavaScript-only test helpers |
| notification, opener, OS, positioner, store | Yes | Explicitly compiled and initialized by Tauriless; positioner includes its tray feature |
| autostart, cli, clipboard-manager, deep-link, dialog, fs, geolocation, global-shortcut, http, log, process, SQL, stronghold, updater, upload, websocket, window-state | No | Event/channel mapping is ready where applicable, but commands are unavailable until the corresponding Rust crate is compiled and initialized |
| barcode-scanner, biometric, haptics, nfc | No; unsupported | Mobile functionality is outside the supported platforms |

Other official workspace plugins not in the shorter reference list, including
localhost, persisted-scope, and single-instance, are also not compiled.

`dialog` is intentionally not linked into the Windows `cdylib`: its current
`rfd/common-controls-v6` dependency imports `TaskDialogIndirect`, which requires
a Common Controls v6 activation manifest on the embedding executable. Deno,
Node, Bun, PHP, and other arbitrary hosts cannot be assumed to provide one;
linking it would make the DLL fail during loading before Tauriless can run.

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
