//! Critical lifecycle exclusion: no package replacement while any old RAD process lives.
use rad_update::gate::SessionGate;
use std::{os::unix::fs::PermissionsExt, time::Duration};
struct Private(std::path::PathBuf);
impl Private {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "rad-update-gate-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
}
impl Drop for Private {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn handoff_blocks_new_processes_but_waits_for_existing_processes_without_killing_them() {
    let private = Private::new();
    let gate = SessionGate::at(&private.0).unwrap();
    let existing = gate.startup_guard().unwrap().expect("normal startup");
    let update = gate.acquire_update().unwrap();
    assert!(!gate.startup_allowed().unwrap());
    assert!(gate.startup_guard().unwrap().is_none());
    assert!(gate.acquire_update().is_err(), "one update only");
    assert!(
        update.wait_quiescent(Duration::ZERO).is_err(),
        "never replace a running process"
    );
    drop(existing);
    let quiet = update.wait_quiescent(Duration::ZERO).unwrap();
    assert!(!gate.startup_allowed().unwrap());
    drop(quiet);
    drop(update);
    assert!(gate.startup_allowed().unwrap());
}
#[test]
fn rejects_a_symlinked_gate_instead_of_locking_a_different_directory() {
    let private = Private::new();
    let elsewhere = private.0.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, private.0.join("rad-update")).unwrap();
    assert!(SessionGate::at(&private.0).is_err());
}
