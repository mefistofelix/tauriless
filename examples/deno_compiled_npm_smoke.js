// Minimal single-file smoke app for the public precompiled npm package.
// After publishing the version below, compile from the repository root with:
//   deno compile --allow-ffi --allow-env --icon tauriless/icons/icon.ico \
//     --output .build/tauriless-deno-smoke.exe examples/deno_compiled_npm_smoke.js

import { Tauriless } from "npm:@mefistofelix/tauriless@0.1.11";

const APP_ID = "dev.tauriless.deno-compiled-smoke";
const APP_NAME = "Tauriless Deno Compiled Smoke";
const WINDOW_LABEL = "main";
const READY_EVENT = "tauriless://compiled-smoke-ready";
const DATA_EVENT = "tauriless://compiled-smoke-data";
const runtime = new Tauriless();
const pending = new Map();
let nextId = 1;
let closed = false;
let readyResolve;
const ready = new Promise((resolve) => readyResolve = resolve);

function request(cmd, payload = {}, webview) {
  const id = nextId++;
  const message = { id, cmd, payload };
  if (webview !== undefined) message.webview = webview;
  runtime.send(message);
  return new Promise((resolve, reject) => {
    pending.set(id, { cmd, resolve, reject });
  });
}

function drain() {
  if (closed) return;
  for (const message of runtime.drain().messages) {
    console.log(JSON.stringify(message));
    if (message.kind === "asset-request") {
      void handleAssetRequest(message).catch((error) =>
        console.error("asset-response:", error)
      );
    } else if (message.kind === "result") {
      const callback = pending.get(message.id);
      if (!callback) continue;
      pending.delete(message.id);
      if (message.ok) callback.resolve(message.value);
      else callback.reject(message.error);
    } else if (message.kind === "event" && message.event === READY_EVENT) {
      readyResolve();
    } else if (
      message.kind === "event" &&
      message.event === "tauri://destroyed" &&
      message.window === WINDOW_LABEL
    ) {
      shutdown(0);
    }
  }
}

