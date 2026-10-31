//! Native plugin routing, separate from Authority and the launcher process.
use rad_authority::Authority;
use rad_core::execution::{Completion, DispatchId, Failure};
use rad_plugin_host::{Host, State};
use std::collections::BTreeMap;
#[derive(Default)]
pub(super) struct Plugins {
    pub host: Host,
    jobs: BTreeMap<(u64, u64), DispatchId>,
    observed: BTreeMap<String, (u64, State)>,
    deferred: BTreeMap<DispatchId, rad_core::execution::DispatchIntent>,
    credentials: std::collections::BTreeSet<(String, u64, u64)>,
    holds: BTreeMap<DispatchId, (rad_plugin_host::HoldTicket, u64)>,
    end_budgets: BTreeMap<DispatchId, u64>,
    hold_ends: BTreeMap<(u64, u64), rad_core::execution::ExecutionId>,
    releasing: std::collections::BTreeSet<rad_core::execution::ExecutionId>,
    /// Events for the server to record; taken by `Native::take_events`.
    pub events: Vec<crate::events::Occurrence>,
}
impl Plugins {
    pub fn sync_credentials(
        &mut self,
        actor: &Authority,
        extra: &[(String, u64, rad_authority::plugins::CredentialSnapshot)],
    ) {
        while let Some(key) = self.host.take_credentials_released() {
            self.credentials.remove(&key);
        }
        let bindings: Vec<_> = self
            .host
            .bindings()
            .map(|(m, g, s)| (m.id.clone(), g, s))
            .collect();
        self.credentials.retain(|(id, g, _)| {
            bindings
                .iter()
                .any(|(p, generation, state)| p == id && generation == g && *state != State::Reaped)
        });
        let mut desired = BTreeMap::new();
        for (id, generation, state) in bindings {
            // Admin may not have published a newly Ready host yet. Wait until both
            // current and captured options revisions resolve to the same process,
            // otherwise a newer current map can overtake an older capture.
            if state != State::Ready || actor.plugin_generation(&id) != Some(generation) {
                continue;
            }
            let selection = actor
                .plugin_manifests()
                .find(|(m, _, _)| m.id == id)
                .map(|(_, s, _)| s);
            let Some(selection) = selection else {
                continue;
            };
            for snapshot in actor.retained_plugin_credentials(&id, selection) {
                desired.insert((id.clone(), generation, snapshot.revision()), snapshot);
            }
        }
        for (id, selection, snapshot) in extra {
            if !actor.plugin_available(id, *selection) {
                continue;
            }
            let Some(generation) = actor.plugin_generation(id) else {
                continue;
            };
            desired.insert(
                (id.clone(), generation, snapshot.revision()),
                snapshot.clone(),
            );
        }
        for (id, generation, revision) in &self.credentials {
            if !desired.contains_key(&(id.clone(), *generation, *revision))
                && self.host.credentials_state(id, *generation, *revision)
                    == Some(rad_plugin_host::CredentialState::Applied)
            {
                let _ = self.host.release_credentials(id, *generation, *revision);
            }
        }
        // BTree order installs older execution captures before a newer current map.
        for ((id, generation, revision), snapshot) in desired {
            if !self
                .credentials
                .contains(&(id.clone(), generation, revision))
                && self
                    .host
                    .install_credentials(&id, generation, revision, snapshot.values().clone())
                    .is_ok()
            {
                self.credentials.insert((id, generation, revision));
            }
        }
    }

