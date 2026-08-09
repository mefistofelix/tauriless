import path from "node:path";
import { fileURLToPath } from "node:url";

const POINTER_SIZE = 8;
const OWNED_BUFFER_WORDS = 3;
const encoder = new TextEncoder();
const decoder = new TextDecoder();
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
      parameters: ["pointer", "buffer", "usize"],
      result: "i32",
    },
    tauriless_drain: {
      parameters: ["pointer", "buffer"],
      result: "i32",
    },
    tauriless_destroy: { parameters: ["pointer"], result: "i32" },
    tauriless_last_error: { parameters: ["buffer"], result: "i32" },
    tauriless_buffer_free: {
      parameters: ["pointer", "usize", "usize"],
      result: "void",
    },
  }).symbols;

  backend = {
    words: (length) => new BigUint64Array(length),
    create: (out) => functions.tauriless_create(out),
    handle: (out) => Deno.UnsafePointer.create(out[0]),
    send: (runtime, bytes) =>
      functions.tauriless_send(runtime, bytes, BigInt(bytes.byteLength)),
    drain: (runtime, out) => functions.tauriless_drain(runtime, out),
    destroy: (runtime) => functions.tauriless_destroy(runtime),
    lastError: (out) => functions.tauriless_last_error(out),
    readOwned(out) {
      const [address, length, capacity] = out;
      const pointer = Deno.UnsafePointer.create(address);
      if (pointer === null) return "";
      try {
        return decoder.decode(
          new Deno.UnsafePointerView(pointer).getArrayBuffer(Number(length)),
        );
      } finally {
        functions.tauriless_buffer_free(pointer, length, capacity);
      }
    },
  };
} else if (bun) {
  const bunFfi = await import("bun:ffi");
  const functions = bunFfi.dlopen(libraryPath, {
    tauriless_create: { args: ["ptr"], returns: "i32" },
    tauriless_send: {
      args: ["ptr", "ptr", "usize"],
      returns: "i32",
    },
    tauriless_drain: { args: ["ptr", "ptr"], returns: "i32" },
    tauriless_destroy: { args: ["ptr"], returns: "i32" },
    tauriless_last_error: { args: ["ptr"], returns: "i32" },
    tauriless_buffer_free: {
      args: ["ptr", "usize", "usize"],
      returns: "void",
    },
  }).symbols;

  ({ isMainThread } = await import("node:worker_threads"));
  backend = {
    words: (length) => new BigUint64Array(length),
    create: (out) => functions.tauriless_create(bunFfi.ptr(out)),
    handle: (out) => Number(out[0]),
    send: (runtime, bytes) =>
      functions.tauriless_send(
        runtime,
        bunFfi.ptr(bytes),
        bytes.byteLength,
      ),
    drain: (runtime, out) =>
      functions.tauriless_drain(runtime, bunFfi.ptr(out)),
    destroy: (runtime) => functions.tauriless_destroy(runtime),
    lastError: (out) => functions.tauriless_last_error(bunFfi.ptr(out)),
    readOwned(out) {
      const [address, length, capacity] = out;
      if (address === 0n) return "";
      const pointer = Number(address);
      try {
        return decoder.decode(
          bunFfi.toArrayBuffer(pointer, 0, Number(length)),
        );
      } finally {
        functions.tauriless_buffer_free(
          pointer,
          Number(length),
          Number(capacity),
        );
      }
    },
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
      arguments: ["pointer", "pointer", "u64"],
      return: "i32",
    },
    tauriless_drain: {
      arguments: ["pointer", "pointer"],
      return: "i32",
    },
    tauriless_destroy: { arguments: ["pointer"], return: "i32" },
    tauriless_last_error: { arguments: ["pointer"], return: "i32" },
    tauriless_buffer_free: {
      arguments: ["pointer", "u64", "u64"],
      return: "void",
    },
  }).functions;

  const readPointer = (buffer, offset = 0) =>
    ffi.getUint64(ffi.getRawPointer(buffer), offset);
  backend = {
    words: (length) => Buffer.alloc(length * POINTER_SIZE),
    create: (out) => functions.tauriless_create(ffi.getRawPointer(out)),
    handle: (out) => readPointer(out),
    send: (runtime, bytes) =>
      functions.tauriless_send(
        runtime,
        ffi.getRawPointer(bytes),
        BigInt(bytes.byteLength),
      ),
    drain: (runtime, out) =>
      functions.tauriless_drain(runtime, ffi.getRawPointer(out)),
    destroy: (runtime) => functions.tauriless_destroy(runtime),
    lastError: (out) => functions.tauriless_last_error(ffi.getRawPointer(out)),
    readOwned(out) {
      const address = readPointer(out);
      const length = readPointer(out, POINTER_SIZE);
      const capacity = readPointer(out, POINTER_SIZE * 2);
      if (address === 0n) return "";
      try {
        return ffi.toBuffer(address, Number(length), true).toString("utf8");
      } finally {
        functions.tauriless_buffer_free(address, length, capacity);
      }
    },
  };
}

function lastError() {
  const out = backend.words(OWNED_BUFFER_WORDS);
  const status = backend.lastError(out);
  return status === 0
    ? backend.readOwned(out)
    : `unable to read native error (status ${status})`;
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
    const bytes = encoder.encode(
      typeof request === "string" ? request : JSON.stringify(request),
    );
    check(backend.send(this.#runtime, bytes), "tauriless_send");
  }

  drain() {
    this.#assertOpen();
    const out = backend.words(OWNED_BUFFER_WORDS);
    check(backend.drain(this.#runtime, out), "tauriless_drain");
    return JSON.parse(backend.readOwned(out));
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
