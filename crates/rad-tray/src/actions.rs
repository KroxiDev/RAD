//! A bounded worker owns process launches and blocking calls, never the desktop loop.
use gio::glib::variant::ToVariant;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
const EDITOR: &str = "io.github.KroxiDev.RAD";
#[derive(Clone, Copy)]
pub enum Action {
    Open,
    Stop,
}
#[derive(Clone)]
pub struct Actions {
    sender: mpsc::SyncSender<Action>,
    open: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}
impl Actions {
    pub fn new(bus: gio::DBusConnection, editor: PathBuf) -> Self {
        let (sender, receiver) = mpsc::sync_channel(2);
        let open = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let o = open.clone();
        let s = stop.clone();
        std::thread::Builder::new()
            .name("rad-tray-actions".into())
            .spawn(move || {
                let mut launch: Option<Child> = None;
                loop {
                    if launch.as_mut().is_some_and(|child| match child.try_wait() {
                        Ok(Some(status)) => {
                            if !status.success() {
                                eprintln!("RAD editor launcher: {status}");
                            }
                            true
                        }
                        Ok(None) => false,
                        Err(error) => {
                            eprintln!("RAD editor launcher wait: {error}");
                            false
                        }
                    }) {
                        launch = None;
                    }
                    match receiver.recv_timeout(Duration::from_millis(100)) {
                        Ok(Action::Open) => {
                            open_editor(&bus, &editor, &mut launch);
                            o.store(false, Ordering::Release);
                        }
                        Ok(Action::Stop) => {
                            stop_runtime();
                            s.store(false, Ordering::Release);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("tray action worker");
        Self { sender, open, stop }
    }
    pub fn request(&self, action: Action) {
        let pending = match action {
            Action::Open => &self.open,
            Action::Stop => &self.stop,
        };
        if pending.swap(true, Ordering::AcqRel) {
            return;
        }
        if self.sender.try_send(action).is_err() {
            pending.store(false, Ordering::Release);
        }
    }
}
fn open_editor(bus: &gio::DBusConnection, editor: &std::path::Path, launch: &mut Option<Child>) {
    let platform = std::collections::BTreeMap::<String, gio::glib::Variant>::new();
    match bus.call_sync(
        Some(EDITOR),
        "/io/github/KroxiDev/RAD",
        "org.freedesktop.Application",
        "Activate",
        Some(&(platform,).to_variant()),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        2000,
        gio::Cancellable::NONE,
    ) {
        Ok(_) => return,
        Err(error)
            if error.matches(gio::DBusError::ServiceUnknown)
                || error.matches(gio::DBusError::NameHasNoOwner) => {}
        Err(error) => {
            eprintln!("RAD editor activation: {error}");
            return;
        }
    }
    // Another click during editor startup must not launch another process. The editor's
    // atomic GApplication registration also covers launches outside this tray.
    if launch.is_some() {
        return;
    }
    match Command::new("systemd-run")
        .args(["--user", "--scope", "--collect", "--quiet", "--"])
        .arg(editor)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
    {
        Ok(child) => *launch = Some(child),
        Err(error) => eprintln!("RAD editor launch: {error}"),
    }
}
fn stop_runtime() {
    let child = Command::new("systemctl")
        .args(["--user", "stop", "rad-runtime.service"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            eprintln!("RAD stop: {error}");
            return;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    eprintln!("RAD stop: {status}");
                }
                break;
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                eprintln!("RAD stop did not finish: {result:?}");
                break;
            }
        }
    }
}
