//! `machine_settings` and `set_machine_settings`: the machine's own options (ADR 0005, 0006).
use rad_api::{EventCode, MachineSettingsView, Operation, Request, ResponseData, Status};
use serde_json::json;

#[test]
fn machine_settings_is_a_closed_read_operation() {
    let request = Request::parse("machine_settings", json!({})).unwrap();
    assert_eq!(request.operation(), Operation::MachineSettings);
    assert_eq!(request.operation().as_str(), "machine_settings");
    assert_eq!(
        Operation::parse("machine_settings"),
        Some(Operation::MachineSettings)
    );
    assert!(!Operation::MachineSettings.is_mutation());
    assert!(Request::parse("machine_settings", json!({"use_stream_deck":true})).is_err());
}

#[test]
fn set_machine_settings_is_a_mutation_with_optional_closed_booleans() {
    for args in [
        json!({"use_stream_deck":true}),
        json!({"start_with_session":false}),
        json!({"use_stream_deck":false,"start_with_session":true}),
        json!({}),
    ] {
        let request = Request::parse("set_machine_settings", args.clone()).unwrap();
        assert_eq!(request.operation(), Operation::SetMachineSettings);
        assert_eq!(serde_json::to_value(&request).unwrap()["args"], args);
    }
    assert!(Operation::SetMachineSettings.is_mutation());
    // Neither an unknown field, a null, nor a non-boolean is read as "unchanged".
    for bad in [
        json!({"use_stream_deck":null}),
        json!({"use_stream_deck":"yes"}),
        json!({"start_with_session":1}),
        json!({"autostart":true}),
    ] {
        assert!(Request::parse("set_machine_settings", bad).is_err());
    }
}

#[test]
fn both_answers_are_ok_and_decode_by_operation() {
    let answer = json!({"use_stream_deck":true,"start_with_session":false});
    for operation in [Operation::MachineSettings, Operation::SetMachineSettings] {
        assert_eq!(operation.success_status(), Status::Ok);
        let ResponseData::MachineSettings(view) =
            ResponseData::decode(operation, Status::Ok, answer.clone()).unwrap()
        else {
            panic!("machine settings answer");
        };
        assert!(view.use_stream_deck && !view.start_with_session);
        assert_eq!(serde_json::to_value(&view).unwrap(), answer);
    }
    for bad in [
        json!({"use_stream_deck":true}),
        json!({"use_stream_deck":"true","start_with_session":false}),
        json!({"use_stream_deck":true,"start_with_session":false,"extra":1}),
    ] {
        assert!(ResponseData::decode(Operation::MachineSettings, Status::Ok, bad).is_err());
    }
    let _: Option<MachineSettingsView> = None;
}

#[test]
fn an_unreadable_machine_file_has_its_own_event_code() {
    assert_eq!(
        serde_json::to_value(EventCode::MachineSettingsUnreadable).unwrap(),
        json!("machine_settings_unreadable")
    );
}
