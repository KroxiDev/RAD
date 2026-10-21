//! Startup in two steps (ADR 0011). First the instance lock and the sockets, which both
//! modes keep; then the data folder, which decides between serving and recovery mode.
use crate::{
    bootstrap::{self, Diagnosis},
    events::Events,
    exit,
    recovery::Recovery,
    server::{RuntimeError, Server},
};
use rad_api::EventCode;
use rad_launcher::monotonic_ms;
use rad_transport::{Transport, TransportError};
use std::{num::NonZeroU128, path::Path};

pub enum Opened {
    Serving(Box<Server>),
    Recovery(Box<Recovery>),
}

/// The instance lock and the sockets, before anything in the data folder is touched.
pub struct Bound {
    transport: Transport,
    session: NonZeroU128,
}
impl Bound {
    /// The instance lock precedes opening the Store, so a second process cannot reaffirm it.
    pub fn bind(runtime: &Path, session: NonZeroU128) -> Result<Self, RuntimeError> {
        let transport = Transport::bind(runtime, session.get()).map_err(|error| match error {
            // What keeps requests from being served, and no restart fixes.
            TransportError::InstanceBusy | TransportError::UnsafePath | TransportError::Io => {
                RuntimeError::Blocked
            }
            _ => RuntimeError::Transport,
        })?;
        Ok(Self { transport, session })
    }
    /// Recovery mode for a data folder that was refused before anything in it was read.
    /// Nothing is written in `data`.
    pub fn unsafe_folder(self, data: &Path) -> Recovery {
        Recovery::new(
            self.transport,
            self.session,
            data,
            Diagnosis::UNSAFE_DIRECTORY,
            None,
        )
    }
    /// Opens the Store in `data`: the server when it opens, recovery mode when it cannot be
    /// opened for a reason recovery handles, an error for anything unexpected.
    pub fn open(self, data: &Path) -> Result<Opened, RuntimeError> {
        let now = monotonic_ms().map_err(|_| RuntimeError::Clock)?;
        match bootstrap::open(data, self.session, now) {
            Ok(boot) => {
                // After the instance lock, so a refused second instance never consumes the record.
                let previous_exit = exit::take_previous(data);
                let mut events = Events::new();
                events.record(EventCode::Started, None);
                Server::from_parts(
                    self.transport,
                    boot,
                    data,
                    self.session,
                    previous_exit,
                    events,
                )
                .map(|server| Opened::Serving(Box::new(server)))
            }
            Err(error) => {
                let diagnosis = error.diagnosis().ok_or(RuntimeError::Startup)?;
                // Only an unreadable file has counters worth reading; the read writes nothing.
                let peeked = (diagnosis.cause == rad_api::RecoveryCause::Unreadable)
                    .then(|| rad_store::peek_counters(&data.join(bootstrap::DATABASE)))
                    .flatten();
                Ok(Opened::Recovery(Box::new(Recovery::new(
                    self.transport,
                    self.session,
                    data,
                    diagnosis,
                    peeked,
                ))))
            }
        }
    }
}
