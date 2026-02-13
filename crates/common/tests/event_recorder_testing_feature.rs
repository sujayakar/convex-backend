#![cfg(feature = "testing")]

use common::event_recorder::{
    Event,
    EventRecorder,
};
use serde_json::json;

#[test]
fn testing_feature_event_recorder_collects_events() {
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
    assert!(recorder.is_empty());
}