    pub fn submit(
        &mut self,
        actor: &Authority,
        intent: &rad_core::execution::DispatchIntent,
    ) -> Result<(), rad_launcher::LauncherError> {
        if intent
            .control
            .is_some_and(|c| c.gesture == rad_core::input::ControlGesture::HoldStart)
        {
            self.end_budgets.insert(
                intent.id,
                intent
                    .deadline_ms
                    .saturating_sub(actor.input_clock_ms())
                    .max(1),
            );
        }
        let sent = match self.send(actor, intent) {
            Ok(sent) => sent,
            Err(error) => {
                self.end_budgets.remove(&intent.id);
                return Err(error);
            }
        };
        if !sent {
            // Every entry owns an existing engine job credit, including deliberate waits.
            if self.deferred.len() >= rad_core::execution::MAX_JOBS {
                self.end_budgets.remove(&intent.id);
                return Err(rad_launcher::LauncherError::Capacity);
            }
            self.deferred.insert(intent.id, intent.clone());
        }
        Ok(())
    }
    fn send(
        &mut self,
        actor: &Authority,
        intent: &rad_core::execution::DispatchIntent,
    ) -> Result<bool, rad_launcher::LauncherError> {
        use rad_launcher::LauncherError;
        let rad_core::execution::Operation::External {
            type_key,
            parameters,
        } = &intent.operation
        else {
            return Err(LauncherError::Protocol);
        };
        let (plugin, selection) = actor
            .job_plugin_selection(intent.id)
            .ok_or(LauncherError::Unavailable)?;
        let Some((_, generation)) = actor.job_plugin(intent.id) else {
            return if actor.plugin_available(plugin, selection) {
                Ok(false)
            } else {
                Err(LauncherError::Unavailable)
            };
        };
        if self.host.state(plugin) != Some(State::Ready) {
            return Err(LauncherError::Unavailable);
        }
        let snapshot = actor.job_credentials(intent.id);
        let revision = snapshot.as_ref().map(|s| s.revision());
        if let Some(revision) = revision {
            if self.host.state(plugin) != Some(State::Ready) {
                return Err(LauncherError::Unavailable);
            }
            if self.host.credentials_state(plugin, generation, revision)
                != Some(rad_plugin_host::CredentialState::Applied)
            {
                return Ok(false);
            }
        }
        let params = rad_wire::parse_json(parameters)
            .and_then(|v| serde_json::from_value(v).map_err(|_| rad_wire::JsonError))
            .map_err(|_| LauncherError::Protocol)?;
        let settings = actor
            .job_settings(intent.id)
            .map(|bytes| {
                rad_wire::parse_json(&bytes)
                    .and_then(|v| serde_json::from_value(v).map_err(|_| rad_wire::JsonError))
            })
            .transpose()
            .map_err(|_| LauncherError::Protocol)?;
        let physical = intent.control.filter(|event| {
            event.gesture != rad_core::input::ControlGesture::Press
                || self.host.negotiated_capability(
                    plugin,
                    generation,
                    rad_plugin_contract::CONTROLS,
                )
        });
        let control = physical.map(|event| rad_plugin_contract::ControlEvent {
            control: rad_plugin_contract::Control {
                kind: "key".into(),
                index: event.key as u32,
            },
            gesture: match event.gesture {
                rad_core::input::ControlGesture::Press => {
                    rad_plugin_contract::ControlGesture::Press
                }
                rad_core::input::ControlGesture::LongPress => {
                    rad_plugin_contract::ControlGesture::LongPress
                }
                rad_core::input::ControlGesture::HoldStart => {
                    rad_plugin_contract::ControlGesture::HoldStart
                }
                rad_core::input::ControlGesture::HoldEnd => {
                    rad_plugin_contract::ControlGesture::HoldEnd
                }
            },
        });
        let context = rad_plugin_host::InvokeContext { settings, control };
        let key = if intent
            .control
            .is_some_and(|c| c.gesture == rad_core::input::ControlGesture::HoldStart)
        {
            let (key, ticket) = self
                .host
                .invoke_hold_start(
                    plugin,
                    generation,
                    type_key.name(),
                    params,
                    (intent.deadline_ms, revision),
                    context,
                )
                .map_err(|_| LauncherError::Unavailable)?;
            self.holds.insert(
                intent.id,
                (
                    ticket,
                    self.end_budgets
                        .remove(&intent.id)
                        .expect("captured Start response budget"),
                ),
            );
            key
        } else {
            self.host
                .invoke_with_context(
                    plugin,
                    generation,
                    type_key.name(),
                    params,
                    (intent.deadline_ms, revision),
                    context,
                )
                .map_err(|_| LauncherError::Unavailable)?
        };
        self.jobs.insert((key.generation, key.request), intent.id);
        Ok(true)
    }
    /// Records the lifecycle observation independently of how many pump turns
    /// separate a failure from its eventual reap.
    fn observe_state(
        &mut self,
        actor: &mut Authority,
        plugin: &str,
        generation: u64,
        state: State,
        stopped_normally: bool,
    ) {
        let previous = self.observed.insert(plugin.into(), (generation, state));
        let already_failed = previous.is_some_and(|(previous_generation, previous_state)| {
            previous_generation == generation
                && matches!(previous_state, State::Failed | State::Reaped)
        });
        if matches!(state, State::Failed | State::Reaped) && !stopped_normally && !already_failed {
            actor.plugin_failed(plugin, generation);
            self.events.push(crate::events::Occurrence::new(
                rad_api::EventCode::PluginFailed,
                Some(plugin),
            ));
        }
    }
    pub fn pump(
        &mut self,
        actor: &mut Authority,
        now: u64,
    ) -> Result<Vec<(DispatchId, Completion)>, rad_launcher::LauncherError> {
        self.host
            .pump(now)
            .map_err(|_| rad_launcher::LauncherError::Clock)?;
        let bindings: Vec<_> = self
            .host
            .bindings()
            .filter(|(m, g, s)| self.observed.get(&m.id) != Some(&(*g, *s)))
            .map(|(m, g, s)| (m.clone(), g, s))
            .collect();
        self.observed.retain(|id, _| self.host.state(id).is_some());
        for (m, g, s) in bindings {
            let stopped_normally = self.host.stopped_normally(&m.id);
            self.observe_state(actor, &m.id, g, s, stopped_normally);
        }
        let mut done = vec![];
        for (id, intent) in std::mem::take(&mut self.deferred) {
            if now >= intent.deadline_ms {
                self.end_budgets.remove(&id);
                done.push((id, Completion::Failed(Failure::ServiceUnavailable)));
                continue;
            }
            match self.send(actor, &intent) {
                Ok(true) => {}
                Ok(false) => {
                    self.deferred.insert(id, intent);
                }
                Err(_) => {
                    self.end_budgets.remove(&id);
                    done.push((id, Completion::Failed(Failure::ServiceUnavailable)));
                }
            }
        }

        while let Some((key, outcome)) = self.host.take_completion() {
            if let Some(execution) = self.hold_ends.remove(&(key.generation, key.request))
                && !matches!(outcome, rad_plugin_contract::Outcome::Success)
            {
                actor.report_hold_end_failure(execution);
            }
            if let Some(id) = self.jobs.remove(&(key.generation, key.request)) {
                let result = match outcome {
                    rad_plugin_contract::Outcome::Success => Completion::Succeeded,
                    rad_plugin_contract::Outcome::Failure {
                        code: rad_plugin_contract::FailureCode::Unavailable,
                    } => Completion::Failed(Failure::ServiceUnavailable),
                    // Host's deadline and a plugin's own timeout are both the action timing out.
                    rad_plugin_contract::Outcome::Failure {
                        code: rad_plugin_contract::FailureCode::TimedOut,
                    } => Completion::TimedOut,
                    _ => Completion::Failed(Failure::ActionFailed),
                };
                done.push((id, result));
            }
        }
        self.finish_releases(actor);
        Ok(done)
    }
    pub fn release_hold(
        &mut self,
        actor: &mut Authority,
        execution: rad_core::execution::ExecutionId,
        now: u64,
    ) -> Vec<(DispatchId, Completion)> {
        self.releasing.insert(execution);
        let canceled: Vec<_> = self
            .deferred
            .keys()
            .copied()
            .filter(|id| id.execution() == execution)
            .collect();
        let mut done = vec![];
        for id in canceled {
            self.deferred.remove(&id);
            self.end_budgets.remove(&id);
            done.push((id, Completion::Failed(Failure::ActionFailed)));
        }
        let sent: Vec<_> = self
            .holds
            .keys()
            .copied()
            .filter(|id| id.execution() == execution)
            .collect();
        for id in sent {
            let (ticket, budget) = self.holds.remove(&id).expect("collected hold");
            // At the clock ceiling Host terminally fails the original process rather
            // than silently dropping release ownership for a live effect.
            match self.host.release_hold(ticket, now.saturating_add(budget)) {
                Ok(key) => {
                    self.hold_ends
                        .insert((key.generation, key.request), execution);
                }
                Err(_) => actor.report_hold_end_failure(execution),
            }
        }
        self.finish_releases(actor);
        done
    }
    fn finish_releases(&mut self, actor: &mut Authority) {
        let finished: Vec<_> = self
            .releasing
            .iter()
            .copied()
            .filter(|execution| !self.hold_ends.values().any(|e| e == execution))
            .collect();
        for execution in finished {
            self.releasing.remove(&execution);
            actor.hold_released(execution);
        }
    }
}

