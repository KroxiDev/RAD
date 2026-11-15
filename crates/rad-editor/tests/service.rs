//! The editor installs, starts and enables the runtime's user unit through a systemctl seam.
use rad_editor::service::{Answer, Service, Systemctl, UNIT, Unavailable};
use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const ACTIVE: &str = "ActiveState=active\nResult=success\nExecMainStatus=0\n";

/// A systemctl in memory: records every call and answers `show` and failures as told.
#[derive(Clone, Default)]
struct Fake {
    calls: Arc<Mutex<Vec<Vec<String>>>>,
    answers: Arc<Mutex<HashMap<String, Answer>>>,
}
impl Fake {
    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
    fn verbs(&self) -> Vec<String> {
        self.calls().into_iter().map(|c| c[0].clone()).collect()
    }
}
impl Systemctl for Fake {
    fn run(&self, args: &[&str]) -> Result<Answer, Unavailable> {
        self.calls
            .lock()
            .unwrap()
            .push(args.iter().map(|a| a.to_string()).collect());
        Ok(self
            .answers
            .lock()
            .unwrap()
            .get(args[0])
            .cloned()
            .unwrap_or_else(|| Answer {
                code: Some(0),
                stdout: if args[0] == "show" {
                    ACTIVE.into()
                } else {
                    String::new()
                },
                stderr: String::new(),
            }))
    }
}

struct Home {
    units: PathBuf,
    editor: PathBuf,
}
impl Home {
    fn new(name: &str, with_runtime: bool) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("service-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/rad-editor"), b"").unwrap();
        if with_runtime {
            std::fs::write(root.join("bin/rad-runtime"), b"").unwrap();
        }
        Self {
            units: root.join("config/systemd/user"),
            editor: root.join("bin/rad-editor"),
        }
    }
    fn service(&self, fake: &Fake) -> Service<Fake> {
        Service::new(fake.clone(), self.units.clone(), self.editor.clone())
    }
    fn unit(&self) -> PathBuf {
        self.units.join(UNIT)
    }
}

#[test]
fn starts_without_autostart_by_installing_reloading_and_starting_never_enabling() {
    let home = Home::new("start", true);
    let fake = Fake::default();
    assert!(home.service(&fake).start().unwrap().is_none());
    assert_eq!(fake.verbs()[..2], ["daemon-reload", "start"]);
    assert_eq!(fake.calls()[1], ["start", UNIT]);
    assert!(
        !fake
            .verbs()
            .iter()
            .any(|v| matches!(v.as_str(), "enable" | "disable" | "reenable"))
    );
    let mode = std::fs::metadata(home.unit()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o644);
    let leftovers: Vec<_> = std::fs::read_dir(&home.units).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "no temporary file stays behind");
}

