// Single-file Tauriless demo using the precompiled public npm module.
//
// Run directly with Deno:
//   deno run --allow-ffi --allow-write examples/deno_npm_demo.js
//
// On Windows this demo sets an explicit process AppUserModelID before creating
// the first webview, then sends a notification through Tauri's notification plugin.

import { Tauriless } from "npm:@mefistofelix/tauriless";

const WINDOW_LABEL = "main";
const TRAY_ID = "tauriless-deno-npm-tray";
const APP_USER_MODEL_ID = "com.mefistofelix.tauriless.deno-npm-demo";
const WEBVIEW_TO_HOST_EVENT = "tauriless://webview-message";
const HOST_TO_WEBVIEW_EVENT = "tauriless://host-message";

const tauriless = new Tauriless();
const pending = new Map();
let nextId = 1;
let drainTimer = 0;
let closed = false;
let quitting = false;
let indexHtmlPath = null;

let resolveWebviewReady;
let rejectWebviewReady;
const webviewReady = new Promise((resolve, reject) => {
  resolveWebviewReady = resolve;
  rejectWebviewReady = reject;
});

function request(cmd, payload = {}, webview) {
  const id = nextId++;
  const message = { id, cmd, payload };
  if (webview !== undefined) message.webview = webview;

  return new Promise((resolve, reject) => {
    pending.set(id, { cmd, resolve, reject });
    try {
      tauriless.send(message);
    } catch (error) {
      pending.delete(id);
      reject(error);
    }
  });
}

