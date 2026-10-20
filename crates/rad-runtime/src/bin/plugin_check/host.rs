use super::{Result, SECRET, scenario::Scenario};
use rad_plugin_contract::*;
use rad_plugin_host::{CredentialState, Host, InvokeContext, OptionsOutcome, OptionsQuery, State};
use std::{
    os::unix::process::CommandExt,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

struct Session {
    host: Host,
    id: String,
    start: Instant,
    pid: i32,
}
impl Session {
    fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    fn tick(&mut self) -> Result {
        super::budget()?;
        self.host.pump(self.now()).map_err(|_| "host_pump")?;
        if self.host.credential_leak_detected(&self.id)
            || self
                .host
                .stderr(&self.id)
                .iter()
                .any(|s| s.contains(SECRET))
        {
            return Err("credential_leak");
        }
        if self.host.state(&self.id) != Some(State::Ready)
            && self.host.state(&self.id) != Some(State::Starting)
        {
            return Err("host_protocol_or_process_failure");
        }
        if self.now() > 175_000 {
            return Err("total_budget");
        }
        std::thread::sleep(Duration::from_millis(1));
        Ok(())
    }
    fn until(&mut self, timeout: u64, mut f: impl FnMut(&mut Host) -> bool) -> Result {
        let end = self.now() + timeout;
        loop {
            self.tick()?;
            if f(&mut self.host) {
                return Ok(());
            }
            if self.now() >= end {
                return Err("response_timeout");
            }
        }
    }
    fn completion(&mut self, c: Correlation, timeout: u64) -> Result {
        let mut outcome = None;
        self.until(timeout, |h| {
            outcome = h.take_completion();
            outcome.is_some()
        })?;
        match outcome {
            Some((key, Outcome::Success)) if key == c => Ok(()),
            _ => Err("action_failed_or_timeout"),
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        // Only our isolated process group; cleanup is not evidence of normal EOF.
        if self.pid > 0 {
            unsafe {
                libc::kill(-self.pid, libc::SIGKILL);
            }
        }
        self.host.shutdown();
        let end = Instant::now() + Duration::from_secs(2);
        while self.host.resources().0 > 0 && Instant::now() < end {
            let _ = self.host.pump(self.now());
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
pub(super) fn check(path: &Path, manifest: &Manifest, scenario: &Scenario) -> Result {
    let mut command = Command::new(path);
    command.args(&scenario.args).process_group(0);
    let mut host = Host::default();
    let generation = host
        .activate(command, manifest.clone(), 0)
        .map_err(|_| "host_launch")?;
    let pid = host.process_id(&manifest.id).ok_or("host_pid")? as i32;
    let mut s = Session {
        host,
        id: manifest.id.clone(),
        start: Instant::now(),
        pid,
    };
    s.until(scenario.timeout_ms, |h| {
        h.state(&manifest.id) == Some(State::Ready)
    })?;
    for capability in &manifest.capabilities {
        if SUPPORTED_CAPABILITIES.contains(&capability.as_str())
            && !s
                .host
                .negotiated_capability(&manifest.id, generation, capability)
        {
            return Err("missing_declared_capability");
        }
    }
    let slots = manifest.credential_slots();
    let revision = (!slots.is_empty()).then_some(1);
    if revision.is_some() {
        let values = CredentialValues::new(
            slots
                .into_iter()
                .map(|slot| CredentialValue {
                    slot: slot.into(),
                    value: SECRET.into(),
                })
                .collect(),
        )
        .map_err(|_| "credential_values")?;
        s.host
            .install_credentials(&manifest.id, generation, 1, values)
            .map_err(|_| "credential_install")?;
        s.until(scenario.timeout_ms, |h| {
            h.credentials_state(&manifest.id, generation, 1) == Some(CredentialState::Applied)
        })?;
    }
    for action in &scenario.actions {
        let control = s
            .host
            .negotiated_capability(&manifest.id, generation, CONTROLS)
            .then(|| ControlEvent {
                control: Control {
                    kind: "key".into(),
                    index: 0,
                },
                gesture: match action.gesture {
                    Gesture::Press => ControlGesture::Press,
                    Gesture::Hold => ControlGesture::HoldStart,
                    Gesture::LongPress => ControlGesture::LongPress,
                },
            });
        let context = InvokeContext {
            settings: scenario.settings.clone(),
            control,
        };
        let timing = (s.now() + scenario.timeout_ms, revision);
        if action.gesture == Gesture::Hold {
            let (c, ticket) = s
                .host
                .invoke_hold_start(
                    &manifest.id,
                    generation,
                    &action.id,
                    action.parameters.clone(),
                    timing,
                    context,
                )
                .map_err(|_| "hold_start")?;
            // The linear End is dispatched even if Start fails or times out.
            let first = s.completion(c, scenario.timeout_ms);
            let c = s
                .host
                .release_hold(ticket, s.now() + scenario.timeout_ms)
                .map_err(|_| "hold_end")?;
            s.completion(c, scenario.timeout_ms)?;
            first?;
        } else {
            let c = s
                .host
                .invoke_with_context(
                    &manifest.id,
                    generation,
                    &action.id,
                    action.parameters.clone(),
                    timing,
                    context,
                )
                .map_err(|_| "action_dispatch")?;
            s.completion(c, scenario.timeout_ms)?;
        }
    }
    for o in &scenario.options {
        let c = s
            .host
            .options_with_settings(
                &manifest.id,
                generation,
                OptionsQuery {
                    scope: o.scope,
                    definition: o.definition.clone(),
                    field: o.field.clone(),
                    context: o.context.clone(),
                },
                s.now() + scenario.timeout_ms,
                revision,
                scenario.settings.clone(),
            )
            .map_err(|_| "options_dispatch")?;
        let mut result = None;
        s.until(scenario.timeout_ms, |h| {
            result = h.take_options();
            result.is_some()
        })?;
        match result {
            Some((key, OptionsOutcome::Choices(ChoiceList::Available { options }))) if key == c => {
                if options
                    .iter()
                    .any(|o| o.label.contains(SECRET) || o.value.contains(SECRET))
                {
                    return Err("credential_leak");
                }
            }
            _ => return Err("options_unobservable_or_timeout"),
        }
    }
    let mut subscriptions = Vec::new();
    for visual in &scenario.visuals {
        let c = s
            .host
            .subscribe_with_settings(
                &manifest.id,
                &visual.id,
                visual.parameters.clone(),
                60000,
                revision,
                scenario.settings.clone(),
            )
            .map_err(|_| "subscribe")?;
        s.until(scenario.timeout_ms, |h| {
            matches!(h.visual(&manifest.id, c), Some(VisualState::Current { .. }))
        })?;
        let expiry = s
            .host
            .visual_valid_until(&manifest.id, c)
            .ok_or("visual_validity")?;
        subscriptions.push((c, expiry, s.host.visual_renewals(&manifest.id, c)));
    }
    let end = s.now() + scenario.freshness_ms;
    while !subscriptions.is_empty() && s.now() < end {
        s.tick()?;
        for (c, _, _) in &subscriptions {
            let state = s.host.visual(&manifest.id, *c).ok_or("visual_missing")?;
            if !matches!(state, VisualState::Current { .. }) {
                return Err("visual_stale");
            }
            if serde_json::to_string(state)
                .map_err(|_| "visual_encode")?
                .contains(SECRET)
            {
                return Err("credential_leak");
            }
        }
    }
    for (c, expiry, renewals) in subscriptions {
        if s.now() <= expiry || s.host.visual_renewals(&manifest.id, c) <= renewals {
            return Err("freshness_not_observed");
        }
        s.host
            .unsubscribe(&manifest.id, c)
            .map_err(|_| "unsubscribe")?;
        if s.host.visual(&manifest.id, c).is_some() {
            return Err("unsubscribe_retained");
        }
    }
    s.until(scenario.timeout_ms, |h| h.idle(&manifest.id))?;
    if revision.is_some() {
        s.host
            .release_credentials(&manifest.id, generation, 1)
            .map_err(|_| "credential_release")?;
        s.until(scenario.timeout_ms, |h| {
            h.take_credentials_released().is_some()
        })?;
    }
    Ok(())
}
