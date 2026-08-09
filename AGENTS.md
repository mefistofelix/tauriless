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
- Forward host requests to Tauri's own `Webview::on_message` dispatcher using
  native Tauri command names and payloads. Do not reimplement window, webview,
  tray, menu, notification, opener, or resource operations in this bridge.
- The hidden `__tauriless` webview is the default IPC context for host calls;
  it is a normal Tauri-configured webview, not a second resource manager.
- The only custom application command is `tauriless:event`, used to forward a
  webview event to the embedding host.
- Return asynchronous results and events from `tauriless_drain`; never call a
  foreign-language callback from Rust.
- Do not add dependency scheduling or readiness queues. A host that creates a
  window, webview, tray, or resource must observe that request's result `id` in
  `tauriless_drain` before issuing operations that depend on the new object.
- No background GUI thread. This would violate macOS main-thread requirements
  and would not solve JavaScript main-thread callback affinity.
- All exported C functions must prevent Rust panics from crossing the ABI.
- Any upstream Tauri modification requires a written rationale and a focused
  patch; none is expected for the initial prototype.

## Toolchain

The repository carries `rustup-init.exe` and `msvcup.exe`. Install the requested
Windows toolchain with:

```powershell
.\msvcup.exe install msvc msvc-14.44.17.14 sdk-10.0.22621.7 --manifest-update-always
```

Use the Rust stable toolchain unless a pinned `rust-toolchain.toml` is added.

## Verification

Run these from the repository root after changes:

```powershell
cargo fmt --all -- --check
cargo check
cargo test
```

Keep the protocol examples and `include/tauriless.h` synchronized with exports.
