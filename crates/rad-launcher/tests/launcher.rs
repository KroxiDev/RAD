use rad_launcher::*;
#[test]
fn shared_clock_is_linux_monotonic_epoch_not_process_elapsed_time() {
    let before = monotonic_ms().unwrap();
    let mut ts = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: writable timespec and valid Linux clock identifier.
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, ts.as_mut_ptr()) },
        0
    );
    // SAFETY: successful clock_gettime initialized the structure.
    let ts = unsafe { ts.assume_init() };
    let os = ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000;
    let after = monotonic_ms().unwrap();
    assert!(before <= os && os <= after);
}
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
#[test]
fn permanent_helper_publishes_only_private_xdg_catalog() {
    let f = Fixture::new();
    f.desktop("org.rad.One.desktop", "One", "");
    let mut launcher = Launcher::spawn(f.command()).unwrap();
    until(|| {
        launcher.pump().unwrap();
        launcher.catalog().is_some()
    });
    let entries = launcher.catalog().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id.id(), "org.rad.One.desktop");
    assert_eq!(entries[0].name, "One");
    stop(&mut launcher);
}
fn intent(entry: &str) -> rad_core::execution::DispatchIntent {
    use rad_core::{
        configuration::*,
        execution::{Engine, Target, Trigger},
        identity::*,
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
#[test]
fn catalog_over_4096_is_explicit_capacity_not_truncated_success() {
    let f = Fixture::new();
    for i in 0..4097 {
        f.desktop(&format!("org.rad.{i}.desktop"), "fixture", "");
    }
    let mut l = Launcher::spawn(f.command()).unwrap();
    let mut failure = None;
    until(|| {
        failure = l.pump().err();
        failure.is_some() || l.catalog().is_some()
    });
    assert_eq!(failure, Some(LauncherError::Capacity));
    assert!(l.catalog().is_none());
    stop(&mut l);
}
#[test]
fn helper_death_reap_completes_original_once_without_claiming_no_effect() {
    let f = Fixture::new();
    f.desktop("org.rad.One.desktop", "One", "");
    let mut l = Launcher::spawn(f.command()).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let job = intent("org.rad.One.desktop");
    l.submit(&job).unwrap();
    l.shutdown().unwrap();
    let mut completion = None;
    until(|| {
        let _ = l.pump();
        completion = l.take_completion();
        completion.is_some()
    });
    assert_eq!(completion, Some((job.id, LaunchOutcome::HelperLost)));
    assert_eq!(l.outstanding(), 0);
    assert!(l.take_completion().is_none());
    assert!(l.process_id().is_none());
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
#[test]
fn live_helper_cannot_be_replaced_before_actual_reap() {
    let f = Fixture::new();
    let mut l = Launcher::spawn(f.command()).unwrap();
    assert!(matches!(
        Launcher::spawn(f.command()),
        Err(LauncherError::Unavailable)
    ));
    stop(&mut l);
    let mut next = Launcher::spawn(f.command()).unwrap();
    stop(&mut next);
}
#[test]
fn catalogue_uses_ids_xdg_precedence_and_visibility_not_names() {
    let f = Fixture::new();
    f.desktop("same.desktop", "Higher", "");
    fs::copy(
        f.0.join("data/applications/same.desktop"),
        f.0.join("system/applications/same.desktop"),
    )
    .unwrap();
    let lower = fs::read_to_string(f.0.join("system/applications/same.desktop"))
        .unwrap()
        .replace("Higher", "Lower");
    fs::write(f.0.join("system/applications/same.desktop"), lower).unwrap();
    f.desktop("hidden.desktop", "Hidden", "Hidden=true");
    f.desktop("nodisplay.desktop", "NoDisplay", "NoDisplay=true");
    f.desktop("only.desktop", "Only", "OnlyShowIn=Other;");
    f.desktop("shown.desktop", "Shown", "OnlyShowIn=RADTest;");
    fs::create_dir(f.0.join("data/applications/sub")).unwrap();
    f.desktop("sub/app.desktop", "Nested", "");
    let mut l = Launcher::spawn(f.command()).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let entries = l.catalog().unwrap();
    assert_eq!(entries.len(), 3);
    assert!(
        entries
            .iter()
            .any(|e| e.id.id() == "same.desktop" && e.name == "Higher")
    );
    assert!(entries.iter().any(|e| e.id.id() == "sub-app.desktop"));
    assert!(entries.iter().any(|e| e.id.id() == "shown.desktop"));
    stop(&mut l);
}
#[test]
fn native_terminal_uses_private_terminal_fixture() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let output = f.0.join("terminal.json");
    let app = env!("CARGO_BIN_EXE_rad-launcher-fixture");
    symlink(app, f.0.join("bin/xdg-terminal-exec")).unwrap();
    fs::write(f.0.join("data/applications/terminal.desktop"),format!("[Desktop Entry]\nType=Application\nName=Terminal\nExec={app} \"one argument\"\nTerminal=true\n")).unwrap();
    let mut command = f.command();
    command.env("RAD_FIXTURE_OUTPUT", &output);
    let mut l = Launcher::spawn(command).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let job = intent("terminal.desktop");
    l.submit(&job).unwrap();
    let mut outcome = None;
    until(|| {
        l.pump().unwrap();
        outcome = l.take_completion();
        outcome.is_some()
    });
    assert_eq!(outcome, Some((job.id, LaunchOutcome::Accepted)));
    until(|| output.exists());
    let args: Vec<String> = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(args, vec![app, "one argument"]);
    stop(&mut l);
}
#[test]
fn expired_original_deadline_never_launches_and_disappeared_id_never_substitutes() {
    let f = Fixture::new();
    let output = f.0.join("args.json");
    let app = env!("CARGO_BIN_EXE_rad-launcher-fixture");
    fs::write(
        f.0.join("data/applications/one.desktop"),
        format!("[Desktop Entry]\nType=Application\nName=SameName\nExec={app}\n"),
    )
    .unwrap();
    let mut command = f.command();
    command.env("RAD_FIXTURE_OUTPUT", &output);
    let mut l = Launcher::spawn(command).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let mut job = intent("one.desktop");
    job.deadline_ms = monotonic_ms().unwrap();
    l.submit(&job).unwrap();
    let mut result = None;
    until(|| {
        l.pump().unwrap();
        result = l.take_completion();
        result.is_some()
    });
    assert_eq!(result, Some((job.id, LaunchOutcome::Expired)));
    assert!(!output.exists());
    fs::rename(
        f.0.join("data/applications/one.desktop"),
        f.0.join("data/applications/another.desktop"),
    )
    .unwrap();
    let job = intent("one.desktop");
    l.submit(&job).unwrap();
    until(|| {
        l.pump().unwrap();
        result = l.take_completion();
        result.is_some()
    });
    assert_eq!(result, Some((job.id, LaunchOutcome::Failed)));
    assert!(!output.exists());
    stop(&mut l);
}
struct Bus(std::process::Child);
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn native_dbus_activates_without_exec_and_acceptance_is_not_readiness() {
    use std::io::BufRead;
    let f = Fixture::new();
    let activated = f.0.join("activated");
    let gate = f.0.join("gate");
    let ready = f.0.join("ready");
    let app = env!("CARGO_BIN_EXE_rad-launcher-fixture");
    let services = f.0.join("services");
    fs::create_dir(&services).unwrap();
    fs::write(
        services.join("org.rad.Bus.service"),
        format!("[D-BUS Service]\nName=org.rad.Bus\nExec={app} --dbus\n"),
    )
    .unwrap();
    let buspath = f.0.join("bus");
    let config = f.0.join("bus.conf");
    fs::write(&config,format!("<busconfig><type>session</type><listen>unix:path={}</listen><auth>EXTERNAL</auth><servicedir>{}</servicedir><policy context=\"default\"><allow own=\"*\"/><allow send_destination=\"*\"/><allow receive_sender=\"*\"/></policy></busconfig>",buspath.display(),services.display())).unwrap();
    let mut command = f.command_for("/usr/bin/dbus-daemon");
    command
        .arg("--nofork")
        .arg(format!("--config-file={}", config.display()))
        .arg("--print-address=1")
        .env("RAD_FIXTURE_OUTPUT", &activated)
        .env("RAD_FIXTURE_GATE", &gate)
        .env("RAD_FIXTURE_READY", &ready)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut bus = Bus(command.spawn().unwrap());
    let mut address = String::new();
    std::io::BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    fs::write(f.0.join("data/applications/org.rad.Bus.desktop"),format!("[Desktop Entry]\nType=Application\nName=DBusFixture\nDBusActivatable=true\nExec={app} exec-must-not-run\n")).unwrap();
    let mut command = f.command();
    command
        .env("DBUS_SESSION_BUS_ADDRESS", address.trim())
        .env("RAD_FIXTURE_OUTPUT", f.0.join("wrong-exec"));
    let mut l = Launcher::spawn(command).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let job = intent("org.rad.Bus.desktop");
    l.submit(&job).unwrap();
    let mut outcome = None;
    until(|| {
        l.pump().unwrap();
        outcome = l.take_completion();
        outcome.is_some()
    });
    assert_eq!(outcome, Some((job.id, LaunchOutcome::Accepted)));
    assert!(activated.exists());
    assert!(!ready.exists());
    assert!(!f.0.join("wrong-exec").exists());
    stop(&mut l);
    fs::write(gate, b"go").unwrap();
    until(|| ready.exists());
    // Explicit native control: same D-Bus-capable ID, false must use Exec.
    fs::write(f.0.join("data/applications/org.rad.Bus.desktop"), format!("[Desktop Entry]\nType=Application\nName=DBusFixture\nDBusActivatable=false\nExec={app} exec-control\n")).unwrap();
    let output = f.0.join("exec-control");
    let mut command = f.command();
    command
        .env("DBUS_SESSION_BUS_ADDRESS", address.trim())
        .env("RAD_FIXTURE_OUTPUT", &output);
    let mut l = Launcher::spawn(command).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let job = intent("org.rad.Bus.desktop");
    l.submit(&job).unwrap();
    let mut outcome = None;
    until(|| {
        l.pump().unwrap();
        outcome = l.take_completion();
        outcome.is_some()
    });
    assert_eq!(outcome, Some((job.id, LaunchOutcome::Accepted)));
    until(|| output.exists());
    assert_eq!(
        serde_json::from_slice::<Vec<String>>(&fs::read(output).unwrap()).unwrap(),
        vec!["exec-control"]
    );
    stop(&mut l);
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
#[test]
fn refresh_is_explicit_and_publishes_a_complete_replacement() {
    let f = Fixture::new();
    f.desktop("one.desktop", "Before", "");
    let mut l = Launcher::spawn(f.command()).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    f.desktop("one.desktop", "After", "");
    l.request_refresh().unwrap();
    assert!(l.catalog_stale());
    assert_eq!(l.catalog().unwrap()[0].name, "Before");
    until(|| {
        l.pump().unwrap();
        !l.catalog_stale()
    });
    assert_eq!(l.catalog().unwrap()[0].name, "After");
    stop(&mut l);
}
#[test]
fn file_icon_is_normalized_in_helper_and_returned_without_launching() {
    let f = Fixture::new();
    let icon = f.0.join("icon.png");
    fs::write(&icon, include_bytes!("../assets/generic.png")).unwrap();
    f.desktop("one.desktop", "One", &format!("Icon={}", icon.display()));
    let mut l = Launcher::spawn(f.command()).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let id = rad_core::configuration::DesktopEntry::new("one.desktop".into()).unwrap();
    l.request_icon(&id, "hicolor").unwrap();
    let mut preview = None;
    until(|| {
        l.pump().unwrap();
        preview = l.take_icon();
        preview.is_some()
    });
    let preview = preview.unwrap();
    assert_eq!(preview.id, id);
    assert!(!preview.generic);
    assert_eq!(
        preview.image,
        rad_media::normalize(include_bytes!("../assets/generic.png")).unwrap()
    );
    assert_eq!(l.outstanding(), 0);
    stop(&mut l);
}
#[test]
fn themed_icon_resolves_inheritance_and_hicolor_without_svg_decode() {
    let f = Fixture::new();
    let icons = f.0.join("data/icons");
    for theme in ["Child", "Parent", "hicolor"] {
        fs::create_dir_all(icons.join(theme).join("32x32/apps")).unwrap();
        fs::write(icons.join(theme).join("index.theme"),format!("[Icon Theme]\nName={theme}\nDirectories=32x32/apps\n{}\n[32x32/apps]\nSize=32\nType=Fixed\nContext=Applications\n",if theme=="Child"{"Inherits=Parent"}else{""})).unwrap();
    }
    fs::write(
        icons.join("Parent/32x32/apps/inherited.png"),
        include_bytes!("../assets/generic.png"),
    )
    .unwrap();
    f.desktop("one.desktop", "One", "Icon=inherited");
    let mut l = Launcher::spawn(f.command()).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let id = rad_core::configuration::DesktopEntry::new("one.desktop".into()).unwrap();
    l.request_icon(&id, "Child").unwrap();
    let mut icon = None;
    until(|| {
        l.pump().unwrap();
        icon = l.take_icon();
        icon.is_some()
    });
    assert!(!icon.unwrap().generic);
    stop(&mut l);
}
fn icon(l: &mut Launcher, id: &rad_core::configuration::DesktopEntry, theme: &str) -> IconPreview {
    l.request_icon(id, theme).unwrap();
    let mut result = None;
    until(|| {
        l.pump().unwrap();
        result = l.take_icon();
        result.is_some()
    });
    result.unwrap()
}
#[test]
fn icon_cache_survives_source_loss_until_explicit_catalog_refresh() {
    let f = Fixture::new();
    let path = f.0.join("icon.png");
    fs::write(&path, include_bytes!("../assets/generic.png")).unwrap();
    f.desktop("one.desktop", "One", &format!("Icon={}", path.display()));
    let mut l = Launcher::spawn(f.command()).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let id = rad_core::configuration::DesktopEntry::new("one.desktop".into()).unwrap();
    assert!(!icon(&mut l, &id, "hicolor").generic);
    fs::remove_file(path).unwrap();
    assert!(!icon(&mut l, &id, "hicolor").generic);
    l.request_refresh().unwrap();
    until(|| {
        l.pump().unwrap();
        !l.catalog_stale()
    });
    assert!(icon(&mut l, &id, "hicolor").generic);
    stop(&mut l);
}
#[test]
fn jpeg_hicolor_and_generic_fallback_are_bounded_and_never_prevent_launch() {
    let f = Fixture::new();
    let icons = f.0.join("data/icons/hicolor/256x256/apps");
    fs::create_dir_all(&icons).unwrap();
    fs::write(f.0.join("data/icons/hicolor/index.theme"),"[Icon Theme]\nName=Hicolor\nDirectories=256x256/apps\n[256x256/apps]\nSize=256\nType=Fixed\n").unwrap();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode(
            &[20, 60, 90, 30, 80, 120],
            2,
            1,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    fs::write(icons.join("themed.jpg"), jpeg).unwrap();
    f.desktop("jpeg.desktop", "JPEG", "Icon=themed");
    let svg = f.0.join("bad.svg");
    fs::write(&svg, b"<svg/>").unwrap();
    f.desktop("svg.desktop", "SVG", &format!("Icon={}", svg.display()));
    let corrupt = f.0.join("bad.png");
    fs::write(&corrupt, b"not png").unwrap();
    f.desktop("bad.desktop", "Bad", &format!("Icon={}", corrupt.display()));
    f.desktop("missing.desktop", "Missing", "Icon=missing");
    let mut l = Launcher::spawn(f.command()).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let id = |s: &str| rad_core::configuration::DesktopEntry::new(s.into()).unwrap();
    let preview = icon(&mut l, &id("jpeg.desktop"), "AbsentTheme");
    assert!(!preview.generic);
    assert_eq!(preview.image.dimensions(), (2, 1));
    for entry in ["svg.desktop", "bad.desktop", "missing.desktop"] {
        let p = icon(&mut l, &id(entry), "hicolor");
        assert!(p.generic);
        assert!(p.image.png_bytes().len() <= 512 * 1024);
    }
    let job = intent("svg.desktop");
    l.submit(&job).unwrap();
    let mut result = None;
    until(|| {
        l.pump().unwrap();
        result = l.take_completion();
        result.is_some()
    });
    assert_eq!(result, Some((job.id, LaunchOutcome::Accepted)));
    stop(&mut l);
}
#[test]
fn helper_overdue_is_unavailable_without_releasing_residual_slot() {
    let f = Fixture::new();
    f.desktop("one.desktop", "One", "");
    let mut l = Launcher::spawn(f.command()).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let pid = l.process_id().unwrap();
    // SAFETY: only our unreaped fixture helper.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGSTOP) }, 0);
    let mut job = intent("one.desktop");
    job.deadline_ms = monotonic_ms().unwrap() + 30;
    l.submit(&job).unwrap();
    l.pump().unwrap();
    until(|| monotonic_ms().unwrap() >= job.deadline_ms);
    l.pump().unwrap();
    assert!(!l.is_available());
    assert_eq!(
        l.submit(&intent("one.desktop")),
        Err(LauncherError::Unavailable)
    );
    assert_eq!(l.outstanding(), 1);
    // SAFETY: resume same owned child.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGCONT) }, 0);
    let mut completion = None;
    until(|| {
        l.pump().unwrap();
        completion = l.take_completion();
        completion.is_some()
    });
    assert_eq!(completion, Some((job.id, LaunchOutcome::Expired)));
    assert!(l.is_available());
    stop(&mut l);
}
#[test]
fn large_icon_chunks_are_bounded_and_job_completion_remains_reserved() {
    use image::ImageEncoder;
    let f = Fixture::new();
    let path = f.0.join("noise.png");
    let mut seed = 1u32;
    let pixels: Vec<_> = (0..512 * 512 * 4)
        .map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 24) as u8
        })
        .collect();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&pixels, 512, 512, image::ExtendedColorType::Rgba8)
        .unwrap();
    fs::write(&path, &png).unwrap();
    f.desktop("one.desktop", "One", &format!("Icon={}", path.display()));
    let mut l = Launcher::spawn(f.command()).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    let id = rad_core::configuration::DesktopEntry::new("one.desktop".into()).unwrap();
    l.request_icon(&id, "hicolor").unwrap();
    assert_eq!(l.request_icon(&id, "hicolor"), Err(LauncherError::Capacity));
    let job = intent("one.desktop");
    l.submit(&job).unwrap();
    let mut completion = None;
    let mut preview = None;
    until(|| {
        l.pump().unwrap();
        if completion.is_none() {
            completion = l.take_completion();
        }
        if preview.is_none() {
            preview = l.take_icon();
        }
        completion.is_some() && preview.is_some()
    });
    assert_eq!(completion, Some((job.id, LaunchOutcome::Accepted)));
    let preview = preview.unwrap();
    assert!(!preview.generic);
    assert_eq!(preview.image.dimensions(), (256, 256));
    assert!(preview.image.png_bytes().len() > 4096);
    assert!(preview.image.png_bytes().len() <= 512 * 1024);
    assert_eq!(fs::read(path).unwrap(), png);
    stop(&mut l);
}
#[test]
fn catalog_accepts_inclusive_4096_entries() {
    let f = Fixture::new();
    for i in 0..4096 {
        f.desktop(&format!("org.rad.{i}.desktop"), "Bound fixture", "");
    }
    let mut l = Launcher::spawn(f.command()).unwrap();
    until(|| {
        l.pump().unwrap();
        l.catalog().is_some()
    });
    assert_eq!(l.catalog().unwrap().len(), 4096);
    assert!(l.is_available());
    stop(&mut l);
}
