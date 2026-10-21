//! The runtime as a systemd user service that the editor installs and starts (ADR 0012).
//! Every call to systemd goes through [`Systemctl`], so nothing here needs a real manager.
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const UNIT: &str = "rad-runtime.service";
/// The unit the editor installs; only its `ExecStart=` line is replaced.
const TEMPLATE: &str = include_str!("../../../packaging/rad-runtime.service");
const TRAY_TEMPLATE: &str = include_str!("../../../packaging/rad-tray.service");
/// Most that is kept of what `systemctl` prints on each stream.
const OUTPUT_LIMIT: u64 = 64 * 1024;
/// Longest a single `systemctl` call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// What `systemctl --user` answered: its exit code (`None` when a signal ended it) and the
/// bounded text it printed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Answer {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}
/// `systemctl` could not be run at all; the text says why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable(pub String);

/// The only door to systemd.
pub trait Systemctl: Send + 'static {
    /// Runs `systemctl --user <args>`.
    fn run(&self, args: &[&str]) -> Result<Answer, Unavailable>;
}

/// `systemctl --user` as a child process: no shell, no stdin, bounded output and time.
pub struct UserSystemctl {
    program: PathBuf,
    timeout: Duration,
}
impl UserSystemctl {
    pub fn new(program: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            timeout,
        }
    }
}
impl Default for UserSystemctl {
    fn default() -> Self {
        Self::new("systemctl", CALL_TIMEOUT)
    }
}
/// Keeps the first [`OUTPUT_LIMIT`] bytes and drains the rest so the child never blocks.
fn capture(mut stream: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let _ = (&mut stream).take(OUTPUT_LIMIT).read_to_end(&mut kept);
        let _ = std::io::copy(&mut stream, &mut std::io::sink());
        String::from_utf8_lossy(&kept).into_owned()
    })
}
impl Systemctl for UserSystemctl {
    fn run(&self, args: &[&str]) -> Result<Answer, Unavailable> {
        let mut child = Command::new(&self.program)
            .arg("--user")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Unavailable(format!("{}: {e}", self.program.display())))?;
        let stdout = capture(child.stdout.take().expect("piped stdout"));
        let stderr = capture(child.stderr.take().expect("piped stderr"));
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Unavailable(format!(
                        "{} timed out after {:?}",
                        self.program.display(),
                        self.timeout
                    )));
                }
                Err(e) => return Err(Unavailable(e.to_string())),
            }
        };
        Ok(Answer {
            code: status.code(),
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The restart limit was reached: "RAD stopped because of an error".
    StoppedByError,
    /// "RAD could not start", for every other reason.
    CouldNotStart,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Retry,
    ViewDetails,
}
impl FailureKind {
    pub fn actions(self) -> &'static [Action] {
        &[Action::Retry, Action::ViewDetails]
    }
    /// Catalog key of the sentence the user reads.
    pub fn message(self) -> &'static str {
        match self {
            Self::StoppedByError => "RAD stopped because of an error",
            Self::CouldNotStart => "RAD could not start",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    /// Technical text for "View details"; never part of the normal view.
    pub details: String,
}
impl Failure {
    fn could_not_start(details: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::CouldNotStart,
            details: details.into(),
        }
    }
    fn no_systemd(why: &str) -> Self {
        Self::could_not_start(format!("systemd is not available: {why}"))
    }
}

/// What `systemctl show` says about the unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitState {
    Running,
    Stopped,
    Failed {
        start_limit_hit: bool,
        result: String,
        exit_status: Option<i32>,
    },
    Unreadable,
    /// `systemctl` is missing or there is no user manager; the text says why.
    NoSystemd(String),
}
/// Reads `Key=Value` lines, ignoring what it does not know.
pub fn parse_show(stdout: &str) -> UnitState {
    let (mut active, mut result, mut status) = ("", "", None);
    for (key, value) in stdout.lines().filter_map(|l| l.split_once('=')) {
        match key {
            "ActiveState" => active = value,
            "Result" => result = value,
            "ExecMainStatus" => status = value.parse().ok(),
            _ => {}
        }
    }
    match active {
        "active" | "activating" | "reloading" => UnitState::Running,
        "inactive" | "deactivating" => UnitState::Stopped,
        "failed" => UnitState::Failed {
            start_limit_hit: result == "start-limit-hit",
            result: result.into(),
            exit_status: status,
        },
        _ => UnitState::Unreadable,
    }
}

