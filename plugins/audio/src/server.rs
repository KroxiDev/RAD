use crate::{
    Pactl, Snapshot,
    devices::{Kind, parse_defaults, parse_devices},
    pactl::RunError,
};
use rad_plugin_sdk::FailureCode;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    Unavailable,
    TimedOut,
    Failed,
}

impl Failure {
    pub(crate) fn code(self) -> FailureCode {
        match self {
            Self::Unavailable => FailureCode::Unavailable,
            Self::TimedOut => FailureCode::TimedOut,
            Self::Failed => FailureCode::Failed,
        }
    }
}

impl From<RunError> for Failure {
    fn from(error: RunError) -> Self {
        match error {
            RunError::Spawn(_) | RunError::Failed => Self::Unavailable,
            RunError::TimedOut => Self::TimedOut,
            RunError::TooMuchOutput | RunError::Io(_) => Self::Failed,
        }
    }
}

pub(crate) fn snapshot(pactl: &Pactl, deadline: Instant) -> Result<Snapshot, Failure> {
    let sinks = pactl.run(&["-f", "json", "list", "sinks"], deadline)?;
    let sources = pactl.run(&["-f", "json", "list", "sources"], deadline)?;
    let info = pactl.run(&["-f", "json", "info"], deadline)?;
    let parse = |_| Failure::Failed;
    let (default_sink, default_source) = parse_defaults(&info).map_err(parse)?;
    Ok(Snapshot {
        sinks: parse_devices(&sinks, Kind::Output).map_err(parse)?,
        sources: parse_devices(&sources, Kind::Microphone).map_err(parse)?,
        default_sink,
        default_source,
    })
}
