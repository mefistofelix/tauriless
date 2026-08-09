//! Minimal, manually-pumped Tauri runtime.
//!
//! The Rust API and the exported C ABI intentionally share the same JSON
//! protocol. Tauri remains the owner of every window and webview; this crate
//! only owns the `tauri::App` that must stay on the host's main thread.

mod asset_protocol;
mod event_subscriptions;

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{c_char, CStr, CString},
    ptr,
    sync::{Arc, Mutex},
    thread::{self, ThreadId},
};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{
    http::HeaderMap,
    ipc::{CallbackFn, InvokeBody, InvokeResponse, InvokeResponseBody, OwnedInvokeResponder},
    utils::config::WindowConfig,
    webview::InvokeRequest,
    Manager, RunEvent, Webview, WebviewWindow, WebviewWindowBuilder,
};
use thiserror::Error;

const FORWARDED_EVENT_NAMES: &[&str] = &[
    "tauri://resize",
    "tauri://move",
    "tauri://close-requested",
    "tauri://destroyed",
    "tauri://focus",
    "tauri://blur",
    "tauri://scale-change",
    "tauri://theme-changed",
    "tauri://window-created",
    "tauri://webview-created",
    "tauri://drag-enter",
    "tauri://drag-over",
    "tauri://drag-drop",
    "tauri://drag-leave",
    "tauri://suspended",
    "tauri://resumed",
    // Named Rust event-bus emissions from the official plugins-workspace v2.
    // Most other asynchronous plugin APIs use Channel<T> and are caught by
    // the global channel_interceptor instead of this exact-name list.
    "deep-link://new-url",
    "log://log",
    "store://change",
    // Application messages emitted by webview JavaScript through Tauri's
    // standard event plugin. This is an event name, not a custom command.
    "tauriless://webview-message",
];
const CREATE_WEBVIEW_WINDOW_COMMAND: &str = "plugin:webview|create_webview_window";

pub(crate) type Outbox = Arc<Mutex<Vec<Value>>>;

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
    #[serde(default, alias = "target")]
    webview: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateWebviewWindowPayload {
    options: WindowConfig,
}

/// The single opaque object held by foreign-language hosts.
pub struct Tauriless {
    app: Option<tauri::App>,
    owner: ThreadId,
    outbox: Outbox,
    asset_protocol: asset_protocol::AssetProtocol,
    event_subscriptions: event_subscriptions::SharedEventSubscriptions,
    draining: bool,
    drain_buffer: CString,
}

impl Tauriless {
    pub fn new() -> Result<Self> {
        let outbox = Outbox::default();
        let event_subscriptions = event_subscriptions::EventSubscriptions::new(
            Arc::clone(&outbox),
            FORWARDED_EVENT_NAMES,
        );
        let event_plugin = event_forwarder(Arc::clone(&event_subscriptions));
        let channel_outbox = Arc::clone(&outbox);
        let asset_protocol = asset_protocol::AssetProtocol::new(Arc::clone(&outbox));
        let builder = tauri::Builder::default()
            .plugin(event_plugin)
            .plugin(tauri_plugin_notification::init())
            .plugin(tauri_plugin_opener::init())
            .plugin(tauri_plugin_os::init())
            .plugin(tauri_plugin_positioner::init())
            .plugin(tauri_plugin_store::Builder::default().build());
        let app = asset_protocol
            .register(builder)
            .channel_interceptor(move |webview, callback, index, body| {
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
                // This experiment routes every Tauri Channel<T> to the host.
                // Returning true prevents the normal JavaScript delivery.
                true
            })
            .build(tauri::generate_context!())?;

        // Complete Tauri setup without creating a bootstrap window or webview.
        let mut app = app;
        let setup_outbox = Arc::clone(&outbox);
        #[allow(deprecated)]
        app.run_iteration(move |_app, event| collect_event(&setup_outbox, event));

        Ok(Self {
            app: Some(app),
            owner: thread::current().id(),
            outbox,
            asset_protocol,
            event_subscriptions,
            draining: false,
            drain_buffer: CString::default(),
        })
    }

