use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{
    http::{header, Response, StatusCode},
    UriSchemeResponder,
};

pub const RESPONSE_COMMAND: &str = "tauriless:asset-response";

type Outbox = Arc<Mutex<Vec<Value>>>;
type PendingRequests = Arc<Mutex<HashMap<u64, PendingRequest>>>;

struct PendingRequest {
    responder: UriSchemeResponder,
    url_path: String,
}

pub struct AssetProtocol {
    outbox: Outbox,
    pending: PendingRequests,
    next_id: Arc<AtomicU64>,
}

impl AssetProtocol {
    pub fn new(outbox: Outbox) -> Self {
        Self {
            outbox,
            pending: PendingRequests::default(),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn register(&self, builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
        let pending = Arc::clone(&self.pending);
        let outbox = Arc::clone(&self.outbox);
        let next_id = Arc::clone(&self.next_id);

        // Override only Tauri's compile-time asset resolver. The standard
        // tauri://localhost origin and injected IPC remain unchanged.
        builder.register_asynchronous_uri_scheme_protocol(
            "tauri",
            move |context, request, responder| {
                let request_id = next_id.fetch_add(1, Ordering::Relaxed);
                let url = request.uri().to_string();
                let url_path = request.uri().path().to_owned();
                let headers = request
                    .headers()
                    .iter()
                    .filter_map(|(name, value)| {
                        value.to_str().ok().map(|value| (name.as_str(), value))
                    })
                    .collect::<HashMap<_, _>>();

                pending
                    .lock()
                    .expect("asset request mutex poisoned")
                    .insert(
                        request_id,
                        PendingRequest {
                            responder,
                            url_path,
                        },
                    );
                outbox.lock().expect("outbox mutex poisoned").push(json!({
                  "kind": "asset-request",
                  "requestId": request_id,
                  "webview": context.webview_label(),
                  "method": request.method().as_str(),
                  "url": url,
                  "headers": headers
                }));
            },
        )
    }

    pub fn respond(&self, payload: Value) -> Result<(), String> {
        let payload: ResponsePayload =
            serde_json::from_value(payload).map_err(|error| error.to_string())?;
        let pending = self
            .pending
            .lock()
            .expect("asset request mutex poisoned")
            .remove(&payload.request_id)
            .ok_or_else(|| {
                format!(
                    "unknown or already answered asset request {}",
                    payload.request_id
                )
            })?;

        match build_response(payload, &pending.url_path) {
            Ok(response) => {
                pending.responder.respond(response);
                Ok(())
            }
            Err(error) => {
                pending.responder.respond(error_response(&error));
                Err(error)
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResponsePayload {
    request_id: u64,
    #[serde(default = "default_status")]
    status: u16,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default)]
    content: Option<String>,
}

fn default_status() -> u16 {
    StatusCode::OK.as_u16()
}

fn build_response(payload: ResponsePayload, url_path: &str) -> Result<Response<Vec<u8>>, String> {
    let status = StatusCode::from_u16(payload.status).map_err(|error| error.to_string())?;
    let mime = if payload
        .headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case(header::CONTENT_TYPE.as_str()))
    {
        None
    } else {
        Some(
            payload
                .mime
                .clone()
                .unwrap_or_else(|| guessed_mime(payload.path.as_ref(), url_path)),
        )
    };
    let body = if let Some(content) = payload.content {
        content.into_bytes()
    } else if let Some(path) = payload.path {
        fs::read(&path).map_err(|error| format!("failed to read `{}`: {error}", path.display()))?
    } else {
        return Err("asset response requires either `path` or `content`".into());
    };

    let mut response = Response::builder().status(status);
    for (name, value) in payload.headers {
        response = response.header(name, value);
    }
    if let Some(mime) = mime {
        response = response.header(header::CONTENT_TYPE, mime);
    }
    response.body(body).map_err(|error| error.to_string())
}

fn guessed_mime(path: Option<&PathBuf>, url_path: &str) -> String {
    path.and_then(|path| mime_guess::from_path(path).first())
        .or_else(|| mime_guess::from_path(url_path).first())
        .unwrap_or(mime_guess::mime::APPLICATION_OCTET_STREAM)
        .essence_str()
        .to_owned()
}

fn error_response(error: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(error.as_bytes().to_vec())
        .expect("the fixed error response is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_content_infers_mime_from_requested_url() {
        let response = build_response(
            ResponsePayload {
                request_id: 1,
                status: 200,
                headers: HashMap::new(),
                mime: None,
                path: None,
                content: Some("body {}".into()),
            },
            "/styles/app.css",
        )
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/css");
        assert_eq!(response.body(), b"body {}");
    }

    #[test]
    fn local_path_is_read_as_bytes() {
        let path =
            std::env::temp_dir().join(format!("tauriless-asset-test-{}.svg", std::process::id()));
        fs::write(&path, b"<svg/>").unwrap();
        let response = build_response(
            ResponsePayload {
                request_id: 1,
                status: 200,
                headers: HashMap::new(),
                mime: None,
                path: Some(path.clone()),
                content: None,
            },
            "/without-an-extension",
        );
        fs::remove_file(path).unwrap();
        let response = response.unwrap();

        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert_eq!(response.body(), b"<svg/>");
    }

    #[test]
    fn response_requires_a_body_source() {
        let error = build_response(
            ResponsePayload {
                request_id: 1,
                status: 200,
                headers: HashMap::new(),
                mime: None,
                path: None,
                content: None,
            },
            "/index.html",
        )
        .unwrap_err();

        assert!(error.contains("either `path` or `content`"));
    }
}
