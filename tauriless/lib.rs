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
#[cfg(windows)]
use sha2::{Digest, Sha256};
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
const SET_APP_USER_MODEL_ID_COMMAND: &str = "tauriless:set-app-user-model-id";
const DEFAULT_IDENTIFIER: &str = "Tauriless";

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

#[derive(Debug, Deserialize)]
struct SetAppUserModelIdPayload {
    #[serde(rename = "appId", alias = "appID")]
    app_id: String,
    #[serde(default)]
    name: Option<String>,
}

/// The single opaque object held by foreign-language hosts.
pub struct Tauriless {
    app: Option<tauri::App>,
    app_user_model_id: Option<String>,
    closed: bool,
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
        let asset_protocol = asset_protocol::AssetProtocol::new(Arc::clone(&outbox));

        Ok(Self {
            app: None,
            app_user_model_id: None,
            closed: false,
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

        if request.cmd == SET_APP_USER_MODEL_ID_COMMAND {
            let outcome = if self.app.is_some() {
                Err(json!({
                  "operation": "set-app-user-model-id",
                  "message": "AppUserModelID must be set before the Tauri app is initialized"
                }))
            } else {
                set_current_process_app_user_model_id(request.payload).map(|(app_id, value)| {
                    self.app_user_model_id = Some(app_id);
                    value
                })
            }
            .map_err(|error| error);
            result(&self.outbox, request.id, outcome);
            return Ok(());
        }

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

        if request.cmd == CREATE_WEBVIEW_WINDOW_COMMAND {
            return self.create_webview(request);
        }

        if self.app.is_none() {
            return Err(Error::Request(format!(
                "no webview exists; the first command must be `{CREATE_WEBVIEW_WINDOW_COMMAND}`"
            )));
        }

        let webviews = self.app()?.webview_windows();
        if webviews.is_empty() {
            return Err(Error::Request(format!(
                "no webview exists; create one with `{CREATE_WEBVIEW_WINDOW_COMMAND}`"
            )));
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

        if let Some(app) = self.app.as_mut() {
            let outbox = Arc::clone(&self.outbox);
            #[allow(deprecated)]
            app.run_iteration(move |_app, event| collect_event(&outbox, event));
        }

        let messages = std::mem::take(&mut *self.outbox.lock().expect("outbox mutex poisoned"));
        self.draining = false;
        Ok(serde_json::to_vec(&json!({ "messages": messages }))?)
    }

    pub fn shutdown(&mut self) -> Result<()> {
        self.check_thread()?;
        if self.closed {
            return Ok(());
        }
        self.event_subscriptions
            .lock()
            .expect("event subscriptions mutex poisoned")
            .clear_targets();
        if let Some(app) = self.app.take() {
            app.cleanup_before_exit();
        }
        self.closed = true;
        Ok(())
    }

    fn app(&self) -> Result<&tauri::App> {
        self.app.as_ref().ok_or(Error::Shutdown)
    }

    fn create_webview(&mut self, request: Request) -> Result<()> {
        let payload = match serde_json::from_value::<CreateWebviewWindowPayload>(request.payload) {
            Ok(payload) => payload,
            Err(error) => {
                result(
                    &self.outbox,
                    request.id,
                    Err(json!({
                      "operation": "parse-create-webview-window",
                      "message": error.to_string()
                    })),
                );
                return Ok(());
            }
        };
        if let Err(error) = self.build_app() {
            result(
                &self.outbox,
                request.id,
                Err(json!({
                  "operation": "build-tauri-app",
                  "message": error.to_string()
                })),
            );
            return Ok(());
        }

        let app = self.app()?;
        #[cfg(windows)]
        let data_directory = {
            let local_data = match app.path().local_data_dir() {
                Ok(path) => path,
                Err(error) => {
                    result(
                        &self.outbox,
                        request.id,
                        Err(json!({
                          "operation": "resolve-local-data-directory",
                          "message": error.to_string()
                        })),
                    );
                    return Ok(());
                }
            };
            let resolved = match &payload.options.data_directory {
                Some(relative) => resolved_explicit_webview_data_directory(
                    &local_data,
                    &payload.options.label,
                    relative,
                ),
                None => default_webview_data_directory(&local_data),
            };
            let path = match resolved {
                Ok(path) => path,
                Err(error) => {
                    result(&self.outbox, request.id, Err(error));
                    return Ok(());
                }
            };
            path
        };

        let mut builder = match WebviewWindowBuilder::from_config(app.handle(), &payload.options) {
            Ok(builder) => builder,
            Err(error) => {
                #[cfg(windows)]
                let error = json!({
                  "operation": "create-webview-window",
                  "message": error.to_string(),
                  "webviewDataDirectory": path_string(&data_directory)
                });
                #[cfg(not(windows))]
                let error = json!({
                  "operation": "create-webview-window",
                  "message": error.to_string()
                });
                result(&self.outbox, request.id, Err(error));
                return Ok(());
            }
        };
        #[cfg(windows)]
        {
            builder = builder.data_directory(data_directory.clone());
        }

        let outcome = builder.build().map_err(|error| {
            #[cfg(windows)]
            {
                json!({
                  "operation": "create-webview-window",
                  "message": error.to_string(),
                  "webviewDataDirectory": path_string(&data_directory)
                })
            }
            #[cfg(not(windows))]
            {
                json!({
                  "operation": "create-webview-window",
                  "message": error.to_string()
                })
            }
        });
        let outcome = outcome.map(|_| {
            #[cfg(windows)]
            {
                json!({
                  "label": payload.options.label,
                  "webviewDataDirectory": path_string(&data_directory)
                })
            }
            #[cfg(not(windows))]
            {
                json!({ "label": payload.options.label })
            }
        });

        result(&self.outbox, request.id, outcome);
        Ok(())
    }

    fn build_app(&mut self) -> Result<()> {
        if self.app.is_some() {
            return Ok(());
        }

        let event_plugin = event_forwarder(Arc::clone(&self.event_subscriptions));
        let channel_outbox = Arc::clone(&self.outbox);
        let builder = tauri::Builder::default()
            .plugin(event_plugin)
            .plugin(tauri_plugin_notification::init())
            .plugin(tauri_plugin_opener::init())
            .plugin(tauri_plugin_os::init())
            .plugin(tauri_plugin_positioner::init())
            .plugin(tauri_plugin_store::Builder::default().build());
        let mut context = tauri::generate_context!();
        context.config_mut().identifier = self
            .app_user_model_id
            .clone()
            .unwrap_or_else(|| DEFAULT_IDENTIFIER.into());
        let mut app = self
            .asset_protocol
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
            .build(context)?;

        // Complete Tauri setup without creating a bootstrap window or webview.
        let setup_outbox = Arc::clone(&self.outbox);
        #[allow(deprecated)]
        app.run_iteration(move |_app, event| collect_event(&setup_outbox, event));
        self.app = Some(app);
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
        if self.closed {
            Err(Error::Shutdown)
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
fn resolved_explicit_webview_data_directory(
    local_data: &std::path::Path,
    label: &str,
    relative: &std::path::Path,
) -> std::result::Result<std::path::PathBuf, Value> {
    let path = local_data.join(label).join(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(json!({
          "operation": "resolve-webview-data-directory",
          "message": "dataDirectory must be a safe relative path",
          "webviewDataDirectory": path_string(&path)
        }));
    }
    Ok(path)
}

#[cfg(windows)]
fn set_current_process_app_user_model_id(
    payload: Value,
) -> std::result::Result<(String, Value), Value> {
    let payload: SetAppUserModelIdPayload = serde_json::from_value(payload).map_err(|error| {
        json!({
          "operation": "parse-app-user-model-id",
          "message": error.to_string()
        })
    })?;
    if payload.app_id.contains('\0') {
        return Err(json!({
          "operation": "validate-app-user-model-id",
          "message": "appId must not contain a NUL character"
        }));
    }
    let executable = current_executable_path().map_err(|message| {
        json!({
          "operation": "resolve-executable",
          "message": message
        })
    })?;
    let name = application_name(payload.name.as_deref(), &executable).map_err(|message| {
        json!({
          "operation": "resolve-shortcut-name",
          "message": message,
          "executablePath": path_string(&executable)
        })
    })?;
    let shortcut = install_start_menu_shortcut(&payload.app_id, &name, &executable)?;
    set_current_process_explicit_app_user_model_id(&payload.app_id).map_err(|message| {
        registration_error(
            "set-current-process-app-user-model-id",
            message,
            &executable,
            &shortcut,
        )
    })?;
    let value = json!({
      "appId": payload.app_id,
      "name": name,
      "executablePath": path_string(&executable),
      "shortcutPath": path_string(&shortcut)
    });
    Ok((payload.app_id, value))
}

#[cfg(not(windows))]
fn set_current_process_app_user_model_id(
    _payload: Value,
) -> std::result::Result<(String, Value), Value> {
    Err(json!({
      "operation": "set-app-user-model-id",
      "message": "AppUserModelID is only available on Windows"
    }))
}

#[cfg(windows)]
fn current_executable_path() -> std::result::Result<std::path::PathBuf, String> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;

    let mut buffer = vec![0u16; 260];
    loop {
        let written = unsafe { GetModuleFileNameW(None, &mut buffer) } as usize;
        if written == 0 {
            return Err(windows::core::Error::from_win32().to_string());
        }
        if written < buffer.len() {
            return Ok(std::path::PathBuf::from(OsString::from_wide(
                &buffer[..written],
            )));
        }
        buffer.resize(buffer.len() * 2, 0);
    }
}

#[cfg(windows)]
fn install_start_menu_shortcut(
    app_id: &str,
    name: &str,
    executable: &std::path::Path,
) -> std::result::Result<std::path::PathBuf, Value> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        core::{Interface, PCWSTR},
        Win32::{
            Foundation::RPC_E_CHANGED_MODE,
            Storage::EnhancedStorage::PKEY_AppUserModel_ID,
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
                StructuredStorage::PROPVARIANT, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
            },
            UI::Shell::{
                FOLDERID_Programs, IShellLinkW, PropertiesSystem::IPropertyStore,
                SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT,
            },
        },
    };

    let programs = unsafe {
        SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None).map_err(|error| {
            json!({
              "operation": "resolve-start-menu-directory",
              "message": error.to_string(),
              "executablePath": path_string(executable)
            })
        })?
    };
    let programs_path = unsafe { programs.to_string() };
    unsafe { CoTaskMemFree(Some(programs.0.cast())) };
    let programs_path = programs_path.map_err(|error| {
        json!({
          "operation": "resolve-start-menu-directory",
          "message": error.to_string(),
          "executablePath": path_string(executable)
        })
    })?;
    let shortcut = start_menu_shortcut_path(std::path::Path::new(&programs_path), name);

    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let owns_com = if initialized.is_ok() {
        true
    } else if initialized == RPC_E_CHANGED_MODE {
        false
    } else {
        return Err(registration_error(
            "initialize-com",
            windows::core::Error::from_hresult(initialized).to_string(),
            executable,
            &shortcut,
        ));
    };

