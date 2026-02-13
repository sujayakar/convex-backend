use common::event_recorder::{
    Event,
    EventRecorder,
};
use serde_json::json;

#[test]
fn prod_event_recorder_is_noop_and_api_is_consistent() {
    let recorder = EventRecorder::new();

    assert!(!recorder.is_active());
    assert_eq!(recorder.len(), 0);
    assert!(recorder.is_empty());
    assert!(recorder.snapshot().is_empty());
    assert!(recorder.drain().is_empty());

    recorder.record(Event::TransactionCommit);
    recorder.record_custom("label", json!({ "value": 1 }));

    assert_eq!(recorder.len(), 0);
    assert!(recorder.is_empty());
    assert!(recorder.snapshot().is_empty());
    assert!(recorder.drain().is_empty());
}

#[test]
fn prod_event_recorder_default_matches_new() {
    fn defaulted<T: Default>() -> T {
        T::default()
    }

    let recorder: EventRecorder = defaulted();

    assert!(!recorder.is_active());
    assert_eq!(recorder.len(), 0);
    assert!(recorder.is_empty());
    assert!(recorder.snapshot().is_empty());
    assert!(recorder.drain().is_empty());
}

#[test]
fn prod_event_recorder_clone_remains_noop() {
    let recorder = EventRecorder::new();
    let clone = recorder.clone();

    clone.record(Event::TransactionConflict);
    clone.record_custom("label", json!({ "value": 2 }));

    assert_eq!(recorder.len(), 0);
    assert!(recorder.is_empty());
    assert!(recorder.snapshot().is_empty());
    assert!(recorder.drain().is_empty());

    assert_eq!(clone.len(), 0);
    assert!(clone.is_empty());
    assert!(clone.snapshot().is_empty());
    assert!(clone.drain().is_empty());
}
