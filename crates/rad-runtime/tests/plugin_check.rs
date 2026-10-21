//! Critical certification boundary: incomplete or invalid evidence never certifies.
use std::process::Command;
#[test]
fn missing_inputs_never_certify() {
    let result = Command::new(env!("CARGO_BIN_EXE_rad-plugin-check"))
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "missing scenario and package must fail closed"
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_ne!(report["status"], "PASS");
}

fn certify(mode: &str) -> serde_json::Value {
    certify_with(mode, |_| {})
}
fn certify_with(mode: &str, change: impl FnOnce(&mut serde_json::Value)) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!("rad-check-test-{}-{}", std::process::id(), mode));
    std::fs::create_dir_all(&dir).unwrap();
    let mut manifest = serde_json::json!({"contract":1,"capabilities":["credentials","dynamic_options","visual_freshness"],"id":"org.example.check","version":{"major":1,"minor":0,"patch":0},"host_min":1,"host_max":1,"targets":[rad_plugin_host::TARGET],"actions":[{"id":"act","label":"Action","schema":{"version":1,"fields":[{"id":"choice","label":"Choice","required":false,"kind":{"type":"dynamic_choice","depends_on":[]}}]},"credential_slots":["password"]}],"visuals":[{"id":"state","label":"State","schema":{"version":1,"fields":[]},"credential_slots":[]}]});
    if mode == "gestures-settings" {
        manifest["capabilities"] = serde_json::json!([
            "credentials",
            "dynamic_options",
            "visual_freshness",
            "controls",
            "settings"
        ]);
        manifest["actions"][0]["gestures"] = serde_json::json!(["press", "hold", "long_press"]);
        manifest["settings"] = serde_json::json!({"version":1,"fields":[{"id":"enabled","label":"Enabled","required":true,"kind":{"type":"boolean"}}]});
    }
    if mode == "dynamic-settings" {
        manifest["capabilities"]
            .as_array_mut()
            .unwrap()
            .push("settings".into());
        manifest["settings"] = serde_json::json!({"version":1,"fields":[{"id":"choice","label":"Choice","required":false,"kind":{"type":"dynamic_choice","depends_on":[]}}]});
    }
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let executable = std::fs::read(
        std::path::Path::new(env!("CARGO_BIN_EXE_rad-plugin-check"))
            .parent()
            .unwrap()
            .join("examples/plugin_check_fixture"),
    )
    .unwrap();
    let candidate = rad_plugin_package::Candidate::from_parts(
        &manifest,
        &executable,
        1,
        rad_plugin_host::TARGET,
    )
    .unwrap();
    std::fs::write(
        dir.join("test.radp"),
        rad_plugin_package::encode(&candidate),
    )
    .unwrap();
    let p = serde_json::json!({"schema_version":1,"values":{}});
    let mut scenario = serde_json::json!({"version":1,"trusted_native_code":true,"timeout_ms":1000,"freshness_ms":1300,"args":[mode],"actions":[{"id":"act","parameters":p}],"options":[{"scope":"action","definition":"act","field":"choice","context":p}],"visuals":[{"id":"state","parameters":p}],"pressure":{"kind":"options","index":0,"safe_to_repeat":true}});
    if mode == "gestures-settings" {
        scenario["settings"] = serde_json::json!({"schema_version":1,"values":{"enabled":true}});
        scenario["actions"] = serde_json::json!([
            {"id":"act","gesture":"press","parameters":p},
            {"id":"act","gesture":"hold","parameters":p},
            {"id":"act","gesture":"long_press","parameters":p}
        ]);
    }
    if mode == "dynamic-settings" {
        scenario["settings"] = p.clone();
    }
    change(&mut scenario);
    std::fs::write(
        dir.join("scenario.json"),
        serde_json::to_vec(&scenario).unwrap(),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_rad-plugin-check"))
        .args([dir.join("test.radp"), dir.join("scenario.json")])
        .output()
        .unwrap();
    let report = serde_json::from_slice(&result.stdout).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    report
}
#[test]
fn complete_synthetic_plugin_certifies() {
    let report = certify("valid");
    assert_eq!(report["status"], "PASS", "{report}");
}
#[test]
fn invalid_identity_and_capabilities_never_certify() {
    for mode in ["identity", "capabilities"] {
        let r = certify(mode);
        assert_ne!(r["status"], "PASS", "{r}");
    }
}
#[test]
fn credential_leaks_never_certify() {
    for mode in ["stderr-leak", "message-leak", "visual-leak"] {
        let r = certify(mode);
        assert_ne!(r["status"], "PASS", "{r}");
        assert_eq!(r["failure"], "credential_leak", "{r}");
    }
}
#[test]
fn missing_source_renewal_never_certifies() {
    let r = certify("no-renew");
    assert_eq!(r["failure"], "visual_stale", "{r}");
}
#[test]
fn eof_surviving_child_never_certifies() {
    let r = certify("eof-child");
    assert_eq!(r["failure"], "eof_live_descendant", "{r}");
}