async function handleAssetRequest(message) {
  const pathname = new URL(message.url).pathname;
  if (pathname === "/" || pathname === "/index.html") {
    await request("tauriless:asset-response", {
      requestId: message.requestId,
      mime: "text/html; charset=utf-8",
      content: WEBVIEW_HTML,
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

const timer = setInterval(() => {
  try {
    drain();
  } catch (error) {
    console.error("drain:", error);
    shutdown(1);
  }
}, 16);

const WEBVIEW_URL = "index.html";
const WEBVIEW_HTML = `<!doctype html>
<html lang="it">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Tauriless Deno compiled smoke</title>
  <style>
    :root { color-scheme: dark; font: 15px/1.45 system-ui, sans-serif; }
    body { margin: 0; padding: 24px; background: #09131d; color: #eaf5ff; }
    main { max-width: 760px; margin: auto; display: grid; gap: 14px; }
    section { padding: 16px; border: 1px solid #31516a; border-radius: 12px; background: #102334; }
    dl { display: grid; grid-template-columns: 150px 1fr; gap: 8px 12px; margin: 0; }
    dt { color: #8fc9ef; } dd { margin: 0; overflow-wrap: anywhere; }
    textarea { width: 100%; min-height: 72px; box-sizing: border-box; padding: 10px; }
    button { padding: 10px 16px; border: 0; border-radius: 999px; background: #48b8ff; font-weight: 700; cursor: pointer; }
    pre { min-height: 70px; white-space: pre-wrap; overflow-wrap: anywhere; color: #91efb5; }
    .error { color: #ff9b9b; }
  </style>
</head>
<body><main>
  <h1>Tauriless · Deno EXE</h1>
  <section><dl>
    <dt>AppUserModelID</dt><dd id="app-id">—</dd>
    <dt>Eseguibile</dt><dd id="exe-path">—</dd>
    <dt>Collegamento .lnk</dt><dd id="lnk-path">—</dd>
    <dt>WebView2 data dir</dt><dd id="webview-path">—</dd>
  </dl></section>
  <section>
    <textarea id="body">Notifica inviata dalla webview Tauri dentro un EXE Deno compilato.</textarea>
    <p><button id="notify">Invia notifica</button></p>
    <pre id="status">Bootstrap…</pre>
  </section>
</main><script>
  const internals = window.__TAURI_INTERNALS__;
  const invoke = (command, payload = {}) => internals.invoke(command, payload);
  const status = document.querySelector('#status');
  const show = value => typeof value === 'string' ? value : JSON.stringify(value, null, 2);

  async function listen(event, handler) {
    const handlerId = internals.transformCallback(handler);
    return await invoke('plugin:event|listen', {
      event,
      target: { kind: 'Any' },
      handler: handlerId,
    });
  }

  listen('${DATA_EVENT}', event => {
    const data = event.payload || {};
    const registration = data.registration || {};
    const created = data.created || {};
    document.querySelector('#app-id').textContent = registration.appId || 'non impostato';
    document.querySelector('#exe-path').textContent = registration.executablePath || '—';
    document.querySelector('#lnk-path').textContent = registration.shortcutPath || '—';
    document.querySelector('#webview-path').textContent = created.webviewDataDirectory || '—';
    status.className = data.registrationError ? 'error' : '';
    status.textContent = data.registrationError
      ? 'Registrazione Windows fallita:\\n' + show(data.registrationError)
      : 'Pronto. Premi il pulsante per provare la notifica.';
  });

  document.querySelector('#notify').addEventListener('click', async () => {
    status.className = '';
    status.textContent = 'Invio notifica…';
    try {
      const value = await invoke('plugin:notification|notify', {
        options: { title: '${APP_NAME}', body: document.querySelector('#body').value },
      });
      status.textContent = 'Notifica inviata. Ritorno:\\n' + show(value);
    } catch (error) {
      status.className = 'error';
      status.textContent = 'Errore notifica:\\n' + show(error);
    }
  });

  invoke('plugin:event|emit', { event: '${READY_EVENT}', payload: null });
</script></body></html>`;

async function main() {
  let registration = null;
  let registrationError = null;
  try {
    registration = await request("tauriless:set-app-user-model-id", {
      appId: APP_ID,
      name: APP_NAME,
    });
    console.log("shortcutPath:", registration.shortcutPath);
  } catch (error) {
    registrationError = error;
    console.error("set-app-user-model-id:", JSON.stringify(error));
  }

  await request("tauriless:subscribe", { event: READY_EVENT });
  const created = await request("plugin:webview|create_webview_window", {
    options: {
      label: WINDOW_LABEL,
      title: APP_NAME,
      url: WEBVIEW_URL,
      width: 820,
      height: 620,
      center: true,
      visible: true,
    },
  });
  console.log("webviewDataDirectory:", created.webviewDataDirectory);

  await Promise.race([
    ready,
    new Promise((_, reject) =>
      setTimeout(() => reject(new Error("webview ready timeout")), 10_000)
    ),
  ]);
  await request("plugin:event|emit_to", {
    target: { kind: "WebviewWindow", label: WINDOW_LABEL },
    event: DATA_EVENT,
    payload: { registration, registrationError, created },
  });

  const autoExit = Number(Deno.env.get("TAURILESS_COMPILED_AUTO_EXIT_MS"));
  if (Number.isFinite(autoExit) && autoExit > 0) {
    setTimeout(() => shutdown(0), autoExit);
  }
}

function shutdown(code) {
  if (closed) return;
  closed = true;
  clearInterval(timer);
  for (const callback of pending.values()) {
    callback.reject(new Error("Tauriless closed"));
  }
  pending.clear();
  runtime.close();
  Deno.exit(code);
}

main().catch((error) => {
  console.error("fatal:", JSON.stringify(error));
  shutdown(1);
});
