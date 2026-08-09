# tauriless

Tauriless binds the same five C-ABI functions through the built-in FFI of Node,
Deno, and Bun. There is no Node-API addon and no JavaScript resource or callback
layer.

The package is hosted by GitHub Packages. Configure the consuming project's
`.npmrc` with the scope and a classic GitHub token containing `read:packages`:

```ini
@mefistofelix:registry=https://npm.pkg.github.com
//npm.pkg.github.com/:_authToken=${GITHUB_TOKEN}
```

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

All calls for an instance must execute on the main OS thread. The application
owns the approximately 16 ms timer that calls `drain()`; Tauriless never invokes
a JavaScript callback.

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
const char *tauriless_drain(Tauriless *);
int tauriless_destroy(Tauriless *);
const char *tauriless_last_error(void);
C, $library);
```

The repository README contains a complete PHP example with JSON send, a 16 ms
drain loop, synchronous JSON decoding of the borrowed result, and destruction.
It also shows the TrueAsync variant: the same FFI setup runs in an
`Async\spawn` coroutine and uses the event-loop-backed `Async\delay(16)` instead
of blocking with `usleep`. Tauriless must stay on that main OS thread; do not use
`Async\spawn_thread` for its runtime.
