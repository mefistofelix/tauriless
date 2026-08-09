//! Minimal, manually-pumped Tauri runtime.
//!
//! The Rust API and the exported C ABI intentionally share the same JSON
//! protocol. Tauri remains the owner of every window and webview; this crate
//! only owns the `tauri::App` that must stay on the host's main thread.

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::c_void,
    ptr, slice,
    sync::{Arc, Mutex},
    thread::{self, ThreadId},
};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{
    http::HeaderMap,
    ipc::{CallbackFn, InvokeBody, InvokeResponse, InvokeResponseBody, OwnedInvokeResponder},
    webview::InvokeRequest,
    EventId, Listener, Manager, RunEvent,
};
use thiserror::Error;

const BRIDGE_SCRIPT: &str = r#"
;(() => {
  if (window.__TAURILESS__) return;
  const send = async json => {
    const request = typeof json === "string" ? JSON.parse(json) : json;
    const command = request.cmd ?? request.method;
    const payload = request.payload ?? request.params ?? {};
    return window.__TAURI_INTERNALS__.invoke(command, payload);
  };
  Object.defineProperty(window, "__TAURILESS__", {
    value: Object.freeze({
      send,
      emit(event, payload = null) {
        return window.__TAURI_INTERNALS__.invoke("tauriless:event", { event, payload });
      }
    })
  });
  Object.defineProperty(window, "tauriless_send", { value: send });
})();
"#;
const BOOTSTRAP_LABEL: &str = "__tauriless";
const NATIVE_EVENT_NAMES: &[&str] = &[
    "tauri://resize",
    "tauri://move",
    "tauri://close-requested",
    "tauri://destroyed",
    "tauri://focus",
    "tauri://blur",
    "tauri://scale-change",
    "tauri://theme-changed",
    "tauri://drag-enter",
    "tauri://drag-over",
    "tauri://drag-drop",
    "tauri://drag-leave",
    "tauri://suspended",
    "tauri://resumed",
];

