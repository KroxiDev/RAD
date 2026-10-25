//! Critical lifecycle contract with synthetic effects, never real systemd/polkit/pacman.
use rad_update::{
    Candidate, Error, InstallOutcome, Prepared, ReleaseMetadata,
    coordinator::{self, SessionControl},
    gate::SessionGate,
};
use std::{cell::RefCell, os::unix::fs::PermissionsExt, rc::Rc};
struct Fake {
    calls: Rc<RefCell<Vec<&'static str>>>,
    stop_ok: bool,
    auth_ok: bool,
    grant_ok: bool,
    actual: String,
}
impl SessionControl for Fake {
    fn authorize(&mut self, _: &Prepared) -> Result<(), Error> {
        self.calls.borrow_mut().push("authorize");
        if self.auth_ok {
            Ok(())
        } else {
            Err(Error("denied".into()))
        }
    }
    fn validate_handoff(&mut self) -> Result<(), Error> {
        self.calls.borrow_mut().push("grant");
        if self.grant_ok {
            Ok(())
        } else {
            Err(Error("handoff expired or launcher failed".into()))
        }
    }
    fn wait_editor_closed(&mut self, _: u32) -> Result<(), Error> {
        self.calls.borrow_mut().push("editor");
        Ok(())
    }
    fn stop_cleanly(&mut self) -> Result<(), Error> {
        self.calls.borrow_mut().push("stop");
        if self.stop_ok {
            Ok(())
        } else {
            Err(Error("still running".into()))
        }
    }
    fn commit(&mut self) -> Result<InstallOutcome, Error> {
        self.calls.borrow_mut().push("commit");
        Ok(InstallOutcome {
            installed_version: "0.2.0-1".into(),
            details: "installed".into(),
        })
    }
    fn installed_version(&mut self) -> Result<String, Error> {
        self.calls.borrow_mut().push("verify");
        Ok(self.actual.clone())
    }
}
fn prepared() -> Prepared {
    Prepared {
        directory: "/synthetic/staging".into(),
        candidate: Candidate {
            tag: "v0.2.0-1".into(),
            installed_version: "0.1.0-1".into(),
            metadata: ReleaseMetadata {
                schema: 1,
                product: "RAD".into(),
                package: "rust-action-deck".into(),
                version: "0.2.0".into(),
                pkgrel: 1,
                architecture: "x86_64".into(),
                file: "rust-action-deck-0.2.0-1-x86_64.pkg.tar.zst".into(),
                size: 3,
                sha256: "0".repeat(64),
                base: "omarchy-4.0.6".into(),
                commit: "a".repeat(40),
            },
        },
    }
}
#[test]
fn authentication_precedes_ack_and_no_transaction_starts_unless_shutdown_is_demonstrated() {
    let root = std::env::temp_dir().join(format!("rad-update-transaction-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let gate = SessionGate::at(&root).unwrap();
    let update = gate.acquire_update().unwrap();
    for (auth_ok, stop_ok, actual, expected) in [
        (
            true,
            true,
            "0.2.0-1",
            vec![
                "authorize",
                "ack",
                "grant",
                "editor",
                "grant",
                "stop",
                "grant",
                "commit",
                "verify",
            ],
        ),
        (false, true, "0.2.0-1", vec!["authorize"]),
        (
            true,
            false,
            "0.2.0-1",
            vec!["authorize", "ack", "grant", "editor", "grant", "stop"],
        ),
        (
            true,
            true,
            "0.1.0-1",
            vec![
                "authorize",
                "ack",
                "grant",
                "editor",
                "grant",
                "stop",
                "grant",
                "commit",
                "verify",
            ],
        ),
    ] {
        let calls = Rc::new(RefCell::new(vec![]));
        let ready = calls.clone();
        let mut fake = Fake {
            calls: calls.clone(),
            auth_ok,
            grant_ok: true,
            stop_ok,
            actual: actual.into(),
        };
        let result = coordinator::transact(&mut fake, &prepared(), 42, &update, move || {
            ready.borrow_mut().push("ack");
            Ok(())
        });
        assert_eq!(*calls.borrow(), expected);
        assert_eq!(result.is_ok(), auth_ok && stop_ok && actual == "0.2.0-1");
    }
    drop(update);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_ack_without_a_current_ui_grant_never_waits_for_editor_exit_or_stops_rad() {
    let root =
        std::env::temp_dir().join(format!("rad-update-expired-grant-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let gate = SessionGate::at(&root).unwrap();
    let update = gate.acquire_update().unwrap();
    let calls = Rc::new(RefCell::new(vec![]));
    let mut fake = Fake {
        calls: calls.clone(),
        auth_ok: true,
        grant_ok: false,
        stop_ok: true,
        actual: "0.2.0-1".into(),
    };
    let ready = calls.clone();
    let result = coordinator::transact(&mut fake, &prepared(), 42, &update, move || {
        ready.borrow_mut().push("late_ready");
        Ok(())
    });
    assert!(
        result.is_err(),
        "late authorization is not renewed consent after the UI saw an error"
    );
    assert_eq!(*calls.borrow(), ["authorize", "late_ready", "grant"]);
    drop(update);
    std::fs::remove_dir_all(root).unwrap();
}
