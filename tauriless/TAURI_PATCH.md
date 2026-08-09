# Tauriless patch to Tauri 2.11.5

The repository does not commit the complete Tauri crate. It commits only the
patch kit in `patches/tauri-2.11.5`; `prepare-tauri.ps1` creates the ignored
`vendor/tauri` working copy used by the `[patch.crates-io]` entry in
`Cargo.toml`. No WRY or `tauri-runtime-wry` source is modified.

The functional upstream delta is confined to `tauri/src/webview`:

- `headless.rs` is a new internal real-or-headless `WebviewDispatch` adapter and
  defines the hidden unstable `Webview::new_headless` constructor.
- `mod.rs` only declares the module, stores the adapter in `Webview`, and
  converts real runtime webviews into it.

The headless branch owns a real native `WindowDispatcher`, so main-thread work
still runs through the runtime. Operations that require an actual platform
webview return neutral values or no-op successfully. `Channel::send` is not
changed: Tauri serializes it normally, then Tauriless' public
`channel_interceptor` consumes it before any JavaScript evaluation.

The kit deliberately contains:

- `webview-mod.patch`: the three-line logical modification;
- `headless.rs`: the only new Tauri source file;
- `webview-mod.original.rs`: the exact pinned upstream input;
- `webview-mod.patched.rs`: the expected output used for verification.

Before the first build run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\prepare-tauri.ps1
```

The script obtains the pinned crate from Cargo's registry cache, verifies the
original file, applies the committed patch, copies `headless.rs`, and verifies
both outputs. When upgrading Tauri, regenerate these four artifacts and rerun
the Rust tests plus the single-file Deno FFI demo.
