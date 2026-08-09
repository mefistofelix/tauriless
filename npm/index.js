import path from "node:path";
import { fileURLToPath } from "node:url";

const POINTER_SIZE = 8;
const encoder = new TextEncoder();
const deno = typeof globalThis.Deno?.dlopen === "function";
const bun = !deno && typeof globalThis.Bun?.version === "string";
const libraries = {
  "darwin-x64": "libtauriless.dylib",
  "linux-x64": "libtauriless.so",
  "win32-x64": "tauriless.dll",
};

function nativeLibraryPath() {
  const platform = deno
    ? (Deno.build.os === "windows" ? "win32" : Deno.build.os)
    : process.platform;
  const arch = deno
    ? (Deno.build.arch === "x86_64" ? "x64" : Deno.build.arch)
    : process.arch;
  let override;
  try {
    override = deno
      ? Deno.env.get("TAURILESS_LIBRARY_PATH")
      : process.env.TAURILESS_LIBRARY_PATH;
  } catch {
    // Deno needs --allow-env only when this optional override is used.
  }
  if (override) return path.resolve(override);

  const target = `${platform}-${arch}`;
  const filename = libraries[target];
  if (!filename) {
    throw new Error(
      `Tauriless does not ship a binary for ${target}; supported targets are ${
        Object.keys(libraries).join(", ")
      }`,
    );
  }
  return fileURLToPath(
    new URL(`./native/${target}/${filename}`, import.meta.url),
  );
}

let backend;
let isMainThread = true;
const libraryPath = nativeLibraryPath();

if (deno) {
  const functions = Deno.dlopen(libraryPath, {
    tauriless_create: { parameters: ["buffer"], result: "i32" },
    tauriless_send: {
      parameters: ["pointer", "buffer"],
      result: "i32",
    },
    tauriless_drain: {
      parameters: ["pointer"],
      result: "pointer",
    },
    tauriless_destroy: { parameters: ["pointer"], result: "i32" },
    tauriless_last_error: { parameters: [], result: "pointer" },
  }).symbols;

  backend = {
    words: (length) => new BigUint64Array(length),
    create: (out) => functions.tauriless_create(out),
    handle: (out) => Deno.UnsafePointer.create(out[0]),
    send: (runtime, bytes) => functions.tauriless_send(runtime, bytes),
    drain: (runtime) => {
      const pointer = functions.tauriless_drain(runtime);
      return pointer === null
        ? null
        : new Deno.UnsafePointerView(pointer).getCString();
    },
    destroy: (runtime) => functions.tauriless_destroy(runtime),
    lastError: () => {
      const pointer = functions.tauriless_last_error();
      return pointer === null
        ? ""
        : new Deno.UnsafePointerView(pointer).getCString();
    },
  };
} else if (bun) {
  const bunFfi = await import("bun:ffi");
  const functions = bunFfi.dlopen(libraryPath, {
    tauriless_create: { args: ["ptr"], returns: "i32" },
    tauriless_send: {
      args: ["ptr", "ptr"],
      returns: "i32",
    },
    tauriless_drain: { args: ["ptr"], returns: "cstring" },
    tauriless_destroy: { args: ["ptr"], returns: "i32" },
    tauriless_last_error: { args: [], returns: "cstring" },
  }).symbols;

  ({ isMainThread } = await import("node:worker_threads"));
  backend = {
    words: (length) => new BigUint64Array(length),
    create: (out) => functions.tauriless_create(bunFfi.ptr(out)),
    handle: (out) => Number(out[0]),
    send: (runtime, bytes) =>
      functions.tauriless_send(runtime, bunFfi.ptr(bytes)),
    drain: (runtime) => {
      const value = functions.tauriless_drain(runtime);
      return value == null ? null : String(value);
    },
    destroy: (runtime) => functions.tauriless_destroy(runtime),
    lastError: () => String(functions.tauriless_last_error() ?? ""),
  };
} else {
  let ffi;
  try {
    [ffi, { isMainThread }] = await Promise.all([
      import("node:ffi"),
      import("node:worker_threads"),
    ]);
  } catch (cause) {
    throw new Error(
      "Tauriless requires Node.js 26.1+ started with --experimental-ffi",
      { cause },
    );
  }

  const functions = ffi.dlopen(libraryPath, {
    tauriless_create: { arguments: ["pointer"], return: "i32" },
    tauriless_send: {
      arguments: ["pointer", "pointer"],
      return: "i32",
    },
    tauriless_drain: {
      arguments: ["pointer"],
      return: "pointer",
    },
    tauriless_destroy: { arguments: ["pointer"], return: "i32" },
    tauriless_last_error: { arguments: [], return: "pointer" },
  }).functions;

  const readPointer = (buffer, offset = 0) =>
    ffi.getUint64(ffi.getRawPointer(buffer), offset);
  backend = {
    words: (length) => Buffer.alloc(length * POINTER_SIZE),
    create: (out) => functions.tauriless_create(ffi.getRawPointer(out)),
    handle: (out) => readPointer(out),
    send: (runtime, bytes) =>
      functions.tauriless_send(runtime, ffi.getRawPointer(bytes)),
    drain: (runtime) => ffi.toString(functions.tauriless_drain(runtime)),
    destroy: (runtime) => functions.tauriless_destroy(runtime),
    lastError: () => ffi.toString(functions.tauriless_last_error()) ?? "",
  };
}

function lastError() {
  return backend.lastError();
}

function check(status, operation) {
  if (status !== 0) {
    throw new Error(`${operation} failed (${status}): ${lastError()}`);
  }
}

export class Tauriless {
  #runtime = null;

  constructor() {
    if (!isMainThread) {
      throw new Error(
        "Tauriless must be created and drained on the main OS thread",
      );
    }
    const out = backend.words(1);
    check(backend.create(out), "tauriless_create");
    this.#runtime = backend.handle(out);
    if (this.#runtime === null || this.#runtime === 0 || this.#runtime === 0n) {
      throw new Error("tauriless_create returned a null handle");
    }
  }

  send(request) {
    this.#assertOpen();
    const encoded = encoder.encode(
      typeof request === "string" ? request : JSON.stringify(request),
    );
    if (encoded.includes(0)) {
      throw new TypeError("JSON must not contain a raw NUL byte");
    }
    const bytes = new Uint8Array(encoded.byteLength + 1);
    bytes.set(encoded);
    check(backend.send(this.#runtime, bytes), "tauriless_send");
  }

  drain() {
    this.#assertOpen();
    // Each backend copies Rust's borrowed C string before returning here.
    const json = backend.drain(this.#runtime);
    if (json === null) {
      throw new Error(`tauriless_drain failed: ${lastError()}`);
    }
    return JSON.parse(json);
  }

  close() {
    if (this.#runtime === null) return;
    const runtime = this.#runtime;
    check(backend.destroy(runtime), "tauriless_destroy");
    this.#runtime = null;
  }

  [Symbol.dispose]() {
    this.close();
  }

  #assertOpen() {
    if (this.#runtime === null) throw new Error("Tauriless is closed");
  }
}
