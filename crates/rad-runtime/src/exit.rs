//! How the runtime process ends, and what it leaves behind (ADR 0011).
//!
//! Every end of the process belongs to one class. systemd decides from the exit code
//! (`packaging/rad-runtime.service`); the Editor never reads codes, it reads the unit's
//! state and, after a restart, [`rad_api::PreviousExit`] from `overview` and `status`.
//!
//! | Code | Class | Examples | systemd |
//! |---|---|---|---|
//! | 0 | Clean | SIGTERM or SIGINT served | does not restart |
//! | 75 | Blocking | another live instance, unusable socket, invalid environment or arguments, unsafe runtime directory | `RestartPreventExitStatus=75`: does not restart |
//! | 1 | Unexpected | unreadable configuration, unsafe data directory, storage, clock, internal protocol | `Restart=on-failure` with growing delay, at most 5 starts in 300 s |
//! | 101 | Unexpected | panic of the main thread (Rust's default code) | same as 1 |
//!
//! 75 is `EX_TEMPFAIL`, reserved for what keeps the runtime from serving requests and
//! that a restart cannot fix. Anything bound to one request, plugin or job is contained
//! where it happens ([`contained`]) and never reaches this table.
//!
//! Only unexpected exits are recorded in `last-exit.json`; for the other classes the unit
//! state already says what happened.
use crate::{
    private_file::{read_bounded, write_atomic},
    server::RuntimeError,
};
use serde::Deserialize;
use std::{
    io,
    path::{Path, PathBuf},
};

/// SIGTERM or SIGINT served.
pub const CLEAN: u8 = 0;
/// Anything not classified as clean or blocking.
pub const UNEXPECTED: u8 = 1;
/// Reserved (`EX_TEMPFAIL`): the unit lists it in `RestartPreventExitStatus=`.
pub const BLOCKING: u8 = 75;

/// File in the runtime's data directory with the last unexpected exit.
pub const FILE: &str = "last-exit.json";
const FILE_LIMIT: u64 = 4096;

/// Stable code of an unexpected exit; never prose, never data from the failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Startup,
    Storage,
    Transport,
    Clock,
    Protocol,
    Panic,
}
impl Reason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Storage => "storage",
            Self::Transport => "transport",
            Self::Clock => "clock",
            Self::Protocol => "protocol",
            Self::Panic => "panic",
        }
    }
    fn parse(code: &str) -> Option<Self> {
        [
            Self::Startup,
            Self::Storage,
            Self::Transport,
            Self::Clock,
            Self::Protocol,
            Self::Panic,
        ]
        .into_iter()
        .find(|reason| reason.code() == code)
    }
}

/// An end of the process other than a clean one, with the line printed to stderr.
#[derive(Debug, PartialEq, Eq)]
pub enum Exit {
    Blocking(&'static str),
    Unexpected(Reason, &'static str),
}
impl Exit {
    pub fn code(&self) -> u8 {
        match self {
            Self::Blocking(_) => BLOCKING,
            Self::Unexpected(..) => UNEXPECTED,
        }
    }
    /// What `last-exit.json` records; only unexpected exits are recorded.
    pub fn reason(&self) -> Option<Reason> {
        match self {
            Self::Blocking(_) => None,
            Self::Unexpected(reason, _) => Some(*reason),
        }
    }
    pub fn message(&self) -> &'static str {
        match self {
            Self::Blocking(message) | Self::Unexpected(_, message) => message,
        }
    }
}
impl From<RuntimeError> for Exit {
    fn from(error: RuntimeError) -> Self {
        match error {
            RuntimeError::Blocked => Self::Blocking(
                "no se pudo operar el socket/instancia privada; compruebe si RAD ya está ejecutándose",
            ),
            RuntimeError::Startup => Self::Unexpected(
                Reason::Startup,
                "no se pudo abrir la configuración. Material conservado: no se crea una configuración sustitutiva ni se mueve la base. Opciones: salir y conservar los archivos, o restaurar manualmente un respaldo válido con RAD detenido antes de reintentar.",
            ),
            RuntimeError::Transport => {
                Self::Unexpected(Reason::Transport, "falló el transporte local del runtime")
            }
            RuntimeError::Clock => {
                Self::Unexpected(Reason::Clock, "falló el reloj monotónico del sistema")
            }
            RuntimeError::Storage => Self::Unexpected(
                Reason::Storage,
                "falló el almacenamiento; conserve los archivos y compruebe el estado antes de reintentar",
            ),
            RuntimeError::Protocol => {
                Self::Unexpected(Reason::Protocol, "falló la integración interna del runtime")
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    code: String,
    at_unix_ms: u64,
}

/// Leaves `{"version":1,"code":…,"at_unix_ms":…}` as a private file, atomically.
pub fn record(data: &Path, reason: Reason, at_unix_ms: u64) -> io::Result<()> {
    let bytes = serde_json::to_vec(
        &serde_json::json!({"version":1,"code":reason.code(),"at_unix_ms":at_unix_ms}),
    )?;
    write_atomic(data, FILE, &bytes)
}

/// Reads and removes the record: whoever starts next consumes it. An unreadable record
/// is ignored and removed too; it never prevents the start.
pub fn take_previous(data: &Path) -> Option<rad_api::PreviousExit> {
    let path = data.join(FILE);
    let bytes = read_bounded(&path, FILE_LIMIT);
    let removed = std::fs::remove_file(&path);
    let record: Record = serde_json::from_slice(&bytes.ok()??).ok()?;
    if removed.is_err() || record.version != 1 {
        return None;
    }
    Some(rad_api::PreviousExit {
        code: Reason::parse(&record.code)?.code().to_owned(),
        at_unix_ms: record.at_unix_ms.to_string(),
    })
}

pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Records a panic of the thread that calls this, then runs the previous hook. The
/// panic message is not recorded: it can carry data.
pub fn install_panic_record(data: PathBuf) {
    let main = std::thread::current().id();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == main {
            let _ = record(&data, Reason::Panic, now_unix_ms());
        }
        previous(info);
    }));
}

/// The stderr line for a failure contained where it happened, with a stable code.
pub fn contained_line(code: &str) -> String {
    format!("rad-runtime: contenido {code}")
}

/// Reports a contained failure: the runtime keeps serving and the visible state says so.
pub fn contained(code: &str) {
    eprintln!("{}", contained_line(code));
}