/// `$XDG_CONFIG_HOME/systemd/user`, or `$HOME/.config/systemd/user`. Like the XDG rules,
/// a relative or empty `XDG_CONFIG_HOME` is ignored.
pub fn unit_directory(
    xdg_config_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    let base = xdg_config_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".config"))
        })?;
    Some(base.join("systemd/user"))
}
/// The `rad-runtime` that sits next to the editor, if there is one.
pub fn sibling_runtime(editor: &Path) -> Option<PathBuf> {
    let runtime = editor.parent()?.join("rad-runtime");
    runtime.is_file().then_some(runtime)
}
/// The template with its single `ExecStart=` line pointing at `runtime`, quoted and escaped
/// the way systemd reads it.
pub fn render_unit(template: &str, runtime: &Path) -> Result<String, String> {
    let path = runtime
        .to_str()
        .filter(|p| runtime.is_absolute() && !p.chars().any(char::is_control))
        .ok_or_else(|| format!("{} cannot be written in a unit", runtime.display()))?;
    let escaped = path.replace('%', "%%").replace('$', "$$");
    let exec = if escaped.contains(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '\\')) {
        format!("\"{}\"", escaped.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        escaped
    };
    let mut lines = template.split_inclusive('\n');
    if lines
        .clone()
        .filter(|l| l.starts_with("ExecStart="))
        .count()
        != 1
    {
        return Err("the unit template needs exactly one ExecStart= line".into());
    }
    Ok(lines
        .by_ref()
        .map(|line| {
            if line.starts_with("ExecStart=") {
                let end = &line[line.trim_end_matches(['\n', '\r']).len()..];
                format!("ExecStart={exec}{end}")
            } else {
                line.to_owned()
            }
        })
        .collect())
}