    let outcome = (|| -> std::result::Result<(), (&'static str, String)> {
        let executable = executable
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let shortcut = shortcut
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();

        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|error| ("create-shell-link", error.to_string()))?;
            link.SetPath(PCWSTR(executable.as_ptr()))
                .map_err(|error| ("set-shortcut-target", error.to_string()))?;

            let store: IPropertyStore = link
                .cast()
                .map_err(|error| ("open-shortcut-properties", error.to_string()))?;
            let value = PROPVARIANT::from(app_id);
            store
                .SetValue(&PKEY_AppUserModel_ID, &value)
                .map_err(|error| ("set-shortcut-app-user-model-id", error.to_string()))?;
            store
                .Commit()
                .map_err(|error| ("commit-shortcut-properties", error.to_string()))?;

            let persist: IPersistFile = link
                .cast()
                .map_err(|error| ("open-shortcut-persistence", error.to_string()))?;
            persist
                .Save(PCWSTR(shortcut.as_ptr()), true)
                .map_err(|error| ("save-start-menu-shortcut", error.to_string()))?;
        }
        Ok(())
    })();

    if owns_com {
        unsafe { CoUninitialize() };
    }
    outcome
        .map(|_| shortcut.clone())
        .map_err(|(operation, message)| {
            registration_error(operation, message, executable, &shortcut)
        })
}

