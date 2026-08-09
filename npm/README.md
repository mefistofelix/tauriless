# tauriless

Tauriless embeds Tauri's native Rust runtime in a dynamic library while Node
keeps ownership of its main thread. It exposes the project's small C ABI through
Node's built-in FFI; there is no Node-API addon.

Requirements:

- Node.js 26.1 or newer, started with `--experimental-ffi`
- x86-64 Windows, macOS, or glibc Linux
- Linux system WebKitGTK/GTK libraries required by Tauri

```js
import { createTauriless } from "tauriless";

const runtime = createTauriless();
runtime.send({
  id: 1,
  cmd: "plugin:webview|create_webview_window",
  payload: { options: { label: "main", title: "Tauriless" } },
});

const stop = runtime.start((message) => console.log(message), 16);

// Later:
stop();
runtime.close();
```

Run the program with:

```console
node --experimental-ffi app.mjs
```

`send()` and `drain()` must run on the same main OS thread that constructed the
instance. `start()` is only the generic 16 ms timer convenience wrapper; callers
may invoke `drain()` from their own scheduler instead.

The complete protocol and native C header live in the
[GitHub repository](https://github.com/mefistofelix/tauriless).
