//! Desktop contracts on a private bus, never the user's session or service manager.
use gio::glib::{self, variant::ToVariant};
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const TRAY: &str = "io.github.KroxiDev.RAD.Tray";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_XML: &str = r#"<node><interface name="org.kde.StatusNotifierWatcher"><method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method></interface></node>"#;
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn connection(address: &str) -> gio::DBusConnection {
    gio::DBusConnection::for_address_sync(
        address,
        gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
        gio::Cancellable::NONE,
    )
    .unwrap()
}
struct Bus {
    _process: Process,
    address: String,
    client: gio::DBusConnection,
}
impl Bus {
    fn new() -> Self {
        let runtime = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("tray-bus-{}", std::process::id()));
        std::fs::create_dir_all(&runtime).unwrap();
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .arg(format!(
                "--address=unix:path={}",
                runtime.join("bus").display()
            ))
            .env("XDG_RUNTIME_DIR", &runtime)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        let client = connection(&address);
        Self {
            _process: Process(child),
            address,
            client,
        }
    }
    fn has_name(&self, name: &str) -> bool {
        self.client
            .call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameHasOwner",
                Some(&(name,).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                1000,
                gio::Cancellable::NONE,
            )
            .unwrap()
            .get::<(bool,)>()
            .unwrap()
            .0
    }
    fn call(
        &self,
        path: &str,
        interface: &str,
        method: &str,
        parameters: &glib::Variant,
    ) -> glib::Variant {
        self.client
            .call_sync(
                Some(TRAY),
                path,
                interface,
                method,
                Some(parameters),
                None,
                gio::DBusCallFlags::NONE,
                2000,
                gio::Cancellable::NONE,
            )
            .unwrap()
    }
}
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "desktop condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct Watcher {
    context: glib::MainContext,
    loop_: glib::MainLoop,
    thread: Option<std::thread::JoinHandle<()>>,
    registrations: mpsc::Receiver<String>,
}
impl Watcher {
    fn new(address: String) -> Self {
        let context = glib::MainContext::new();
        let loop_ = glib::MainLoop::new(Some(&context), false);
        let (tx, registrations) = mpsc::channel();
        let (ready_tx, ready) = mpsc::channel();
        let c = context.clone();
        let l = loop_.clone();
        let thread = std::thread::spawn(move || {
            c.with_thread_default(|| {
                let bus = connection(&address);
                let node = gio::DBusNodeInfo::for_xml(WATCHER_XML).unwrap();
                let _registration = bus
                    .register_object("/StatusNotifierWatcher", &node.interfaces()[0])
                    .method_call(move |_, _, _, _, _, params, invocation| {
                        tx.send(params.get::<(String,)>().unwrap().0).unwrap();
                        invocation.return_value(None);
                    })
                    .build()
                    .unwrap();
                bus.call_sync(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "RequestName",
                    Some(&(WATCHER, 4u32).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    1000,
                    gio::Cancellable::NONE,
                )
                .unwrap();
                ready_tx.send(()).unwrap();
                l.run();
                bus.close_sync(gio::Cancellable::NONE).unwrap();
            })
            .unwrap();
        });
        ready.recv_timeout(Duration::from_secs(3)).unwrap();
        Self {
            context,
            loop_,
            thread: Some(thread),
            registrations,
        }
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        let l = self.loop_.clone();
        self.context.invoke(move || l.quit());
        self.thread.take().unwrap().join().unwrap();
    }
}

fn executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
#[test]
fn private_tray_registers_late_reappears_opens_once_and_requests_clean_stop() {
    let bus = Bus::new();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("private-tray");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_rad-tray"), dir.join("rad-tray")).unwrap();
    let log = dir.join("calls");
    std::fs::write(&log, "").unwrap();
    // The launcher stays alive until its editor exits, exactly like systemd-run --scope.
    executable(
        &dir.join("systemd-run"),
        "#!/bin/sh\nprintf 'launch\\n' >> \"$RAD_FIXTURE_LOG\"\nwhile [ \"$1\" != -- ]; do shift; done\nshift\nexec \"$@\"\n",
    );
    executable(
        &dir.join("systemctl"),
        "#!/bin/sh\nprintf 'systemctl %s\\n' \"$*\" >> \"$RAD_FIXTURE_LOG\"\n",
    );
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release/examples/desktop_fixture");
    assert!(
        fixture.is_file(),
        "build rad-tray --example desktop_fixture before this test"
    );
    std::fs::copy(fixture, dir.join("rad-editor")).unwrap();
    let mut tray = Process(
        Command::new(dir.join("rad-tray"))
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .env("RAD_FIXTURE_LOG", &log)
            .env("LC_ALL", "")
            .env("LC_MESSAGES", "es")
            .env("LANG", "C")
            .env("PATH", format!("{}:/usr/bin:/bin", dir.display()))
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    );
    until(|| {
        assert!(
            tray.0.try_wait().unwrap().is_none(),
            "tray exited before registering"
        );
        bus.has_name(TRAY)
    });
    let watcher = Watcher::new(bus.address.clone());
    let registered = watcher
        .registrations
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    assert!(!registered.is_empty());
    drop(watcher);
    until(|| !bus.has_name(WATCHER));
    let watcher = Watcher::new(bus.address.clone());
    watcher
        .registrations
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    for _ in 0..2 {
        bus.call(
            "/StatusNotifierItem",
            "org.kde.StatusNotifierItem",
            "Activate",
            &(0i32, 0i32).to_variant(),
        );
    }
    until(|| {
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("editor-start")
    });
    bus.call(
        "/StatusNotifierItem",
        "org.kde.StatusNotifierItem",
        "Activate",
        &(0i32, 0i32).to_variant(),
    );
    until(|| {
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("editor-activate")
    });
    // A second executable invocation also activates the existing owner rather than running another.
    let status = Command::new(dir.join("rad-editor"))
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("RAD_FIXTURE_LOG", &log)
        .status()
        .unwrap();
    assert!(status.success());
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls.lines().filter(|l| *l == "launch").count(),
        1,
        "{calls}"
    );
    assert_eq!(
        calls.lines().filter(|l| *l == "editor-start").count(),
        1,
        "{calls}"
    );
    let layout = bus.call(
        "/MenuBar",
        "com.canonical.dbusmenu",
        "GetLayout",
        &(0i32, -1i32, Vec::<String>::new()).to_variant(),
    );
    assert!(layout.print(true).contains("Abrir RAD"));
    assert!(layout.print(true).contains("Salir de RAD"));
    bus.call(
        "/MenuBar",
        "com.canonical.dbusmenu",
        "Event",
        &(2i32, "clicked", 0i32.to_variant(), 0u32).to_variant(),
    );
    until(|| {
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("systemctl --user stop rad-runtime.service")
    });
    // Simulate the unit's stop signal without touching the user's systemd.
    Command::new("kill")
        .args(["-TERM", &tray.0.id().to_string()])
        .status()
        .unwrap();
    until(|| tray.0.try_wait().unwrap().is_some());
    until(|| !bus.has_name(TRAY));
    let calls = std::fs::read_to_string(&log).unwrap();
    let editor_pid = calls
        .lines()
        .find_map(|l| l.strip_prefix("editor-pid "))
        .unwrap();
    Command::new("kill")
        .args(["-TERM", editor_pid])
        .status()
        .unwrap();
}

#[test]
fn systemd_binds_tray_to_runtime_without_turning_it_into_the_authority() {
    let runtime = include_str!("../../../packaging/rad-runtime.service");
    let tray_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/rad-tray.service");
    let tray = std::fs::read_to_string(tray_path).expect("the auxiliary tray unit");
    assert!(runtime.lines().any(|l| l == "Wants=rad-tray.service"));
    for directive in [
        "BindsTo=rad-runtime.service",
        "PartOf=rad-runtime.service",
        "After=rad-runtime.service",
    ] {
        assert!(tray.lines().any(|l| l == directive), "{directive}");
    }
    assert!(runtime.lines().any(|l| l == "Restart=on-failure"));
    assert!(
        !tray
            .lines()
            .any(|l| l.starts_with("Restart=") && l != "Restart=no")
    );
}