#[cfg(windows)]
fn set_current_process_explicit_app_user_model_id(app_id: &str) -> std::result::Result<(), String> {
    #[link(name = "shell32")]
    extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(app_id: *const u16) -> i32;
    }

    let app_id = app_id.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let hresult = unsafe { SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr()) };
    if hresult >= 0 {
        Ok(())
    } else {
        Err(format!(
            "SetCurrentProcessExplicitAppUserModelID failed with HRESULT 0x{:08X}",
            hresult as u32
        ))
    }
}

#[cfg(windows)]
fn default_webview_data_directory(
    local_data: &std::path::Path,
) -> std::result::Result<std::path::PathBuf, Value> {
    let executable = current_executable_path().map_err(|message| {
        json!({
          "operation": "resolve-executable",
          "message": message
        })
    })?;
    Ok(webview_data_directory_for_executable(
        local_data,
        &executable,
    ))
}

#[cfg(windows)]
fn webview_data_directory_for_executable(
    local_data: &std::path::Path,
    executable: &std::path::Path,
) -> std::path::PathBuf {
    local_data
        .join(DEFAULT_IDENTIFIER)
        .join(windows_path_hash(executable))
}

#[cfg(windows)]
fn windows_path_hash(path: &std::path::Path) -> String {
    use std::os::windows::ffi::OsStrExt;

    let mut hasher = Sha256::new();
    for unit in path.as_os_str().encode_wide() {
        hasher.update(unit.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(windows)]
fn application_name(
    requested: Option<&str>,
    executable: &std::path::Path,
) -> std::result::Result<String, String> {
    application_name_from_args(requested, executable, std::env::args_os().skip(1))
}

#[cfg(windows)]
fn application_name_from_args<I, S>(
    requested: Option<&str>,
    executable: &std::path::Path,
    arguments: I,
) -> std::result::Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    if let Some(name) = requested {
        return valid_windows_name(name);
    }

    let script_extensions = [
        "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "py", "php",
    ];
    let script = arguments.into_iter().find_map(|argument| {
        let path = std::path::Path::new(argument.as_ref());
        let extension = path.extension()?.to_str()?;
        script_extensions
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            .then(|| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .flatten()
    });
    let inferred = script.or_else(|| {
        executable
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
    });
    valid_windows_name(
        inferred
            .as_deref()
            .ok_or_else(|| "cannot infer a name from the host script or executable".to_owned())?,
    )
}

#[cfg(windows)]
fn start_menu_shortcut_path(programs: &std::path::Path, name: &str) -> std::path::PathBuf {
    programs.join(format!("{name}.lnk"))
}

#[cfg(windows)]
fn valid_windows_name(name: &str) -> std::result::Result<String, String> {
    let name = name.trim();
    let invalid = name.is_empty()
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character));
    let stem = name.split('.').next().unwrap_or_default();
    let reserved = matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if invalid || reserved {
        Err("name must be a valid single Windows file name".into())
    } else {
        Ok(name.to_owned())
    }
}

