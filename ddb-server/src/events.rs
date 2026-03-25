use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

/// Kind of mutation that triggered the event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    Created,
    Updated,
    Deleted,
}

/// Event emitted after a successful doogat mutation.
#[derive(Clone, Debug)]
pub struct DoogatEvent {
    pub kind: EventKind,
    pub doogat_id: String,
    pub doogat_type: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Broadcast bus for doogat mutation events.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<DoogatEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DoogatEvent> {
        self.tx.subscribe()
    }

    /// Fire-and-forget: silently drops if no subscribers.
    pub fn send(&self, event: DoogatEvent) {
        let _ = self.tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribe_and_receive() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let event = DoogatEvent {
            kind: EventKind::Created,
            doogat_id: "20260302120000".into(),
            doogat_type: Some("contact".into()),
            timestamp: Utc::now(),
        };
        bus.send(event.clone());

        let received = rx.recv().await.unwrap();
        assert_eq!(received.doogat_id, "20260302120000");
        assert_eq!(received.kind, EventKind::Created);
        assert_eq!(received.doogat_type, Some("contact".into()));
    }

    #[tokio::test]
    async fn send_without_subscribers_does_not_error() {
        let bus = EventBus::new();
        bus.send(DoogatEvent {
            kind: EventKind::Deleted,
            doogat_id: "20260302120000".into(),
            doogat_type: None,
            timestamp: Utc::now(),
        });
    }
}