/// Writes `contents` to `path` whole or not at all: a 0644 temporary in the same directory,
/// renamed over the target, then the directory is flushed.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().expect("the path has a directory");
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .expect("the path names a file")
        .to_string_lossy();
    let temporary = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&temporary)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        File::open(dir)?.sync_all()
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// The unit of the runtime for one user: installs it, starts it, enables it.
pub struct Service<S> {
    systemctl: S,
    unit_dir: PathBuf,
    editor: PathBuf,
}
/// The service of the current user, with the real `systemctl`.
pub fn user_service() -> Result<Service<UserSystemctl>, Failure> {
    let unit_dir = unit_directory(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .ok_or_else(|| Failure::could_not_start("no usable HOME or XDG_CONFIG_HOME"))?;
    let editor = std::env::current_exe()
        .map_err(|e| Failure::could_not_start(format!("the editor's path is unknown: {e}")))?;
    Ok(Service::new(UserSystemctl::default(), unit_dir, editor))
}
impl<S: Systemctl> Service<S> {
    pub fn new(systemctl: S, unit_dir: PathBuf, editor: PathBuf) -> Self {
        Self {
            systemctl,
            unit_dir,
            editor,
        }
    }
    fn call(&self, args: &[&str]) -> Result<Answer, Failure> {
        self.systemctl
            .run(args)
            .map_err(|Unavailable(why)| Failure::no_systemd(&why))
    }
    /// A call that must exit 0.
    fn call_ok(&self, args: &[&str]) -> Result<Answer, Failure> {
        let answer = self.call(args)?;
        if answer.code == Some(0) {
            Ok(answer)
        } else {
            Err(Failure::could_not_start(format!(
                "systemctl --user {} {}: {}",
                args.join(" "),
                answer
                    .code
                    .map_or("ended by a signal".into(), |c| format!("exit code {c}")),
                answer.stderr.trim()
            )))
        }
    }
    /// Installs the runtime and available tray units if missing or changed, then reloads once.
    pub fn install(&self) -> Result<(), Failure> {
        let runtime = sibling_runtime(&self.editor).ok_or_else(|| {
            Failure::could_not_start(format!(
                "rad-runtime was not found next to {}",
                self.editor.display()
            ))
        })?;
        let mut units = vec![(
            UNIT,
            render_unit(TEMPLATE, &runtime).map_err(Failure::could_not_start)?,
        )];
        // A partial build without the optional tray must not prevent the authority starting.
        if let Some(tray) = self
            .editor
            .parent()
            .map(|dir| dir.join("rad-tray"))
            .filter(|path| path.is_file())
        {
            units.push((
                "rad-tray.service",
                render_unit(TRAY_TEMPLATE, &tray).map_err(Failure::could_not_start)?,
            ));
        }
        let mut changed = false;
        for (name, wanted) in units {
            let path = self.unit_dir.join(name);
            if std::fs::read(&path).is_ok_and(|current| current == wanted.as_bytes()) {
                continue;
            }
            write_atomic(&path, wanted.as_bytes()).map_err(|e| {
                Failure::could_not_start(format!("could not write {}: {e}", path.display()))
            })?;
            changed = true;
        }
        if changed {
            self.call_ok(&["daemon-reload"])?;
        }
        Ok(())
    }

    /// Installs if needed and starts the unit, with or without autostart.
    pub fn start(&self) -> Result<(), Failure> {
        self.install()?;
        self.start_unit()
    }
    /// The user asked again: forgets the failure, then starts once.
    pub fn retry(&self) -> Result<(), Failure> {
        self.install()?;
        // A unit that never failed has nothing to reset, so only a missing systemd matters.
        self.call(&["reset-failed", UNIT])?;
        self.start_unit()
    }
    fn start_unit(&self) -> Result<(), Failure> {
        let answer = self.call(&["start", UNIT])?;
        let state = self.state();
        if answer.code == Some(0) {
            // Not answering yet is fine; a unit that already failed (75 is not restarted) is not.
            return match state {
                UnitState::Failed { .. } => {
                    Err(explain(state).expect("a failed unit has a reason"))
                }
                _ => Ok(()),
            };
        }
        let mut failure = explain(state).unwrap_or_else(|| Failure::could_not_start(""));
        failure.details = format!(
            "systemctl --user start {UNIT}: {}\n{}",
            answer.stderr.trim(),
            failure.details
        );
        Err(failure)
    }
    /// Only enables or disables the unit (installing it first if missing); never starts it.
    pub fn set_autostart(&self, enabled: bool) -> Result<(), Failure> {
        self.install()?;
        self.call_ok(&[if enabled { "enable" } else { "disable" }, UNIT])
            .map(drop)
    }
    pub fn state(&self) -> UnitState {
        match self.systemctl.run(&[
            "show",
            UNIT,
            "-p",
            "ActiveState",
            "-p",
            "Result",
            "-p",
            "ExecMainStatus",
        ]) {
            Err(Unavailable(why)) => UnitState::NoSystemd(why),
            Ok(answer) if answer.code == Some(0) => parse_show(&answer.stdout),
            Ok(answer) => UnitState::NoSystemd(answer.stderr.trim().to_owned()),
        }
    }
    /// Why the unit is not running, or `None` when it is.
    pub fn failure(&self) -> Option<Failure> {
        explain(self.state())
    }
}
fn explain(state: UnitState) -> Option<Failure> {
    match state {
        UnitState::Running => None,
        UnitState::Stopped => Some(Failure::could_not_start("the unit is not running")),
        UnitState::Failed {
            start_limit_hit,
            result,
            exit_status,
        } => Some(Failure {
            kind: if start_limit_hit {
                FailureKind::StoppedByError
            } else {
                FailureKind::CouldNotStart
            },
            details: format!(
                "Result={result}, exit code {}",
                exit_status.map_or("unknown".into(), |c| c.to_string())
            ),
        }),
        UnitState::Unreadable => Some(Failure::could_not_start(
            "systemd's answer about the unit was unreadable",
        )),
        UnitState::NoSystemd(why) => Some(Failure::no_systemd(&why)),
    }
}

/// Work the editor asks of its service worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// Install if needed and start.
    Start,
    /// Reset a failed unit and start again.
    Retry,
    /// Say why RAD does not answer.
    Inspect,
    /// Enable or disable starting RAD with the user's session; never starts it.
    SetAutostart(bool),
}
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub job: Job,
    /// `None` when the job succeeded (for `Inspect`: never).
    pub failure: Option<Failure>,
}
/// The worker that owns every blocking call to systemd, off the GUI thread. After a start it
/// waits `settle` so the runtime can open its socket before the editor connects.
pub fn worker<S: Systemctl>(
    service: Result<Service<S>, Failure>,
    settle: Duration,
) -> crate::worker::Worker<Job, Outcome> {
    crate::worker::Worker::spawn(move |job| {
        let failure = match (&service, job) {
            (Err(failure), _) => Some(failure.clone()),
            (Ok(service), Job::Start | Job::Retry) => {
                let started = if job == Job::Start {
                    service.start()
                } else {
                    service.retry()
                };
                if started.is_ok() {
                    std::thread::sleep(settle);
                }
                started.err()
            }
            (Ok(service), Job::SetAutostart(enabled)) => service.set_autostart(enabled).err(),
            (Ok(service), Job::Inspect) => service.failure().or_else(|| {
                Some(Failure::could_not_start(
                    "the unit is running but RAD did not answer",
                ))
            }),
        };
        Outcome { job, failure }
    })
}
