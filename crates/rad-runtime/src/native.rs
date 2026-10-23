//! One native helper, driven by the runtime's shared readiness wait.
use crate::events::Occurrence;
use rad_api::EventCode;
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
    events: Vec<Occurrence>,
    /// The launcher's end has been reported or was asked for: it is not reported (again).
    launcher_settled: bool,
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
            events: Vec::new(),
            launcher_settled: false,
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
            self.lose_launcher();
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
            // Gone without being asked to stop: the helper died on its own.
            self.report_launcher_lost();
        }
        if self.follow_launcher_interest(transport).is_err() {
            self.lose_launcher();
        }
        // A plugin retired for unregistrable descriptors is a binding change too.
        self.plugin_io
            .sync(&mut self.plugins.host, transport, &mut self.events);
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
    fn sync(
        &mut self,
        host: &mut rad_plugin_host::Host,
        transport: &Transport,
        events: &mut Vec<Occurrence>,
    ) {
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
                events.push(Occurrence::new(EventCode::PluginIoUnregistrable, Some(&id)));
                host.retire(&id);
            }
        }
    }
}

impl Native {
    /// Keeps the helper socket's readiness interest in step with whether it has something to
    /// write. A descriptor that cannot be re-registered cannot be waited on, so the caller
    /// treats it as a lost helper.
    fn follow_launcher_interest(&mut self, transport: &Transport) -> Result<(), LauncherError> {
        let writable = self.launcher.needs_write();
        if self.writable == writable {
            return Ok(());
        }
        // Updated before the attempt: a failure is acted on once, not retried every pump.
        self.writable = writable;
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
            .map_err(|_| LauncherError::Io)
    }
    /// The helper can no longer be trusted: end it, and say so once. Its jobs finish through
    /// the usual `HelperLost` path when the reap is observed.
    fn lose_launcher(&mut self) {
        self.report_launcher_lost();
        let _ = self.launcher.shutdown();
    }
    fn report_launcher_lost(&mut self) {
        if !self.launcher_settled {
            self.launcher_settled = true;
            self.events
                .push(Occurrence::new(EventCode::LauncherLost, None));
        }
    }
    pub(crate) fn sync_credentials(
        &mut self,
        actor: &Authority,
        extra: &[(String, u64, rad_authority::plugins::CredentialSnapshot)],
        transport: &Transport,
    ) {
        self.plugins.sync_credentials(actor, extra);
        self.plugin_io
            .sync(&mut self.plugins.host, transport, &mut self.events);
    }
    /// What happened below the server since the last call, for it to record.
    pub(crate) fn take_events(&mut self) -> Vec<Occurrence> {
        let mut events = std::mem::take(&mut self.events);
        events.append(&mut self.plugins.events);
        events
    }
    /// The launcher is stopped on purpose (shutdown): its end is not a loss.
    pub(crate) fn stop_launcher(&mut self) {
        self.launcher_settled = true;
        let _ = self.launcher.shutdown();
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
        let mut events = Vec::new();
        io.sync(&mut host, &transport, &mut events);
        assert_eq!(
            events,
            [Occurrence::new(
                rad_api::EventCode::PluginIoUnregistrable,
                Some(BROKEN)
            )]
        );
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
        io.sync(&mut host, &transport, &mut events);
        assert_eq!(events.len(), 1, "a retired plugin is not reported again");
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

#[cfg(all(test, feature = "fixture-tests"))]
mod launcher_loss_tests {
    use super::*;
    use rad_api::EventCode;
    use rad_core::configuration::Configuration;
    use rad_core::identity::IdentityRegistry;
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    struct Fixture {
        _helper: std::sync::MutexGuard<'static, ()>,
        _directory: Directory,
        transport: Transport,
        other: Transport,
        actor: Authority,
        native: Native,
    }
    /// A real helper under a private environment, attached to `transport`; `other` is a
    /// second transport the helper's descriptor is not registered with.
    fn fixture(name: &str) -> Fixture {
        let helper = crate::server::FIXTURE_RUNTIME
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let directory = Directory(
            std::env::temp_dir().join(format!("rad-launcher-loss-{name}-{}", std::process::id())),
        );
        for sub in [
            "home", "data", "system", "config", "cache", "runtime", "other",
        ] {
            std::fs::create_dir_all(directory.0.join(sub)).unwrap();
        }
        for sub in ["runtime", "other"] {
            std::fs::set_permissions(
                directory.0.join(sub),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        let mut command = std::process::Command::new(
            std::env::var("RAD_TEST_LAUNCHER_HELPER").expect("runner builds the helper"),
        );
        command
            .env_clear()
            .env("HOME", directory.0.join("home"))
            .env("XDG_DATA_HOME", directory.0.join("data"))
            .env("XDG_DATA_DIRS", directory.0.join("system"))
            .env("XDG_CONFIG_HOME", directory.0.join("config"))
            .env("XDG_CACHE_HOME", directory.0.join("cache"))
            .env("XDG_RUNTIME_DIR", directory.0.join("runtime"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent-rad-private-bus",
            )
            .env("PATH", "/nonexistent-rad-private-bin");
        let transport = Transport::bind(&directory.0.join("runtime"), 31).unwrap();
        let other = Transport::bind(&directory.0.join("other"), 32).unwrap();
        let native = Native::attach(Launcher::spawn(command).unwrap(), &transport).unwrap();
        let actor = Authority::new(
            Configuration::prepare_new(&IdentityRegistry::new(9).unwrap()).unwrap(),
            1,
            std::num::NonZeroU128::new(33).unwrap(),
        );
        Fixture {
            _helper: helper,
            _directory: directory,
            transport,
            other,
            actor,
            native,
        }
    }
    fn pump_until_gone(fixture: &mut Fixture, transport_of_other: bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while fixture.native.launcher.process_id().is_some() {
            assert!(Instant::now() < end, "the helper must be reaped");
            let transport = if transport_of_other {
                &fixture.other
            } else {
                &fixture.transport
            };
            fixture
                .native
                .pump(&mut fixture.actor, transport)
                .expect("the launcher's end is not an error of the runtime");
            std::thread::sleep(Duration::from_millis(2));
        }
        fixture
            .native
            .pump(&mut fixture.actor, &fixture.transport)
            .unwrap();
    }
    fn lost(fixture: &mut Fixture) -> usize {
        fixture
            .native
            .take_events()
            .iter()
            .filter(|event| event.code == EventCode::LauncherLost)
            .count()
    }

    #[test]
    fn a_helper_descriptor_that_cannot_be_reregistered_is_a_lost_launcher_not_a_runtime_error() {
        let mut fixture = fixture("reregister");
        // The helper wants to write, but this transport never saw its descriptor.
        fixture.native.writable = !fixture.native.launcher.needs_write();
        fixture
            .native
            .pump(&mut fixture.actor, &fixture.other)
            .expect("contained: the launcher is shut down, the runtime goes on");
        pump_until_gone(&mut fixture, true);
        assert_eq!(lost(&mut fixture), 1);
    }

    #[test]
    fn a_helper_that_dies_on_its_own_is_reported_lost_once() {
        let mut fixture = fixture("killed");
        let pid = fixture.native.launcher.process_id().unwrap();
        // The helper is the child this test spawned.
        assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
        pump_until_gone(&mut fixture, false);
        assert_eq!(lost(&mut fixture), 1);
        fixture
            .native
            .pump(&mut fixture.actor, &fixture.transport)
            .unwrap();
        assert_eq!(lost(&mut fixture), 0, "reported once");
    }

    #[test]
    fn a_helper_stopped_on_purpose_is_not_a_loss() {
        let mut fixture = fixture("stopped");
        fixture.native.stop_launcher();
        pump_until_gone(&mut fixture, false);
        assert_eq!(lost(&mut fixture), 0);
    }
}
