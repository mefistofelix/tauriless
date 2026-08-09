// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Internal real-or-headless webview dispatcher used by custom invoke systems.

use std::sync::{Arc, Mutex};

use tauri_runtime::{
  dpi::{PhysicalPosition, PhysicalSize, Position, Rect, Size},
  webview::DetachedWebview,
  window::{WebviewEvent, WindowId},
  Cookie, WebviewDispatch, WebviewEventId, WindowDispatch,
};
use tauri_utils::config::Color;
use url::Url;

use crate::{EventLoopMessage, Runtime, Webview, Window};

const HEADLESS_URL: &str = "tauriless://headless";

pub(crate) struct ManagedWebview<R: Runtime> {
  pub(crate) label: String,
  pub(crate) dispatcher: ManagedWebviewDispatcher<R>,
}

impl<R: Runtime> std::fmt::Debug for ManagedWebview<R> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ManagedWebview")
      .field("label", &self.label)
      .field("dispatcher", &self.dispatcher)
      .finish()
  }
}

impl<R: Runtime> Clone for ManagedWebview<R> {
  fn clone(&self) -> Self {
    Self {
      label: self.label.clone(),
      dispatcher: self.dispatcher.clone(),
    }
  }
}

impl<R: Runtime> ManagedWebview<R> {
  pub(crate) fn headless(label: String, dispatcher: R::WindowDispatcher) -> Self {
    Self {
      label,
      dispatcher: ManagedWebviewDispatcher::Headless(dispatcher),
    }
  }
}

impl<R: Runtime> Webview<R> {
  /// Creates a logical webview context backed by a native window but without
  /// constructing a platform webview.
  ///
  /// This unstable hook is intended for custom invoke transports. Webview
  /// operations are no-ops, while application state, resources and channels
  /// keep using Tauri's standard implementations.
  #[doc(hidden)]
  #[cfg(feature = "unstable")]
  pub fn new_headless(window: Window<R>, label: impl Into<String>) -> Self {
    let dispatcher = window.window.dispatcher.clone();
    Self {
      manager: window.manager.clone(),
      app_handle: window.app_handle.clone(),
      window: Arc::new(Mutex::new(window)),
      webview: ManagedWebview::headless(label.into(), dispatcher),
      resources_table: Default::default(),
      use_https_scheme: false,
    }
  }
}

impl<R: Runtime> From<DetachedWebview<EventLoopMessage, R>> for ManagedWebview<R> {
  fn from(webview: DetachedWebview<EventLoopMessage, R>) -> Self {
    Self {
      label: webview.label,
      dispatcher: ManagedWebviewDispatcher::Real(webview.dispatcher),
    }
  }
}

pub(crate) enum ManagedWebviewDispatcher<R: Runtime> {
  Real(R::WebviewDispatcher),
  Headless(R::WindowDispatcher),
}

impl<R: Runtime> std::fmt::Debug for ManagedWebviewDispatcher<R> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::Real(dispatcher) => f.debug_tuple("Real").field(dispatcher).finish(),
      Self::Headless(_) => f.write_str("Headless"),
    }
  }
}

impl<R: Runtime> Clone for ManagedWebviewDispatcher<R> {
  fn clone(&self) -> Self {
    match self {
      Self::Real(dispatcher) => Self::Real(dispatcher.clone()),
      Self::Headless(dispatcher) => Self::Headless(dispatcher.clone()),
    }
  }
}

impl<R: Runtime> WebviewDispatch<EventLoopMessage> for ManagedWebviewDispatcher<R> {
  type Runtime = R;