function drain() {
  if (closed) return;

  for (const message of tauriless.run(0).messages) {
    console.log("[tauri]", JSON.stringify(message));

    if (message.kind === "asset-request") {
      void handleAssetRequest(message).catch(reportAsyncError);
    } else if (message.kind === "result") {
      const callback = pending.get(message.id);
      if (!callback) continue;
      pending.delete(message.id);
      if (message.ok) callback.resolve(message.value);
      else {
        callback.reject(
          new Error(`${callback.cmd}: ${JSON.stringify(message.error)}`),
        );
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
      void notify("File ricevuto", path).catch(reportAsyncError);
      void postToWebview({ type: "dropped-file", path }).catch(
        reportAsyncError,
      );
    }
    return;
  }

  if (
    message.window !== WINDOW_LABEL ||
    message.event !== WEBVIEW_TO_HOST_EVENT
  ) return;

  const payload = message.payload ?? {};
  console.log("[webview -> Deno]", JSON.stringify(payload));

  switch (payload.type) {
    case "ready":
      resolveWebviewReady();
      break;
    case "message":
      void postToWebview({
        type: "deno-reply",
        text: `Deno ha ricevuto: ${payload.text ?? ""}`,
      }).catch(reportAsyncError);
      break;
    case "notification-request":
      void notify("Dalla webview via Deno", String(payload.text ?? ""))
        .then(() =>
          postToWebview({
            type: "deno-reply",
            text: "Notifica OS inviata da Deno tramite Tauri.",
          })
        )
        .catch(reportAsyncError);
      break;
    case "set-html-request":
      void setHtml(String(payload.html ?? "")).catch(reportAsyncError);
      break;
  }
}

function handleChannel(message) {
  if (message.id !== 9101) return;

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
    case "tray-set-html":
      void setHtml(
        `<strong>Aggiornato dal tray</strong><br>${
          new Date().toLocaleString()
        }`,
      ).catch(reportAsyncError);
      break;
    case "tray-quit":
      queueMicrotask(() => shutdown(0));
      break;
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

function postToWebview(payload) {
  return request("plugin:event|emit_to", {
    target: { kind: "WebviewWindow", label: WINDOW_LABEL },
    event: HOST_TO_WEBVIEW_EVENT,
    payload,
  });
}

function setHtml(html) {
  return postToWebview({ type: "set-html", html });
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
  <title>Tauriless · import npm diretto</title>
  <style>
    :root { color-scheme: dark; font-family: Inter, system-ui, sans-serif; }
    * { box-sizing: border-box; }
    body { margin: 0; min-height: 100vh; padding: 28px; background: #09111c;
      color: #eef7ff; }
    main { width: min(760px, 100%); margin: auto; display: grid; gap: 16px; }
    h1 { margin: 0; }
    p { color: #a9c2d6; line-height: 1.45; }
    section { padding: 18px; border: 1px solid #29465e; border-radius: 14px;
      background: #0e2133; }
    #drop { min-height: 130px; display: grid; place-items: center;
      border: 2px dashed #4db3ff; text-align: center; }
    #drop.over { background: #123b5d; }
    textarea { width: 100%; min-height: 82px; resize: vertical; padding: 10px;
      border: 1px solid #31536c; border-radius: 9px; background: #081521;
      color: inherit; }
    .buttons { display: flex; flex-wrap: wrap; gap: 9px; margin-top: 10px; }
    button { border: 0; border-radius: 999px; padding: 9px 15px;
      background: #32a9ff; color: #03111d; font-weight: 700; cursor: pointer; }
    #host-html { min-height: 48px; }
    #log { max-height: 180px; overflow: auto; white-space: pre-wrap;
      font: 12px/1.5 ui-monospace, monospace; color: #86e1ad; }
  </style>
</head>
<body>
  <main>
    <header>
      <h1>Tauriless + Deno</h1>
      <p>Un solo file JS: import npm, HTML incorporato, IPC Tauri standard,
        drag & drop, notifiche e systray.</p>
    </header>

    <section id="drop">Trascina qui uno o più file.</section>

    <section>
      <label for="message">Messaggio webview → invoke Tauri → drain Deno</label>
      <textarea id="message">Ciao dal JavaScript della webview</textarea>
      <div class="buttons">
        <button id="send" type="button">Invia a Deno</button>
        <button id="notify" type="button">Notifica via Deno</button>
      </div>
    </section>

    <section>
      <label for="html">HTML da chiedere a Deno</label>
      <textarea id="html"><em>Questo HTML ha fatto webview → Deno → webview.</em></textarea>
      <div class="buttons">
        <button id="set-html" type="button">Roundtrip setHtml</button>
      </div>
    </section>

    <section>
      <strong>Contenuto controllato da Deno</strong>
      <div id="host-html">In attesa del primo messaggio host…</div>
    </section>

    <section id="log" aria-live="polite"></section>
  </main>

  <script>
    const WEBVIEW_TO_HOST_EVENT = ${JSON.stringify(WEBVIEW_TO_HOST_EVENT)};
    const HOST_TO_WEBVIEW_EVENT = ${JSON.stringify(HOST_TO_WEBVIEW_EVENT)};
    const internals = window.__TAURI_INTERNALS__;
    const invoke = (command, payload = {}) => internals.invoke(command, payload);
    const logNode = document.querySelector('#log');

    function log(message) {
      logNode.textContent += '[' + new Date().toLocaleTimeString() + '] ' + message + '\\n';
      logNode.scrollTop = logNode.scrollHeight;
    }

    function emitToDeno(payload) {
      return invoke('plugin:event|emit', {
        event: WEBVIEW_TO_HOST_EVENT,
        payload,
      });
    }

    async function listen(event, handler) {
      const handlerId = internals.transformCallback(handler);
      return await invoke('plugin:event|listen', {
        event,
        target: { kind: 'Any' },
        handler: handlerId,
      });
    }

    const drop = document.querySelector('#drop');
    for (const name of ['dragenter', 'dragover']) {
      addEventListener(name, event => {
        event.preventDefault();
        drop.classList.add('over');
      });
    }
    for (const name of ['dragleave', 'drop']) {
      addEventListener(name, event => {
        event.preventDefault();
        drop.classList.remove('over');
      });
    }

    document.querySelector('#send').addEventListener('click', async () => {
      const text = document.querySelector('#message').value;
      await emitToDeno({ type: 'message', text });
      log('Messaggio emesso sul bus Tauri.');
    });

    document.querySelector('#notify').addEventListener('click', async () => {
      const text = document.querySelector('#message').value;
      await emitToDeno({ type: 'notification-request', text });
      log('Richiesta di notifica inviata a Deno.');
    });

    document.querySelector('#set-html').addEventListener('click', async () => {
      const html = document.querySelector('#html').value;
      await emitToDeno({ type: 'set-html-request', html });
      log('Richiesto setHtml via Deno.');
    });

    listen(HOST_TO_WEBVIEW_EVENT, event => {
      const message = event.payload ?? {};
      log('Deno → webview: ' + JSON.stringify(message));
      if (message.type === 'set-html') {
        document.querySelector('#host-html').innerHTML = message.html;
      } else if (message.type === 'dropped-file') {
        document.querySelector('#host-html').textContent = 'File: ' + message.path;
      }
      emitToDeno({
        type: 'host-message-received',
        messageType: message.type,
      }).catch(error => log('Errore ack verso Deno: ' + error));
    }).then(() => emitToDeno({ type: 'ready' })).catch(error => {
      log('Errore inizializzazione IPC: ' + error);
    });
  </script>
</body>
</html>`;

async function createDemo() {
  indexHtmlPath = await Deno.makeTempFile({
    prefix: "tauriless-deno-npm-",
    suffix: ".html",
  });
  await Deno.writeTextFile(indexHtmlPath, WEBVIEW_HTML);

  drainTimer = setInterval(() => {
    try {
      drain();
    } catch (error) {
      console.error("[drain fatale]", error);
      shutdown(1);
    }
  }, 16);

  if (Deno.build.os === "windows") {
    console.error(`[fase] AppUserModelID: ${APP_USER_MODEL_ID}`);
    const registration = await request("tauriless:set-app-user-model-id", {
      appId: APP_USER_MODEL_ID,
      name: "Tauriless Deno Demo",
    });
    console.error(`[path lnk] ${registration.shortcutPath}`);
  }

  console.error("[fase] creazione webview…");
  const created = await request("plugin:webview|create_webview_window", {
    options: {
      label: WINDOW_LABEL,
      title: "Tauriless · Deno npm",
      url: "index.html",
      width: 820,
      height: 800,
      minWidth: 560,
      minHeight: 600,
      center: true,
      visible: true,
      dragDropEnabled: true,
    },
  });
  if (created.webviewDataDirectory) {
    console.error(`[path WebView2] ${created.webviewDataDirectory}`);
  }

  const readyTimeout = setTimeout(() => {
    rejectWebviewReady(new Error("timeout bootstrap IPC della webview"));
  }, 10_000);
  await webviewReady;
  clearTimeout(readyTimeout);

  if (Deno.build.os === "windows") {
    const identifier = await request("plugin:app|identifier");
    console.error(`[verifica] Tauri config identifier: ${identifier}`);
    if (identifier !== APP_USER_MODEL_ID) {
      throw new Error(
        `identifier Tauri inatteso: ${identifier} != ${APP_USER_MODEL_ID}`,
      );
    }

    console.error("[fase] notifica Windows di prova…");
    await notify(
      "Tauriless",
      `Notifica con AppUserModelID ${APP_USER_MODEL_ID}`,
    );
  }

  await postToWebview({
    type: "deno-reply",
    text: "Bootstrap completata: messaggio inviato da Deno.",
  });
  await setHtml(
    `<strong>setHtml da Deno riuscito</strong><br>${
      new Date().toLocaleString()
    }`,
  );

  console.error("[fase] creazione menu tray…");
  const [menuRid] = await request("plugin:menu|new", {
    kind: "Menu",
    options: {
      id: "tauriless-deno-npm-menu",
      items: [
        { id: "tray-show", text: "Mostra finestra", enabled: true },
        { id: "tray-hide", text: "Nascondi finestra", enabled: true },
        { id: "tray-notify", text: "Notifica di prova", enabled: true },
        { id: "tray-set-html", text: "Aggiorna HTML", enabled: true },
        { id: "tray-quit", text: "Esci", enabled: true },
      ],
    },
    handler: "__CHANNEL__:9101",
  });

  await request("plugin:tray|new", {
    options: {
      id: TRAY_ID,
      menu: [menuRid, "Menu"],
      icon: trayIcon(),
      tooltip: "Tauriless · Deno npm",
      showMenuOnLeftClick: true,
    },
    handler: "__CHANNEL__:9102",
  });

  console.log("[pronto] Webview, IPC bidirezionale e systray attive.");
  console.log("[pronto] Usa Esci dal tray o Ctrl+C per terminare.");
  scheduleOptionalAutoExit();
}

function shutdown(exitCode) {
  if (quitting) return;
  quitting = true;
  if (drainTimer) clearInterval(drainTimer);

  for (const { reject } of pending.values()) {
    reject(new Error("Tauriless in chiusura"));
  }
  pending.clear();

  if (!closed) {
    closed = true;
    try {
      tauriless.close();
    } catch (error) {
      console.error("[destroy]", error);
    }
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
