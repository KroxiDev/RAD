//! User-session owner of a single requested installation update. No deck-data access.
use crate::{Error, InstallOutcome, Prepared, gate};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
const LIMIT: usize = 16_384;
const GRANT_LIFETIME: Duration = Duration::from_secs(90);
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(240);
const EDITOR: &str = "/usr/lib/rad/rad-editor";
const COORDINATOR: &str = "/usr/lib/rad/rad-update-coordinator";
const INSTALLER: &str = "/usr/lib/rad/rad-update-installer";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OperationResult {
    pub success: bool,
    pub details: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Authorizing,
    WaitingEditor,
    Installing,
    Reopening,
    Opened,
    ReopenFailed,
    Finished,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u32,
    nonce: String,
    phase: Phase,
    target: String,
    result: Option<OperationResult>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Acknowledgement {
    nonce: String,
    ready: bool,
    details: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    nonce: String,
    editor_pid: u32,
    expires_unix_ms: u64,
}
struct RequestFile(PathBuf);
impl Drop for RequestFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
#[derive(Serialize, Deserialize)]
struct Opened {
    nonce: String,
}

/// Runs in a worker. Success is the authenticated coordinator's ACK: only then may
/// the editor close. No transaction can begin while that editor remains alive.
pub fn handoff(prepared: &Prepared) -> Result<(), Error> {
    prepared
        .candidate
        .metadata
        .validate(&prepared.candidate.tag)?;
    if !crate::installation::managed()? {
        return Err(Error(
            "only the installed official RAD editor can hand off an update".into(),
        ));
    }
    let gate = gate::session_gate()?;
    if !gate.startup_allowed()? {
        return Err(Error("another RAD update is in progress".into()));
    }
    // The editor's live owner marker protects Prepared before handoff; this
    // shared lease overlaps the coordinator's independent pin through READY.
    // SAFETY: geteuid only reads this process's identity.
    let _prepared_lease =
        crate::cleanup::Lease::pin(&prepared.directory, unsafe { libc::geteuid() })?;
    let state = state_directory()?;
    collect_ipc_opportunistically(&state, Some(gate.directory()));
    let nonce = nonce()?;
    let namespace = crate::cleanup::pid_namespace()?;
    let request = state.join(format!(
        "request-{namespace}-{}-{nonce}.json",
        std::process::id()
    ));
    let _request_file = RequestFile(request.clone());
    let payload = serde_json::to_vec(prepared).map_err(|e| Error(e.to_string()))?;
    if payload.len() > LIMIT {
        return Err(Error("update handoff request is too large".into()));
    }
    atomic_write(&request, &payload)?;
    let pid = std::process::id().to_string();
    let args = [
        OsString::from("--editor-pid"),
        OsString::from(&pid),
        OsString::from("--nonce"),
        OsString::from(&nonce),
        OsString::from("--request"),
        request.clone().into_os_string(),
    ];
    let launch = launch_service("rad-update-coordinator.service", COORDINATOR, &args);
    if let Err(error) = launch {
        let _ = std::fs::remove_file(&request);
        return Err(error);
    }
    let ack = gate.directory().join("handoff.json");
    let deadline = Instant::now() + HANDOFF_TIMEOUT;
    loop {
        match read_json::<Acknowledgement>(&ack) {
            Ok(Some(answer)) if answer.nonce == nonce => {
                let _ = std::fs::remove_file(&ack);
                return if answer.ready {
                    // Publication is the local consent commit point. No coordinator
                    // may infer consent from an editor that later closes normally.
                    publish_grant(
                        &gate.directory().join("grant.json"),
                        &nonce,
                        std::process::id(),
                    )?;
                    Ok(())
                } else {
                    Err(Error(answer.details))
                };
            }
            Ok(None) | Ok(Some(_)) => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(Error(
                "RAD update handoff did not complete; the current editor was kept open".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
/// Only read by the Updates screen after a user-requested operation, not globally.
/// An interrupted record is a failure, never success and never an automatic retry.
pub fn operation_result() -> Result<Option<OperationResult>, Error> {
    let state = state_directory()?;
    let session = gate::session_gate().ok();
    collect_ipc_opportunistically(&state, session.as_ref().map(|gate| gate.directory()));
    let Some(record) = read_json::<Record>(&state.join("operation.json"))? else {
        return Ok(None);
    };
    if record.schema != 1 {
        return Err(Error("unsupported RAD update operation record".into()));
    }
    Ok(Some(record.result.unwrap_or(OperationResult {
        success: false,
        details: format!("The previous requested RAD update was interrupted during {:?}. Inspect the installed package before retrying; no automatic retry was made.", record.phase),
    })))
}
/// The reopened Updates screen sends this only after rendering its first frame.
/// It does not erase the durable result or cause network traffic.
pub fn acknowledge_reopen() -> Result<(), Error> {
    let state = state_directory()?;
    let Some(record) = read_json::<Record>(&state.join("operation.json"))? else {
        return Ok(());
    };
    if record.schema == 1 && record.phase == Phase::Reopening && valid_nonce(&record.nonce) {
        atomic_write(
            &state.join("opened.json"),
            &serde_json::to_vec(&Opened {
                nonce: record.nonce,
            })
            .map_err(|e| Error(e.to_string()))?,
        )?;
    }
    Ok(())
}
/// Entrypoint only for the independent user service. Arguments never select a
/// privileged program, a package name or a shell command.
pub fn run(args: impl IntoIterator<Item = OsString>) -> Result<(), Error> {
    // SAFETY: geteuid has no preconditions. Session control never runs as root.
    if unsafe { libc::geteuid() } == 0 {
        return Err(Error(
            "RAD update coordination must run as the original user, not root".into(),
        ));
    }
    let args: Vec<_> = args.into_iter().collect();
    if args.len() != 6
        || args[0] != "--editor-pid"
        || args[2] != "--nonce"
        || args[4] != "--request"
    {
        return Err(Error("invalid RAD update coordinator arguments".into()));
    }
    let editor_pid = args[1]
        .to_str()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|pid| *pid > 1)
        .ok_or_else(|| Error("invalid editor process identity".into()))?;
    let nonce = args[3]
        .to_str()
        .filter(|s| valid_nonce(s))
        .ok_or_else(|| Error("invalid handoff identity".into()))?
        .to_owned();
    let state = state_directory()?;
    let namespace = crate::cleanup::pid_namespace()?;
    let request = state.join(format!("request-{namespace}-{editor_pid}-{nonce}.json"));
    if Path::new(&args[5]) != request {
        return Err(Error("unexpected update handoff request path".into()));
    }
    let prepared = read_json::<Prepared>(&request)?
        .ok_or_else(|| Error("missing update handoff request".into()))?;
    let _ = std::fs::remove_file(&request);
    prepared
        .candidate
        .metadata
        .validate(&prepared.candidate.tag)?;
    // Retain the user's package even after its creator exits and until root
    // has copied it; a collector must not treat the editor's death as abandonment.
    // SAFETY: geteuid only reads the coordinator's original user identity.
    let _prepared_lease =
        crate::cleanup::Lease::pin(&prepared.directory, unsafe { libc::geteuid() })?;
    let session = gate::session_gate()?;
    collect_ipc_opportunistically(&state, Some(session.directory()));
    let ack = session.directory().join("handoff.json");
    let update = match session.acquire_update() {
        Ok(update) => update,
        Err(error) => {
            write_ack(&ack, &nonce, false, &error.0)?;
            return Err(error);
        }
    };
    let mut record = Record {
        schema: 1,
        nonce: nonce.clone(),
        phase: Phase::Authorizing,
        target: prepared.candidate.metadata.package_version(),
        result: None,
    };
    save_record(&state, &record)?;
    let grant = session.directory().join("grant.json");
    let mut control = NativeControl::new(state.clone(), nonce.clone(), editor_pid, grant.clone());
    let result = transact(&mut control, &prepared, editor_pid, &update, || {
        record.phase = Phase::WaitingEditor;
        save_record(&state, &record)?;
        write_ack(&ack, &nonce, true, "authorized")?;
        wait_for_grant(&grant, &nonce, editor_pid, Duration::from_secs(10))
    });
    // Dropping a pre-commit helper closes stdin, aborting its prepared transaction.
    let editor_closed = control.editor_closed;
    drop(control);
    record.phase = Phase::Finished;
    record.result = Some(transaction_result(result));
    save_record(&state, &record)?;
    if !editor_closed {
        // An auth refusal leaves the current window and runtime intact.
        write_ack(
            &ack,
            &nonce,
            false,
            &record.result.as_ref().expect("record result").details,
        )?;
        return Ok(());
    }
    drop(update);
    reopen_record(
        &state,
        &mut record,
        || {
            launch_service(
                "rad-update-reopen.service",
                EDITOR,
                &[OsString::from("--updates")],
            )
        },
        || wait_reopen_ack(&state.join("opened.json"), &nonce),
    )
}
fn transaction_result(result: Result<InstallOutcome, Error>) -> OperationResult {
    match result {
        Ok(outcome) => OperationResult {
            success: true,
            details: bounded(&format!(
                "Installed {}. {}",
                outcome.installed_version, outcome.details
            )),
        },
        Err(error) => OperationResult {
            success: false,
            details: bounded(&error.0),
        },
    }
}
fn reopen_record(
    state: &Path,
    record: &mut Record,
    launch: impl FnOnce() -> Result<(), Error>,
    wait_opened: impl FnOnce() -> Result<bool, Error>,
) -> Result<(), Error> {
    record.phase = Phase::Reopening;
    save_record(state, record)?;
    let launched = launch();
    let opened_ok = if launched.is_ok() {
        wait_opened()?
    } else {
        false
    };
    if opened_ok {
        record.phase = Phase::Opened;
        let _ = std::fs::remove_file(state.join("opened.json"));
    } else {
        record.phase = Phase::ReopenFailed;
        let prior = record.result.take().expect("record result");
        record.result = Some(OperationResult {
            success: false,
            details: bounded(&format!(
                "{} RAD could not be reopened. Open RAD manually; do not downgrade or remove user data. {}",
                prior.details,
                launched
                    .err()
                    .map(|e| e.0)
                    .unwrap_or_else(|| "No first-frame acknowledgement arrived.".into())
            )),
        });
    }
    save_record(state, record)
}
fn wait_reopen_ack(opened: &Path, nonce: &str) -> Result<bool, Error> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if read_json::<Opened>(opened)?.is_some_and(|ack| ack.nonce == nonce) {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(false)
}

fn unix_ms() -> Result<u64, Error> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error("the system clock cannot validate the update handoff".into()))?
        .as_millis()
        .try_into()
        .map_err(|_| Error("the update handoff clock exceeded its bound".into()))
}
fn publish_grant(path: &Path, nonce: &str, editor_pid: u32) -> Result<(), Error> {
    let grant = Grant {
        nonce: nonce.into(),
        editor_pid,
        expires_unix_ms: unix_ms()?.saturating_add(GRANT_LIFETIME.as_millis() as u64),
    };
    let bytes = serde_json::to_vec(&grant).map_err(|error| Error(error.to_string()))?;
    let parent = path
        .parent()
        .ok_or_else(|| Error("missing handoff grant directory".into()))?;
    let namespace = crate::cleanup::pid_namespace()?;
    let temporary = parent.join(format!(
        ".grant-{namespace}-{}-{}",
        std::process::id(),
        grant.nonce
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        // After publication the coordinator may accept this grant. Returning an
        // error now would falsely tell the UI it can remain open or close normally.
        // The runtime-directory grant is intentionally ephemeral across reboot;
        // attempt directory durability without undoing the already-visible consent.
        if let Err(error) = File::open(parent).and_then(|directory| directory.sync_all()) {
            eprintln!("RAD update grant directory sync: {error}");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
fn current_grant(path: &Path, nonce: &str, editor_pid: u32) -> Result<(), Error> {
    let now = unix_ms()?;
    let Some(grant) = read_json::<Grant>(path)? else {
        return Err(Error(
            "the editor did not approve this update handoff".into(),
        ));
    };
    if grant.nonce != nonce
        || grant.editor_pid != editor_pid
        || grant.expires_unix_ms < now
        || grant.expires_unix_ms.saturating_sub(now) > GRANT_LIFETIME.as_millis() as u64
    {
        return Err(Error(
            "the editor's update handoff approval is no longer current".into(),
        ));
    }
    Ok(())
}
fn wait_for_grant(
    path: &Path,
    nonce: &str,
    editor_pid: u32,
    timeout: Duration,
) -> Result<(), Error> {
    let deadline = Instant::now() + timeout;
    loop {
        if current_grant(path, nonce, editor_pid).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error(
                "update handoff was not confirmed; RAD was not stopped".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn valid_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn nonce() -> Result<String, Error> {
    let mut bytes = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn bounded(value: &str) -> String {
    // Budget the JSON representation, not Unicode characters: a control character
    // expands to six bytes and a supplementary character already needs four UTF-8
    // bytes. Keep ample room for the record envelope and later recovery context.
    const JSON_BYTES: usize = 12_000;
    const ELLIPSIS_BYTES: usize = 3;
    let mut kept = String::new();
    let mut encoded = 0;
    for character in value.chars() {
        let cost = match character {
            '"' | '\\' | '\u{08}' | '\u{0c}' | '\n' | '\r' | '\t' => 2,
            c if c < '\u{20}' => 6,
            c => c.len_utf8(),
        };
        if encoded + cost > JSON_BYTES - ELLIPSIS_BYTES {
            kept.push('…');
            break;
        }
        kept.push(character);
        encoded += cost;
    }
    kept
}
fn state_directory() -> Result<PathBuf, Error> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|p| p.join(".local/state"))
        })
        .ok_or_else(|| Error("an absolute XDG_STATE_HOME or HOME is required".into()))?
        .join("rad/update");
    gate::private_directory(&state)?;
    Ok(state)
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() > LIMIT * 2 {
        return Err(Error("RAD update state exceeded its bound".into()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error("missing update state directory".into()))?;
    let namespace = crate::cleanup::pid_namespace()?;
    let temporary = parent.join(format!(
        ".write-{namespace}-{}-{}",
        std::process::id(),
        nonce()?
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
fn read_private(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || metadata.len() > (LIMIT * 2) as u64
    {
        return Err(Error("unsafe or oversized RAD update record".into()));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take((LIMIT * 2 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > LIMIT * 2 {
        return Err(Error("RAD update record exceeded its bound".into()));
    }
    Ok(Some(bytes))
}
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, Error> {
    read_private(path)?
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|_| Error("invalid RAD update record".into()))
        })
        .transpose()
}
fn save_record(state: &Path, record: &Record) -> Result<(), Error> {
    atomic_write(
        &state.join("operation.json"),
        &serde_json::to_vec(record).map_err(|e| Error(e.to_string()))?,
    )
}
fn write_ack(path: &Path, nonce: &str, ready: bool, details: &str) -> Result<(), Error> {
    atomic_write(
        path,
        &serde_json::to_vec(&Acknowledgement {
            nonce: nonce.into(),
            ready,
            details: bounded(details),
        })
        .map_err(|e| Error(e.to_string()))?,
    )
}
fn session_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    for key in [
        "HOME",
        "USER",
        "LOGNAME",
        "XDG_RUNTIME_DIR",
        "XDG_STATE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "DBUS_SESSION_BUS_ADDRESS",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
}
fn launch_service(unit: &str, program: &str, args: &[OsString]) -> Result<(), Error> {
    let mut command = session_command("/usr/bin/systemd-run");
    command.args([
        "--user",
        "--collect",
        "--quiet",
        "--unit",
        unit,
        "--property=Type=exec",
        "--property=TimeoutStopSec=infinity",
        "--property=KillMode=process",
    ]);
    for key in [
        "HOME",
        "USER",
        "LOGNAME",
        "XDG_RUNTIME_DIR",
        "XDG_STATE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "DBUS_SESSION_BUS_ADDRESS",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
    ] {
        if let Some(value) = std::env::var_os(key) {
            let mut setting = OsString::from("--setenv=");
            setting.push(key);
            setting.push("=");
            setting.push(value);
            command.arg(setting);
        }
    }
    command.arg("--").arg(program).args(args);
    let output = crate::installation::capture(
        command,
        Some(Duration::from_secs(10)),
        &crate::Cancellation::default(),
        LIMIT,
    )?;
    if output.code == Some(0) {
        Ok(())
    } else {
        Err(Error(format!(
            "could not launch independent update service: {}",
            output.stderr
        )))
    }
}
struct NativeControl {
    installer: Option<InstallerProcess>,
    state: PathBuf,
    nonce: String,
    editor_closed: bool,
    editor_pid: u32,
    grant: PathBuf,
}
impl NativeControl {
    fn new(state: PathBuf, nonce: String, editor_pid: u32, grant: PathBuf) -> Self {
        Self {
            installer: None,
            state,
            nonce,
            editor_closed: false,
            editor_pid,
            grant,
        }
    }
}
impl SessionControl for NativeControl {
    fn authorize(&mut self, prepared: &Prepared) -> Result<(), Error> {
        let mut installer = InstallerProcess::spawn(prepared)?;
        installer.authorize()?;
        self.installer = Some(installer);
        Ok(())
    }
    fn validate_handoff(&mut self) -> Result<(), Error> {
        current_grant(&self.grant, &self.nonce, self.editor_pid)
    }
    fn wait_editor_closed(&mut self, pid: u32) -> Result<(), Error> {
        wait_editor(pid)?;
        self.editor_closed = true;
        Ok(())
    }
    fn stop_cleanly(&mut self) -> Result<(), Error> {
        stop_session()
    }
    fn commit(&mut self) -> Result<InstallOutcome, Error> {
        if let Some(mut record) = read_json::<Record>(&self.state.join("operation.json"))? {
            if record.nonce != self.nonce {
                return Err(Error("update operation identity changed".into()));
            }
            record.phase = Phase::Installing;
            save_record(&self.state, &record)?;
        } else {
            return Err(Error("update operation record disappeared".into()));
        }
        self.installer
            .as_mut()
            .ok_or_else(|| Error("installer was not authorized".into()))?
            .commit()
    }
    fn installed_version(&mut self) -> Result<String, Error> {
        crate::installation::installed_version()
    }
}
fn wait_editor(pid: u32) -> Result<(), Error> {
    // SAFETY: pidfd_open returns a new owned descriptor, never sends a signal.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error.into())
        };
    }
    // SAFETY: the successful syscall transfers this descriptor exactly once.
    let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        // SAFETY: poll points at one initialized live pollfd.
        let result = unsafe { libc::poll(&mut poll, 1, 100) };
        if result > 0 && poll.revents & libc::POLLIN != 0 {
            return Ok(());
        }
        if result < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::last_os_error().into());
        }
        if Instant::now() >= deadline {
            return Err(Error(
                "the editor did not close; installation was not started".into(),
            ));
        }
    }
}
#[derive(Default)]
struct Unit {
    loaded: String,
    active: String,
    sub: String,
    result: String,
    main_pid: String,
    exit_status: String,
}
fn unit(name: &str) -> Result<Unit, Error> {
    let mut command = session_command("/usr/bin/systemctl");
    command.args([
        "--user",
        "show",
        "--property=LoadState,ActiveState,SubState,Result,MainPID,ExecMainStatus",
        name,
    ]);
    let output = crate::installation::capture(
        command,
        Some(Duration::from_secs(5)),
        &crate::Cancellation::default(),
        LIMIT,
    )?;
    let mut state = Unit::default();
    for (key, value) in output
        .stdout
        .lines()
        .filter_map(|line| line.split_once('='))
    {
        match key {
            "LoadState" => state.loaded = value.into(),
            "ActiveState" => state.active = value.into(),
            "SubState" => state.sub = value.into(),
            "Result" => state.result = value.into(),
            "MainPID" => state.main_pid = value.into(),
            "ExecMainStatus" => state.exit_status = value.into(),
            _ => {}
        }
    }
    if state.loaded.is_empty() || (output.code != Some(0) && state.loaded != "not-found") {
        return Err(Error(format!(
            "could not inspect RAD service: {}",
            output.stderr
        )));
    }
    Ok(state)
}
fn quiet(state: &Unit) -> bool {
    state.loaded == "not-found"
        || (matches!(state.active.as_str(), "inactive" | "failed") && state.main_pid == "0")
}
fn terminate_main(name: &str) -> Result<(), Error> {
    let mut command = session_command("/usr/bin/systemctl");
    command.args([
        "--user",
        "kill",
        "--kill-whom=main",
        "--signal=SIGTERM",
        name,
    ]);
    let output = crate::installation::capture(
        command,
        Some(Duration::from_secs(5)),
        &crate::Cancellation::default(),
        LIMIT,
    )?;
    if output.code == Some(0) {
        Ok(())
    } else {
        Err(Error(format!(
            "could not request a clean RAD shutdown: {}",
            output.stderr
        )))
    }
}
fn stop_session() -> Result<(), Error> {
    let original = unit("rad-runtime.service")?;
    let running = !quiet(&original);
    if running {
        if original.active == "deactivating" {
            return Err(Error(
                "RAD is already stopping outside this update; retry after it finishes".into(),
            ));
        }
        terminate_main("rad-runtime.service")?;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let runtime = unit("rad-runtime.service")?;
        if quiet(&runtime) {
            if running && (runtime.result != "success" || runtime.exit_status != "0") {
                return Err(Error(
                    "RAD did not stop cleanly; installation was not started".into(),
                ));
            }
            break;
        }
        if Instant::now() >= deadline {
            return Err(Error("RAD runtime is still active; installation was not started and no forced stop was sent".into()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // BindsTo normally stops the tray. Request only its main process if it remains.
    if !quiet(&unit("rad-tray.service")?) {
        terminate_main("rad-tray.service")?;
    }
    loop {
        if quiet(&unit("rad-tray.service")?) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error(
                "RAD tray is still active; installation was not started".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
struct InstallerProcess {
    child: Child,
    input: Option<ChildStdin>,
    stdout: ChildStdout,
    stderr: ChildStderr,
    out: Vec<u8>,
    err: Vec<u8>,
    committed: bool,
    finished: bool,
    authorization_deadline: Instant,
}
impl InstallerProcess {
    fn spawn(prepared: &Prepared) -> Result<Self, Error> {
        let mut command = session_command("/usr/bin/pkexec");
        command
            .arg(INSTALLER)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| Error("missing installer stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error("missing installer stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error("missing installer stderr".into()))?;
        let mut process = Self {
            child,
            input: Some(input),
            stdout,
            stderr,
            out: vec![],
            err: vec![],
            committed: false,
            finished: false,
            authorization_deadline: Instant::now() + Duration::from_secs(180),
        };
        nonblocking(process.input.as_ref().expect("installer stdin"))?;
        nonblocking(&process.stdout)?;
        nonblocking(&process.stderr)?;
        let bytes = serde_json::to_vec(prepared).map_err(|e| Error(e.to_string()))?;
        if bytes.len() > LIMIT {
            return Err(Error("installer request exceeded its bound".into()));
        }
        let mut bytes = bytes;
        bytes.push(b'\n');
        let mut written = 0;
        while written < bytes.len() {
            match process
                .input
                .as_mut()
                .expect("installer stdin")
                .write(&bytes[written..])
            {
                Ok(0) => return Err(Error("installer request pipe closed".into())),
                Ok(n) => written += n,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
            if Instant::now() >= process.authorization_deadline {
                return Err(Error("administrative authorization did not finish".into()));
            }
            if written < bytes.len() {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        Ok(process)
    }
    fn poll(&mut self) -> Result<Option<ExitStatus>, Error> {
        drain(&mut self.stdout, &mut self.out)?;
        drain(&mut self.stderr, &mut self.err)?;
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.finished = true;
            drain(&mut self.stdout, &mut self.out)?;
            drain(&mut self.stderr, &mut self.err)?;
        }
        Ok(status)
    }
    fn failure(&self, what: &str) -> Error {
        Error(bounded(&format!(
            "{what}: {}",
            String::from_utf8_lossy(&self.err)
        )))
    }
    fn authorize(&mut self) -> Result<(), Error> {
        let deadline = self.authorization_deadline;
        loop {
            let status = self.poll()?;
            if let Some(end) = self.out.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = self.out.drain(..=end).collect();
                if line == b"READY\n" {
                    return Ok(());
                }
                return Err(self.failure("invalid installer authorization response"));
            }
            if status.is_some() {
                return Err(
                    self.failure("administrative authorization or package preparation failed")
                );
            }
            if self.out.len() > 16 {
                return Err(self.failure("invalid installer authorization response"));
            }
            if Instant::now() >= deadline {
                return Err(self.failure("administrative authorization did not finish"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn commit(&mut self) -> Result<InstallOutcome, Error> {
        // From this point every error is waited for, never cancelled by killing pacman.
        self.committed = true;
        let sent = self
            .input
            .as_mut()
            .ok_or_else(|| Error("missing authorized installer".into()))?
            .write_all(b"commit\n");
        self.input.take();
        let status = loop {
            if let Some(status) = self.poll()? {
                break status;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        if !status.success() {
            return Err(self.failure("RAD package transaction failed"));
        }
        sent?;
        serde_json::from_slice(&self.out)
            .map_err(|_| self.failure("invalid installer transaction result"))
    }
}
impl Drop for InstallerProcess {
    fn drop(&mut self) {
        self.input.take();
        if !self.finished {
            // EOF cancels pre-commit preparation. After commit, wait indefinitely:
            // killing pkexec/pacman is not a transaction cancellation mechanism.
            if !self.committed {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        }
    }
}
fn nonblocking(stream: &impl AsRawFd) -> Result<(), Error> {
    // SAFETY: only changes flags on a live owned pipe descriptor.
    unsafe {
        let flags = libc::fcntl(stream.as_raw_fd(), libc::F_GETFL);
        if flags < 0 || libc::fcntl(stream.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
fn drain(stream: &mut impl Read, bytes: &mut Vec<u8>) -> Result<(), Error> {
    let mut block = [0; 4096];
    // A flooding producer cannot starve process-status/deadline checks.
    for _ in 0..16 {
        match stream.read(&mut block) {
            Ok(0) => break,
            Ok(n) => {
                let kept = n.min(LIMIT.saturating_sub(bytes.len()));
                bytes.extend_from_slice(&block[..kept]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Explicit effect boundary for the ordered, user-requested installation.
pub trait SessionControl {
    fn authorize(&mut self, prepared: &Prepared) -> Result<(), Error>;
    fn validate_handoff(&mut self) -> Result<(), Error>;
    fn wait_editor_closed(&mut self, editor_pid: u32) -> Result<(), Error>;
    fn stop_cleanly(&mut self) -> Result<(), Error>;
    fn commit(&mut self) -> Result<crate::InstallOutcome, Error>;
    fn installed_version(&mut self) -> Result<String, Error>;
}
/// Authentication must succeed before ACK; every old process must close before commit.
pub fn transact(
    control: &mut impl SessionControl,
    prepared: &Prepared,
    editor_pid: u32,
    gate: &crate::gate::UpdateGuard,
    ready: impl FnOnce() -> Result<(), Error>,
) -> Result<crate::InstallOutcome, Error> {
    control.authorize(prepared)?;
    ready()?;
    control.validate_handoff()?;
    control.wait_editor_closed(editor_pid)?;
    control.validate_handoff()?;
    control.stop_cleanly()?;
    let _quiescent = gate.wait_quiescent(std::time::Duration::from_secs(5))?;
    control.validate_handoff()?;
    let outcome = control.commit()?;
    let expected = prepared.candidate.metadata.package_version();
    if outcome.installed_version != expected || control.installed_version()? != expected {
        return Err(Error(
            "pacman result did not match the installed target version".into(),
        ));
    }
    Ok(outcome)
}

/// Collects only demonstrably abandoned updater IPC files in one private directory.
/// Final results and diagnostics are never temporary, regardless of age.
pub fn collect_ipc_in(
    directory: &Path,
    uid: u32,
    liveness: &(impl crate::cleanup::Liveness + ?Sized),
) -> Result<usize, Error> {
    use crate::cleanup::ProcessState;
    // This entrypoint never creates directories or scans an implicit temporary root.
    let anchor = open_ipc_directory(directory, uid)?;
    // A numeric PID is only meaningful in the creator's PID namespace. Unknown
    // or foreign namespace identities never reach the process-liveness probe.
    let namespace = crate::cleanup::pid_namespace()?;
    // The proc-fd link is our own live directory descriptor, not an entry supplied
    // by a user or a candidate. Enumeration and unlinkat stay on this pinned inode.
    let entries = std::fs::read_dir(format!("/proc/self/fd/{}", anchor.as_raw_fd()))?;
    let mut removed = 0;
    for entry in entries.take(256) {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name();
        let Some((pid, name)) = name
            .to_str()
            .and_then(|name| ipc_creator(name, &namespace).map(|pid| (pid, name)))
        else {
            continue;
        };
        if liveness.state(pid) != ProcessState::Dead {
            continue;
        }
        let name = std::ffi::CString::new(name)
            .map_err(|_| Error("invalid updater IPC filename".into()))?;
        // SAFETY: anchor is a live owned directory fd and name has no embedded NUL.
        // NOFOLLOW rejects links, NONBLOCK makes a forged FIFO non-blocking.
        let fd = unsafe {
            libc::openat(
                anchor.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            continue;
        }
        // SAFETY: openat returned a new descriptor transferred exactly once.
        let file = unsafe { File::from_raw_fd(fd) };
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if !metadata.is_file()
            || metadata.uid() != uid
            || metadata.mode() & 0o7777 != 0o600
            || metadata.nlink() != 1
        {
            continue;
        }
        let mut current = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fstatat initializes current on success; AT_SYMLINK_NOFOLLOW
        // checks the directory entry, never its target.
        if unsafe {
            libc::fstatat(
                anchor.as_raw_fd(),
                name.as_ptr(),
                current.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            continue;
        }
        // SAFETY: successful fstatat initialized every stat field.
        let current = unsafe { current.assume_init() };
        if current.st_dev != metadata.dev()
            || current.st_ino != metadata.ino()
            || current.st_uid != uid
            || current.st_nlink != 1
            || current.st_mode & 0o7777 != 0o600
            || current.st_mode & libc::S_IFMT != libc::S_IFREG
            || liveness.state(pid) != ProcessState::Dead
        {
            continue;
        }
        // SAFETY: unlinkat removes only this exact non-directory entry from the
        // pinned private directory. No recursive removal or followed links.
        if unsafe { libc::unlinkat(anchor.as_raw_fd(), name.as_ptr(), 0) } == 0 {
            removed += 1;
        }
    }
    Ok(removed)
}
fn open_ipc_directory(directory: &Path, uid: u32) -> Result<File, Error> {
    // SAFETY: geteuid has no preconditions; even privileged callers only collect
    // IPC belonging to their own identity. The shared walker pins each ancestor
    // using openat/NOFOLLOW and rejects writable or unexpected owners.
    if uid != unsafe { libc::geteuid() } {
        return Err(Error(
            "IPC collection requires this user's private directory".into(),
        ));
    }
    crate::cleanup::private_directory_anchor(directory, uid)
}

fn ipc_creator(name: &str, namespace: &str) -> Option<u32> {
    let identity = if let Some(value) = name
        .strip_prefix(".write-")
        .or_else(|| name.strip_prefix(".grant-"))
    {
        value
    } else {
        name.strip_prefix("request-")?.strip_suffix(".json")?
    };
    let (creator_namespace, identity) = identity.split_once('-')?;
    if creator_namespace != namespace || !valid_nonce(creator_namespace) {
        return None;
    }
    let (pid, nonce) = identity.split_once('-')?;
    if !valid_nonce(nonce)
        || pid.is_empty()
        || pid.starts_with('0')
        || !pid.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let number = pid.parse::<u32>().ok()?;
    (number <= i32::MAX as u32 && number > 0).then_some(number)
}
fn collect_ipc_opportunistically(state: &Path, runtime: Option<&Path>) {
    // Maintenance failure is not an update failure and never emits a global notice.
    // SAFETY: geteuid only reads this process's identity.
    let uid = unsafe { libc::geteuid() };
    let _ = collect_ipc_in(state, uid, &crate::cleanup::ProcessLiveness);
    if let Some(runtime) = runtime {
        let _ = collect_ipc_in(runtime, uid, &crate::cleanup::ProcessLiveness);
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn failed_launcher_and_late_ready_cannot_reuse_missing_expired_or_foreign_grants() {
        let root =
            std::env::temp_dir().join(format!("rad-update-expired-consent-{}", std::process::id()));
        gate::private_directory(&root).unwrap();
        let path = root.join("grant.json");
        let nonce = "0123456789abcdef0123456789abcdef";
        assert!(
            wait_for_grant(&path, nonce, 42, Duration::ZERO).is_err(),
            "a failed launcher leaves no UI consent"
        );
        for grant in [
            Grant {
                nonce: nonce.into(),
                editor_pid: 42,
                expires_unix_ms: unix_ms().unwrap().saturating_sub(1),
            },
            Grant {
                nonce: "fedcba9876543210fedcba9876543210".into(),
                editor_pid: 42,
                expires_unix_ms: unix_ms().unwrap() + 90_000,
            },
            Grant {
                nonce: nonce.into(),
                editor_pid: 43,
                expires_unix_ms: unix_ms().unwrap() + 90_000,
            },
        ] {
            atomic_write(&path, &serde_json::to_vec(&grant).unwrap()).unwrap();
            assert!(
                wait_for_grant(&path, nonce, 42, Duration::ZERO).is_err(),
                "late READY cannot revive a stale or foreign request"
            );
        }
        publish_grant(&path, nonce, 42).unwrap();
        wait_for_grant(&path, nonce, 42, Duration::ZERO).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn noisy_transaction_diagnostics_do_not_prevent_the_reopen_attempt() {
        let root =
            std::env::temp_dir().join(format!("rad-update-noisy-result-{}", std::process::id()));
        gate::private_directory(&root).unwrap();
        for noisy in [
            "\u{0001}".repeat(32_000),
            "🦀".repeat(32_000),
            "\"\\\n".repeat(32_000),
        ] {
            let mut record = Record {
                schema: 1,
                nonce: "0123456789abcdef0123456789abcdef".into(),
                phase: Phase::Finished,
                target: "0.2.0-1".into(),
                result: Some(transaction_result(Ok(InstallOutcome {
                    installed_version: "0.2.0-1".into(),
                    details: noisy,
                }))),
            };
            // This is exactly the durable write before run releases the update gate.
            save_record(&root, &record).unwrap();
            let reopened = std::cell::Cell::new(false);
            reopen_record(
                &root,
                &mut record,
                || {
                    reopened.set(true);
                    Ok(())
                },
                || Ok(true),
            )
            .unwrap();
            assert!(reopened.get());
            assert_eq!(record.phase, Phase::Opened);
            let saved: Record = read_json(&root.join("operation.json")).unwrap().unwrap();
            assert!(saved.result.unwrap().success);
            assert!(
                std::fs::metadata(root.join("operation.json"))
                    .unwrap()
                    .len()
                    <= 32_768
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