type Outbox = Arc<Mutex<Vec<Value>>>;
type WindowListeners = Arc<Mutex<HashMap<String, Vec<EventId>>>>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("the runtime must be used from the OS thread that created it")]
    WrongThread,
    #[error("the runtime has already been shut down")]
    Shutdown,
    #[error("drain is already running")]
    ReentrantDrain,
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid request: {0}")]
    Request(String),
    #[error("Tauri error: {0}")]
    Tauri(#[from] tauri::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Deserialize)]
struct Request {
    id: Value,
    #[serde(alias = "method")]
    cmd: String,
    #[serde(default, alias = "params")]
    payload: Value,
    #[serde(default = "bootstrap_label", alias = "target")]
    webview: String,
}

fn bootstrap_label() -> String {
    BOOTSTRAP_LABEL.into()
}

/// The single opaque object held by foreign-language hosts.
pub struct Tauriless {
    app: Option<tauri::App>,
    owner: ThreadId,
    outbox: Outbox,
    draining: bool,
}

impl Tauriless {
    pub fn new() -> Result<Self> {
        let outbox = Outbox::default();
        let event_plugin = event_forwarder(Arc::clone(&outbox));
        let channel_outbox = Arc::clone(&outbox);
        let ipc_outbox = Arc::clone(&outbox);
        let app = tauri::Builder::default()
            .plugin(event_plugin)
            .plugin(tauri_plugin_notification::init())
            .plugin(tauri_plugin_opener::init())
            .plugin(tauri_plugin_os::init())
            .channel_interceptor(move |webview, callback, index, body| {
                // The hidden webview is the C bridge's dedicated IPC context.
                // Leave channels created by real webview JavaScript untouched.
                if webview.label() != BOOTSTRAP_LABEL {
                    return false;
                }
                push(
                    &channel_outbox,
                    json!({
                      "kind": "channel",
                      "webview": webview.label(),
                      "id": callback.0,
                      "index": index,
                      "message": response_body(body)
                    }),
                );
                true
            })
            .append_invoke_initialization_script(BRIDGE_SCRIPT)
            .invoke_handler(move |invoke| {
                if invoke.message.command() != "tauriless:event" {
                    return false;
                }
                let body = match invoke.message.payload() {
                    InvokeBody::Json(value) => value.clone(),
                    InvokeBody::Raw(bytes) => json!({ "bytes": bytes }),
                };
                push(
                    &ipc_outbox,
                    json!({
                      "kind": "event",
                      "source": "webview",
                      "window": invoke.message.webview_ref().label(),
                      "event": body.get("event").cloned().unwrap_or(Value::Null),
                      "payload": body.get("payload").cloned().unwrap_or(Value::Null)
                    }),
                );
                invoke.resolver.resolve(());
                true
            })
            .build(tauri::generate_context!())?;

        // Tauri installs its built-in plugins and creates the hidden IPC
        // webview during setup, which runs on the first non-blocking turn.
        let mut app = app;
        let setup_outbox = Arc::clone(&outbox);
        #[allow(deprecated)]
        app.run_iteration(move |_app, event| collect_event(&setup_outbox, event));

        Ok(Self {
            app: Some(app),
            owner: thread::current().id(),
            outbox,
            draining: false,
        })
    }

    /// Executes one JSON request immediately on Tauri's owning thread.
    /// Results are returned by the next `drain` call.
    pub fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.check_thread()?;
        self.check_running()?;
        let request: Request = serde_json::from_slice(bytes)?;
        validate(&request)?;
        let webview = self
            .app()?
            .get_webview_window(&request.webview)
            .ok_or_else(|| Error::Request(format!("unknown webview `{}`", request.webview)))?;
        let url = webview.url()?;
        let invoke_key = self.app()?.handle().invoke_key().to_owned();
        let outbox = Arc::clone(&self.outbox);
        let id = request.id;
        let responder: Box<OwnedInvokeResponder<tauri::Wry>> =
            Box::new(move |_webview, _command, response, _callback, _error| {
                result(&outbox, id, invoke_response(response));
            });
        webview.on_message(
            InvokeRequest {
                cmd: request.cmd,
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url,
                body: InvokeBody::Json(request.payload),
                headers: HeaderMap::new(),
                invoke_key,
            },
            responder,
        );
        Ok(())
    }

    /// Pumps exactly one non-blocking Tauri iteration and returns queued JSON.
    pub fn drain(&mut self) -> Result<Vec<u8>> {
        self.check_thread()?;
        self.check_running()?;
        if self.draining {
            return Err(Error::ReentrantDrain);
        }
        self.draining = true;

        let outbox = Arc::clone(&self.outbox);
        #[allow(deprecated)]
        self.app
            .as_mut()
            .ok_or(Error::Shutdown)?
            .run_iteration(move |_app, event| collect_event(&outbox, event));

        let messages = std::mem::take(&mut *self.outbox.lock().expect("outbox mutex poisoned"));
        self.draining = false;
        Ok(serde_json::to_vec(&json!({ "messages": messages }))?)
    }

    pub fn shutdown(&mut self) -> Result<()> {
        self.check_thread()?;
        if let Some(app) = self.app.take() {
            app.cleanup_before_exit();
        }
        Ok(())
    }

    fn app(&self) -> Result<&tauri::App> {
        self.app.as_ref().ok_or(Error::Shutdown)
    }

    fn check_thread(&self) -> Result<()> {
        if thread::current().id() == self.owner {
            Ok(())
        } else {
            Err(Error::WrongThread)
        }
    }

    fn check_running(&self) -> Result<()> {
        self.app.as_ref().map(|_| ()).ok_or(Error::Shutdown)
    }
}

impl Drop for Tauriless {
    fn drop(&mut self) {
        if thread::current().id() == self.owner {
            let _ = self.shutdown();
        }
    }
}

fn validate(request: &Request) -> Result<()> {
    if request.cmd.is_empty() {
        return Err(Error::Request("cmd must not be empty".into()));
    }
    if !request.id.is_string() && !request.id.is_number() {
        return Err(Error::Request("id must be a string or number".into()));
    }
    if request.webview.is_empty() {
        return Err(Error::Request("webview must not be empty".into()));
    }
    Ok(())
}

