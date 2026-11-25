// OWNER: scaffold
//! In-process event bus. Modules emit [`AppEvent`]s; the Tauri layer forwards every event on the
//! `"warden://event"` channel to the UI.

use tokio::sync::broadcast;

use crate::types::AppEvent;

const DEFAULT_CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<AppEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Fire-and-forget. Having no subscribers is not an error.
    pub fn emit(&self, event: AppEvent) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn emit_and_receive() {
        let bus = EventBus::new();
        bus.emit(AppEvent::Notice {
            level: "info".into(),
            message: "dropped, no subscriber".into(),
        });
        let mut rx = bus.subscribe();
        bus.emit(AppEvent::MrsUpdated { project_id: 7 });
        match rx.recv().await.unwrap() {
            AppEvent::MrsUpdated { project_id } => assert_eq!(project_id, 7),
            other => panic!("unexpected {other:?}"),
        }
    }
}
