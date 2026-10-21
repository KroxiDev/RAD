//! Per-application transient systemd scope (ADR 0013).
//!
//! GIO emits `launched` on the launch context after spawning an application.
//! The helper asks the user's systemd to adopt that pid in its own
//! `app-rad-<id>-<pid>.scope`, outside the runtime service's cgroup, so that
//! `KillMode=mixed` on the service never reaches the application.
//!
//! Limits: this depends on systemd and the session bus; without them the
//! application stays in the service's cgroup. Descendants created before the
//! pid is moved stay there too, and D-Bus activated applications (started by
//! the bus, no pid) never get a scope. Failures are reported on the helper's
//! diagnostic descriptor and never change the launch outcome.
use gio::prelude::*;
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::OnceLock,
    time::Duration,
};

const MAX_UNIT_NAME: usize = 255;
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

static DIAGNOSTICS: OnceLock<OwnedFd> = OnceLock::new();

/// Keep the inherited stderr as a private close-on-exec descriptor for the
/// helper's diagnostics and point fd 2 at `/dev/null`, so that launched
/// applications keep inheriting `/dev/null`. Call before launching anything.
/// Without an inherited stderr the helper runs without diagnostics; failing to
/// redirect fd 2 is an error, because applications would inherit the runtime's.
pub(crate) fn isolate_stderr() -> std::io::Result<()> {
    // SAFETY: plain fd syscalls on descriptors owned by this process; each
    // returned descriptor is wrapped or closed exactly once.
    unsafe {
        let private = libc::fcntl(2, libc::F_DUPFD_CLOEXEC, 10);
        let private = (private >= 0).then(|| OwnedFd::from_raw_fd(private));
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if null < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // `open` may already have returned fd 2 when no stderr was inherited.
        if null != 2 {
            let redirected = libc::dup2(null, 2);
            let error = std::io::Error::last_os_error();
            libc::close(null);
            if redirected < 0 {
                return Err(error);
            }
        }
        if let Some(private) = private {
            let _ = DIAGNOSTICS.set(private);
        }
    }
    Ok(())
}

/// Write one diagnostic line; best effort, never blocks launch results.
fn report(desktop_id: &str, reason: &str) {
    let Some(fd) = DIAGNOSTICS.get() else { return };
    let clean = |s: &str| {
        s.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect::<String>()
    };
    let line = format!(
        "rad-launcher-helper: no systemd scope for {}: {}\n",
        clean(desktop_id),
        clean(reason)
    );
    // SAFETY: valid owned descriptor and initialized buffer; one write keeps
    // the line atomic for pipes and files.
    unsafe {
        libc::write(fd.as_raw_fd(), line.as_ptr().cast(), line.len());
    }
}

