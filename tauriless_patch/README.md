# Tauriless patch stack

This directory is intentionally identical in the three repositories involved in the Tauriless patch stack:

- https://github.com/mefistofelix/tauriless
- https://github.com/mefistofelix/tauri
- https://github.com/mefistofelix/tao

It contains the complete cross-repository explanation plus portable patch snapshots. There is no repository-specific version of this document: starting from any of the three repositories should give the same picture of the whole stack.

## Files

- `tao.patch` — the Tao delta required by Tauriless.
- `tauri.patch` — the Tauri delta required by Tauriless, including the dependency on the patched Tao fork.
- `README.md` — this document, describing the design, motivation, dependency chain and maintenance procedure.

The patch files are review/reapplication snapshots. The maintained development sources are the `dev` branches of the Tao and Tauri forks. Do not apply these patches on top of those already-patched branches.

## Why this exists

A normal Tauri desktop application owns the native GUI event loop and enters it with a call that does not return until the application exits. That model is correct for a standalone Tauri application, but it does not fit Tauriless.

Tauriless embeds Tauri inside a host runtime that already owns the process and its main thread. Typical hosts are JavaScript runtimes such as Deno, Node.js or Bun using FFI. The host must remain in control of scheduling while still allowing the native windowing stack to process operating-system messages, webview work, tray events, redraws and Tauri events.

Running the GUI event loop on a second thread is deliberately avoided. Desktop GUI frameworks have main-thread requirements, especially on macOS, and moving Tauri/Tao to a background GUI thread would create a different and less portable architecture. Tauriless also intentionally avoids Rust calling arbitrary foreign callbacks while the event loop is active.

The required primitive is therefore a bounded native event-loop pump:

1. the host calls Tauriless on the same OS/main thread;
2. Tauriless asks Tauri to process native events;
3. Tauri delegates to Tao;
4. Tao waits for work for at most the requested timeout and drains the ready native work;
5. control returns to the host;
6. the host schedules the next slice whenever appropriate.

This keeps one GUI thread, preserves host ownership of scheduling, and allows a timer-driven integration such as repeated ~16 ms calls without creating a second event-loop thread.

## Repository roles

### Tao

Tao is where the native bounded event-loop operation is implemented.

The public API added by the patch is conceptually:

```rust
EventLoop::run_timeout(timeout, event_handler)
```

The implementation delegates to platform-specific bounded-return logic on desktop platforms. The patch covers Windows, macOS and Linux and preserves event-loop state across repeated calls instead of treating every slice as final destruction of the loop.

Important behavior:

- `Duration::ZERO` provides a non-blocking pump;
- a positive timeout may wait for native work but never intentionally waits beyond the requested slice;
- native work can wake the call early;
- repeated calls continue using the same event loop;
- a real application exit is still distinguished from a normal slice return.

The patch also contains `examples/run_timeout_probe.rs`, which exercises repeated timeouts and a native/user-event wake.

Patch snapshot base:

- upstream/base Tao commit: `2f9eecf236f4f6a8acfa03329c57039224a3ce99`
- patched implementation snapshot: `d073951d9ee55eceee16e6088b006420b48e1fb7`

### Tauri

Tauri is the bridge between Tauriless and the patched Tao primitive.

The patch exposes bounded execution through the WRY runtime and through `App<Wry>`:

```rust
Wry::run_timeout(...)
App<Wry>::run_timeout(...)
```

`App<Wry>::run_timeout` performs Tauri setup when required, processes the normal Tauri runtime events, and then returns to the caller after the Tao slice returns.

A repeated bounded run must preserve Tauri's runtime context. The WRY implementation therefore builds the event handler from a cloned shared `Context<T>` rather than consuming or replacing the runtime state between slices.

The Tauri fork also depends directly on the patched Tao `dev` branch. This is intentional: the Tauri patch is not complete if Cargo silently resolves an unpatched crates.io Tao release.

Patch snapshot base:

- upstream/base Tauri commit: `56d19c39e457b528433dc546106cd0bff4066bc2`
- patched implementation snapshot: `6f6636f13b927cb300a61fedd97f7d89b6651e1a`

### Tauriless

Tauriless is the consumer and the reason for the two upstream deltas.

Tauriless exposes a small C ABI to the host. The host sends Tauri-compatible requests and repeatedly calls:

```c
tauriless_run(runtime, timeout_ms)
```

Internally, once the Tauri application exists, this maps the requested timeout to `App<Wry>::run_timeout`. The returned JSON batch contains messages/events accumulated during the slice. No native GUI thread is spawned and Rust does not invoke a foreign event callback from inside the loop.

The host therefore owns the outer scheduling loop while Tao still owns the platform-specific mechanics of pumping Windows/macOS/Linux GUI events.

Tauriless consumes:

- `mefistofelix/tauri` branch `dev`;
- `mefistofelix/tao` branch `dev` through the patched Tauri dependency and root Cargo patching where required.

Its `Cargo.lock` records the exact resolved commits used by a particular build.

## Dependency chain

```text
Host runtime (Deno / Node / Bun / native FFI host)
        |
        | tauriless_run(timeout_ms)
        v
Tauriless
        |
        | App<Wry>::run_timeout(...)
        v
patched Tauri fork
        |
        | Wry::run_timeout(...)
        v
patched Tao fork
        |
        | platform-specific bounded native event-loop pump
        v
Windows / macOS / Linux native GUI event system
```

## WRY

WRY is not patched by this stack. It remains an upstream dependency. The missing primitive is event-loop ownership/return behavior, which lives in Tao and is surfaced by Tauri; no separate WRY fork is currently required.

## What the two patch files contain

`tao.patch` contains only the Tao source/example delta needed for `run_timeout`. It intentionally excludes this `tauriless_patch/` documentation directory so the patch does not recursively contain itself.

`tauri.patch` contains the Tauri source/dependency/lock delta needed to expose `run_timeout` and resolve the patched Tao fork. It likewise excludes this documentation directory.

Both files were generated as full-index Git diffs and were checked by reverse-applying them against the corresponding patched working trees. This verifies that the snapshots describe the maintained implementation delta.

## Applying the snapshots to clean upstream bases

The maintained forks should normally be used directly. For review, reproduction or reapplication to the recorded bases, the intended order is:

```text
1. start Tao at the recorded Tao base commit
2. apply tao.patch
3. start Tauri at the recorded Tauri base commit
4. apply tauri.patch
5. build Tauriless against those patched sources
```

The Tauri patch refers to the maintained Tao fork `dev`; when reproducing entirely offline or from temporary local clones, that dependency can instead be redirected to the locally patched Tao checkout.

## Updating the stack

When upstream Tao or Tauri advances:

1. update the relevant fork to the desired upstream development commit;
2. rebase/port the smallest possible Tauriless-specific delta;
3. verify `run_timeout` semantics on every supported desktop platform;
4. update Tauri's Tao dependency if the Tao fork head changed;
5. build and test Tauriless against the resulting Tauri/Tao heads;
6. regenerate `tao.patch` and/or `tauri.patch` against the new upstream bases;
7. copy the exact same `tauriless_patch/` directory into all three repositories;
8. update the recorded base/implementation commit identifiers in this README.

The goal is to keep the patch stack narrow, auditable and easy either to carry temporarily or to upstream later.
