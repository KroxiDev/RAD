//! Dynamic options: a question to a plugin that never blocks the runtime loop. The request
//! stays pending in the transport while the host waits for the plugin; every turn drains the
//! host and answers whoever is still connected. Not a job: no execution, no reservation.
use super::*;
use rad_plugin_host::{Error as Host, OptionsOutcome, OptionsQuery};
use rad_protocol::PluginOptions;

/// Time a plugin has to answer, counted from the moment the runtime receives the request:
/// well inside the client's ten seconds, enough for a local service (audio server, D-Bus, OBS).
const DEADLINE_MS: u64 = 2000;

pub(super) struct PendingOptions {
    key: RequestKey,
    context: IncomingContext,
    query: PluginOptions,
}

pub(super) struct WaitingOptions {
    pub snapshot: Option<rad_authority::plugins::CredentialSnapshot>,
    settings: Option<rad_plugin_contract::Parameters>,
    generation: Option<u64>,
    pub pending: PendingOptions,
    pub deadline: u64,
}
impl WaitingOptions {
    pub fn binding(&self) -> Option<(String, u64, rad_authority::plugins::CredentialSnapshot)> {
        Some((
            self.pending.query.plugin.clone(),
            self.pending.query.generation,
            self.snapshot.clone()?,
        ))
    }
    pub fn plugin(&self) -> String {
        self.pending.query.plugin.clone()
    }
}
impl Server {
    pub(super) fn plugin_options(
        &mut self,
        key: RequestKey,
        context: IncomingContext,
        query: PluginOptions,
    ) -> Result<(), RuntimeError> {
        let now = monotonic_ms().map_err(|_| RuntimeError::Clock)?;
        let needs_credentials = self.actor.plugin_manifests().any(|(m, g, _)| {
            m.id == query.plugin && g == query.generation && !m.credential_slots().is_empty()
        });
        let snapshot = self
            .actor
            .current_plugin_credentials(&query.plugin, query.generation);
        if needs_credentials && snapshot.is_none() {
            return self.respond(key, self.error(context, "executor_unavailable", false));
        }
        let waiting = self
            .waiting_options
            .iter()
            .filter(|w| w.pending.query.plugin == query.plugin)
            .count();
        let occupied = self
            .native
            .as_ref()
            .map(|n| {
                self.actor
                    .plugin_generation(&query.plugin)
                    .map_or(0, |g| n.plugin_host().options_in_use(&query.plugin, g))
            })
            .unwrap_or(0);
        if waiting + occupied >= 2 {
            return self.respond(key, self.error(context, "resource_exhausted", false));
        }
        let settings = match self
            .actor
            .plugin_settings(Origin::Editor, &query.plugin)
            .map(serde_json::from_slice)
            .transpose()
        {
            Ok(settings) => settings,
            Err(_) => return self.respond(key, self.error(context, "executor_unavailable", false)),
        };
        self.waiting_options.push(WaitingOptions {
            snapshot,
            settings,
            generation: self.actor.plugin_generation(&query.plugin),
            pending: PendingOptions {
                key,
                context,
                query,
            },
            deadline: now + DEADLINE_MS,
        });
        Ok(())
    }

    fn send_options(
        &mut self,
        query: &PluginOptions,
        deadline: u64,
        revision: Option<u64>,
        settings: Option<rad_plugin_contract::Parameters>,
    ) -> Result<rad_plugin_contract::Correlation, Host> {
        if !self.actor.plugin_available(&query.plugin, query.generation) {
            return Err(Host::Unavailable);
        }
        let generation = self
            .actor
            .plugin_generation(&query.plugin)
            .ok_or(Host::Unavailable)?;
        self.native
            .as_mut()
            .ok_or(Host::Unavailable)?
            .plugin_host_mut()
            .options_with_settings(
                &query.plugin,
                generation,
                OptionsQuery {
                    scope: query.scope,
                    definition: query.definition.clone(),
                    field: query.field.clone(),
                    context: query.context.clone(),
                },
                deadline,
                revision,
                settings,
            )
    }

    /// Answer every query the host finished. Runs at the end of every turn (after the
    /// transport's `drive`, see `turn`): the host keeps a plugin "residual" (not removable)
    /// until its options results are taken.
    pub(super) fn pump_options(&mut self) -> Result<(), RuntimeError> {
        let now = monotonic_ms().map_err(|_| RuntimeError::Clock)?;
        for waiting in std::mem::take(&mut self.waiting_options) {
            let pending = waiting.pending;
            if now >= waiting.deadline {
                let response =
                    self.protocol
                        .options_response(pending.context, &pending.query, None);
                self.respond(pending.key, response)?;
                continue;
            }
            let generation = self.actor.plugin_generation(&pending.query.plugin);
            let available = self
                .actor
                .plugin_available(&pending.query.plugin, pending.query.generation);
            let stale = waiting.generation.is_some() && waiting.generation != generation;
            if !available || stale {
                self.respond(
                    pending.key,
                    self.error(pending.context, "executor_unavailable", false),
                )?;
                continue;
            }
            let ready = generation.is_some_and(|g| {
                self.native.as_ref().is_some_and(|n| {
                    n.plugin_host().state(&pending.query.plugin)
                        == Some(rad_plugin_host::State::Ready)
                        && waiting.snapshot.as_ref().is_none_or(|snapshot| {
                            n.plugin_host().credentials_state(
                                &pending.query.plugin,
                                g,
                                snapshot.revision(),
                            ) == Some(rad_plugin_host::CredentialState::Applied)
                        })
                })
            });
            if !ready {
                self.waiting_options.push(WaitingOptions {
                    pending,
                    generation: waiting.generation.or(generation),
                    ..waiting
                });
                continue;
            }
            match self.send_options(
                &pending.query,
                waiting.deadline,
                waiting.snapshot.as_ref().map(|s| s.revision()),
                waiting.settings,
            ) {
                Ok(c) => {
                    self.options.insert((c.generation, c.request), pending);
                }
                Err(_) => self.respond(
                    pending.key,
                    self.error(pending.context, "executor_unavailable", false),
                )?,
            }
        }
        while let Some((correlation, outcome)) = self
            .native
            .as_mut()
            .and_then(|native| native.plugin_host_mut().take_options())
        {
            // Not pending here: its client left, so nobody is waiting for the answer.
            let Some(pending) = self
                .options
                .remove(&(correlation.generation, correlation.request))
            else {
                continue;
            };
            let list = match &outcome {
                OptionsOutcome::Choices(list) => Some(list),
                OptionsOutcome::TimedOut => None,
            };
            let response = self
                .protocol
                .options_response(pending.context, &pending.query, list);
            self.respond(pending.key, response)?;
        }
        Ok(())
    }

    /// The client is gone: free its transport slots. The host still holds the question
    /// until the plugin answers or the deadline passes; that result is then dropped.
    pub(super) fn abandon_options(&mut self, connection: u64) -> Result<(), RuntimeError> {
        for waiting in std::mem::take(&mut self.waiting_options) {
            if waiting.pending.context.connection == connection {
                self.respond(
                    waiting.pending.key,
                    self.error(waiting.pending.context, "runtime_unavailable", false),
                )?;
            } else {
                self.waiting_options.push(waiting);
            }
        }
        let gone: Vec<_> = self
            .options
            .iter()
            .filter(|(_, pending)| pending.context.connection == connection)
            .map(|(question, _)| *question)
            .collect();
        for question in gone {
            let pending = self.options.remove(&question).expect("collected question");
            self.respond(
                pending.key,
                self.error(pending.context, "runtime_unavailable", false),
            )?;
        }
        Ok(())
    }
}