/// `systemd-escape` (without `--path`) of the id, as a unit-name component.
fn escape(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for (i, b) in id.bytes().enumerate() {
        match b {
            b'/' => out.push('-'),
            b'.' if i == 0 => out.push_str("\\x2e"),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b':' | b'_' | b'.' => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out
}

/// Name of the scope for an application launched with `pid`, or `None` when
/// the unit name would exceed systemd's limit.
pub(crate) fn scope_name(desktop_id: &str, pid: u32) -> Option<String> {
    let id = desktop_id.strip_suffix(".desktop").unwrap_or(desktop_id);
    let name = format!("app-rad-{}-{pid}.scope", escape(id));
    (name.len() <= MAX_UNIT_NAME).then_some(name)
}

/// Move every application spawned through `launch_context` into its scope.
/// `main` must be the helper's loop context: D-Bus work always runs there,
/// because GIO may emit `launched` from a worker thread.
pub(crate) fn watch(launch_context: &gio::AppLaunchContext, main: &glib::MainContext) {
    let main = main.clone();
    launch_context.connect_launched(move |_, info, platform_data| {
        let Some(pid) = glib::VariantDict::new(Some(platform_data))
            .lookup::<i32>("pid")
            .ok()
            .flatten()
        else {
            return;
        };
        let Ok(pid) = u32::try_from(pid) else { return };
        let id = info.id().map(|id| id.to_string()).unwrap_or_default();
        let context = main.clone();
        main.invoke(move || start_scope(&context, id, pid));
    });
}

fn start_scope(main: &glib::MainContext, desktop_id: String, pid: u32) {
    let Some(name) = scope_name(&desktop_id, pid) else {
        return report(&desktop_id, "unit name too long");
    };
    let cancellable = gio::Cancellable::new();
    let deadline = cancellable.clone();
    // Bounds the connection on the helper's own loop (the global default
    // context is never iterated here); removed as soon as the bus answers.
    let timer = glib::timeout_source_new(CALL_TIMEOUT, None, glib::Priority::DEFAULT, move || {
        deadline.cancel();
        glib::ControlFlow::Break
    });
    timer.attach(Some(main));
    gio::bus_get(gio::BusType::Session, Some(&cancellable), move |bus| {
        timer.destroy();
        let bus = match bus {
            Ok(bus) => bus,
            Err(e) => return report(&desktop_id, &format!("session bus: {}", e.message())),
        };
        // The bus is shared: losing it must not terminate the helper.
        bus.set_exit_on_close(false);
        let properties = vec![
            ("PIDs".to_string(), vec![pid].to_variant()),
            ("CollectMode".to_string(), "inactive-or-failed".to_variant()),
        ];
        let auxiliary: Vec<(String, Vec<(String, glib::Variant)>)> = Vec::new();
        let parameters = (name, "fail", properties, auxiliary).to_variant();
        bus.call(
            Some("org.freedesktop.systemd1"),
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            Some(&parameters),
            None,
            gio::DBusCallFlags::NONE,
            CALL_TIMEOUT.as_millis() as i32,
            None::<&gio::Cancellable>,
            move |reply| {
                if let Err(e) = reply {
                    report(&desktop_id, &format!("StartTransientUnit: {}", e.message()));
                }
            },
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    // Expected values come from `systemd-escape` (without `--path`).
    #[test]
    fn plain_id_drops_desktop_suffix() {
        assert_eq!(
            scope_name("org.gnome.Foo.desktop", 42).as_deref(),
            Some("app-rad-org.gnome.Foo-42.scope")
        );
    }
    #[test]
    fn escapes_dash_leading_dot_space_and_slash() {
        assert_eq!(
            scope_name("my-app.desktop", 1).as_deref(),
            Some("app-rad-my\\x2dapp-1.scope")
        );
        assert_eq!(
            scope_name(".hidden.desktop", 1).as_deref(),
            Some("app-rad-\\x2ehidden-1.scope")
        );
        assert_eq!(
            scope_name("a b.desktop", 1).as_deref(),
            Some("app-rad-a\\x20b-1.scope")
        );
        assert_eq!(
            scope_name("a/b.desktop", 1).as_deref(),
            Some("app-rad-a-b-1.scope")
        );
        assert_eq!(
            scope_name("a_b:c.desktop", 1).as_deref(),
            Some("app-rad-a_b:c-1.scope")
        );
    }
    #[test]
    fn escapes_non_ascii_bytes_in_lowercase_hex() {
        assert_eq!(
            scope_name("ñandú.desktop", 7).as_deref(),
            Some("app-rad-\\xc3\\xb1and\\xc3\\xba-7.scope")
        );
    }
    #[test]
    fn id_without_suffix_is_used_as_is() {
        assert_eq!(scope_name("x", 9).as_deref(), Some("app-rad-x-9.scope"));
    }
    #[test]
    fn name_of_exactly_255_bytes_is_allowed_and_256_is_not() {
        // "app-rad-" (8) + id + "-5" (2) + ".scope" (6) = 16 + id
        let fits = "a".repeat(255 - 16);
        let name = scope_name(&format!("{fits}.desktop"), 5).unwrap();
        assert_eq!(name.len(), 255);
        assert_eq!(scope_name(&format!("{fits}a.desktop"), 5), None);
    }
}
