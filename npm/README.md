# tauriless

Tauriless binds the same five C-ABI functions through the built-in FFI of Node,
Deno, and Bun. There is no Node-API addon and no JavaScript resource or callback
layer.

The package is published publicly on the npm registry, so consumers do not
need a custom registry or authentication. It bundles native libraries for
Windows x64, Linux x64, and both Intel and Apple Silicon macOS.

```js
import { Tauriless } from "npm:@mefistofelix/tauriless";

const tauriless = new Tauriless();
tauriless.send({
  id: 1,
  cmd: "plugin:webview|create_webview_window",
  payload: { options: { label: "main", title: "Tauriless" } },
});

const timer = setInterval(() => {
  for (const message of tauriless.run(16).messages) console.log(message);
}, 16);

// Later:
clearInterval(timer);
tauriless.close();
```

The repository also contains a
[single-file Deno demo](https://github.com/mefistofelix/tauriless/blob/main/examples/deno_npm_demo.js)
that imports this package directly. Its embedded webview demonstrates
bidirectional Tauri events, host-driven `setHtml`, drag and drop, OS
notifications, and a tray menu without a web server or separate HTML file.

- Node 26.1+: run with `--experimental-ffi` and use `node:ffi` pointer helpers.
- Deno: run with `--allow-ffi`.
- Bun: no additional flag is required.

All calls for an instance must execute on the main OS thread. `run(timeoutMs)`
processes ready GUI work and may wait natively up to the supplied timeout,
returning earlier when the GUI loop wakes. Tauriless never invokes a JavaScript
callback.

On every desktop platform, `tauriless:set-app-user-model-id` may be called before
the first webview with `{ appId: "com.example.app", name?: "Example App" }`.
`appId` becomes Tauri's application identifier before `Builder::build`. Tauriless
builds Tauri in production/custom-protocol mode so packaged macOS notifications
use that application identity rather than development Terminal identity. Windows
uses the same command and additionally performs the native process/Start Menu
AppUserModelID registration; its optional `name` selects the direct Start Menu
`<name>.lnk`. The Windows result exposes `shortcutPath`, and
`plugin:webview|create_webview_window` exposes `webviewDataDirectory`. Failures
are structured objects with `operation`, `message`, and the applicable resolved
path.

Additional exact Tauri event names can be forwarded with the same `send()` API:

```js
tauriless.send({
  id: 2,
  cmd: "tauriless:subscribe",
  payload: { event: "my-plugin://changed" },
});
// Later use `tauriless:unsubscribe` with the same payload. This also works for
// names from Tauriless' initial built-in event set.
```

The complete protocol and C header are in the
[GitHub repository](https://github.com/mefistofelix/tauriless).

## PHP FFI

PHP loads the same native Release binary directly, without a language adapter:

```php
$library = getenv('TAURILESS_LIBRARY_PATH');
if ($library === false) throw new RuntimeException('Set TAURILESS_LIBRARY_PATH');

$ffi = FFI::cdef(<<<'C'
typedef struct Tauriless Tauriless;
int tauriless_create(Tauriless **);
int tauriless_send(Tauriless *, const char *);
const char *tauriless_run(Tauriless *, uint32_t timeout_ms);
int tauriless_destroy(Tauriless *);
const char *tauriless_last_error(void);
C, $library);
```

The repository README contains a complete PHP example with JSON send, bounded
native event-loop slices, synchronous JSON decoding of the borrowed result, and
destruction.
It also shows the TrueAsync variant: the same FFI setup runs in an
`Async\spawn` coroutine and uses the event-loop-backed `Async\delay(16)` instead
of blocking with `usleep`. Tauriless must stay on that main OS thread; do not use
`Async\spawn_thread` for its runtime.
