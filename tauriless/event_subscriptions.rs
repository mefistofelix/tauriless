use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex, Weak},
};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{Event, EventId, Listener, Webview, WebviewWindow, Window, Wry};

use crate::{push, Outbox};

pub const SUBSCRIBE_COMMAND: &str = "tauriless:subscribe";
pub const UNSUBSCRIBE_COMMAND: &str = "tauriless:unsubscribe";

pub type SharedEventSubscriptions = Arc<Mutex<EventSubscriptions>>;

#[derive(Debug, Deserialize)]
struct EventPayload {
    event: String,
}

enum Target {
    Window(Window<Wry>),
    WebviewWindow(WebviewWindow<Wry>),
    Webview(Webview<Wry>),
}

impl Target {
    fn listen(&self, event: &str, outbox: &Outbox, source: &'static str, label: &str) -> EventId {
        match self {
            Self::Window(target) => {
                target.listen(event, forwarder(Arc::clone(outbox), source, label, event))
            }
            Self::WebviewWindow(target) => {
                target.listen(event, forwarder(Arc::clone(outbox), source, label, event))
            }
            Self::Webview(target) => {
                target.listen(event, forwarder(Arc::clone(outbox), source, label, event))
            }
        }
    }

    fn listen_for_destruction(
        &self,
        subscriptions: Weak<Mutex<EventSubscriptions>>,
        key: String,
    ) -> EventId {
        let cleanup = move |_event: Event| {
            if let Some(subscriptions) = subscriptions.upgrade() {
                subscriptions
                    .lock()
                    .expect("event subscriptions mutex poisoned")
                    .remove_target(&key);
            }
        };
        match self {
            Self::Window(target) => target.listen("tauri://destroyed", cleanup),
            Self::WebviewWindow(target) => target.listen("tauri://destroyed", cleanup),
            Self::Webview(target) => target.listen("tauri://destroyed", cleanup),
        }
    }

    fn unlisten(&self, id: EventId) {
        match self {
            Self::Window(target) => target.unlisten(id),
            Self::WebviewWindow(target) => target.unlisten(id),
            Self::Webview(target) => target.unlisten(id),
        }
    }
}

struct TargetSubscriptions {
    target: Target,
    label: String,
    source: &'static str,
    housekeeping: EventId,
    listeners: HashMap<String, EventId>,
}

pub struct EventSubscriptions {
    active: BTreeSet<String>,
    targets: HashMap<String, TargetSubscriptions>,
    outbox: Outbox,
}

