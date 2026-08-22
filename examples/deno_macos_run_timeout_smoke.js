const LABEL = "macos-run-timeout-smoke";
const encoder = new TextEncoder();
const dylib = Deno.dlopen(
  new URL("../tauriless/target/debug/libtauriless.dylib", import.meta.url),
  {
    tauriless_create: { parameters: ["buffer"], result: "i32" },
    tauriless_send: { parameters: ["pointer", "buffer"], result: "i32" },
    tauriless_run: { parameters: ["pointer", "u32"], result: "pointer" },
    tauriless_destroy: { parameters: ["pointer"], result: "i32" },
    tauriless_last_error: { parameters: [], result: "pointer" },
  },
);

const out = new BigUint64Array(1);
check("create", dylib.symbols.tauriless_create(out));
const runtime = Deno.UnsafePointer.create(out[0]);
if (runtime === null) throw new Error("tauriless_create returned null");

let nextId = 1;

function lastError() {
  const pointer = dylib.symbols.tauriless_last_error();
  return pointer === null ? "" : new Deno.UnsafePointerView(pointer).getCString();
}

function check(operation, status) {
  if (status !== 0) throw new Error(`${operation} failed (${status}): ${lastError()}`);
}

function send(cmd, payload = {}, webview) {
  const id = nextId++;
  const request = { id, cmd, payload };
  if (webview !== undefined) request.webview = webview;
  const encoded = encoder.encode(JSON.stringify(request));
  const bytes = new Uint8Array(encoded.length + 1);
  bytes.set(encoded);
  check(`send ${cmd}`, dylib.symbols.tauriless_send(runtime, bytes));
  return id;
}

function run(timeout) {
  const pointer = dylib.symbols.tauriless_run(runtime, timeout);
  if (pointer === null) throw new Error(`run(${timeout}) failed: ${lastError()}`);
  return JSON.parse(new Deno.UnsafePointerView(pointer).getCString()).messages ?? [];
}

function result(id) {
  const deadline = performance.now() + 5000;
  while (performance.now() < deadline) {
    for (const message of run(16)) {
      if (message.kind === "result" && message.id === id) {
        if (!message.ok) throw new Error(`request ${id} failed: ${JSON.stringify(message.error)}`);
        return message.value;
      }
    }
  }
  throw new Error(`timed out waiting for request ${id}`);
}

try {
  const created = result(send("plugin:webview|create_webview_window", {
    options: {
      label: LABEL,
      title: "run_timeout smoke before",
      url: "data:text/html,<title>Tauriless run_timeout smoke</title><body>WKWebView smoke</body>",
      width: 480,
      height: 320,
      visible: true,
    },
  }));
  if (created.label !== LABEL) throw new Error(`unexpected create result: ${JSON.stringify(created)}`);

  const samples = [];
  for (let i = 0; i < 24; i++) {
    const started = performance.now();
    run(40);
    const elapsed = performance.now() - started;
    if (elapsed > 1000) throw new Error(`run_timeout slice ${i} took ${elapsed.toFixed(1)} ms`);
    samples.push(elapsed);
  }

  const visible = result(send("plugin:window|is_visible", { label: LABEL }, LABEL));
  if (visible !== true) throw new Error(`window is not visible: ${JSON.stringify(visible)}`);

  result(send("plugin:window|set_title", { label: LABEL, value: "run_timeout smoke after" }, LABEL));
  for (let i = 0; i < 8; i++) run(16);
  const title = result(send("plugin:window|title", { label: LABEL }, LABEL));
  if (title !== "run_timeout smoke after") throw new Error(`unexpected title: ${JSON.stringify(title)}`);

  result(send("plugin:window|close", { label: LABEL }, LABEL));
  for (let i = 0; i < 4; i++) run(16);

  console.log(`macOS Tauriless run_timeout smoke OK: ${samples.map((x) => x.toFixed(1)).join(", ")} ms`);
} finally {
  const status = dylib.symbols.tauriless_destroy(runtime);
  dylib.close();
  check("destroy", status);
}
