# tauriless

Tauriless binds the same six C-ABI functions through the built-in FFI of Node,
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
typedef struct {
  unsigned char *data;
  unsigned long long len;
  unsigned long long capacity;
} TaurilessBuffer;
int tauriless_create(Tauriless **);
int tauriless_send(Tauriless *, const unsigned char *, unsigned long long);
int tauriless_drain(Tauriless *, TaurilessBuffer *);
int tauriless_destroy(Tauriless *);
int tauriless_last_error(TaurilessBuffer *);
void tauriless_buffer_free(void *, unsigned long long, unsigned long long);
C, $library);
```

The repository README contains a complete PHP example with JSON send, a 16 ms
drain loop, owned-buffer freeing, error copying, and destruction.