#[test]
fn interrupted_autostart_migration_recovers_after_reopening_the_service() {
    #[derive(Clone, Copy, Debug)]
    enum Interruption {
        Failed,
        Unavailable,
        MissingLink,
    }
    #[derive(Clone)]
    struct Migrating {
        fake: Fake,
        units: PathBuf,
        interruption: Arc<Mutex<Option<Interruption>>>,
    }
    impl Systemctl for Migrating {
        fn run(&self, args: &[&str]) -> Result<Answer, Unavailable> {
            let answer = self.fake.run(args)?;
            let old = self.units.join("default.target.wants").join(UNIT);
            let new = self.units.join("graphical-session.target.wants").join(UNIT);
            if matches!(args[0], "reenable" | "disable") {
                for link in [&old, &new] {
                    if link.is_symlink() {
                        std::fs::remove_file(link).unwrap();
                    }
                }
            }
            if args[0] == "reenable" {
                if let Some(interruption) = self.interruption.lock().unwrap().take() {
                    return match interruption {
                        Interruption::Unavailable => {
                            Err(Unavailable("interrupted after removing links".into()))
                        }
                        Interruption::Failed | Interruption::MissingLink => Ok(Answer {
                            code: Some(i32::from(matches!(interruption, Interruption::Failed))),
                            stderr: "interrupted after removing links".into(),
                            ..Answer::default()
                        }),
                    };
                }
                std::fs::create_dir_all(new.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(self.units.join(UNIT), new).unwrap();
            }
            Ok(answer)
        }
    }

    for interruption in [
        Interruption::Failed,
        Interruption::Unavailable,
        Interruption::MissingLink,
    ] {
        let home = Home::new(&format!("migration-{interruption:?}"), true);
        let old = home.units.join("default.target.wants").join(UNIT);
        let new = home.units.join("graphical-session.target.wants").join(UNIT);
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(home.unit(), &old).unwrap();
        let systemctl = Migrating {
            fake: Fake::default(),
            units: home.units.clone(),
            interruption: Arc::new(Mutex::new(Some(interruption))),
        };
        let open = || Service::new(systemctl.clone(), home.units.clone(), home.editor.clone());

        let warning = open().install().unwrap().expect("migration warning");
        assert!(warning.details.contains(match interruption {
            Interruption::MissingLink => "graphical-session.target.wants",
            _ => "interrupted after removing links",
        }));
        assert!(!old.is_symlink());
        assert!(!new.is_symlink());

        assert!(open().install().unwrap().is_none());
        assert!(
            new.is_symlink(),
            "previously enabled autostart must recover"
        );
        assert!(!old.is_symlink());
        open().set_autostart(false).unwrap();
        open().install().unwrap();
        assert!(
            !new.is_symlink(),
            "completed migration must not enable it again"
        );
        assert!(
            !systemctl
                .fake
                .verbs()
                .iter()
                .any(|verb| { matches!(verb.as_str(), "start" | "restart" | "stop") })
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum MigrationFault {
    Failed,
    MissingLink,
    KeepsOldLink,
}

/// Migration alone is broken; starting, inspecting and disabling still work.
#[derive(Clone)]
struct PersistentMigration {
    fake: Fake,
    units: PathBuf,
    fault: MigrationFault,
}
impl PersistentMigration {
    fn new(home: &Home, fault: MigrationFault) -> Self {
        let old = home.units.join("default.target.wants").join(UNIT);
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(home.unit(), old).unwrap();
        Self {
            fake: Fake::default(),
            units: home.units.clone(),
            fault,
        }
    }

    fn open(&self, home: &Home) -> Service<Self> {
        Service::new(self.clone(), home.units.clone(), home.editor.clone())
    }
}
impl Systemctl for PersistentMigration {
    fn run(&self, args: &[&str]) -> Result<Answer, Unavailable> {
        let answer = self.fake.run(args)?;
        if (args[0] == "reenable" && !matches!(self.fault, MigrationFault::KeepsOldLink))
            || args[0] == "disable"
        {
            for target in ["default.target.wants", "graphical-session.target.wants"] {
                let link = self.units.join(target).join(UNIT);
                if link.is_symlink() {
                    std::fs::remove_file(link).unwrap();
                }
            }
        }
        if args[0] == "reenable" && !matches!(self.fault, MigrationFault::MissingLink) {
            return Ok(Answer {
                code: Some(1),
                stderr: "persistent synthetic migration failure".into(),
                ..Answer::default()
            });
        }
        if args[0] == "enable"
            && answer.code == Some(0)
            && !matches!(self.fault, MigrationFault::MissingLink)
        {
            let new = self.units.join("graphical-session.target.wants").join(UNIT);
            std::fs::create_dir_all(new.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(self.units.join(UNIT), new).unwrap();
        }
        Ok(answer)
    }
}

#[test]
fn persistent_migration_failure_does_not_block_start_or_retry_after_reopening() {
    for fault in [MigrationFault::Failed, MigrationFault::MissingLink] {
        let home = Home::new(&format!("persistent-start-{fault:?}"), true);
        let manager = PersistentMigration::new(&home, fault);
        let _ = manager.open(&home).install();
        assert!(
            !home
                .units
                .join("default.target.wants")
                .join(UNIT)
                .is_symlink()
        );
        manager.fake.calls.lock().unwrap().clear();

        let start = manager.open(&home).start();
        let retry = manager.open(&home).retry();
        assert!(
            start.is_ok() && retry.is_ok(),
            "migration must not block independent starts: {fault:?}, start={start:?}, retry={retry:?}"
        );
        for warning in [start.unwrap(), retry.unwrap()] {
            let warning = warning.expect("the incomplete migration must be reported");
            assert!(warning.details.contains(match fault {
                MigrationFault::Failed | MigrationFault::KeepsOldLink => {
                    "persistent synthetic migration failure"
                }
                MigrationFault::MissingLink => "graphical-session.target.wants",
            }));
        }
        assert_eq!(
            manager
                .fake
                .verbs()
                .iter()
                .filter(|verb| *verb == "start")
                .count(),
            2
        );
        assert!(
            manager
                .fake
                .verbs()
                .iter()
                .any(|verb| verb == "reset-failed")
        );
    }
}

#[test]
fn disabling_autostart_cancels_persistent_migration_without_retrying_it() {
    for fault in [MigrationFault::Failed, MigrationFault::MissingLink] {
        let home = Home::new(&format!("persistent-disable-{fault:?}"), true);
        let manager = PersistentMigration::new(&home, fault);
        let _ = manager.open(&home).install();
        manager.fake.calls.lock().unwrap().clear();

        assert!(manager.open(&home).set_autostart(false).unwrap().is_none());
        assert!(manager.open(&home).install().unwrap().is_none());

        assert_eq!(manager.fake.verbs(), ["disable"]);
        for target in ["default.target.wants", "graphical-session.target.wants"] {
            assert!(!home.units.join(target).join(UNIT).is_symlink());
        }
    }
}

#[test]
fn failed_disable_keeps_migration_for_the_next_install() {
    let home = Home::new("persistent-failed-disable", true);
    let manager = PersistentMigration::new(&home, MigrationFault::Failed);
    let _ = manager.open(&home).install();
    manager.fake.calls.lock().unwrap().clear();
    manager.fake.answers.lock().unwrap().insert(
        "disable".into(),
        Answer {
            code: Some(1),
            stderr: "synthetic disable failure".into(),
            ..Answer::default()
        },
    );

    let failure = manager.open(&home).set_autostart(false).unwrap_err();
    assert!(failure.details.contains("synthetic disable failure"));
    let _ = manager.open(&home).install();
    assert_eq!(manager.fake.verbs(), ["disable", "reenable"]);
}

#[test]
fn installation_reload_and_start_failures_remain_fatal_during_migration() {
    for retry in [false, true] {
        for fault in ["missing-runtime", "write-unit", "daemon-reload", "start"] {
            let home = Home::new(
                &format!("fatal-{fault}-retry-{retry}"),
                fault != "missing-runtime",
            );
            let manager = PersistentMigration::new(&home, MigrationFault::Failed);
            let expected = match fault {
                "missing-runtime" => "rad-runtime was not found",
                "write-unit" => {
                    std::fs::create_dir(home.unit()).unwrap();
                    "could not write"
                }
                verb => {
                    manager.fake.answers.lock().unwrap().insert(
                        verb.into(),
                        Answer {
                            code: Some(1),
                            stderr: "synthetic fatal service failure".into(),
                            ..Answer::default()
                        },
                    );
                    "synthetic fatal service failure"
                }
            };
            let service = manager.open(&home);
            let failure = if retry {
                service.retry()
            } else {
                service.start()
            }
            .unwrap_err();
            assert!(failure.details.contains(expected), "{fault}: {failure:?}");
            assert_eq!(
                failure.kind,
                rad_editor::service::FailureKind::CouldNotStart
            );
            assert_eq!(
                manager
                    .fake
                    .verbs()
                    .iter()
                    .filter(|verb| *verb == "start")
                    .count(),
                usize::from(fault == "start"),
                "installation failures must not start an invalid unit"
            );
        }
    }
}

#[test]
fn explicit_enable_resolves_migration_only_after_the_new_link_replaces_the_old_one() {
    for fault in [
        MigrationFault::Failed,
        MigrationFault::MissingLink,
        MigrationFault::KeepsOldLink,
    ] {
        let home = Home::new(&format!("enable-resolves-migration-{fault:?}"), true);
        let manager = PersistentMigration::new(&home, fault);
        let warning = manager.open(&home).set_autostart(true).unwrap();
        let resolved = matches!(fault, MigrationFault::Failed);
        assert_eq!(warning.is_none(), resolved, "{fault:?}: {warning:?}");
        assert_eq!(
            manager.fake.verbs(),
            ["daemon-reload", "reenable", "enable"]
        );
        manager.fake.calls.lock().unwrap().clear();

        let reopened = manager.open(&home).install().unwrap();
        assert_eq!(reopened.is_none(), resolved, "{fault:?}: {reopened:?}");
        assert_eq!(
            manager.fake.verbs(),
            if resolved { vec![] } else { vec!["reenable"] },
            "reopening must not repeat an already completed migration"
        );
    }
}

#[test]
fn partially_failed_first_disable_keeps_legacy_intent_for_reopening() {
    let home = Home::new("partial-first-disable", true);
    let manager = PersistentMigration::new(&home, MigrationFault::Failed);
    manager.fake.answers.lock().unwrap().insert(
        "disable".into(),
        Answer {
            code: Some(1),
            stderr: "synthetic failure after removing links".into(),
            ..Answer::default()
        },
    );

    let failure = manager.open(&home).set_autostart(false).unwrap_err();
    assert!(
        failure
            .details
            .contains("synthetic failure after removing links")
    );
    assert_eq!(manager.fake.verbs(), ["daemon-reload", "disable"]);
    assert!(
        !home
            .units
            .join("default.target.wants")
            .join(UNIT)
            .is_symlink()
    );
    manager.fake.calls.lock().unwrap().clear();

    assert!(manager.open(&home).install().unwrap().is_some());
    assert_eq!(manager.fake.verbs(), ["reenable"]);
}
