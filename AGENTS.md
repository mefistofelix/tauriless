# Tauriless project instructions

## Objective

Embed Tauri's native Rust runtime in a library that a host language can pump
from its main thread without surrendering control of that thread.

The first milestone is a Rust `cdylib` exposing a small C ABI. Do not add an
N-API layer until the C ABI has been validated with a real host. Deno can call
the C ABI directly; Node may receive a thin N-API adapter later.

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
  bridge. A hidden native `Window` plus Tauri's focused headless-Webview patch
  provides the standard invoke context, resource table and channel plumbing
  without constructing a platform webview.
- Forward host requests to Tauri's own `Webview::on_message` dispatcher using
  native Tauri command names and payloads. Requests without an explicit real
  webview label use the persistent `__tauriless` headless context; do not
  special-case plugin commands or duplicate their implementations.
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
- Keep the upstream Tauri modification isolated in the committed patch kit
  under `tauriless/patches/tauri-2.11.5`: a generic real-or-headless dispatcher
  wrapper in `headless.rs` and only the minimal `webview/mod.rs` patch. The
  prepared `vendor/tauri` working copy is ignored; never commit the complete
  upstream crate. Do not patch WRY.

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