  fn run_on_main_thread<F: FnOnce() + Send + 'static>(
    &self,
    task: F,
  ) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.run_on_main_thread(task),
      Self::Headless(dispatcher) => dispatcher.run_on_main_thread(task),
    }
  }

  fn on_webview_event<F: Fn(&WebviewEvent) + Send + 'static>(
    &self,
    handler: F,
  ) -> WebviewEventId {
    match self {
      Self::Real(dispatcher) => dispatcher.on_webview_event(handler),
      Self::Headless(_) => 0,
    }
  }

  fn with_webview<F: FnOnce(Box<dyn std::any::Any>) + Send + 'static>(
    &self,
    handler: F,
  ) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.with_webview(handler),
      Self::Headless(_) => Ok(()),
    }
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  fn open_devtools(&self) {
    if let Self::Real(dispatcher) = self {
      dispatcher.open_devtools();
    }
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  fn close_devtools(&self) {
    if let Self::Real(dispatcher) = self {
      dispatcher.close_devtools();
    }
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  fn is_devtools_open(&self) -> tauri_runtime::Result<bool> {
    match self {
      Self::Real(dispatcher) => dispatcher.is_devtools_open(),
      Self::Headless(_) => Ok(false),
    }
  }

  fn url(&self) -> tauri_runtime::Result<String> {
    match self {
      Self::Real(dispatcher) => dispatcher.url(),
      Self::Headless(_) => Ok(HEADLESS_URL.into()),
    }
  }

  fn bounds(&self) -> tauri_runtime::Result<Rect> {
    match self {
      Self::Real(dispatcher) => dispatcher.bounds(),
      Self::Headless(_) => Ok(Rect::default()),
    }
  }

  fn position(&self) -> tauri_runtime::Result<PhysicalPosition<i32>> {
    match self {
      Self::Real(dispatcher) => dispatcher.position(),
      Self::Headless(_) => Ok((0, 0).into()),
    }
  }

  fn size(&self) -> tauri_runtime::Result<PhysicalSize<u32>> {
    match self {
      Self::Real(dispatcher) => dispatcher.size(),
      Self::Headless(_) => Ok((0, 0).into()),
    }
  }

  fn navigate(&self, url: Url) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.navigate(url),
      Self::Headless(_) => Ok(()),
    }
  }

  fn reload(&self) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.reload(),
      Self::Headless(_) => Ok(()),
    }
  }

  fn print(&self) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.print(),
      Self::Headless(_) => Ok(()),
    }
  }

  fn close(&self) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.close(),
      Self::Headless(_) => Ok(()),
    }
  }

  fn set_bounds(&self, bounds: Rect) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.set_bounds(bounds),
      Self::Headless(_) => Ok(()),
    }
  }

  fn set_size(&self, size: Size) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.set_size(size),
      Self::Headless(_) => Ok(()),
    }
  }

  fn set_position(&self, position: Position) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.set_position(position),
      Self::Headless(_) => Ok(()),
    }
  }

  fn set_focus(&self) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.set_focus(),
      Self::Headless(_) => Ok(()),
    }
  }

  fn hide(&self) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.hide(),
      Self::Headless(_) => Ok(()),
    }
  }

  fn show(&self) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.show(),
      Self::Headless(_) => Ok(()),
    }
  }

  fn eval_script<S: Into<String>>(&self, script: S) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.eval_script(script),
      Self::Headless(_) => Ok(()),
    }
  }

  fn eval_script_with_callback<S: Into<String>>(
    &self,
    script: S,
    callback: impl Fn(String) + Send + 'static,
  ) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.eval_script_with_callback(script, callback),
      Self::Headless(_) => {
        callback("null".into());
        Ok(())
      }
    }
  }

  fn reparent(&self, window_id: WindowId) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.reparent(window_id),
      Self::Headless(_) => Ok(()),
    }
  }

  fn cookies_for_url(&self, url: Url) -> tauri_runtime::Result<Vec<Cookie<'static>>> {
    match self {
      Self::Real(dispatcher) => dispatcher.cookies_for_url(url),
      Self::Headless(_) => Ok(Vec::new()),
    }
  }

  fn cookies(&self) -> tauri_runtime::Result<Vec<Cookie<'static>>> {
    match self {
      Self::Real(dispatcher) => dispatcher.cookies(),
      Self::Headless(_) => Ok(Vec::new()),
    }
  }

  fn set_cookie(&self, cookie: cookie::Cookie<'_>) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.set_cookie(cookie),
      Self::Headless(_) => Ok(()),
    }
  }

  fn delete_cookie(&self, cookie: cookie::Cookie<'_>) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.delete_cookie(cookie),
      Self::Headless(_) => Ok(()),
    }
  }

  fn set_auto_resize(&self, auto_resize: bool) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.set_auto_resize(auto_resize),
      Self::Headless(_) => Ok(()),
    }
  }

  fn set_zoom(&self, scale_factor: f64) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.set_zoom(scale_factor),
      Self::Headless(_) => Ok(()),
    }
  }

  fn set_background_color(&self, color: Option<Color>) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.set_background_color(color),
      Self::Headless(_) => Ok(()),
    }
  }

  fn clear_all_browsing_data(&self) -> tauri_runtime::Result<()> {
    match self {
      Self::Real(dispatcher) => dispatcher.clear_all_browsing_data(),
      Self::Headless(_) => Ok(()),
    }
  }
}
