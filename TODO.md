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

## Current solution

The bounded event-loop approach is now the maintained design:

1. `mefistofelix/tao` tracks upstream `dev` and carries the cross-platform
   `EventLoop::run_for` implementation plus its probe.
2. `mefistofelix/tauri` tracks upstream `dev`, exposes bounded `run_for` through
   `tauri-runtime-wry` and `App<Wry>`, and depends directly on the Tao fork `dev`.
3. Tauriless consumes the fork `dev` branches through Cargo git dependencies;
   `Cargo.lock` records the exact commits used by each build.
4. WRY remains upstream and unpatched. No Tauri/Tao source trees or duplicate
   patch files live in the Tauriless repository.

When syncing either fork with upstream, rebase/merge upstream `dev`, preserve the
bounded-run changes on that same `dev` branch, refresh Tauriless' lock file, and
rerun the native/WebView regression suite.

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

## Remaining maintenance

- Keep both forks synchronized with their upstream `dev` branches.
- Keep the Tauri fork pointed at the Tao fork `dev` so the pair cannot drift.
- Refresh `Cargo.lock` after fork updates and verify the resolved git SHAs.
- Keep the Apple Silicon WebView/bootstrap regression test green; add Intel
  coverage after the fast path remains stable.
- Upstream the bounded-run API when practical; until then the forks are the
  authoritative source of the delta.
