// Manual end-to-end demo for Tauriless' C ABI through Deno FFI.
// Run from the workspace root after building and copying Deno:
//   .\tauriless\target\debug\deno.exe run --allow-ffi examples\deno_ffi_demo.js

const WINDOW_LABEL = "deno-ffi-demo";
const TRAY_ID = "tauriless-deno-tray";
const encoder = new TextEncoder();
const decoder = new TextDecoder();

const dylib = Deno.dlopen(
  new URL("../tauriless/target/debug/tauriless.dll", import.meta.url),
  {
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
  },
);

const handleOut = new BigUint64Array(1);
checkStatus("tauriless_create", dylib.symbols.tauriless_create(handleOut));
const runtime = Deno.UnsafePointer.create(handleOut[0]);
if (runtime === null) throw new Error("tauriless_create returned a null handle");

let nextId = 1;
let timer = 0;
let closed = false;
let quitting = false;
const pending = new Map();

function readOwnedBuffer(buffer) {
  const address = buffer[0];
  const length = buffer[1];
  const capacity = buffer[2];
  if (address === 0n) return "";

  const pointer = Deno.UnsafePointer.create(address);
  if (pointer === null) return "";
  try {
    const bytes = new Uint8Array(
      new Deno.UnsafePointerView(pointer).getArrayBuffer(Number(length)),
    );
    return decoder.decode(bytes);
  } finally {
    dylib.symbols.tauriless_buffer_free(pointer, length, capacity);
  }
}

function lastError() {
  const buffer = new BigUint64Array(3);
  const status = dylib.symbols.tauriless_last_error(buffer);
  return status === 0 ? readOwnedBuffer(buffer) : `status ${status}`;
}

function checkStatus(operation, status) {
  if (status !== 0) throw new Error(`${operation} failed (${status}): ${lastError()}`);
}

function request(cmd, payload = {}, webview = "__tauriless") {
  const id = nextId++;
  const bytes = encoder.encode(JSON.stringify({ id, cmd, payload, webview }));

  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject, cmd });
    try {
      checkStatus(
        `tauriless_send(${cmd})`,
        dylib.symbols.tauriless_send(runtime, bytes, BigInt(bytes.length)),
      );
    } catch (error) {
      pending.delete(id);
      reject(error);
    }
  });
}

function drain() {
  if (closed) return;
  const buffer = new BigUint64Array(3);
  checkStatus("tauriless_drain", dylib.symbols.tauriless_drain(runtime, buffer));
  const text = readOwnedBuffer(buffer);
  if (!text) return;

  const batch = JSON.parse(text);
  for (const message of batch.messages ?? []) {
    console.log("[tauri]", JSON.stringify(message));
    if (message.kind === "result") {
      const callback = pending.get(message.id);
      if (callback) {
        pending.delete(message.id);
        if (message.ok) callback.resolve(message.value);
        else callback.reject(new Error(`${callback.cmd}: ${JSON.stringify(message.error)}`));
      }
    } else if (message.kind === "channel") {
      handleChannel(message);
    } else if (message.kind === "event") {
      handleEvent(message);
    }
  }
}

function handleEvent(message) {
  if (
    message.window === WINDOW_LABEL &&
    message.event === "tauri://drag-drop"
  ) {
    for (const path of message.payload.paths ?? []) {
      console.log(`[drop] ${path}`);
      void notify("File ricevuto da Tauriless", path).catch(reportAsyncError);
    }
  }
}

function handleChannel(message) {
  if (message.id === 9001) {
    switch (message.message) {
      case "tray-show":
        void showWindow().catch(reportAsyncError);
        break;
      case "tray-hide":
        void request("plugin:window|hide", { label: WINDOW_LABEL }).catch(
          reportAsyncError,
        );
        break;
      case "tray-notify":
        void notify("Tauriless", "Notifica richiesta dal menu tray").catch(
          reportAsyncError,
        );
        break;
      case "tray-quit":
        queueMicrotask(() => shutdown(0));
        break;
    }
  }
}

function reportAsyncError(error) {
  console.error("[errore asincrono]", error);
}

function notify(title, body) {
  return request("plugin:notification|notify", {
    options: { title, body },
  });
}

async function showWindow() {
  await request("plugin:window|show", { label: WINDOW_LABEL });
  await request("plugin:window|set_focus", { label: WINDOW_LABEL });
}

function trayIcon() {
  const rgba = [];
  for (let y = 0; y < 16; y++) {
    for (let x = 0; x < 16; x++) {
      const edge = x < 2 || y < 2 || x > 13 || y > 13;
      rgba.push(edge ? 20 : 39, edge ? 90 : 174, edge ? 140 : 245, 255);
    }
  }
  return { rgba, width: 16, height: 16 };
}

