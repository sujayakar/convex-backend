//! Structured event recording for deterministic simulation testing.
//!
//! `EventRecorder` follows the same pattern as `PauseClient`:
//! - In production: a zero-cost no-op
//! - In tests with the `testing` feature: records events to a shared log
//!
//! Access via `Runtime::event_recorder()`.

use std::time::Duration;

use serde::{
    Deserialize,
    Serialize,
};

/// A structured event recorded during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    /// Monotonically increasing sequence number.
    pub seq: u64,
    /// Simulated time elapsed since the start of the run.
    pub elapsed: Duration,
    /// The event payload.
    pub event: Event,
}

/// Event types that can be recorded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Event {
    /// A database transaction began.
    TransactionBegin {
        /// Human-readable identity description.
        identity: String,
    },
    /// A transaction was committed.
    TransactionCommit,
    /// A transaction failed due to OCC conflict.
    TransactionConflict,
    /// A UDF (mutation/query/action) started executing.
    UdfStart {
        udf_path: String,
        udf_type: String,
    },
    /// A UDF finished executing.
    UdfEnd {
        udf_path: String,
        success: bool,
    },
    /// A pause point was hit.
    PausePointHit {
        label: String,
    },
    /// Custom event from scenario code.
    Custom {
        label: String,
        data: serde_json::Value,
    },
}

#[cfg(any(test, feature = "testing"))]
mod test_recorder {
    use std::{
        sync::{
            atomic::{
                AtomicU64,
                Ordering,
            },
            Arc,
        },
    };

    use parking_lot::Mutex;

    use super::{
        Event,
        TraceEvent,
    };

    /// A recorder that captures events to a shared log.
    #[derive(Clone)]
    pub struct EventRecorder {
        inner: Option<Arc<EventRecorderInner>>,
    }

    struct EventRecorderInner {
        events: Mutex<Vec<TraceEvent>>,
        next_seq: AtomicU64,
        start: std::time::Instant,
    }

    impl EventRecorder {
        /// Create an inactive recorder (no-op).
        pub fn new() -> Self {
            Self { inner: None }
        }

        /// Create an active recorder that collects events.
        pub fn active() -> Self {
            Self {
                inner: Some(Arc::new(EventRecorderInner {
                    events: Mutex::new(Vec::new()),
                    next_seq: AtomicU64::new(0),
                    start: std::time::Instant::now(),
                })),
            }
        }

        /// Record an event. No-op if the recorder is inactive.
        pub fn record(&self, event: Event) {
            if let Some(inner) = &self.inner {
                let seq = inner.next_seq.fetch_add(1, Ordering::Relaxed);
                let elapsed = inner.start.elapsed();
                inner.events.lock().push(TraceEvent {
                    seq,
                    elapsed,
                    event,
                });
            }
        }

        /// Record a custom event with a label and JSON data.
        pub fn record_custom(&self, label: impl Into<String>, data: serde_json::Value) {
            self.record(Event::Custom {
                label: label.into(),
                data,
            });
        }

        /// Check if this recorder is actively collecting events.
        pub fn is_active(&self) -> bool {
            self.inner.is_some()
        }

        /// Drain all recorded events.
        pub fn drain(&self) -> Vec<TraceEvent> {
            match &self.inner {
                Some(inner) => {
                    let mut events = inner.events.lock();
                    std::mem::take(&mut *events)
                },
                None => vec![],
            }
        }

        /// Get a snapshot of all recorded events (without draining).
        pub fn snapshot(&self) -> Vec<TraceEvent> {
            match &self.inner {
                Some(inner) => inner.events.lock().clone(),
                None => vec![],
            }
        }

        /// Number of events recorded so far.
        pub fn len(&self) -> usize {
            match &self.inner {
                Some(inner) => inner.events.lock().len(),
                None => 0,
            }
        }

        /// Whether no events have been recorded.
        pub fn is_empty(&self) -> bool {
            match &self.inner {
                Some(inner) => inner.events.lock().is_empty(),
                None => true,
            }
        }
    }

    impl Default for EventRecorder {
        fn default() -> Self {
            Self::new()
        }
    }
}
#[cfg(any(test, feature = "testing"))]
pub use self::test_recorder::EventRecorder;

