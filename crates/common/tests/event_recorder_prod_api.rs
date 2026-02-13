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
