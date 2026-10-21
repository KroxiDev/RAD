//! Exit classes (ADR 0011): codes, the last unexpected exit, and the unit file that depends on them.
use rad_runtime::{
    exit::{self, Exit, Reason},
    server::RuntimeError,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

struct Directory(PathBuf);
impl Directory {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rad-exit-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn each_runtime_error_belongs_to_one_documented_class() {
    assert_eq!((exit::CLEAN, exit::UNEXPECTED, exit::BLOCKING), (0, 1, 75));
    let blocked = Exit::from(RuntimeError::Blocked);
    assert_eq!(blocked.code(), 75);
    assert_eq!(blocked.reason(), None);
    assert!(!blocked.message().is_empty());
    for (error, reason, text) in [
        (RuntimeError::Startup, Reason::Startup, "startup"),
        (RuntimeError::Storage, Reason::Storage, "storage"),
        (RuntimeError::Transport, Reason::Transport, "transport"),
        (RuntimeError::Clock, Reason::Clock, "clock"),
        (RuntimeError::Protocol, Reason::Protocol, "protocol"),
    ] {
        let exit = Exit::from(error);
        assert_eq!(exit.code(), 1, "{error:?}");
        assert_eq!(exit.reason(), Some(reason), "{error:?}");
        assert_eq!(reason.code(), text);
        assert!(!exit.message().is_empty());
    }
    assert_eq!(Reason::Panic.code(), "panic");
}

#[test]
fn the_record_is_private_exact_and_carries_no_free_text() {
    let directory = Directory::new("record");
    exit::record(&directory.0, Reason::Panic, 1_234).unwrap();
    let path = directory.0.join("last-exit.json");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"version":1,"code":"panic","at_unix_ms":1234})
    );
    // No temporary file is left behind by the atomic write.
    let names: Vec<_> = fs::read_dir(&directory.0)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["last-exit.json"]);
}

#[test]
fn the_previous_exit_is_consumed_once_and_the_next_start_sees_nothing() {
    let directory = Directory::new("consume");
    assert_eq!(exit::take_previous(&directory.0), None);
    exit::record(&directory.0, Reason::Startup, 1_700_000_000_123).unwrap();
    let previous = exit::take_previous(&directory.0).unwrap();
    assert_eq!(previous.code, "startup");
    assert_eq!(previous.at_unix_ms, "1700000000123");
    assert!(!directory.0.join("last-exit.json").exists());
    assert_eq!(exit::take_previous(&directory.0), None);
}

#[test]
fn an_unreadable_record_is_ignored_and_removed() {
    let directory = Directory::new("unreadable");
    let path = directory.0.join("last-exit.json");
    for bytes in [
        &b"not json"[..],
        br#"{"version":2,"code":"startup","at_unix_ms":1}"#,
        br#"{"version":1,"code":"free text with a path /home/x","at_unix_ms":1}"#,
        br#"{"version":1,"code":"startup","at_unix_ms":-1}"#,
        br#"{"version":1,"code":"startup","at_unix_ms":1,"message":"x"}"#,
        &vec![b' '; 5000],
    ] {
        fs::write(&path, bytes).unwrap();
        assert_eq!(exit::take_previous(&directory.0), None, "{bytes:?}");
        assert!(!path.exists(), "{bytes:?}");
    }
}

#[test]
fn a_contained_failure_is_one_line_with_a_stable_code() {
    use rad_api::EventCode;
    let line = rad_runtime::events::Events::line(EventCode::DeckSpawnFailed, None);
    assert_eq!(line, "rad-runtime: deck_spawn_failed");
    assert!(!line.contains('\n'));
}

#[test]
fn the_unit_restarts_only_unexpected_failures_with_a_cap_and_never_the_blocking_code() {
    let unit = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/rad-runtime.service"),
    )
    .unwrap();
    // Directives by section: the start limit is only honored in [Unit].
    let mut section = "";
    let mut directives = Vec::new();
    for line in unit.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
        } else if !line.starts_with('#') && !line.is_empty() {
            directives.push((section, line));
        }
    }
    let has = |section: &str, directive: &str| directives.contains(&(section, directive));
    for directive in ["StartLimitBurst=5", "StartLimitIntervalSec=300"] {
        assert!(has("[Unit]", directive), "{directive}");
    }
    for directive in ["Restart=on-failure", "KillMode=mixed", "RestartSec=1"] {
        assert!(has("[Service]", directive), "{directive}");
    }
    assert!(
        has(
            "[Service]",
            &format!("RestartPreventExitStatus={}", exit::BLOCKING)
        ),
        "the blocking code in the unit must be the one the runtime exits with"
    );
    for growing in ["RestartSteps=", "RestartMaxDelaySec="] {
        assert!(
            directives
                .iter()
                .any(|(section, line)| *section == "[Service]" && line.starts_with(growing)),
            "{growing}"
        );
    }
}