const WEBVIEW_HTML = `<!doctype html>
<html lang="it">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Tauriless · Deno FFI</title>
  <style>
    :root { color-scheme: dark; font-family: Inter, system-ui, sans-serif; }
    * { box-sizing: border-box; }
    body { margin: 0; min-height: 100vh; display: grid; place-items: center;
      background: radial-gradient(circle at top, #17324c, #09111c 55%); color: #eef7ff; }
    main { width: min(680px, calc(100vw - 40px)); display: grid; gap: 18px; }
    h1 { margin: 0; font-size: clamp(28px, 5vw, 48px); }
    p { color: #a9c2d6; line-height: 1.5; }
    #drop { min-height: 180px; border: 2px dashed #4db3ff; border-radius: 18px;
      display: grid; place-items: center; text-align: center; padding: 28px;
      background: #0e2133aa; transition: .15s ease; }
    #drop.over { transform: scale(1.015); background: #123b5d; }
    .form { display: grid; gap: 10px; }
    textarea { width: 100%; min-height: 110px; resize: vertical; border: 1px solid #31536c;
      border-radius: 12px; padding: 14px; background: #081521; color: inherit; font: inherit; }
    button { justify-self: start; border: 0; border-radius: 999px; padding: 11px 20px;
      background: #32a9ff; color: #03111d; font-weight: 700; cursor: pointer; }
    #status { min-height: 1.5em; color: #75d69c; }
  </style>
</head>
<body>
  <main>
    <header><h1>Tauriless + Deno FFI</h1>
      <p>Trascina uno o più file nell'area: Deno stamperà i path completi e mostrerà una notifica OS.</p></header>
    <section id="drop">Drop dei file qui</section>
    <section class="form">
      <label for="message">Notifica personalizzata</label>
      <textarea id="message">Messaggio inviato dalla webview tramite le API native Tauri.</textarea>
      <button id="notify" type="button">Invia notifica OS</button>
      <div id="status" role="status"></div>
    </section>
  </main>
  <script>
    const drop = document.querySelector('#drop');
    const status = document.querySelector('#status');
    for (const name of ['dragenter', 'dragover']) {
      addEventListener(name, event => { event.preventDefault(); drop.classList.add('over'); });
    }
    for (const name of ['dragleave', 'drop']) {
      addEventListener(name, event => { event.preventDefault(); drop.classList.remove('over'); });
    }

    const emitHost = (event, payload) => window.__TAURILESS__?.emit
      ? window.__TAURILESS__.emit(event, payload)
      : Promise.resolve();
    status.textContent = typeof window.tauriless_send === 'function'
      ? 'Bridge Tauriless pronto.'
      : 'Bridge Tauriless non disponibile.';

    document.querySelector('#notify').addEventListener('click', async () => {
      const text = document.querySelector('#message').value.trim();
      if (!text) { status.textContent = 'Inserisci un messaggio.'; return; }
      status.textContent = 'Invio…';
      try {
        await window.tauriless_send({
          cmd: 'plugin:notification|notify',
          payload: { options: { title: 'Dalla webview Tauriless', body: text } }
        });
        await emitHost('custom-notification', { text, ok: true });
        status.textContent = 'Notifica inviata.';
      } catch (error) {
        await emitHost('custom-notification', { text, ok: false, error: String(error) });
        status.textContent = 'Errore: ' + error;
      }
    });

    emitHost('webview-ready', {
      href: location.href,
      bridge: typeof window.tauriless_send === 'function'
    });
  </script>
</body>
</html>`;

async function createDemo() {
  timer = setInterval(() => {
    try {
      drain();
    } catch (error) {
      console.error("[drain fatale]", error);
      shutdown(1);
    }
  }, 16);

  // tauriless/assets/index.html is only a generic inline-document loader. The complete
  // demo page remains in this file and the fragment is never sent to a server.
  const appUrl = `index.html#${encodeURIComponent(WEBVIEW_HTML)}`;
  console.error("[fase] creazione webview…");
  await request("plugin:webview|create_webview_window", {
    options: {
      label: WINDOW_LABEL,
      title: "Tauriless · Deno FFI",
      url: appUrl,
      width: 760,
      height: 720,
      minWidth: 520,
      minHeight: 560,
      center: true,
      visible: true,
      dragDropEnabled: true,
    },
  });

  console.error("[fase] creazione menu tray…");
  const [menuRid] = await request("plugin:menu|new", {
    kind: "Menu",
    options: {
      id: "tauriless-deno-menu",
      items: [
        { id: "tray-show", text: "Mostra finestra", enabled: true },
        { id: "tray-hide", text: "Nascondi finestra", enabled: true },
        { id: "tray-notify", text: "Notifica di prova", enabled: true },
        { id: "tray-quit", text: "Esci", enabled: true },
      ],
    },
    handler: "__CHANNEL__:9001",
  });

  console.error("[fase] creazione systray…");
  await request("plugin:tray|new", {
    options: {
      id: TRAY_ID,
      menu: [menuRid, "Menu"],
      icon: trayIcon(),
      tooltip: "Tauriless · Deno FFI",
      showMenuOnLeftClick: true,
    },
    handler: "__CHANNEL__:9002",
  });

  console.log("[pronto] Webview e systray attive.");
  console.log("[pronto] Droppa file nella finestra; usa 'Esci' dal tray per terminare.");
}

function shutdown(exitCode) {
  if (quitting) return;
  quitting = true;
  if (timer) clearInterval(timer);
  for (const { reject } of pending.values()) reject(new Error("Tauriless in chiusura"));
  pending.clear();

  if (!closed) {
    closed = true;
    const status = dylib.symbols.tauriless_destroy(runtime);
    if (status !== 0) console.error(`[destroy] status ${status}: ${lastError()}`);
    dylib.close();
  }
  Deno.exit(exitCode);
}

for (const signal of ["SIGINT", "SIGBREAK"]) {
  try {
    Deno.addSignalListener(signal, () => shutdown(0));
  } catch {
    // Signal availability differs by platform.
  }
}

createDemo().catch((error) => {
  console.error("[avvio fallito]", error);
  shutdown(1);
});