fn invoke_response(response: InvokeResponse) -> std::result::Result<Value, Value> {
    match response {
        InvokeResponse::Ok(body) => Ok(response_body(&body)),
        InvokeResponse::Err(error) => Err(error.0),
    }
}

fn response_body(body: &InvokeResponseBody) -> Value {
    match body {
        InvokeResponseBody::Json(value) => {
            serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.clone()))
        }
        InvokeResponseBody::Raw(bytes) => json!({ "bytes": bytes }),
    }
}

fn result(outbox: &Outbox, id: Value, value: std::result::Result<Value, Value>) {
    match value {
        Ok(value) => push(
            outbox,
            json!({ "kind": "result", "id": id, "ok": true, "value": value }),
        ),
        Err(error) => push(
            outbox,
            json!({ "kind": "result", "id": id, "ok": false, "error": error }),
        ),
    }
}

fn push(outbox: &Outbox, value: Value) {
    outbox.lock().expect("outbox mutex poisoned").push(value);
}

fn event_forwarder(outbox: Outbox) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let window_listeners = WindowListeners::default();
    let window_outbox = Arc::clone(&outbox);
    let pending_window_listeners = Arc::clone(&window_listeners);
    let webview_outbox = outbox;

    tauri::plugin::Builder::new("tauriless-events")
        .on_window_ready(move |window| {
            let label = window.label().to_owned();
            if label == BOOTSTRAP_LABEL {
                return;
            }
            let ids = forward_native_events(&window, &window_outbox, "window", &label);
            pending_window_listeners
                .lock()
                .expect("window listener mutex poisoned")
                .insert(label, ids);
        })
        .on_webview_ready(move |webview| {
            let label = webview.label().to_owned();
            if label == BOOTSTRAP_LABEL {
                return;
            }

            // A WebviewWindow is first announced as a Window and then as a
            // Webview. Replace the temporary Window listeners with one
            // WebviewWindow target so Tauri's AnyLabel events are not doubled.
            if let Some(ids) = window_listeners
                .lock()
                .expect("window listener mutex poisoned")
                .remove(&label)
            {
                for id in ids {
                    webview.unlisten(id);
                }
            }

            if let Some(window) = webview.app_handle().get_webview_window(&label) {
                forward_native_events(&window, &webview_outbox, "webview-window", &label);
            } else {
                forward_native_events(&webview, &webview_outbox, "webview", &label);
            }
        })
        .build()
}

fn forward_native_events<M: Listener<tauri::Wry>>(
    target: &M,
    outbox: &Outbox,
    source: &'static str,
    label: &str,
) -> Vec<EventId> {
    NATIVE_EVENT_NAMES
        .iter()
        .map(|&name| {
            let outbox = Arc::clone(outbox);
            let label = label.to_owned();
            target.listen(name, move |event| {
                let payload = serde_json::from_str(event.payload())
                    .unwrap_or_else(|_| Value::String(event.payload().to_owned()));
                push(
                    &outbox,
                    json!({
                      "kind": "event",
                      "source": source,
                      "window": label,
                      "event": name,
                      "payload": payload
                    }),
                );
            })
        })
        .collect()
}

fn collect_event(outbox: &Outbox, run_event: RunEvent) {
    if let RunEvent::ExitRequested { code, api, .. } = run_event {
        // The embedding host owns process lifetime and may create another window.
        api.prevent_exit();
        push(
            outbox,
            json!({
              "kind": "event", "source": "runtime", "event": "exitRequested",
              "payload": { "code": code }
            }),
        );
    }
}

// --- C ABI ---------------------------------------------------------------

