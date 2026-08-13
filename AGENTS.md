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
- `tauriless_create` creates only the bridge state. Build `tauri::App` lazily on
  the first `plugin:webview|create_webview_window` request. Before that build,
  `tauriless_drain` only flushes bridge-owned outbox messages; after the build it
  performs exactly one non-blocking `App::run_iteration` per call. The host owns
  the 16 ms timer.
- Create the bridge and lazily build/pump Tauri on the host's main OS thread.
  Every call on a given instance must happen on the thread that created it.
- Keep the foreign interface to opaque instance pointers plus NUL-terminated
  UTF-8 JSON strings. Do not mirror Tauri's resource table in the bridge.
- Windows and webviews remain owned by Tauri and are addressed by Tauri labels.
- Do not implement a compatibility layer for `@tauri-apps/api`. It is available
  only insofar as the standard Tauri Builder and plugins provide it for free.
  Never replace `window.__TAURI_INTERNALS__` or duplicate built-in core plugins.
- Do not create a bootstrap WebView2 instance or inject a Tauriless JavaScript
  bridge. Start with no window or webview.
- With no webview, accept `plugin:webview|create_webview_window` plus the
  bridge-owned asset response and named-event subscription controls. Deserialize
  the create command's upstream `WindowConfig` and build it with Tauri's public
  `WebviewWindowBuilder`.
- Override Tauri's compile-time `tauri` asset resolver through its public
  asynchronous URI scheme hook. Forward requests to drain as `asset-request`
  messages and accept `tauriless:asset-response` through the existing send ABI.
  A response may contain a local `path` for Rust to read or UTF-8 `content`,
  with optional status, headers, and MIME. Bridge-owned controls are limited to
  that asset response, Windows `tauriless:set-app-user-model-id`, webview-window
  creation, and exact-name event subscribe/unsubscribe. The Windows AppUserModelID
  control remains available only before the lazy Tauri app build. Its canonical
  payload is `{ "appId": "...", "name": "..." }` (`appID` remains an accepted
  alias and `name` is optional). It must
  obtain the current process executable with `GetModuleFileNameW`, create or
  update the shortcut directly as `FOLDERID_Programs/<name>.lnk`, without a
  `Tauriless` subdirectory. If `name` is omitted, infer the script filename
  stem from the process command line and fall back to the executable filename
  stem. `IShellLinkW::SetPath` must always be refreshed to the absolute current
  executable, and `PKEY_AppUserModel_ID` must be refreshed through
  `IPropertyStore` before saving with `IPersistFile`. Then call
  `SetCurrentProcessExplicitAppUserModelID`. Do not spawn PowerShell or any
  helper process. The successful result must include `shortcutPath`; failures
  must be structured with clear `operation` and `message` fields and include
  `shortcutPath` whenever it has been resolved. Persist the successful value in
  bridge state and copy it into `generate_context!().config_mut().identifier`
  immediately before `Builder::build`; if the command is never sent, set the
  Tauri identifier to exactly `Tauriless`. Reject attempts to change it after
  the app is built. Shortcut creation and the explicit process AppUserModelID
  must both complete before Tauri and WebView initialization.
- On Windows, every `plugin:webview|create_webview_window` request must explicitly
  set the builder data directory. If upstream `WindowConfig.dataDirectory` is
  present, preserve the Tauri 2.11.5 relative LocalData resolution and reject
  absolute/parent-traversing values. If it is omitted, use
  `%LOCALAPPDATA%/Tauriless/<sha256>` where the SHA-256 input is the exact UTF-16LE
  absolute executable path returned by `GetModuleFileNameW(NULL, ...)`. Thus the
  default WebView2 profile depends only on the executable path, never on the
  Tauri/AppUserModel identifier, and moving/copying an executable yields a new
  profile. Apply this rule to subsequent webview-window creation too, not only
  the first one. Do not replace this hash with the shortcut `name`. The command
  result must expose the resolved `webviewDataDirectory`; creation/path errors
  must be structured with `operation` and `message` and include that path once
  it has been resolved.
- Forward host requests to Tauri's own `Webview::on_message` dispatcher using
  native Tauri command names and payloads after the first real webview exists.
  Consider only stable `WebviewWindow` instances, not child webviews. An omitted
  source label selects the sole webview window, then `main` if several exist; if
  several exist without `main`, require an explicit label.
- Intercept every Tauri `Channel<T>` delivery into the drain outbox and consume
  it before JavaScript delivery. This experimental behavior intentionally also
  applies to channels created by code inside real webviews.
- Forward the exact named Rust event-bus emissions from Tauri core and the
  audited official plugins-workspace to the drain outbox. This includes the
  `tauriless://webview-message` application event emitted by webviews through
  Tauri's standard event plugin; do not replace it with a custom application
  command. Keep dynamic `Channel<T>` traffic in the global interceptor instead.
- Use that default named-event set as the initial subscription set. Let hosts
  add, remove, and later restore any exact name, including a default, with
  bridge-owned controls backed by Tauri's public target listeners; never
  interpret an event name as a wildcard.
- Do not add custom application commands or reimplement window, tray, menu,
  notification, opener, or resource operations in this bridge.
- Keep the official notification, opener, OS, positioner (with tray support),
  and store plugins initialized. Other plugins-workspace crates remain optional
  and uncompiled unless the user explicitly changes that set. Do not link the
  current dialog plugin into the Windows cdylib: its forced Common Controls v6
  import prevents loading from arbitrary hosts without an activation manifest.
- Return asynchronous results and events from `tauriless_drain`; never call a
  foreign-language callback from Rust.
- Do not add dependency scheduling or readiness queues. A host that creates a
  window, webview, tray, or resource must observe that request's result `id` in
  `tauriless_drain` before issuing operations that depend on the new object.
- No background GUI thread. This would violate macOS main-thread requirements
  and would not solve JavaScript main-thread callback affinity.
- All exported C functions must prevent Rust panics from crossing the ABI.
- Use the pinned, unmodified crates.io Tauri release. Do not patch Tauri or WRY.
- Keep the default application icon based on the official `create-tauri-app`
  scaffold assets. `tauriless/icons/icon.png` must remain a real RGBA PNG (PNG
  color type 6), not indexed/grayscale; `build.rs` deliberately asserts this so
  GitHub release builds fail locally and clearly if the icon regresses. Keep the
  Windows `.ico` alongside it and do not restore the old generated 1x1 icons.

## npm release

- The npm package ships the same C-ABI dynamic library; it must not introduce a
  Node-API addon or a second Rust bridge.
- Publish it publicly to the main npm registry as `@mefistofelix/tauriless`
  through npm Trusted Publishing (OIDC). The publish workflow must request
  `id-token: write` and must not depend on an npm access token.
- Supported release targets are x86-64 Windows MSVC, macOS Intel, and Linux
  glibc. ARM and musl are intentionally out of scope.
- Android and iOS are out of scope. Do not add mobile-only plugin APIs or map
  mobile plugin-listener events into the desktop bridge.
- Keep the npm JavaScript adapter in one file and bind exactly the five C ABI
  functions through the built-in FFI of Node, Deno, or Bun. Its class may only
  normalize create, JSON send, borrowed JSON drain, error copying, and
  destruction. Do not add callbacks, resource wrappers, or a timer.
- Follow the TDLib receive lifetime pattern: Rust owns the NUL-terminated drain
  JSON and may replace it on the next drain or destroy. Every host must copy or
  decode it synchronously before that point; no exported buffer-free function.
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