#[test]
fn a_helper_child_ended_on_eof_certifies() {
    let r = certify("child-until-eof");
    assert_eq!(r["status"], "PASS", "{r}");
}

#[test]
fn output_blocked_on_a_reply_larger_than_the_queued_ones_certifies() {
    let r = certify("late-large-reply");
    assert_eq!(r["status"], "PASS", "{r}");
}

#[test]
fn blocked_stdout_never_certifies() {
    let r = certify("blocked");
    assert_eq!(r["failure"], "blocked_stdout_not_terminal", "{r}");
}
#[test]
fn frame_after_truncation_never_certifies() {
    let r = certify("truncated");
    assert_eq!(r["failure"], "output_after_truncated_frame", "{r}");
}

#[test]
fn overwritten_visual_secret_never_certifies() {
    let r = certify("transient-leak");
    assert_eq!(r["failure"], "credential_leak", "{r}");
}

#[test]
fn incomplete_or_unsafe_scenarios_never_certify() {
    for (mode, field) in [
        ("missing-action", "actions"),
        ("missing-options", "options"),
        ("missing-visual", "visuals"),
    ] {
        let r = certify_with(mode, |s| s[field] = serde_json::json!([]));
        assert_ne!(r["status"], "PASS", "{r}");
    }
    let r = certify_with("unsafe", |s| s["pressure"]["safe_to_repeat"] = false.into());
    assert_eq!(r["failure"], "no_safe_pressure_vector", "{r}");
}

#[test]
fn all_declared_gestures_and_settings_are_exercised() {
    let r = certify_with("gestures-settings", |s| {
        s["pressure"]["kind"] = "action".into();
        s["pressure"]["index"] = 1.into();
    });
    assert_eq!(r["status"], "PASS", "{r}");
}

#[test]
fn unsubscribed_source_must_really_stop() {
    let r = certify("ignored-unsubscribe");
    assert_eq!(r["failure"], "unsubscribe_source_still_active", "{r}");
}

#[test]
fn explicitly_repeatable_visual_pressure_certifies() {
    let r = certify_with("visual-pressure", |s| {
        s["pressure"]["kind"] = "visual".into()
    });
    assert_eq!(r["status"], "PASS", "{r}");
}

#[test]
fn eof_cannot_hide_stderr_leaks_or_escaped_children() {
    let r = certify("eof-stderr-leak");
    assert_eq!(r["failure"], "credential_leak", "{r}");
    let r = certify("eof-escaped-child");
    assert_eq!(r["failure"], "eof_live_descendant", "{r}");
}

#[test]
fn unobservable_dynamic_settings_never_certify() {
    let r = certify("dynamic-settings");
    assert_eq!(r["failure"], "settings_dynamic_options_unobservable", "{r}");
}
