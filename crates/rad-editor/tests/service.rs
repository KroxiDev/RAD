//! The editor installs, starts and enables the runtime's user unit through a systemctl seam.
use rad_editor::service::{
    Action, Answer, Failure, FailureKind, Job, Service, Systemctl, UNIT, Unavailable, UnitState,
    parse_show, render_unit, sibling_runtime, unit_directory, worker,
};
use std::{
    collections::HashMap,
    ffi::OsString,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const TEMPLATE: &str = include_str!("../../../packaging/rad-runtime.service");
const ACTIVE: &str = "ActiveState=active\nResult=success\nExecMainStatus=0\n";

/// A systemctl in memory: records every call and answers `show` and failures as told.
#[derive(Clone, Default)]
struct Fake {
    calls: Arc<Mutex<Vec<Vec<String>>>>,
    answers: Arc<Mutex<HashMap<String, Answer>>>,
}
impl Fake {
    fn answer(&self, verb: &str, code: i32, stdout: &str, stderr: &str) {
        self.answers.lock().unwrap().insert(
            verb.into(),
            Answer {
                code: Some(code),
                stdout: stdout.into(),
                stderr: stderr.into(),
            },
        );
    }
    fn show(&self, text: &str) {
        self.answer("show", 0, text, "");
    }
    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
    fn verbs(&self) -> Vec<String> {
        self.calls().into_iter().map(|c| c[0].clone()).collect()
    }
    fn clear(&self) {
        self.calls.lock().unwrap().clear();
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

#[test]
fn an_identical_unit_is_not_rewritten_or_reloaded_and_a_different_one_is() {
    let home = Home::new("identical", true);
    let fake = Fake::default();
    let service = home.service(&fake);
    service.start().unwrap();
    fake.clear();
    service.start().unwrap();
    assert_eq!(fake.verbs()[0], "start");
    assert!(!fake.verbs().contains(&"daemon-reload".to_string()));
    std::fs::write(home.unit(), "[Service]\nExecStart=/old/rad-runtime\n").unwrap();
    fake.clear();
    service.start().unwrap();
    assert_eq!(fake.verbs()[..2], ["daemon-reload", "start"]);
    let runtime = sibling_runtime(&home.editor).unwrap();
    assert_eq!(
        std::fs::read_to_string(home.unit()).unwrap(),
        render_unit(TEMPLATE, &runtime).unwrap()
    );
}

#[test]
fn autostart_only_enables_or_disables_and_installs_the_unit_when_missing() {
    let home = Home::new("autostart", true);
    let fake = Fake::default();
    let service = home.service(&fake);
    service.set_autostart(true).unwrap();
    assert_eq!(fake.calls(), [vec!["daemon-reload"], vec!["enable", UNIT]]);
    fake.clear();
    service.set_autostart(true).unwrap();
    assert_eq!(fake.calls(), [vec!["enable", UNIT]]);
    fake.clear();
    service.set_autostart(false).unwrap();
    assert_eq!(fake.calls(), [vec!["disable", UNIT]]);
}

#[test]
fn without_a_sibling_runtime_nothing_is_installed_and_the_user_is_told() {
    let home = Home::new("no-sibling", false);
    let fake = Fake::default();
    let failure = home.service(&fake).start().unwrap_err();
    assert_eq!(failure.kind, FailureKind::CouldNotStart);
    assert!(failure.details.contains("rad-runtime"), "{failure:?}");
    assert!(fake.calls().is_empty());
    assert!(!home.unit().exists());
    assert!(home.service(&fake).set_autostart(true).is_err());
    assert!(sibling_runtime(&home.editor).is_none());
}

#[test]
fn the_unit_runs_the_sibling_runtime_and_keeps_every_other_line_of_the_template() {
    let runtime = Path::new("/opt/rad/bin/rad-runtime");
    let unit = render_unit(TEMPLATE, runtime).unwrap();
    let (mine, template): (Vec<_>, Vec<_>) = (unit.lines().collect(), TEMPLATE.lines().collect());
    assert_eq!(mine.len(), template.len());
    let changed: Vec<_> = mine.iter().zip(&template).filter(|(a, b)| a != b).collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(*changed[0].0, "ExecStart=/opt/rad/bin/rad-runtime");
    assert!(changed[0].1.starts_with("ExecStart="));
    // systemd would split or expand these, so they are quoted and escaped.
    let odd = render_unit(TEMPLATE, Path::new("/opt/my rad/100%/$x/rad-runtime")).unwrap();
    assert!(
        odd.contains("ExecStart=\"/opt/my rad/100%%/$$x/rad-runtime\"\n"),
        "{odd}"
    );
    assert!(render_unit(TEMPLATE, Path::new("/opt/a\nb/rad-runtime")).is_err());
    assert!(render_unit("[Service]\n", runtime).is_err());
}

#[test]
fn the_unit_directory_follows_xdg_and_falls_back_to_home() {
    let os = |s: &str| Some(OsString::from(s));
    assert_eq!(
        unit_directory(os("/x"), os("/h")),
        Some("/x/systemd/user".into())
    );
    assert_eq!(
        unit_directory(None, os("/h")),
        Some("/h/.config/systemd/user".into())
    );
    assert_eq!(
        unit_directory(os(""), os("/h")),
        Some("/h/.config/systemd/user".into())
    );
    assert_eq!(
        unit_directory(os("rel"), os("/h")),
        Some("/h/.config/systemd/user".into())
    );
    assert_eq!(unit_directory(None, None), None);
}

#[test]
fn show_output_becomes_a_unit_state_tolerantly() {
    let failed = |limit, result: &str, status| UnitState::Failed {
        start_limit_hit: limit,
        result: result.into(),
        exit_status: status,
    };
    assert_eq!(parse_show(ACTIVE), UnitState::Running);
    assert_eq!(parse_show("ActiveState=activating\n"), UnitState::Running);
    assert_eq!(
        parse_show("ActiveState=inactive\nResult=success\n"),
        UnitState::Stopped
    );
    assert_eq!(
        parse_show("ActiveState=failed\nResult=start-limit-hit\nExecMainStatus=1\n"),
        failed(true, "start-limit-hit", Some(1))
    );
    assert_eq!(
        parse_show("Result=exit-code\nExecMainStatus=75\nActiveState=failed\n"),
        failed(false, "exit-code", Some(75))
    );
    assert_eq!(
        parse_show("ActiveState=failed\nResult=exit-code\nExecMainStatus=1\nExtra=1\nnoise\n"),
        failed(false, "exit-code", Some(1))
    );
    assert_eq!(parse_show("ActiveState=failed\n"), failed(false, "", None));
    for unreadable in ["", "\u{0}\u{1}garbage", "ActiveState=\n", "no equals here"] {
        assert_eq!(
            parse_show(unreadable),
            UnitState::Unreadable,
            "{unreadable:?}"
        );
    }
}

fn failure_after(show: &str) -> Option<Failure> {
    let home = Home::new("failure", true);
    let fake = Fake::default();
    fake.show(show);
    home.service(&fake).failure()
}

#[test]
fn each_unit_state_gives_the_message_and_actions_of_the_ux_table() {
    let both = [Action::Retry, Action::ViewDetails];
    let limit =
        failure_after("ActiveState=failed\nResult=start-limit-hit\nExecMainStatus=1\n").unwrap();
    assert_eq!(limit.kind, FailureKind::StoppedByError);
    assert_eq!(limit.kind.actions(), both);
    assert_eq!(limit.kind.message(), "RAD stopped because of an error");
    assert!(limit.details.contains("start-limit-hit"), "{limit:?}");
    let blocked =
        failure_after("ActiveState=failed\nResult=exit-code\nExecMainStatus=75\n").unwrap();
    assert_eq!(blocked.kind, FailureKind::CouldNotStart);
    assert_eq!(blocked.kind.actions(), both);
    assert_eq!(blocked.kind.message(), "RAD could not start");
    assert!(blocked.details.contains("75"), "{blocked:?}");
    let crashed =
        failure_after("ActiveState=failed\nResult=exit-code\nExecMainStatus=1\n").unwrap();
    assert_eq!(crashed.kind, FailureKind::CouldNotStart);
    assert!(crashed.details.contains("exit-code") && crashed.details.contains('1'));
    assert_eq!(failure_after(ACTIVE), None);
    let unreadable = failure_after("???").unwrap();
    assert_eq!(unreadable.kind, FailureKind::CouldNotStart);
    assert!(!unreadable.details.is_empty());
}

#[test]
fn a_refused_start_is_classified_by_the_state_of_the_unit() {
    let home = Home::new("refused", true);
    let fake = Fake::default();
    fake.answer("start", 1, "", "Start request repeated too quickly.");
    fake.show("ActiveState=failed\nResult=start-limit-hit\nExecMainStatus=1\n");
    let failure = home.service(&fake).start().unwrap_err();
    assert_eq!(failure.kind, FailureKind::StoppedByError);
    // A unit that starts but is already failed (code 75 is not restarted) is also reported.
    let fake = Fake::default();
    fake.show("ActiveState=failed\nResult=exit-code\nExecMainStatus=75\n");
    let failure = home.service(&fake).start().unwrap_err();
    assert_eq!(failure.kind, FailureKind::CouldNotStart);
    assert!(failure.details.contains("75"));
}

#[test]
fn retry_clears_the_failure_before_starting_once() {
    let home = Home::new("retry", true);
    let fake = Fake::default();
    let service = home.service(&fake);
    service.install().unwrap();
    fake.clear();
    service.retry().unwrap();
    assert_eq!(fake.verbs()[..2], ["reset-failed", "start"]);
    assert_eq!(fake.calls()[0], ["reset-failed", UNIT]);
    assert_eq!(fake.verbs().iter().filter(|v| *v == "start").count(), 1);
}

#[test]
fn the_state_is_asked_for_with_exact_properties_and_no_systemd_is_reported() {
    struct Missing;
    impl Systemctl for Missing {
        fn run(&self, _: &[&str]) -> Result<Answer, Unavailable> {
            Err(Unavailable("No such file or directory".into()))
        }
    }
    let home = Home::new("missing", true);
    let fake = Fake::default();
    home.service(&fake).state();
    assert_eq!(
        fake.calls(),
        [vec![
            "show",
            UNIT,
            "-p",
            "ActiveState",
            "-p",
            "Result",
            "-p",
            "ExecMainStatus"
        ]]
    );
    let service = Service::new(Missing, home.units.clone(), home.editor.clone());
    assert!(matches!(service.state(), UnitState::NoSystemd(why) if why.contains("No such file")));
    let failure = service.start().unwrap_err();
    assert_eq!(failure.kind, FailureKind::CouldNotStart);
    assert!(failure.details.contains("No such file"), "{failure:?}");
}

#[test]
fn the_service_worker_runs_each_job_and_explains_why_rad_does_not_answer() {
    let home = Home::new("worker", true);
    let fake = Fake::default();
    let mut jobs = worker(Ok(home.service(&fake)), std::time::Duration::ZERO);
    let mut run = |job| {
        jobs.submit(job).unwrap();
        jobs.receive().ready().unwrap()
    };
    let started = run(Job::Start);
    assert_eq!((started.job, started.failure), (Job::Start, None));
    assert_eq!(fake.verbs()[..2], ["daemon-reload", "start"]);
    fake.clear();
    assert_eq!(run(Job::Retry).failure, None);
    assert_eq!(fake.verbs()[..2], ["reset-failed", "start"]);
    // A unit that runs yet is not answered is still a reason to show, never a silent success.
    let inspected = run(Job::Inspect).failure.unwrap();
    assert_eq!(inspected.kind, FailureKind::CouldNotStart);
    assert!(
        inspected.details.contains("did not answer"),
        "{inspected:?}"
    );
    fake.show("ActiveState=failed\nResult=start-limit-hit\n");
    assert_eq!(
        run(Job::Inspect).failure.unwrap().kind,
        FailureKind::StoppedByError
    );
    fake.answer("start", 1, "", "boom");
    let refused = run(Job::Start).failure.unwrap();
    assert_eq!(refused.kind, FailureKind::StoppedByError);
    // Without a usable home or editor path the failure is reported, not a panic.
    let mut broken = worker::<Fake>(
        Err(Failure {
            kind: FailureKind::CouldNotStart,
            details: "x".into(),
        }),
        std::time::Duration::ZERO,
    );
    broken.submit(Job::Start).unwrap();
    assert_eq!(
        broken.receive().ready().unwrap().failure.unwrap().details,
        "x"
    );
}

#[test]
fn the_messages_of_both_failures_exist_in_every_language_in_plain_words() {
    for (kind, en, es) in [
        (
            FailureKind::StoppedByError,
            "RAD stopped because of an error",
            "RAD se detuvo por un error",
        ),
        (
            FailureKind::CouldNotStart,
            "RAD could not start",
            "RAD no pudo arrancar",
        ),
    ] {
        assert_eq!(rad_editor::texts::text("en", kind.message()), en);
        assert_eq!(rad_editor::texts::text("es", kind.message()), es);
    }
}
