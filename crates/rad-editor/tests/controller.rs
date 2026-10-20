use rad_editor::{Controller, Request};
use serde_json::json;
#[test]
fn selection_never_executes_and_try_is_explicit() {
    let mut c = Controller::default();
    assert_eq!(c.try_selected(), None);
    c.select(Some("b7".into()));
    assert_eq!(c.selected(), Some("b7"));
    assert_eq!(
        c.try_selected(),
        Some(Request::new("run", json!({"id":"b7"})))
    );
    c.select(None);
    assert_eq!(c.try_selected(), None);
}
#[test]
fn status_poll_is_coalesced_and_respects_visible_inactive_intervals() {
    let mut p = rad_editor::PollSchedule::default();
    assert!(p.due(1000, true, false));
    assert!(!p.due(1249, true, false));
    assert!(!p.due(1250, true, true));
    assert!(p.due(1250, true, false));
    assert!(!p.due(2249, false, false));
    assert!(p.due(2250, false, false));
}
#[test]
fn accepted_execution_is_observed_by_result_never_replayed_run() {
    let mut c = Controller::default();
    c.select(Some("b7".into()));
    assert!(c.result_request().is_none());
    c.admitted("00000000000000000000000000000001:9".into());
    assert_eq!(
        c.result_request(),
        Some(Request::new(
            "result",
            json!({"execution_id":"00000000000000000000000000000001:9"})
        ))
    );
    c.terminal();
    assert!(c.result_request().is_none());
}
