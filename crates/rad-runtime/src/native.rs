//! One native helper, driven by the runtime's shared readiness wait.
use rad_authority::{Authority, Effect};
use rad_core::execution::{Completion, Failure};
use rad_launcher::{LaunchOutcome, Launcher, LauncherError, monotonic_ms};
use rad_transport::Transport;
#[cfg(all(test, feature = "fixture-tests"))]
use std::time::Instant;
use std::{
    collections::VecDeque,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
};
pub struct Native {
    pub launcher: Launcher,
    plugins: crate::plugin_dispatch::Plugins,
    _exit: Option<OwnedFd>,
    writable: bool,
    plugin_io: PluginIo,
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
                Effect::ReleaseHold { execution } => {
                    for (id, result) in self.plugins.release_hold(actor, execution, monotonic_ms()?)
                    {
                        queue.extend(
                            actor
                                .job_complete(id, result)
                                .map_err(|_| LauncherError::Protocol)?,
                        );
                    }
                }
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
    /// Pumps plugins and the launcher. The flag is `true` when this call changed anything
    /// another step may depend on: a plugin binding changed state, an execution
    /// completed, or Authority produced effects. An idle pump returns `false`.
    pub fn pump(
        &mut self,
        actor: &mut Authority,
        transport: &Transport,
    ) -> Result<(Vec<Effect>, bool), LauncherError> {
        let now = monotonic_ms()?;
        let bindings_before = self.binding_states();
        let completed = self.plugins.pump(actor, now)?;
        let effects = actor.before_event(now).map_err(|_| LauncherError::Clock)?;
        let mut advanced = !completed.is_empty() || !effects.is_empty();
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
            advanced = true;
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
        // A plugin retired for unregistrable descriptors is a binding change too.
        self.plugin_io.sync(&mut self.plugins.host, transport);
        advanced |= self.binding_states() != bindings_before;
        Ok((output, advanced))
    }
}

/// Descriptors of the plugin processes that the shared wait already watches.
#[derive(Default)]
struct PluginIo {
    registered: std::collections::BTreeSet<(u64, i32)>,
}
impl PluginIo {
    /// Registers the descriptors that appeared since the last call. Whatever cannot be
    /// registered belongs to one plugin, so that plugin is retired and the rest go on.
    fn sync(&mut self, host: &mut rad_plugin_host::Host, transport: &Transport) {
        let mut live = std::collections::BTreeSet::new();
        let mut failed = std::collections::BTreeSet::new();
        for (generation, fd, write) in host.io_sources() {
            let raw = fd.as_raw_fd();
            let identity = (generation, raw);
            live.insert(identity);
            if !self.registered.contains(&identity)
                && transport
                    .register_worker_io(
                        &mut mio::unix::SourceFd(&raw),
                        if write {
                            mio::Interest::WRITABLE
                        } else {
                            mio::Interest::READABLE
                        },
                    )
                    .is_err()
            {
                failed.insert(generation);
            }
        }
        // A failed identity stays in `live`: it is not retried while the plugin winds down.
        // Closing a pipe/pidfd automatically removes its kernel registration.
        self.registered = live;
        for generation in failed {
            let id = host
                .bindings()
                .find(|(_, bound, _)| *bound == generation)
                .map(|(manifest, ..)| manifest.id.clone());
            if let Some(id) = id {
                crate::exit::contained("plugin_io_unregistrable");
                host.retire(&id);
            }
        }
    }
}

impl Native {
    pub(crate) fn sync_credentials(
        &mut self,
        actor: &Authority,
        extra: &[(String, u64, rad_authority::plugins::CredentialSnapshot)],
        transport: &Transport,
    ) {
        self.plugins.sync_credentials(actor, extra);
        self.plugin_io.sync(&mut self.plugins.host, transport);
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
    fn binding_states(&self) -> Vec<(String, u64, rad_plugin_host::State)> {
        self.plugins
            .host
            .bindings()
            .map(|(m, g, s)| (m.id.clone(), g, s))
            .collect()
    }
}

#[cfg(all(test, feature = "fixture-tests"))]
mod plugin_io_tests {
    use super::*;
    use rad_plugin_host::{Host, State};
    use std::{os::unix::fs::PermissionsExt, process::Command, time::Duration};

    fn binary(name: &str) -> std::path::PathBuf {
        std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(name)
    }

    fn manifest(id: &str) -> rad_plugin_contract::Manifest {
        serde_json::from_value(serde_json::json!({"contract":1,"capabilities":["controls"],"id":id,"version":{"major":1,"minor":0,"patch":0},"host_min":1,"host_max":1,"targets":[rad_plugin_host::TARGET],"actions":[{"id":"run","label":"Run","gestures":["hold"],"schema":{"version":1,"fields":[{"id":"mode","label":"Mode","required":true,"kind":{"type":"choice","options":["echo","end_fail"]}}]},"credential_slots":[]}],"visuals":[]})).unwrap()
    }

    #[test]
    fn a_plugin_whose_descriptors_cannot_be_registered_is_retired_and_the_others_go_on() {
        const BROKEN: &str = "org.rad.fixture";
        const HEALTHY: &str = "org.rad.test.options";
        let runtime = std::env::temp_dir().join(format!("rad-plugin-io-{}", std::process::id()));
        std::fs::create_dir(&runtime).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        let transport = Transport::bind(&runtime, 7).unwrap();
        let mut host = Host::default();
        let broken = host
            .activate(
                Command::new(binary("rad-plugin-fixture-v1")),
                manifest(BROKEN),
                0,
            )
            .unwrap();
        let healthy_manifest = serde_json::from_slice(include_bytes!(
            "../../rad-plugin-host/test-plugins/options-manifest.json"
        ))
        .unwrap();
        let healthy = host
            .activate(
                Command::new(binary("rad-plugin-options-fixture")),
                healthy_manifest,
                0,
            )
            .unwrap();
        // The kernel refuses a second registration of the same descriptor.
        let register = |host: &Host, generation: u64| {
            let (_, fd, _) = host
                .io_sources()
                .into_iter()
                .find(|(g, _, _)| *g == generation)
                .unwrap();
            transport.register_worker_io(
                &mut mio::unix::SourceFd(&fd.as_raw_fd()),
                mio::Interest::READABLE,
            )
        };
        register(&host, broken).unwrap();
        let mut io = PluginIo::default();
        io.sync(&mut host, &transport);
        let end = Instant::now() + Duration::from_secs(5);
        let mut now = 0;
        while host.state(BROKEN) != Some(State::Reaped) {
            assert!(Instant::now() < end, "the broken plugin must be retired");
            now += 1;
            host.pump(now).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_ne!(host.state(HEALTHY), Some(State::Reaped));
        assert!(
            register(&host, healthy).is_err(),
            "the healthy plugin's descriptors are registered"
        );
        // Nothing more to do, and nothing fails, once the broken one is gone.
        io.sync(&mut host, &transport);
        host.shutdown();
        while host.resources().0 > 0 {
            assert!(Instant::now() < end);
            now += 1;
            host.pump(now).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        let _ = std::fs::remove_dir_all(&runtime);
    }
}