fn path_string(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(windows)]
fn registration_error(
    operation: &'static str,
    message: String,
    executable: &std::path::Path,
    shortcut: &std::path::Path,
) -> Value {
    json!({
      "operation": operation,
      "message": message,
      "executablePath": path_string(executable),
      "shortcutPath": path_string(shortcut)
    })
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
    fn forwarded_tauri_commands_require_webview_creation_without_a_context() {
        let rejected: Request =
            serde_json::from_str(r#"{"id":1,"cmd":"plugin:app|name","payload":{}}"#).unwrap();
        assert_ne!(rejected.cmd, CREATE_WEBVIEW_WINDOW_COMMAND);

        let accepted: Request = serde_json::from_str(
            r#"{"id":1,"cmd":"plugin:webview|create_webview_window","payload":{"options":{"label":"main"}}}"#,
        )
        .unwrap();
        assert_eq!(accepted.cmd, CREATE_WEBVIEW_WINDOW_COMMAND);
    }

    #[cfg(windows)]
    #[test]
    fn explicit_webview_data_directory_is_resolved_and_confined() {
        let local_data = std::path::Path::new(r"C:\Users\test\AppData\Local");
        assert_eq!(
            resolved_explicit_webview_data_directory(
                local_data,
                "main",
                std::path::Path::new(r"profiles\webview"),
            )
            .unwrap(),
            local_data.join("main").join(r"profiles\webview")
        );
        assert!(resolved_explicit_webview_data_directory(
            local_data,
            "main",
            std::path::Path::new(r"..\escape"),
        )
        .is_err());
        assert!(resolved_explicit_webview_data_directory(
            local_data,
            "main",
            std::path::Path::new(r"C:\absolute"),
        )
        .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn default_webview_data_directory_is_stable_per_executable_path() {
        let local_data = std::path::Path::new(r"C:\Users\test\AppData\Local");
        let executable = std::path::Path::new(r"C:\Apps\Example\app.exe");
        let first = webview_data_directory_for_executable(local_data, executable);
        let second = webview_data_directory_for_executable(local_data, executable);
        let moved = webview_data_directory_for_executable(
            local_data,
            std::path::Path::new(r"C:\Other\app.exe"),
        );

        assert_eq!(first, second);
        assert_eq!(first.parent().unwrap(), local_data.join(DEFAULT_IDENTIFIER));
        assert_ne!(first, moved);
        assert_eq!(first.file_name().unwrap().to_string_lossy().len(), 64);
    }

    #[test]
    fn app_user_model_id_payload_accepts_both_id_spellings() {
        let camel: SetAppUserModelIdPayload =
            serde_json::from_str(r#"{"appId":"com.example.app","name":"Example App"}"#).unwrap();
        let win32: SetAppUserModelIdPayload =
            serde_json::from_str(r#"{"appID":"com.example.app"}"#).unwrap();

        assert_eq!(camel.app_id, "com.example.app");
        assert_eq!(camel.name.as_deref(), Some("Example App"));
        assert_eq!(win32.app_id, "com.example.app");
        assert!(win32.name.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn shortcut_is_directly_under_programs_and_uses_only_its_name() {
        let programs = std::path::Path::new(
            r"C:\Users\test\AppData\Roaming\Microsoft\Windows\Start Menu\Programs",
        );
        let shortcut = start_menu_shortcut_path(programs, "Example App");

        assert_eq!(shortcut, programs.join("Example App.lnk"));
        assert_eq!(shortcut.parent(), Some(programs));
    }

    #[cfg(windows)]
    #[test]
    fn shortcut_name_prefers_argument_then_script_then_executable() {
        let executable = std::path::Path::new(r"C:\Tools\deno.exe");
        assert_eq!(
            application_name_from_args(
                Some("Passed Name"),
                executable,
                [std::ffi::OsString::from(r"C:\apps\ignored.js")],
            )
            .unwrap(),
            "Passed Name"
        );
        assert_eq!(
            application_name_from_args(
                None,
                executable,
                [
                    std::ffi::OsString::from("run"),
                    std::ffi::OsString::from(r"C:\apps\demo.js")
                ],
            )
            .unwrap(),
            "demo"
        );
        assert_eq!(
            application_name_from_args(None, executable, std::iter::empty::<std::ffi::OsString>())
                .unwrap(),
            "deno"
        );
    }

    #[cfg(windows)]
    #[test]
    fn path_errors_are_structured_for_hosts() {
        let error = registration_error(
            "save-start-menu-shortcut",
            "access denied".into(),
            std::path::Path::new(r"C:\Tools\deno.exe"),
            std::path::Path::new(r"C:\Programs\Demo.lnk"),
        );
        assert_eq!(error["operation"], "save-start-menu-shortcut");
        assert_eq!(error["message"], "access denied");
        assert_eq!(error["shortcutPath"], r"C:\Programs\Demo.lnk");
    }

    #[test]
    fn bridge_can_drain_before_tauri_app_is_built() {
        let mut runtime = Tauriless::new().unwrap();
        assert!(runtime.app.is_none());
        assert_eq!(
            serde_json::from_slice::<Value>(&runtime.drain().unwrap()).unwrap(),
            json!({ "messages": [] })
        );
    }

    #[test]
    fn default_identifier_is_tauriless() {
        assert_eq!(DEFAULT_IDENTIFIER, "Tauriless");
    }

    #[test]
    fn tauri_context_identifier_is_mutable_before_build() {
        let mut context: tauri::Context<tauri::Wry> = tauri::generate_context!();
        context.config_mut().identifier = "com.example.lazy".into();
        assert_eq!(context.config().identifier, "com.example.lazy");
    }
}