pub const TAURILESS_OK: i32 = 0;
pub const TAURILESS_INVALID_ARGUMENT: i32 = 1;
pub const TAURILESS_ERROR: i32 = 2;
pub const TAURILESS_PANIC: i32 = 3;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TaurilessBuffer {
    pub data: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl TaurilessBuffer {
    const EMPTY: Self = Self {
        data: ptr::null_mut(),
        len: 0,
        capacity: 0,
    };

    fn from_vec(mut value: Vec<u8>) -> Self {
        let buffer = Self {
            data: value.as_mut_ptr(),
            len: value.len(),
            capacity: value.capacity(),
        };
        std::mem::forget(value);
        buffer
    }
}

thread_local! {
  static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn ffi<F>(operation: F) -> i32
where
    F: FnOnce() -> std::result::Result<(), (i32, String)>,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(Ok(())) => TAURILESS_OK,
        Ok(Err((code, message))) => {
            LAST_ERROR.with(|slot| *slot.borrow_mut() = message);
            code
        }
        Err(_) => {
            LAST_ERROR.with(|slot| *slot.borrow_mut() = "Rust panic at the C ABI boundary".into());
            TAURILESS_PANIC
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn tauriless_create(out: *mut *mut Tauriless) -> i32 {
    ffi(|| {
        if out.is_null() {
            return Err((TAURILESS_INVALID_ARGUMENT, "out is null".into()));
        }
        *out = ptr::null_mut();
        let runtime = Tauriless::new().map_err(|error| (TAURILESS_ERROR, error.to_string()))?;
        *out = Box::into_raw(Box::new(runtime));
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn tauriless_send(
    runtime: *mut Tauriless,
    json: *const u8,
    len: usize,
) -> i32 {
    ffi(|| {
        let runtime = runtime
            .as_mut()
            .ok_or((TAURILESS_INVALID_ARGUMENT, "runtime is null".into()))?;
        if json.is_null() && len != 0 {
            return Err((TAURILESS_INVALID_ARGUMENT, "json is null".into()));
        }
        let bytes = if len == 0 {
            &[]
        } else {
            slice::from_raw_parts(json, len)
        };
        runtime
            .send(bytes)
            .map_err(|error| (TAURILESS_ERROR, error.to_string()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn tauriless_drain(
    runtime: *mut Tauriless,
    out: *mut TaurilessBuffer,
) -> i32 {
    ffi(|| {
        if out.is_null() {
            return Err((TAURILESS_INVALID_ARGUMENT, "out is null".into()));
        }
        *out = TaurilessBuffer::EMPTY;
        let runtime = runtime
            .as_mut()
            .ok_or((TAURILESS_INVALID_ARGUMENT, "runtime is null".into()))?;
        let bytes = runtime
            .drain()
            .map_err(|error| (TAURILESS_ERROR, error.to_string()))?;
        *out = TaurilessBuffer::from_vec(bytes);
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn tauriless_destroy(runtime: *mut Tauriless) -> i32 {
    ffi(|| {
        if runtime.is_null() {
            return Ok(());
        }
        runtime
            .as_ref()
            .expect("runtime was checked for null")
            .check_thread()
            .map_err(|error| (TAURILESS_ERROR, error.to_string()))?;
        let mut runtime = Box::from_raw(runtime);
        runtime
            .shutdown()
            .map_err(|error| (TAURILESS_ERROR, error.to_string()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn tauriless_last_error(out: *mut TaurilessBuffer) -> i32 {
    ffi(|| {
        if out.is_null() {
            return Err((TAURILESS_INVALID_ARGUMENT, "out is null".into()));
        }
        *out = LAST_ERROR.with(|slot| TaurilessBuffer::from_vec(slot.borrow().as_bytes().to_vec()));
        Ok(())
    })
}

#[no_mangle]
pub unsafe extern "C" fn tauriless_buffer_free(data: *mut c_void, len: usize, capacity: usize) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !data.is_null() {
            drop(Vec::from_raw_parts(data.cast::<u8>(), len, capacity));
        }
    }))
    .is_err()
    {
        LAST_ERROR.with(|slot| *slot.borrow_mut() = "Rust panic while freeing a C buffer".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_script_is_safe_to_append_to_tauri_ipc_script() {
        assert!(BRIDGE_SCRIPT.trim_start().starts_with(';'));
    }

    #[test]
    fn request_validation_is_small_and_explicit() {
        let request: Request =
            serde_json::from_str(r#"{"id":1,"cmd":"plugin:app|name","payload":{}}"#).unwrap();
        validate(&request).unwrap();
    }

    #[test]
    fn rejects_requests_without_callback_id() {
        let request: Request =
            serde_json::from_str(r#"{"id":null,"cmd":"plugin:app|name"}"#).unwrap();
        assert!(validate(&request).is_err());
    }
}
