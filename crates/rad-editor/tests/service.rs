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
    home.service(&fake).start().unwrap();
    assert_eq!(fake.verbs()[..2], ["daemon-reload", "start"]);
    assert_eq!(fake.calls()[1], ["start", UNIT]);
    assert!(!fake.verbs().iter().any(|v| v == "enable" || v == "disable"));
    let mode = std::fs::metadata(home.unit()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o644);
    let leftovers: Vec<_> = std::fs::read_dir(&home.units).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "no temporary file stays behind");
}
