// Manual end-to-end demo for Tauriless' C ABI through Deno FFI.
// Run from the workspace root after building and copying Deno:
//   .\tauriless\target\debug\deno.exe run --allow-ffi --allow-write examples\deno_ffi_demo.js

const WINDOW_LABEL = "deno-ffi-demo";
const TRAY_ID = "tauriless-deno-tray";
const encoder = new TextEncoder();

const dylib = Deno.dlopen(
  new URL("../tauriless/target/debug/tauriless.dll", import.meta.url),
  {
    tauriless_create: { parameters: ["buffer"], result: "i32" },
    tauriless_send: {
      parameters: ["pointer", "buffer"],
      result: "i32",
    },
    tauriless_run: {
      parameters: ["pointer", "u32"],
      result: "pointer",
    },
    tauriless_destroy: { parameters: ["pointer"], result: "i32" },
    tauriless_last_error: { parameters: [], result: "pointer" },
  },
);

const handleOut = new BigUint64Array(1);
checkStatus("tauriless_create", dylib.symbols.tauriless_create(handleOut));
const runtime = Deno.UnsafePointer.create(handleOut[0]);
if (runtime === null) {
  throw new Error("tauriless_create returned a null handle");
}

let nextId = 1;
let timer = 0;
let closed = false;
let quitting = false;
let indexHtmlPath = null;
const pending = new Map();

function lastError() {
  const pointer = dylib.symbols.tauriless_last_error();
  return pointer === null
    ? ""
    : new Deno.UnsafePointerView(pointer).getCString();
}

function checkStatus(operation, status) {
  if (status !== 0) {
    throw new Error(`${operation} failed (${status}): ${lastError()}`);
  }
}

function request(cmd, payload = {}, webview) {
  const id = nextId++;
  const request = { id, cmd, payload };
  if (webview !== undefined) request.webview = webview;
  const encoded = encoder.encode(JSON.stringify(request));
  const bytes = new Uint8Array(encoded.length + 1);
  bytes.set(encoded);

  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject, cmd });
    try {
      checkStatus(
        `tauriless_send(${cmd})`,
        dylib.symbols.tauriless_send(runtime, bytes),
      );
    } catch (error) {
      pending.delete(id);
      reject(error);
    }
  });
}

function run(timeout = 0) {
  if (closed) return;
  const pointer = dylib.symbols.tauriless_run(runtime, timeout);
  if (pointer === null) throw new Error(`tauriless_run: ${lastError()}`);
  const text = new Deno.UnsafePointerView(pointer).getCString();

  const batch = JSON.parse(text);
  for (const message of batch.messages ?? []) {
    console.log("[tauri]", JSON.stringify(message));
    if (message.kind === "asset-request") {
      void handleAssetRequest(message).catch(reportAsyncError);
    } else if (message.kind === "result") {
      const callback = pending.get(message.id);
      if (callback) {
        pending.delete(message.id);
        if (message.ok) callback.resolve(message.value);
        else {callback.reject(
            new Error(`${callback.cmd}: ${JSON.stringify(message.error)}`),
          );}
      }
    } else if (message.kind === "channel") {
      handleChannel(message);
    } else if (message.kind === "event") {
      handleEvent(message);
    }
  }
}

async function handleAssetRequest(message) {
  const pathname = new URL(message.url).pathname;
  if (pathname === "/" || pathname === "/index.html") {
    await request("tauriless:asset-response", {
      requestId: message.requestId,
      path: indexHtmlPath,
    });
  } else if (pathname === "/runtime.css") {
    await request("tauriless:asset-response", {
      requestId: message.requestId,
      mime: "text/css; charset=utf-8",
      content:
        "body::before{content:'CSS da Deno: content + mime';display:block;padding:7px 12px;background:#165b85;color:white;text-align:center;font:12px system-ui}",
    });
  } else {
    await request("tauriless:asset-response", {
      requestId: message.requestId,
      status: 404,
      mime: "text/plain; charset=utf-8",
      content: `Asset non trovato: ${pathname}`,
    });
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

function scheduleOptionalAutoExit() {
  try {
    const milliseconds = Number(Deno.env.get("TAURILESS_DEMO_AUTO_EXIT_MS"));
    if (Number.isFinite(milliseconds) && milliseconds > 0) {
      setTimeout(() => shutdown(0), milliseconds);
    }
  } catch {
    // Reading the optional variable is not required during an interactive run.
  }
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
  <link rel="stylesheet" href="runtime.css">
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

    const invoke = (command, payload = {}) =>
      window.__TAURI_INTERNALS__.invoke(command, payload);
    status.textContent = window.__TAURI_INTERNALS__?.invoke
      ? 'IPC Tauri standard pronto.'
      : 'IPC Tauri non disponibile.';

    document.querySelector('#notify').addEventListener('click', async () => {
      const text = document.querySelector('#message').value.trim();
      if (!text) { status.textContent = 'Inserisci un messaggio.'; return; }
      status.textContent = 'Invio…';
      try {
        await invoke('plugin:notification|notify', {
          options: { title: 'Dalla webview Tauriless', body: text }
        });
        status.textContent = 'Notifica inviata.';
      } catch (error) {
        status.textContent = 'Errore: ' + error;
      }
    });
  </script>
</body>
</html>`;

async function createDemo() {
  indexHtmlPath = await Deno.makeTempFile({
    prefix: "tauriless-deno-ffi-",
    suffix: ".html",
  });
  await Deno.writeTextFile(indexHtmlPath, WEBVIEW_HTML);

  timer = setInterval(() => {
    try {
      run(16);
    } catch (error) {
      console.error("[run fatale]", error);
      shutdown(1);
    }
  }, 16);

  console.error("[fase] creazione webview…");
  await request("plugin:webview|create_webview_window", {
    options: {
      label: WINDOW_LABEL,
      title: "Tauriless · Deno FFI",
      url: "index.html",
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
  console.log(
    "[pronto] Droppa file nella finestra; usa 'Esci' dal tray per terminare.",
  );
  scheduleOptionalAutoExit();
}

function shutdown(exitCode) {
  if (quitting) return;
  quitting = true;
  if (timer) clearInterval(timer);
  for (const { reject } of pending.values()) {
    reject(new Error("Tauriless in chiusura"));
  }
  pending.clear();

  if (!closed) {
    closed = true;
    const status = dylib.symbols.tauriless_destroy(runtime);
    if (status !== 0) {
      console.error(`[destroy] status ${status}: ${lastError()}`);
    }
    dylib.close();
  }
  if (indexHtmlPath !== null) {
    try {
      Deno.removeSync(indexHtmlPath);
    } catch {
      // The temporary file may already have been removed.
    }
    indexHtmlPath = null;
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