    /// Executes one JSON request immediately on Tauri's owning thread.
    /// Results are returned by the next `drain` call.
    pub fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.check_thread()?;
        self.check_running()?;
        let request: Request = serde_json::from_slice(bytes)?;
        validate(&request)?;

        if request.cmd == asset_protocol::RESPONSE_COMMAND {
            let outcome = self
                .asset_protocol
                .respond(request.payload)
                .map(|_| Value::Null)
                .map_err(Value::String);
            result(&self.outbox, request.id, outcome);
            return Ok(());
        }

        if event_subscriptions::EventSubscriptions::handles(&request.cmd) {
            let outcome = self
                .event_subscriptions
                .lock()
                .expect("event subscriptions mutex poisoned")
                .handle(&request.cmd, request.payload)
                .map_err(Value::String);
            result(&self.outbox, request.id, outcome);
            return Ok(());
        }

        let webviews = self.app()?.webview_windows();
        if webviews.is_empty() {
            return self.create_first_webview(request);
        }

        let webview = select_webview(webviews, request.webview.as_deref())?;
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
        self.event_subscriptions
            .lock()
            .expect("event subscriptions mutex poisoned")
            .clear_targets();
        if let Some(app) = self.app.take() {
            app.cleanup_before_exit();
        }
        Ok(())
    }

    fn app(&self) -> Result<&tauri::App> {
        self.app.as_ref().ok_or(Error::Shutdown)
    }

    fn create_first_webview(&self, request: Request) -> Result<()> {
        require_initial_create_command(&request)?;

        let app = self.app()?;
        let outcome = serde_json::from_value::<CreateWebviewWindowPayload>(request.payload)
            .map_err(|error| error.to_string())
            .and_then(|payload| {
                WebviewWindowBuilder::from_config(app.handle(), &payload.options)
                    .and_then(|builder| builder.build())
                    .map(|_| Value::Null)
                    .map_err(|error| error.to_string())
            });

        result(&self.outbox, request.id, outcome.map_err(Value::String));
        Ok(())
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

fn require_initial_create_command(request: &Request) -> Result<()> {
    if request.cmd == CREATE_WEBVIEW_WINDOW_COMMAND {
        Ok(())
    } else {
        Err(Error::Request(format!(
            "no webview exists; the first command must be `{CREATE_WEBVIEW_WINDOW_COMMAND}`"
        )))
    }
}

fn select_webview(
    mut webviews: HashMap<String, WebviewWindow>,
    requested: Option<&str>,
) -> Result<Webview> {
    if let Some(label) = requested {
        return webviews
            .remove(label)
            .map(|webview_window| webview_window.as_ref().clone())
            .ok_or_else(|| Error::Request(format!("unknown webview `{label}`")));
    }

    let label = choose_default_webview_label(webviews.keys().cloned().collect())?;
    Ok(webviews
        .remove(&label)
        .expect("the selected webview label came from the same map")
        .as_ref()
        .clone())
}

fn choose_default_webview_label(mut labels: Vec<String>) -> Result<String> {
    labels.sort();
    match labels.as_slice() {
        [label] => Ok(label.clone()),
        labels if labels.iter().any(|label| label == "main") => Ok("main".into()),
        [] => Err(Error::Request("no webview exists".into())),
        _ => Err(Error::Request(format!(
            "multiple webviews exist ({labels}); set `webview` explicitly",
            labels = labels.join(", ")
        ))),
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
    if request.webview.as_deref() == Some("") {
        return Err(Error::Request(
            "webview must not be empty when provided".into(),
        ));
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

pub(crate) fn push(outbox: &Outbox, value: Value) {
    outbox.lock().expect("outbox mutex poisoned").push(value);
}

fn event_forwarder(
    subscriptions: event_subscriptions::SharedEventSubscriptions,
) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let window_subscriptions = Arc::clone(&subscriptions);
    tauri::plugin::Builder::new("tauriless-events")
        .on_window_ready(move |window| {
            event_subscriptions::EventSubscriptions::register_window(&window_subscriptions, window);
        })
        .on_webview_ready(move |webview| {
            let label = webview.label().to_owned();
            // A WebviewWindow is first announced as a Window and then as a
            // Webview. Replace its Window target with one WebviewWindow target
            // so Tauri's AnyLabel events are not doubled.
            if let Some(window) = webview.app_handle().get_webview_window(&label) {
                event_subscriptions::EventSubscriptions::register_webview_window(
                    &subscriptions,
                    window,
                );
            } else {
                event_subscriptions::EventSubscriptions::register_webview(&subscriptions, webview);
            }
        })
        .build()
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

thread_local! {
  static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_last_error(mut message: String) {
    message.retain(|character| character != '\0');
    LAST_ERROR
        .with(|slot| *slot.borrow_mut() = CString::new(message).expect("NUL bytes were removed"));
}

fn ffi<F>(operation: F) -> i32
where
    F: FnOnce() -> std::result::Result<(), (i32, String)>,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(Ok(())) => TAURILESS_OK,
        Ok(Err((code, message))) => {
            set_last_error(message);
            code
        }
        Err(_) => {
            set_last_error("Rust panic at the C ABI boundary".into());
            TAURILESS_PANIC
        }
    }
}

fn ffi_pointer<F>(operation: F) -> *const c_char
where
    F: FnOnce() -> std::result::Result<*const c_char, (i32, String)>,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(Ok(pointer)) => pointer,
        Ok(Err((_code, message))) => {
            set_last_error(message);
            ptr::null()
        }
        Err(_) => {
            set_last_error("Rust panic at the C ABI boundary".into());
            ptr::null()
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
pub unsafe extern "C" fn tauriless_send(runtime: *mut Tauriless, json: *const c_char) -> i32 {
    ffi(|| {
        let runtime = runtime
            .as_mut()
            .ok_or((TAURILESS_INVALID_ARGUMENT, "runtime is null".into()))?;
        if json.is_null() {
            return Err((TAURILESS_INVALID_ARGUMENT, "json is null".into()));
        }
        let bytes = CStr::from_ptr(json).to_bytes();
        runtime
            .send(bytes)
            .map_err(|error| (TAURILESS_ERROR, error.to_string()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn tauriless_drain(runtime: *mut Tauriless) -> *const c_char {
    ffi_pointer(|| {
        let runtime = runtime
            .as_mut()
            .ok_or((TAURILESS_INVALID_ARGUMENT, "runtime is null".into()))?;
        let bytes = runtime
            .drain()
            .map_err(|error| (TAURILESS_ERROR, error.to_string()))?;
        runtime.drain_buffer = CString::new(bytes)
            .map_err(|_| (TAURILESS_ERROR, "drain JSON contains a NUL byte".into()))?;
        Ok(runtime.drain_buffer.as_ptr())
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
pub extern "C" fn tauriless_last_error() -> *const c_char {
    std::panic::catch_unwind(|| LAST_ERROR.with(|slot| slot.borrow().as_ptr()))
        .unwrap_or(ptr::null())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_may_omit_a_webview_label() {
        let request: Request =
            serde_json::from_str(r#"{"id":1,"cmd":"plugin:app|name","payload":{}}"#).unwrap();
        assert!(request.webview.is_none());
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

    #[test]
    fn default_webview_selection_is_deterministic() {
        assert_eq!(
            choose_default_webview_label(vec!["only".into()]).unwrap(),
            "only"
        );
        assert_eq!(
            choose_default_webview_label(vec!["secondary".into(), "main".into()]).unwrap(),
            "main"
        );

        let error = choose_default_webview_label(vec!["zeta".into(), "alpha".into()]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "invalid request: multiple webviews exist (alpha, zeta); set `webview` explicitly"
        );
    }

    #[test]
    fn only_webview_window_creation_is_allowed_without_a_context() {
        let rejected: Request =
            serde_json::from_str(r#"{"id":1,"cmd":"plugin:app|name","payload":{}}"#).unwrap();
        assert!(require_initial_create_command(&rejected).is_err());

        let accepted: Request = serde_json::from_str(
            r#"{"id":1,"cmd":"plugin:webview|create_webview_window","payload":{"options":{"label":"main"}}}"#,
        )
        .unwrap();
        require_initial_create_command(&accepted).unwrap();
    }
}