#[cfg(all(test, feature = "fixture-tests"))]
mod credential_tests {
    use super::*;

    use rad_authority::{Authority, Command, Effect};
    use rad_core::{
        configuration::{ActionKind, Configuration, Edit, ExternalType, Origin},
        execution::{DispatchIntent, Target},
        identity::{EntityIdentity, IdentityRegistry},
    };
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    fn setup(plugin: &str) -> (Authority, Plugins, EntityIdentity, u64) {
        let mut c = Configuration::prepare_new(&IdentityRegistry::new(910).unwrap()).unwrap();
        let root = c.configuration().data().principal;
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::CreateButton {
                    folder: root,
                    position: 0,
                    name: "Credentials".into(),
                },
            )
            .unwrap();
        let button = c.configuration().data().buttons[0].binding.identity();
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::AddAction {
                    button,
                    kind: ActionKind::External {
                        type_key: ExternalType::new(plugin.into(), "authenticate".into()).unwrap(),
                        parameters: br#"{"schema_version":1,"values":{}}"#.to_vec(),
                    },
                },
            )
            .unwrap();
        let mut actor = Authority::new(c, 1, std::num::NonZeroU128::new(911).unwrap());
        let manifest: rad_plugin_contract::Manifest = serde_json::from_value(serde_json::json!({"contract":1,"capabilities":["credentials","dynamic_options","visual_freshness"],"id":plugin,"version":{"major":1,"minor":0,"patch":0},"host_min":1,"host_max":1,"targets":[rad_plugin_host::TARGET],"actions":[{"id":"authenticate","label":"Authenticate","schema":{"version":1,"fields":[{"id":"invocations","label":"Invocations","required":false,"kind":{"type":"dynamic_choice","depends_on":[]}}]},"credential_slots":["password"]}],"visuals":[]})).unwrap();
        let mut plugins = Plugins::default();
        let launch = plugins.host.reserve_launch(manifest.clone(), 0).unwrap();
        let bin = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("rad-plugin-credentials-fixture");
        let mut prepared = Some(launch.spawn(std::process::Command::new(bin)));
        let generation = plugins.host.attach(&mut prepared, 0).unwrap();
        let end = Instant::now() + Duration::from_secs(3);
        while plugins.host.state(&manifest.id) != Some(State::Ready) {
            plugins.pump(&mut actor, 0).unwrap();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        actor
            .install_plugin(manifest, generation, rad_plugin_host::TARGET)
            .unwrap();
        actor
            .publish_plugin_credentials(
                plugin,
                generation,
                1,
                Arc::new(
                    rad_plugin_contract::CredentialValues::new(vec![
                        rad_plugin_contract::CredentialValue {
                            slot: "password".into(),
                            value: "correct".into(),
                        },
                    ])
                    .unwrap(),
                ),
            )
            .unwrap();
        (actor, plugins, button, generation)
    }
    fn run(actor: &mut Authority, button: EntityIdentity) -> DispatchIntent {
        let ticket = actor.reserve(1, Origin::Editor).unwrap();
        actor
            .submit(
                ticket,
                Command::Run {
                    target: Target::Button(button),
                },
            )
            .into_iter()
            .find_map(|e| {
                if let Effect::Dispatch(d) = e {
                    Some(d)
                } else {
                    None
                }
            })
            .unwrap()
    }

    #[test]
    fn expired_dispatch_never_invokes_after_a_valid_late_credentials_ack() {
        let plugin = "org.rad.test.credentials.deferred";
        let (mut actor, mut plugins, button, generation) = setup(plugin);
        let mut intent = run(&mut actor, button);
        // Adapter deadline expires before the fixture's real 100ms ACK; host ACK deadline remains 2000.
        intent.deadline_ms = 1;
        plugins.submit(&actor, &intent).unwrap();
        plugins.sync_credentials(&actor, &[]);
        assert_eq!(
            plugins.pump(&mut actor, 1).unwrap(),
            [(intent.id, Completion::Failed(Failure::ServiceUnavailable))]
        );
        let end = Instant::now() + Duration::from_secs(3);
        while plugins.host.credentials_state(plugin, generation, 1)
            != Some(rad_plugin_host::CredentialState::Applied)
        {
            assert!(plugins.pump(&mut actor, 1).unwrap().is_empty());
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        let key = plugins
            .host
            .options_with_credentials(
                plugin,
                generation,
                rad_plugin_host::OptionsQuery {
                    scope: rad_plugin_contract::OptionsScope::Action,
                    definition: "authenticate".into(),
                    field: "invocations".into(),
                    context: rad_plugin_contract::Parameters {
                        schema_version: 1,
                        values: serde_json::Map::new(),
                    },
                },
                1001,
                Some(1),
            )
            .unwrap();
        let outcome = loop {
            assert!(plugins.pump(&mut actor, 1).unwrap().is_empty());
            if let Some((got, outcome)) = plugins.host.take_options() {
                assert_eq!(got, key);
                break outcome;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        };
        let rad_plugin_host::OptionsOutcome::Choices(rad_plugin_contract::ChoiceList::Available {
            options,
        }) = outcome
        else {
            panic!("counter unavailable");
        };
        assert_eq!(
            options[0].value, "count0",
            "expired intent must have zero plugin effects"
        );
        plugins.host.shutdown();
        while plugins.host.resources().0 > 0 {
            plugins.pump(&mut actor, 1).unwrap();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
mod hold_tests {
    use super::*;

    use rad_authority::Effect;
    use rad_core::{
        configuration::{ActionKind, Configuration, Edit, ExternalType, Gesture, Origin},
        identity::IdentityRegistry,
    };

    fn fixture_mode(plugin: &str, mode: &str) -> Authority {
        let c = Configuration::prepare_new(&IdentityRegistry::new(941).unwrap()).unwrap();
        let c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::CreateButton {
                    folder: c.configuration().data().principal,
                    position: 0,
                    name: "Hold".into(),
                },
            )
            .unwrap();
        let button = c.configuration().data().buttons[0].binding.identity();
        let c = c
            .configuration()
            .prepare_edits(
                c.registry(),
                Origin::Editor,
                &[
                    Edit::AddAction {
                        button,
                        kind: ActionKind::External {
                            type_key: ExternalType::new(plugin.into(), "run".into()).unwrap(),
                            parameters: serde_json::to_vec(
                                &serde_json::json!({"schema_version":1,"values":{"mode":mode}}),
                            )
                            .unwrap(),
                        },
                    },
                    Edit::SetGesture {
                        button,
                        gesture: Gesture::Hold,
                    },
                ],
            )
            .unwrap();
        let mut actor = Authority::new(c, 1, std::num::NonZeroU128::new(942).unwrap());
        let manifest:rad_plugin_contract::Manifest=serde_json::from_value(serde_json::json!({"contract":1,"capabilities":["controls"],"id":plugin,"version":{"major":1,"minor":0,"patch":0},"host_min":1,"host_max":1,"targets":[rad_plugin_host::TARGET],"actions":[{"id":"run","label":"Run","gestures":["hold"],"schema":{"version":1,"fields":[{"id":"mode","label":"Mode","required":true,"kind":{"type":"choice","options":["echo","end_fail","crash"]}}]},"credential_slots":[]}],"visuals":[]})).unwrap();
        actor
            .register_plugin(manifest, rad_plugin_host::TARGET)
            .unwrap();
        actor
    }

    #[cfg(feature = "fixture-tests")]
    #[test]
    fn release_reaches_real_original_plugin_and_keeps_authority_pin_until_ack() {
        real_release("echo");
    }

    #[cfg(feature = "fixture-tests")]
    fn real_release(mode: &str) {
        use std::time::{Duration, Instant};
        let plugin = "org.rad.fixture";
        let mut actor = fixture_mode(plugin, mode);
        let manifest = actor.plugin_manifests().next().unwrap().0.clone();
        let mut plugins = Plugins::default();
        let launch = plugins.host.reserve_launch(manifest.clone(), 0).unwrap();
        let bin = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("rad-plugin-fixture-v1");
        let mut prepared = Some(launch.spawn(std::process::Command::new(bin)));
        let generation = plugins.host.attach(&mut prepared, 0).unwrap();
        let end = Instant::now() + Duration::from_secs(3);
        while plugins.host.state(plugin) != Some(State::Ready) {
            assert!(Instant::now() < end);
            plugins.pump(&mut actor, 0).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        actor
            .install_plugin(manifest, generation, rad_plugin_host::TARGET)
            .unwrap();
        let mut pressed = [false; 32];
        pressed[0] = true;
        let intent = actor
            .input_report(1, pressed)
            .unwrap()
            .into_iter()
            .find_map(|e| {
                if let Effect::Dispatch(i) = e {
                    Some(i)
                } else {
                    None
                }
            })
            .unwrap();
        plugins.submit(&actor, &intent).unwrap();
        assert_eq!(plugins.holds.len(), 1);
        let done = loop {
            assert!(Instant::now() < end);
            let done = plugins.pump(&mut actor, 1).unwrap();
            if !done.is_empty() {
                break done;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(done, vec![(intent.id, Completion::Succeeded)]);
        actor.job_complete(intent.id, done[0].1).unwrap();
        actor.input_report(2, [false; 32]).unwrap();
        assert!(
            plugins
                .release_hold(&mut actor, intent.id.execution(), 2)
                .is_empty()
        );
        assert_eq!(plugins.hold_ends.len(), 1);
        assert_eq!(
            actor.plugin_change_allowed(plugin),
            Err(rad_authority::plugins::PluginError::InUse)
        );
        while !plugins.hold_ends.is_empty() {
            assert!(Instant::now() < end);
            assert!(plugins.pump(&mut actor, 2).unwrap().is_empty());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(actor.plugin_change_allowed(plugin), Ok(()));
        assert_eq!(plugins.host.state(plugin), Some(State::Ready));
        assert_eq!(
            actor
                .result(Origin::Editor, intent.id.execution())
                .unwrap()
                .status,
            if mode == "end_fail" {
                rad_core::execution::ExecutionStatus::Failed
            } else {
                rad_core::execution::ExecutionStatus::Succeeded
            }
        );
        plugins.host.shutdown();
        while plugins.host.resources().0 > 0 {
            assert!(Instant::now() < end);
            plugins.pump(&mut actor, 2).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
