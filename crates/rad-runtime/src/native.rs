//! One native helper, driven by the runtime's shared readiness wait.
use rad_authority::{Authority, Effect};
use rad_core::execution::{Completion, Failure};
use rad_launcher::{LaunchOutcome, Launcher, LauncherError, monotonic_ms};
use rad_transport::Transport;
use std::{
    collections::VecDeque,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
};
pub struct Native {
    pub launcher: Launcher,
    plugins: crate::plugin_dispatch::Plugins,
    _exit: Option<OwnedFd>,
    writable: bool,
    plugin_io: std::collections::BTreeSet<(u64, i32)>,
}
impl Native {
    pub fn attach(launcher: Launcher, transport: &Transport) -> Result<Self, LauncherError> {
        let pid = launcher.process_id().ok_or(LauncherError::Unavailable)?;
        // SAFETY: pid is our retained, unreaped child, flags0; returns a new owned fd.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            return Err(LauncherError::Io);
        }
        // SAFETY: successful pidfd_open returns an exclusive, valid descriptor.
        let exit = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        let writable = launcher.needs_write();
        let interest = if writable {
            mio::Interest::READABLE | mio::Interest::WRITABLE
        } else {
            mio::Interest::READABLE
        };
        transport
            .register_worker_io(
                &mut mio::unix::SourceFd(&launcher.as_fd().as_raw_fd()),
                interest,
            )
            .map_err(|_| LauncherError::Io)?;
        transport
            .register_worker_io(
                &mut mio::unix::SourceFd(&exit.as_raw_fd()),
                mio::Interest::READABLE,
            )
            .map_err(|_| LauncherError::Io)?;
        Ok(Self {
            launcher,
            plugins: Default::default(),
            _exit: Some(exit),
            writable,
            plugin_io: Default::default(),
        })
    }
    pub fn route(
        &mut self,
        actor: &mut Authority,
        effects: Vec<Effect>,
    ) -> Result<Vec<Effect>, LauncherError> {
        let mut queue: VecDeque<_> = effects.into();
        let mut output = vec![];
        while let Some(effect) = queue.pop_front() {
            match effect {
                Effect::Dispatch(intent) => {
                    let submitted = match &intent.operation {
                        rad_core::execution::Operation::External { .. } => {
                            self.plugins.submit(actor, &intent)
                        }
                        rad_core::execution::Operation::Launcher(_) => {
                            self.launcher.submit(&intent)
                        }
                        _ => Err(LauncherError::Protocol),
                    };
                    if let Err(error) = submitted {
                        let failure = if error == LauncherError::Unavailable {
                            Failure::ServiceUnavailable
                        } else {
                            Failure::ActionFailed
                        };
                        queue.extend(
                            actor
                                .before_event(monotonic_ms()?)
                                .map_err(|_| LauncherError::Clock)?,
                        );
                        queue.extend(
                            actor
                                .job_complete(intent.id, Completion::Failed(failure))
                                .map_err(|_| LauncherError::Protocol)?,
                        );
                    }
                }
                other => output.push(other),
            }
        }
        Ok(output)
    }
    pub fn pump(
        &mut self,
        actor: &mut Authority,
        transport: &Transport,
    ) -> Result<Vec<Effect>, LauncherError> {
        let now = monotonic_ms()?;
        let completed = self.plugins.pump(actor, now)?;
        let effects = actor.before_event(now).map_err(|_| LauncherError::Clock)?;
        let mut output = self.route(actor, effects)?;
        for (id, result) in completed {
            let effects = actor
                .job_complete(id, result)
                .map_err(|_| LauncherError::Protocol)?;
            output.extend(self.route(actor, effects)?);
        }
        if let Err(error) = self.launcher.pump()
            && error != LauncherError::Capacity
        {
            // Do not release jobs merely on channel error. Reap proves helper loss.
            let _ = self.launcher.shutdown();
        }
        while let Some((id, result)) = self.launcher.take_completion() {
            let effects = actor
                .before_event(monotonic_ms()?)
                .map_err(|_| LauncherError::Clock)?;
            output.extend(self.route(actor, effects)?);
            let result = match result {
                LaunchOutcome::Accepted => Completion::Succeeded,
                LaunchOutcome::Expired | LaunchOutcome::Failed | LaunchOutcome::HelperLost => {
                    Completion::Failed(Failure::ActionFailed)
                }
            };
            let effects = actor
                .job_complete(id, result)
                .map_err(|_| LauncherError::Protocol)?;
            output.extend(self.route(actor, effects)?);
        }
        // Closing our reaped helper's pidfd removes its permanent exit readiness.
        if self.launcher.process_id().is_none() {
            self._exit.take();
        }
        let writable = self.launcher.needs_write();
        if self.writable != writable {
            let interest = if writable {
                mio::Interest::READABLE | mio::Interest::WRITABLE
            } else {
                mio::Interest::READABLE
            };
            transport
                .reregister_worker_io(
                    &mut mio::unix::SourceFd(&self.launcher.as_fd().as_raw_fd()),
                    interest,
                )
                .map_err(|_| LauncherError::Io)?;
            self.writable = writable;
        }
        self.register_plugin_io(transport)?;
        Ok(output)
    }
}

impl Native {
    fn register_plugin_io(&mut self, transport: &Transport) -> Result<(), LauncherError> {
        let mut live = std::collections::BTreeSet::new();
        for (generation, fd, write) in self.plugins.host.io_sources() {
            let raw = fd.as_raw_fd();
            let identity = (generation, raw);
            live.insert(identity);
            if !self.plugin_io.contains(&identity) {
                transport
                    .register_worker_io(
                        &mut mio::unix::SourceFd(&raw),
                        if write {
                            mio::Interest::WRITABLE
                        } else {
                            mio::Interest::READABLE
                        },
                    )
                    .map_err(|_| LauncherError::Io)?;
            }
        }
        // Closing a pipe/pidfd automatically removes its kernel registration.
        self.plugin_io = live;
        Ok(())
    }
    pub(crate) fn sync_credentials(
        &mut self,
        actor: &Authority,
        extra: &[(String, u64, rad_authority::plugins::CredentialSnapshot)],
        transport: &Transport,
    ) -> Result<(), LauncherError> {
        self.plugins.sync_credentials(actor, extra);
        self.register_plugin_io(transport)
    }
    pub(crate) fn plugin_host_mut(&mut self) -> &mut rad_plugin_host::Host {
        &mut self.plugins.host
    }
    pub fn plugin_host(&self) -> &rad_plugin_host::Host {
        &self.plugins.host
    }
    pub fn next_plugin_poll(&self) -> Option<u64> {
        self.plugins.host.next_poll()
    }
    pub fn shutdown_plugins(&mut self) {
        self.plugins.host.shutdown();
    }
    pub fn plugins_stopped(&self) -> bool {
        self.plugins.host.resources().0 == 0
    }
}