impl EventSubscriptions {
    pub fn new(
        outbox: Outbox,
        default_events: &'static [&'static str],
    ) -> SharedEventSubscriptions {
        Arc::new(Mutex::new(Self {
            active: default_events
                .iter()
                .map(|event| (*event).to_owned())
                .collect(),
            targets: HashMap::new(),
            outbox,
        }))
    }

    pub fn handles(command: &str) -> bool {
        matches!(command, SUBSCRIBE_COMMAND | UNSUBSCRIBE_COMMAND)
    }

    pub fn handle(&mut self, command: &str, payload: Value) -> Result<Value, String> {
        let EventPayload { event } =
            serde_json::from_value(payload).map_err(|error| error.to_string())?;
        validate_event_name(&event)?;

        match command {
            SUBSCRIBE_COMMAND => Ok(self.subscribe(event)),
            UNSUBSCRIBE_COMMAND => Ok(self.unsubscribe(event)),
            _ => Err(format!("unknown event subscription command `{command}`")),
        }
    }

    pub fn clear_targets(&mut self) {
        let keys = self.targets.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            self.remove_target(&key);
        }
    }

    pub fn register_window(subscriptions: &SharedEventSubscriptions, window: Window<Wry>) {
        let label = window.label().to_owned();
        Self::replace_target(subscriptions, label, "window", Target::Window(window));
    }

    pub fn register_webview_window(
        subscriptions: &SharedEventSubscriptions,
        window: WebviewWindow<Wry>,
    ) {
        let label = window.label().to_owned();
        Self::replace_target(
            subscriptions,
            label,
            "webview-window",
            Target::WebviewWindow(window),
        );
    }

    pub fn register_webview(subscriptions: &SharedEventSubscriptions, webview: Webview<Wry>) {
        let label = webview.label().to_owned();
        Self::replace_target(subscriptions, label, "webview", Target::Webview(webview));
    }

    fn replace_target(
        subscriptions: &SharedEventSubscriptions,
        key: String,
        source: &'static str,
        target: Target,
    ) {
        let mut state = subscriptions
            .lock()
            .expect("event subscriptions mutex poisoned");
        state.remove_target(&key);

        let label = key.clone();
        let listeners = state
            .active
            .iter()
            .map(|event| {
                (
                    event.clone(),
                    target.listen(event, &state.outbox, source, &label),
                )
            })
            .collect();
        let housekeeping =
            target.listen_for_destruction(Arc::downgrade(subscriptions), key.clone());
        state.targets.insert(
            key,
            TargetSubscriptions {
                target,
                label,
                source,
                housekeeping,
                listeners,
            },
        );
    }

    fn subscribe(&mut self, event: String) -> Value {
        let changed = self.active.insert(event.clone());
        if changed {
            for target in self.targets.values_mut() {
                let id = target
                    .target
                    .listen(&event, &self.outbox, target.source, &target.label);
                target.listeners.insert(event.clone(), id);
            }
        }

        json!({ "event": event, "subscribed": true, "changed": changed })
    }

    fn unsubscribe(&mut self, event: String) -> Value {
        let removed = self.active.remove(&event);
        if removed {
            for target in self.targets.values_mut() {
                if let Some(id) = target.listeners.remove(&event) {
                    target.target.unlisten(id);
                }
            }
        }

        json!({ "event": event, "subscribed": false, "removed": removed })
    }

    fn remove_target(&mut self, key: &str) {
        if let Some(target) = self.targets.remove(key) {
            target.target.unlisten(target.housekeeping);
            for id in target.listeners.into_values() {
                target.target.unlisten(id);
            }
        }
    }
}

fn forwarder(
    outbox: Outbox,
    source: &'static str,
    label: &str,
    event: &str,
) -> impl Fn(Event) + Send + 'static {
    let label = label.to_owned();
    let event = event.to_owned();
    move |incoming| {
        let payload = serde_json::from_str(incoming.payload())
            .unwrap_or_else(|_| Value::String(incoming.payload().to_owned()));
        push(
            &outbox,
            json!({
              "kind": "event",
              "source": source,
              "window": label,
              "event": event,
              "payload": payload
            }),
        );
    }
}

fn validate_event_name(event: &str) -> Result<(), String> {
    if !event.is_empty()
        && event
            .chars()
            .all(|character| character.is_alphanumeric() || "-/:_".contains(character))
    {
        Ok(())
    } else {
        Err(
            "event must be non-empty and contain only alphanumeric characters, `-`, `/`, `:` and `_`"
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_names_like_tauri_without_panicking() {
        assert!(validate_event_name("plugin://ready_2").is_ok());
        assert!(validate_event_name("").is_err());
        assert!(validate_event_name("spaces are invalid").is_err());
        assert!(validate_event_name("wildcard://*").is_err());
    }

    #[test]
    fn recognizes_only_bridge_subscription_commands() {
        assert!(EventSubscriptions::handles(SUBSCRIBE_COMMAND));
        assert!(EventSubscriptions::handles(UNSUBSCRIBE_COMMAND));
        assert!(!EventSubscriptions::handles("plugin:event|listen"));
    }

    #[test]
    fn an_initial_default_can_be_removed_and_restored() {
        let subscriptions = EventSubscriptions::new(Outbox::default(), &["tauri://focus"]);
        let mut subscriptions = subscriptions.lock().unwrap();

        let removed = subscriptions
            .handle(UNSUBSCRIBE_COMMAND, json!({ "event": "tauri://focus" }))
            .unwrap();
        assert_eq!(removed["removed"], true);
        assert!(!subscriptions.active.contains("tauri://focus"));

        let restored = subscriptions
            .handle(SUBSCRIBE_COMMAND, json!({ "event": "tauri://focus" }))
            .unwrap();
        assert_eq!(restored["changed"], true);
        assert!(subscriptions.active.contains("tauri://focus"));
    }
}
