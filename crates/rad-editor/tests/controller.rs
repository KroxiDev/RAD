use rad_editor::Controller;
use serde_json::json;
#[test]
fn selection_never_executes_and_try_is_explicit() {
    let mut c = Controller::default();
    assert_eq!(c.try_selected(), None);
    c.select(Some("b7".into()));
    assert_eq!(c.selected(), Some("b7"));
    assert_eq!(c.try_selected(), Some(request("run", json!({"id":"b7"}))));
    c.select(None);
    assert_eq!(c.try_selected(), None);
}
#[test]
fn background_work_is_coalesced_and_no_category_monopolizes_turns() {
    let mut schedule = rad_editor::schedule::Schedule::default();
    assert_eq!(schedule.choose([false; 3]), None);
    assert_eq!(schedule.choose([true, true, true]), Some(0));
    assert_eq!(schedule.choose([true, true, true]), Some(1));
    assert_eq!(schedule.choose([true, false, true]), Some(2));
    assert_eq!(schedule.choose([true, false, false]), Some(0));
}
#[test]
fn accepted_execution_is_observed_by_result_never_replayed_run() {
    let mut c = Controller::default();
    c.select(Some("b7".into()));
    assert!(c.result_request().is_none());
    c.admitted("00000000000000000000000000000001:9".into());
    assert_eq!(
        c.result_request(),
        Some(request(
            "result",
            json!({"execution_id":"00000000000000000000000000000001:9"})
        ))
    );
    c.terminal();
    assert!(c.result_request().is_none());
}

fn request(op: &str, args: serde_json::Value) -> rad_api::Request {
    rad_api::Request::parse(op, args).expect("valid typed fixture request")
}
