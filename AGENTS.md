# Tauriless project instructions

## Objective

Embed Tauri's native Rust runtime in a library that a host language can pump
from its main thread without surrendering control of that thread.

The first milestone is a Rust `cdylib` exposing a small C ABI. Do not add an
N-API layer: Deno calls the C ABI directly and Node 26.1+ can use its built-in
experimental `node:ffi` module.

## Design constraints

- Reuse the published Tauri crates and their public APIs. Prefer adding code in
  this repository over patching or forking Tauri.
- `tauriless_drain` performs exactly one non-blocking `App::run_iteration` and
  returns immediately. The host owns the 16 ms timer.
- Create and pump Tauri on the host's main OS thread. Every call on a given
  instance must happen on the thread that created it.
- Keep the foreign interface to opaque instance pointers plus UTF-8 JSON byte
  slices. Do not mirror Tauri's resource table in the bridge.
- Windows and webviews remain owned by Tauri and are addressed by Tauri labels.
- Do not implement a compatibility layer for `@tauri-apps/api`. It is available
  only insofar as the standard Tauri Builder and plugins provide it for free.
  Never replace `window.__TAURI_INTERNALS__` or duplicate built-in core plugins.
- Do not create a bootstrap WebView2 instance or inject a Tauriless JavaScript
  bridge. Start with no window or webview.
- With no webview, accept only `plugin:webview|create_webview_window`,
  deserialize its upstream `WindowConfig`, and build it with Tauri's public
  `WebviewWindowBuilder`. This is the bridge's only special-cased command.
- Forward host requests to Tauri's own `Webview::on_message` dispatcher using
  native Tauri command names and payloads after the first real webview exists.
  Consider only stable `WebviewWindow` instances, not child webviews. An omitted
  source label selects the sole webview window, then `main` if several exist; if
  several exist without `main`, require an explicit label.
- Intercept every Tauri `Channel<T>` delivery into the drain outbox and consume
  it before JavaScript delivery. This experimental behavior intentionally also
  applies to channels created by code inside real webviews.
- Do not add custom application commands or reimplement window, tray, menu,
  notification, opener, or resource operations in this bridge.
- Return asynchronous results and events from `tauriless_drain`; never call a
  foreign-language callback from Rust.
- Do not add dependency scheduling or readiness queues. A host that creates a
  window, webview, tray, or resource must observe that request's result `id` in
  `tauriless_drain` before issuing operations that depend on the new object.
- No background GUI thread. This would violate macOS main-thread requirements
  and would not solve JavaScript main-thread callback affinity.
- All exported C functions must prevent Rust panics from crossing the ABI.
- Use the pinned, unmodified crates.io Tauri release. Do not patch Tauri or WRY.

## npm release

- The npm package ships the same C-ABI dynamic library; it must not introduce a
  Node-API addon or a second Rust bridge.
- Publish it to GitHub Packages as `@mefistofelix/tauriless`. The publishing
  workflow must use the repository `GITHUB_TOKEN`, not an npmjs token.
- Supported release targets are x86-64 Windows MSVC, macOS Intel, and Linux
  glibc. ARM and musl are intentionally out of scope.
- Keep the npm JavaScript adapter in one file and bind exactly the six C ABI
  functions through the built-in FFI of Node, Deno, or Bun. Its class may only
  normalize create, JSON send, JSON drain, error copying, destruction, and
  native-buffer freeing. Do not add callbacks, resource wrappers, or a timer.
- Build each release binary on its matching GitHub-hosted x86-64 runner and
  publish them in a GitHub Release. A separate release-triggered workflow
  downloads those binaries and publishes the single npm tarball. Do not mix npm
  publishing into a native build job, and do not cross-compile unless native
  runner availability changes.
- Keep native platform builds and npm publication manually runnable as
  independent workflow components. A full tagged release may orchestrate them,
  but retrying one native platform or npm publication must not require rebuilding
  unrelated platforms.
- Never commit generated native binaries or npm tarballs. The release workflow
  stages them under `npm/native` and publishes the resulting package.

## Toolchain

The workspace root carries `rustup-init.exe` and `msvcup.exe`; the Rust/Tauri
crate lives in `tauriless/`. Install the requested Windows toolchain from the
workspace root with:

```powershell
.\msvcup.exe install msvc msvc-14.44.17.14 sdk-10.0.22621.7 --manifest-update-always
```

Use the Rust stable toolchain unless a pinned `rust-toolchain.toml` is added.

## Verification

Run these from the `tauriless` project directory after changes:

```powershell
cargo fmt --all -- --check
cargo check
cargo test
```

Keep the protocol examples and `tauriless/include/tauriless.h` synchronized with
exports.
