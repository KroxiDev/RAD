use rad_launcher::*;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
struct Fixture(
    PathBuf,
    #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
);
impl Fixture {
    fn new() -> Self {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        until(Launcher::can_spawn);
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "rad-launcher-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        for d in [
            "home",
            "data/applications",
            "system/applications",
            "config",
            "cache",
            "runtime",
            "bin",
        ] {
            fs::create_dir_all(p.join(d)).unwrap();
        }
        Self(p, guard)
    }
    fn command(&self) -> Command {
        self.command_for(env!("CARGO_BIN_EXE_rad-launcher-helper"))
    }
    fn command_for(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.env_clear()
            .env("HOME", self.0.join("home"))
            .env("XDG_DATA_HOME", self.0.join("data"))
            .env("XDG_DATA_DIRS", self.0.join("system"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_CONFIG_DIRS", self.0.join("config"))
            .env("XDG_CACHE_HOME", self.0.join("cache"))
            .env("XDG_RUNTIME_DIR", self.0.join("runtime"))
            .env("XDG_CURRENT_DESKTOP", "RADTest")
            .env("PATH", self.0.join("bin"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", self.0.join("no-bus").display()),
            );
        c
    }
    fn desktop(&self, id: &str, name: &str, extra: &str) {
        fs::write(
            self.0.join("data/applications").join(id),
            format!(
                "[Desktop Entry]\nType=Application\nName={name}\nExec={}\n{extra}\n",
                env!("CARGO_BIN_EXE_rad-launcher-helper")
            ),
        )
        .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn until(mut predicate: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < end, "fixture deadline");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn intent(entry: &str) -> rad_core::execution::DispatchIntent {
    use rad_core::{
        configuration::{ActionKind, Configuration, DesktopEntry, Edit, Origin},
        execution::{Engine, Target, Trigger},
        identity::IdentityRegistry,
    };
    let c = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder: c.configuration().data().principal,
                position: 0,
                name: "fixture".into(),
            },
        )
        .unwrap();
    let b = c.configuration().data().buttons[0].binding.identity();
    let c = c
        .configuration()
        .prepare_edit(
            c.registry(),
            Origin::Editor,
            Edit::AddAction {
                button: b,
                kind: ActionKind::Launcher(DesktopEntry::new(entry.into()).unwrap()),
            },
        )
        .unwrap();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let mut e = Engine::new(1, NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)).unwrap();
    e.advance(monotonic_ms().unwrap()).unwrap();
    e.admit(c.configuration(), Target::Button(b), Trigger::Editor, &[])
        .unwrap()
        .dispatches
        .remove(0)
}
#[test]
fn native_exec_preserves_literal_arguments_and_finishes_via_live_context() {
    let f = Fixture::new();
    let output = f.0.join("args.json");
    let app = env!("CARGO_BIN_EXE_rad-launcher-fixture");
    fs::write(f.0.join("data/applications/org.rad.Exec.desktop"),format!("[Desktop Entry]\nType=Application\nName=Fixture With Spaces\nExec=\"{app}\" \"space value\" semi;colon %c %%\nTerminal=false\n")).unwrap();
    let mut command = f.command();
    command.env("RAD_FIXTURE_OUTPUT", &output);
    let mut l = Launcher::spawn(command).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let job = intent("org.rad.Exec.desktop");
    l.submit(&job).unwrap();
    let mut completion = None;
    until(|| {
        l.pump().unwrap();
        completion = l.take_completion();
        completion.is_some()
    });
    assert_eq!(completion, Some((job.id, LaunchOutcome::Accepted)));
    until(|| output.exists());
    let args: Vec<String> = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(
        args,
        vec!["space value", "semi;colon", "Fixture With Spaces", "%"]
    );
    stop(&mut l);
}

fn stop(l: &mut Launcher) {
    if l.process_id().is_some() {
        l.shutdown().unwrap();
        until(|| {
            let _ = l.pump();
            l.process_id().is_none()
        });
    }
}

struct Bus(std::process::Child);
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn bounded_32_jobs_keep_residuals_while_helper_is_stopped_and_never_retry() {
    let f = Fixture::new();
    f.desktop("one.desktop", "One", "");
    let mut l = Launcher::spawn(f.command()).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let pid = l.process_id().unwrap();
    // SAFETY: signal only our retained, unreaped helper child PID.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGSTOP) }, 0);
    let deadline = monotonic_ms().unwrap() + 30;
    let mut originals = std::collections::BTreeSet::new();
    for _ in 0..32 {
        let mut job = intent("one.desktop");
        job.deadline_ms = deadline;
        originals.insert(job.id);
        l.submit(&job).unwrap();
    }
    assert_eq!(
        l.submit(&intent("one.desktop")),
        Err(LauncherError::Capacity)
    );
    l.pump().unwrap();
    until(|| monotonic_ms().unwrap() >= deadline);
    l.pump().unwrap();
    assert_eq!(l.outstanding(), 32);
    assert!(l.take_completion().is_none());
    // SAFETY: resume only the same owned child; no other process/session touched.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGCONT) }, 0);
    until(|| {
        l.pump().unwrap();
        while let Some((id, outcome)) = l.take_completion() {
            assert!(originals.remove(&id));
            assert_eq!(outcome, LaunchOutcome::Expired);
        }
        originals.is_empty()
    });
    assert_eq!(l.outstanding(), 0);
    assert!(l.take_completion().is_none());
    stop(&mut l);
}

// --- Per-application systemd scope (ADR 0013) ---
/// Private session bus whose activatable services are `(name, exec)`.
fn private_bus(
    f: &Fixture,
    services: &[(&str, String)],
    envs: &[(&str, &PathBuf)],
) -> (Bus, String) {
    use std::io::BufRead;
    let dir = f.0.join("bus-services");
    fs::create_dir(&dir).unwrap();
    for (name, exec) in services {
        fs::write(
            dir.join(format!("{name}.service")),
            format!("[D-BUS Service]\nName={name}\nExec={exec}\n"),
        )
        .unwrap();
    }
    let config = f.0.join("bus.conf");
    fs::write(&config,format!("<busconfig><type>session</type><listen>unix:path={}</listen><auth>EXTERNAL</auth><servicedir>{}</servicedir><policy context=\"default\"><allow own=\"*\"/><allow send_destination=\"*\"/><allow receive_sender=\"*\"/></policy></busconfig>",f.0.join("bus").display(),dir.display())).unwrap();
    let mut command = f.command_for("/usr/bin/dbus-daemon");
    command
        .arg("--nofork")
        .arg(format!("--config-file={}", config.display()))
        .arg("--print-address=1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    for (k, v) in envs {
        command.env(k, v);
    }
    let mut bus = Bus(command.spawn().unwrap());
    let mut address = String::new();
    std::io::BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    (bus, address.trim().to_string())
}
fn launch_fixture_app(
    f: &Fixture,
    address: &str,
    stderr: fs::File,
) -> (Launcher, PathBuf, PathBuf) {
    let app = env!("CARGO_BIN_EXE_rad-launcher-fixture");
    let output = f.0.join("app-output");
    let pid = f.0.join("app-pid");
    fs::write(
        f.0.join("data/applications/scoped.app.desktop"),
        format!("[Desktop Entry]\nType=Application\nName=Scoped\nExec={app} one\n"),
    )
    .unwrap();
    let mut command = f.command();
    command
        .env("DBUS_SESSION_BUS_ADDRESS", address)
        .env("RAD_FIXTURE_OUTPUT", &output)
        .env("RAD_FIXTURE_PID", &pid)
        .env("RAD_FIXTURE_STDERR", "1")
        .stderr(stderr);
    let mut l = Launcher::spawn(command).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let job = intent("scoped.app.desktop");
    l.submit(&job).unwrap();
    let mut outcome = None;
    until(|| {
        l.pump().unwrap();
        outcome = l.take_completion();
        outcome.is_some()
    });
    assert_eq!(outcome, Some((job.id, LaunchOutcome::Accepted)));
    until(|| output.exists() && pid.exists());
    (l, output, pid)
}
#[test]
fn launched_app_is_moved_to_its_own_transient_scope() {
    let f = Fixture::new();
    let log = f.0.join("systemd-log");
    let (_bus, address) = private_bus(
        &f,
        &[(
            "org.freedesktop.systemd1",
            env!("CARGO_BIN_EXE_rad-launcher-fake-systemd").into(),
        )],
        &[("RAD_FAKE_SYSTEMD_LOG", &log)],
    );
    let stderr = fs::File::create(f.0.join("helper-stderr")).unwrap();
    let (mut l, _, pid) = launch_fixture_app(&f, &address, stderr);
    let pid: u32 = fs::read_to_string(pid).unwrap().trim().parse().unwrap();
    until(|| fs::read_to_string(&log).is_ok_and(|s| s.ends_with('\n')));
    let call: serde_json::Value = serde_json::from_str(&fs::read_to_string(&log).unwrap()).unwrap();
    assert_eq!(call["name"], format!("app-rad-scoped.app-{pid}.scope"));
    assert_eq!(call["mode"], "fail");
    assert_eq!(call["pids"], serde_json::json!([pid]));
    assert_eq!(call["aux"], 0);
    stop(&mut l);
    assert_eq!(fs::read_to_string(f.0.join("helper-stderr")).unwrap(), "");
}
