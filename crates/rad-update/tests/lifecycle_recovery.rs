//! Durable recovery and first-frame ACK, isolated in child test processes.
use rad_update::coordinator;
use std::{os::unix::fs::PermissionsExt, path::PathBuf, process::Command};
#[test]
fn interrupted_transactions_are_not_success_and_reopen_ack_preserves_the_result() {
    if let Ok(mode) = std::env::var("RAD_UPDATE_RECOVERY_TEST_CHILD") {
        // Check isolation before operation_result may run opportunistic IPC GC.
        // A missing child override must never silently fall back to the session.
        let fixture = PathBuf::from(std::env::var_os("XDG_STATE_HOME").unwrap());
        let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
        assert_eq!(runtime, fixture.join("runtime"));
        assert_eq!(
            std::fs::metadata(&runtime).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let result = coordinator::operation_result();
        match mode.as_str() {
            "interrupted" => {
                let result = result.unwrap().unwrap();
                assert!(!result.success);
                assert!(result.details.contains("interrupted"));
            }
            "reopened" => {
                let result = result.unwrap().unwrap();
                assert!(result.success);
                coordinator::acknowledge_reopen().unwrap();
                assert_eq!(coordinator::operation_result().unwrap().unwrap(), result);
                let ack = std::env::var("XDG_STATE_HOME").unwrap() + "/rad/update/opened.json";
                assert!(
                    std::fs::read_to_string(ack)
                        .unwrap()
                        .contains("0123456789abcdef0123456789abcdef")
                );
            }
            "corrupt" => assert!(result.is_err()),
            _ => panic!("unexpected child scenario"),
        }
        return;
    }
    let root = std::env::temp_dir().join(format!("rad-update-recovery-{}", std::process::id()));
    let state = root.join("rad/update");
    let runtime = root.join("runtime");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    for (mode, record) in [
        (
            "interrupted",
            r#"{"schema":1,"nonce":"0123456789abcdef0123456789abcdef","phase":"installing","target":"0.2.0-1","result":null}"#,
        ),
        (
            "reopened",
            r#"{"schema":1,"nonce":"0123456789abcdef0123456789abcdef","phase":"reopening","target":"0.2.0-1","result":{"success":true,"details":"Installed 0.2.0-1"}}"#,
        ),
        ("corrupt", "{broken"),
    ] {
        let path = state.join("operation.json");
        std::fs::write(&path, record).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "interrupted_transactions_are_not_success_and_reopen_ack_preserves_the_result",
                "--nocapture",
            ])
            .env("XDG_STATE_HOME", &root)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("RAD_UPDATE_RECOVERY_TEST_CHILD", mode)
            .status()
            .unwrap();
        assert!(status.success(), "{mode}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
