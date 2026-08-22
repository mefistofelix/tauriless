# Tauriless / Tauri / Tao patch stack

This directory is the canonical overview of the small fork stack required by Tauriless.
It explains what is patched, why the patches are split across repositories, and how the three repositories must be kept synchronized.

## Why the patch stack exists

Tauriless is not a normal standalone Tauri executable. It embeds Tauri behind a small C ABI so another runtime or native program can own the process and the GUI/main thread.

That host must be able to:

1. create and keep one Tauri application/runtime alive;
2. submit normal Tauri IPC/plugin/window/WebView work;
3. enter the real native event loop for a bounded duration;
4. return to the host scheduler without destroying the Tauri/Tao application state;
5. repeat that bounded pump for the lifetime of the host process.

A permanent `App::run()` does not satisfy that ownership model. Reimplementing the native event loop inside Tauriless would duplicate Tao/Tauri internals and would bypass the framework state that Tauriless deliberately wants to reuse. The solution is therefore a narrow layered patch: Tao owns the native bounded pump, Tauri exposes it through its runtime/app abstractions, and Tauriless consumes it.

The bounded operation is named **`run_timeout`** throughout the patched Rust APIs.

## Repository map

| Repository | Maintained branch | Responsibility |
| --- | --- | --- |
| [`mefistofelix/tao`](https://github.com/mefistofelix/tao) | `dev` | Implements the cross-platform native `EventLoop::run_timeout` behavior and its probe. |
| [`mefistofelix/tauri`](https://github.com/mefistofelix/tauri) | `dev` | Depends on the Tao fork, exposes `run_timeout` through `tauri-runtime-wry` and `App<Wry>`, and preserves normal Tauri event processing/state. |
| [`mefistofelix/tauriless`](https://github.com/mefistofelix/tauriless) | `main` | Embedding/C-ABI layer. Uses the patched Tauri/Tao stack and maps `tauriless_run(runtime, timeout_ms)` to the bounded Tauri pump. |

WRY is intentionally **not forked**. WebView creation/rendering stays on normal upstream WRY; the missing capability is event-loop ownership/pumping, not WebView behavior.

## How the three patches fit together

```text
host runtime / native program
        |
        | C ABI: tauriless_create / send / run / destroy
        v
mefistofelix/tauriless
        |
        | App<Wry>::run_timeout(...)
        v
mefistofelix/tauri (dev)
        |
        | Wry<T>::run_timeout(...)
        v
mefistofelix/tao (dev)
        |
        | EventLoop::run_timeout(...)
        v
AppKit / Win32 / GTK native event loop
```

### Tao patch

Tao is the only layer that knows the platform event-loop details. Its patch adds a repeated bounded run that can wait up to a supplied `Duration` but returns without applying normal permanent-loop teardown semantics. Platform state and control flow must survive across calls. A dedicated `run_timeout_probe` example checks repeated timeouts and native wake-up behavior.

See the companion documentation in [`mefistofelix/tao/tauriless_patch/README.md`](https://github.com/mefistofelix/tao/blob/dev/tauriless_patch/README.md).

### Tauri patch

Tauri's patch is an adapter, not a second event-loop implementation. `tauri-runtime-wry` is wired to the Tao fork and exposes `run_timeout`; `App<Wry>` exposes the same operation after normal setup and routes each native event through Tauri's standard event conversion/manager path. Runtime `Context` is preserved so windows, WebViews, plugins, IDs, resources and proxies remain the same between slices.

See the companion documentation in [`mefistofelix/tauri/tauriless_patch/README.md`](https://github.com/mefistofelix/tauri/blob/dev/tauriless_patch/README.md).

### Tauriless integration

Tauriless owns no replacement GUI thread. `tauriless_run(runtime, timeout_ms)` calls the patched Tauri `run_timeout`, collects bridge/event output, and returns a JSON batch to the host. The host can therefore interleave Tauri pumping with its own JS/native scheduler.

Tauriless consumes the fork branches through Cargo git dependencies. `Cargo.lock` pins the exact resolved commits used by a build; the branch names express maintenance intent while the lock file supplies reproducibility.

## Why the patches are kept in the forks

Earlier development carried complete Tauri/Tao source trees and patch files inside Tauriless. That made the dependency relationship harder to understand, duplicated large upstream trees, and made upstream updates cumbersome.

The current layout keeps each change at the layer that owns the behavior:

- Tao patch in the Tao fork;
- Tauri adapter in the Tauri fork;
- Tauriless embedding logic in Tauriless;
- no duplicated `vendor/tao`, `vendor/tauri`, or standalone patch files in Tauriless.

This also means anyone arriving through any one of the three repositories can follow the links here and reconstruct the full reason for the fork stack.

## Updating the stack

Update from the bottom upward:

1. sync `mefistofelix/tao` `dev` with upstream Tao `dev`, preserve/adapt `run_timeout`, and run its probe;
2. sync `mefistofelix/tauri` `dev` with upstream Tauri `dev`, keep it pointed at Tao `dev`, refresh its lock, and validate the runtime bridge;
3. update Tauriless' lock to the new fork heads and run its build/tests/WebView smoke tests;
4. update downstream consumers that pin a Tauriless commit.

Do not silently fall back to crates.io Tao/Tauri for Tauriless builds, do not recreate vendored copies, and do not split these changes into private one-off branches. The maintained fork branches are the authoritative patch sources.
