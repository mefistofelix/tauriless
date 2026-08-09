import { isMainThread } from "node:worker_threads";
import { nativeLibraryPath } from "./native-path.js";

let ffi;
try {
  ffi = await import("node:ffi");
} catch (cause) {
  throw new Error(
    "Tauriless requires Node.js 26.1+ started with --experimental-ffi",
    { cause },
  );
}

const { functions } = ffi.dlopen(nativeLibraryPath, {
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
});

const POINTER_SIZE = 8;
const OWNED_BUFFER_SIZE = POINTER_SIZE * 3;
const encoder = new TextEncoder();

function readPointer(buffer, offset = 0) {
  return ffi.getUint64(ffi.getRawPointer(buffer), offset);
}

function readOwnedBuffer(out) {
  const data = readPointer(out);
  const length = readPointer(out, POINTER_SIZE);
  const capacity = readPointer(out, POINTER_SIZE * 2);
  if (data === 0n) return "";

  try {
    return ffi.toBuffer(data, Number(length), true).toString("utf8");
  } finally {
    functions.tauriless_buffer_free(data, length, capacity);
  }
}

function lastError() {
  const out = Buffer.alloc(OWNED_BUFFER_SIZE);
  const status = functions.tauriless_last_error(ffi.getRawPointer(out));
  if (status !== 0) return `unable to read native error (status ${status})`;
  return readOwnedBuffer(out);
}

function check(status, operation) {
  if (status !== 0) {
    throw new Error(`${operation} failed (${status}): ${lastError()}`);
  }
}

export { nativeLibraryPath };

export class Tauriless {
  #runtime = 0n;
  #timers = new Set();

  constructor() {
    if (!isMainThread) {
      throw new Error(
        "Tauriless must be created and drained on Node's main thread",
      );
    }

    const out = Buffer.alloc(POINTER_SIZE);
    check(
      functions.tauriless_create(ffi.getRawPointer(out)),
      "tauriless_create",
    );
    this.#runtime = readPointer(out);
  }

  send(request) {
    this.#assertOpen();
    const bytes = encoder.encode(
      typeof request === "string" ? request : JSON.stringify(request),
    );
    check(
      functions.tauriless_send(
        this.#runtime,
        ffi.getRawPointer(bytes),
        BigInt(bytes.byteLength),
      ),
      "tauriless_send",
    );
  }

  drain() {
    this.#assertOpen();
    const out = Buffer.alloc(OWNED_BUFFER_SIZE);
    check(
      functions.tauriless_drain(this.#runtime, ffi.getRawPointer(out)),
      "tauriless_drain",
    );
    return JSON.parse(readOwnedBuffer(out));
  }

  start(onMessage, interval = 16) {
    this.#assertOpen();
    if (typeof onMessage !== "function") {
      throw new TypeError("onMessage must be a function");
    }
    if (!Number.isFinite(interval) || interval <= 0) {
      throw new RangeError("interval must be a positive number");
    }

    const timer = setInterval(() => {
      for (const message of this.drain().messages) onMessage(message);
    }, interval);
    this.#timers.add(timer);
    return () => {
      clearInterval(timer);
      this.#timers.delete(timer);
    };
  }

  close() {
    if (this.#runtime === 0n) return;
    for (const timer of this.#timers) clearInterval(timer);
    this.#timers.clear();
    const runtime = this.#runtime;
    check(functions.tauriless_destroy(runtime), "tauriless_destroy");
    this.#runtime = 0n;
  }

  [Symbol.dispose]() {
    this.close();
  }

  #assertOpen() {
    if (this.#runtime === 0n) throw new Error("Tauriless is closed");
  }
}

export function createTauriless() {
  return new Tauriless();
}
