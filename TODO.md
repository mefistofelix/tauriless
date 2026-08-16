# TODO — macOS event-loop support

## Problem

Tauriless currently pumps Tauri from the host main thread with `App::run_iteration()` approximately every 16 ms.

On macOS this creates the native window/WebView, but WKWebView does not advance navigation. The problem reproduces on clean GitHub-hosted Apple Silicon and Intel runners.

Observed so far:

- `plugin:webview|create_webview_window` succeeds.
- `tauri://webview-created`, `tauri://window-created`, and focus events arrive.
- the initial custom-protocol request never arrives.
- replacing the async custom protocol with a synchronous Rust handler does not help.
- replacing `tauri://...` with another custom scheme does not help.
- even ordinary `https://example.com/` navigation never reaches `on_navigation` / `on_page_load` under `run_iteration()`.
- the same Tauri/Wry stack using the normal `App::run()` does navigate and reaches page-load callbacks.

Relevant upstream issue:

- https://github.com/tauri-apps/tauri/issues/5489

The Slint pattern that inspired the 16 ms host-side polling remains valid for Slint, but its externally pumpable event-loop primitive is not equivalent to Tauri/Wry's current macOS `run_iteration()` behavior:

- https://slint.dev/blog/slint-and-the-nodejs-event-loop

## Goal

Find the smallest reliable way to give Tauri/Wry a real macOS run-loop slice with a timeout, then return control to Deno/Node/Bun, preserving Tauriless' architecture:

```text
host main thread
  -> FFI
  -> pump Tauri/AppKit for <= N ms
  -> return to host
```

No background GUI thread.

## Investigate a minimal Tauri patch

The current project rule says to use unmodified crates.io Tauri. For this investigation, explicitly evaluate whether a **small, maintainable Tauri patch** is the correct macOS solution. Do not silently make it permanent; compare the result and then decide whether to relax the project rule.

Focus on the smallest patch around `tauri-runtime-wry` / Tao event-loop pumping, not on WebView, asset protocol, IPC, or Tauriless APIs.

Candidate direction:

1. Start from Tauri 2.11.5 / `tauri-runtime-wry` 2.11.4 / Tao 0.35.3 used by Tauriless.
2. Inspect `Wry::run_iteration()` and the macOS Tao `run_return()` path.
3. Add the minimum API/behavior needed for something equivalent to:

   ```rust
   app.run_for(Duration::from_millis(16), callback)
   ```

4. The timed slice must run the real AppKit/Tao loop long enough for WKWebView tasks, timers, navigation and custom-protocol callbacks to execute, then return without destroying the application/runtime state.
5. Avoid changing normal `App::run()` semantics.
6. Prefer a tiny runtime-level patch over changes spread across Tauri, Wry and Tauriless.

Things to verify while patching:

- Tauri's current `run_iteration()` sets `ControlFlow::Exit` around `MainEventsCleared`.
- `handle_event_loop` and subsequent Tao events can overwrite `ControlFlow`, so a simple external `wry_plugin` that sets `WaitUntil` may not be sufficient.
- determine exactly when Tao/macOS calls `NSApplication.run`, `stop:`, posts its dummy event, and clears the event-loop callback during `run_return()`.
- determine whether a timed `ControlFlow::WaitUntil` is sufficient once applied at the correct level, or whether macOS needs a small dedicated `NSApplication` / CFRunLoop slice.
- verify repeated timed calls do not emit `LoopDestroyed`, tear down callback state, or otherwise make the next call semantically a new event loop.

## Required A/B tests

Run all tests first on **GitHub-hosted Apple Silicon** for speed. Add Intel only after the fix works.

Baseline:

```text
unmodified Tauri
Tauriless run_iteration
https://example.com
=> expected current failure
```

Patched Tauri:

```text
patched timed run
https://example.com
=> navigation started
=> page load started
=> page load finished
```

Then restore the real Tauriless path:

```text
patched timed run
Tauriless async tauri:// asset protocol
=> asset-request reaches Deno
=> asset-response reaches Rust
=> page loads
=> browser bootstrap succeeds
```

Also verify:

- repeated polling for at least a few minutes;
- window create/show/hide/resize/close;
- Tauri IPC/event traffic after bootstrap;
- no busy loop when idle;
- timeout duration is bounded and control returns to the host;
- no regression on Windows/Linux behavior (they may keep the existing implementation if the patch is macOS-only).

## Fast GitHub runner development loop — Pigeons/Iroh

Do **not** use tmate.

Use https://github.com/n0-computer/pigeons to keep one GitHub-hosted Apple Silicon runner alive and iterate directly on it.

Desired workflow:

1. Start a manually dispatched `macos-15` job with a long but bounded timeout.
2. Checkout Tauriless and the exact Tauri source revision being tested.
3. Install Rust/Deno and build dependencies once.
4. Start a local SSH server on the runner using an **ephemeral SSH key** supplied for that run only.
5. Install/start Pigeons `roost` on the runner and expose the local SSH service through Iroh.
6. Publish only the Pigeons endpoint/connection information; never publish the private SSH key.
7. From the development machine connect through Pigeons (`fly --stdio` / SSH ProxyCommand) and work directly in the runner checkout.
8. Repeatedly edit Tauri/Tauriless, `cargo build`, rebuild the dylib, and run the Deno FFI probe on the **same runner** without pushing a new workflow for every attempt.
9. Cancel the workflow when finished so the hosted runner is released.

Security requirements:

- ephemeral SSH credentials per debug run;
- public-key authentication only;
- no tmate/web terminal;
- do not expose repository or GitHub tokens through the tunnel;
- kill the debug job immediately after testing.

## Deliverable

Once a working variant is found:

1. reduce the Tauri change to the smallest possible patch;
2. save that patch in the Tauriless repository in a clear, reproducible form;
3. make Tauriless' macOS build use the patched Tauri source while other platforms stay unchanged if possible;
4. add an automated macOS Apple Silicon regression test that proves a real WebView page reaches bootstrap;
5. document why the patch is required and link the upstream Tauri issue;
6. decide whether to upstream the fix to Tauri/Tao and eventually return to an unmodified crates.io dependency.