#[cfg(not(any(test, feature = "testing")))]
mod prod_recorder {
    use super::{
        Event,
        TraceEvent,
    };

    /// A no-op recorder for production.
    #[derive(Default, Clone)]
    pub struct EventRecorder;

    impl EventRecorder {
        pub fn new() -> Self {
            Self
        }

        pub fn record(&self, _event: Event) {}

        pub fn record_custom(&self, _label: impl Into<String>, _data: serde_json::Value) {}

        pub fn is_active(&self) -> bool {
            false
        }

        pub fn drain(&self) -> Vec<TraceEvent> {
            vec![]
        }

        pub fn snapshot(&self) -> Vec<TraceEvent> {
            vec![]
        }

        pub fn len(&self) -> usize {
            0
        }

        pub fn is_empty(&self) -> bool {
            true
        }
    }
}
#[cfg(not(any(test, feature = "testing")))]
pub use self::prod_recorder::EventRecorder;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        Event,
        EventRecorder,
    };

    #[test]
    fn inactive_recorder_is_noop() {
        let recorder = EventRecorder::new();
        assert!(!recorder.is_active());
        assert_eq!(recorder.len(), 0);
        assert!(recorder.is_empty());

        recorder.record(Event::TransactionCommit);
        recorder.record_custom("label", json!({ "value": 1 }));

        assert!(recorder.snapshot().is_empty());
        assert!(recorder.drain().is_empty());
        assert_eq!(recorder.len(), 0);
        assert!(recorder.is_empty());
    }

    #[test]
    fn default_recorder_is_inactive_and_empty() {
        let recorder = EventRecorder::default();
        assert!(!recorder.is_active());
        assert_eq!(recorder.len(), 0);
        assert!(recorder.is_empty());
    }

    #[test]
    fn snapshot_does_not_drain_events() {
        let recorder = EventRecorder::active();
        recorder.record(Event::TransactionCommit);
        recorder.record(Event::TransactionConflict);

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(recorder.len(), 2);
        assert!(!recorder.is_empty());

        let drained = recorder.drain();
        assert_eq!(drained.len(), 2);
        assert!(recorder.is_empty());
    }

    #[test]
    fn active_recorder_tracks_len_snapshot_and_drain() {
        let recorder = EventRecorder::active();
        assert!(recorder.is_active());
        assert!(recorder.is_empty());

        recorder.record(Event::TransactionBegin {
            identity: "id1".to_string(),
        });
        recorder.record_custom("label", json!({ "value": 1 }));

        assert_eq!(recorder.len(), 2);
        assert!(!recorder.is_empty());

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].seq, 0);
        assert_eq!(snapshot[1].seq, 1);
        assert!(snapshot[0].elapsed <= snapshot[1].elapsed);
        assert!(matches!(
            snapshot[0].event,
            Event::TransactionBegin { ref identity } if identity == "id1"
        ));
        assert!(matches!(
            snapshot[1].event,
            Event::Custom { ref label, ref data } if label == "label" && *data == json!({ "value": 1 })
        ));

        let drained = recorder.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].seq, 0);
        assert_eq!(drained[1].seq, 1);
        assert_eq!(recorder.len(), 0);
        assert!(recorder.is_empty());

        recorder.record(Event::TransactionCommit);
        let post_drain_snapshot = recorder.snapshot();
        assert_eq!(post_drain_snapshot.len(), 1);
        assert_eq!(post_drain_snapshot[0].seq, 2);
    }

    #[test]
    fn cloned_active_recorders_share_state() {
        let recorder = EventRecorder::active();
        let clone = recorder.clone();
        assert!(recorder.is_empty());
        assert!(clone.is_empty());

        clone.record(Event::TransactionCommit);
        recorder.record(Event::TransactionConflict);

        let events = recorder.snapshot();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0].event, Event::TransactionCommit));
        assert!(matches!(events[1].event, Event::TransactionConflict));
        assert_eq!(clone.len(), 2);
        assert!(!clone.is_empty());

        let drained = clone.drain();
        assert_eq!(drained.len(), 2);
        assert!(recorder.is_empty());
        assert!(clone.is_empty());
    }
}
