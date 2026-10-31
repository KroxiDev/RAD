use super::*;
impl Admin {
    pub(super) fn handle_reply(
        &mut self,
        reply: Reply,
        actor: &mut Authority,
        host: &mut Host,
        now: u64,
    ) {
        match reply {
            Reply::Boot(result) => match result {
                Ok(items) => {
                    self.startup = items.iter().map(|i| i.manifest.clone()).collect();
                    self.installed = items;
                    self.busy = false;
                    self.state = AdminState::Idle;
                }
                Err(e) => {
                    self.locked = true;
                    self.busy = false;
                    self.state = AdminState::Failed;
                    self.error = Some(manager_error(e));
                }
            },
            Reply::Prepared(result, manager_ready) => match result {
                Err(e) => {
                    self.locked = !manager_ready;
                    self.finish(actor, false, Some(manager_error(e)));
                }
                Ok(prepared) => {
                    let prepared = *prepared;
                    let id = prepared.plugin_id().to_owned();
                    let manifest = prepared.manifest().cloned();
                    let change = match &manifest {
                        Some(m) => PluginChange::Select {
                            operation: match prepared.operation() {
                                rad_plugin_manager::Operation::Install => Selection::Install,
                                rad_plugin_manager::Operation::Update(_) => Selection::Update,
                                rad_plugin_manager::Operation::Reinstall(_) => Selection::Reinstall,
                                _ => Selection::Rollback,
                            },
                            manifest: Box::new(m.clone()),
                        },
                        None => PluginChange::Uninstall { id: id.clone() },
                    };
                    let permit = if self.stopping {
                        Err(PluginError::PublicationPending)
                    } else if host.state(&id) == Some(State::Failed) {
                        Err(PluginError::Residual)
                    } else {
                        actor.begin_plugin_change(Origin::Editor, change, rad_plugin_host::TARGET)
                    };
                    match permit {
                        Ok(permit) => {
                            self.active = Some(Activation {
                                id,
                                manifest,
                                permit: Some(permit),
                            });
                            self.change_key = Some(prepared.key());
                            self.state = AdminState::Committing;
                            self.send(Job::Commit(prepared));
                        }
                        Err(e) => {
                            self.error = Some(plugin_error(e));
                            self.state = AdminState::Aborting;
                            self.send(Job::Abort(prepared));
                        }
                    }
                }
            },
            Reply::Aborted(result) => {
                if let Err(e) = result {
                    self.locked = true;
                    self.error = Some(manager_error(e));
                }
                self.finish(actor, false, self.error);
            }
            Reply::Committed(key, outcome, inventory) => {
                if self.change_key.as_ref() != Some(&key) {
                    self.locked = true;
                    self.state = AdminState::Uncertain;
                    self.error = Some("recovery_required");
                    return;
                }
                match outcome {
                    CommitOutcome::Uncertain(e) => {
                        self.locked = true;
                        self.state = AdminState::Uncertain;
                        self.error = Some(manager_error(e));
                    }
                    CommitOutcome::NotApplied(e) => {
                        self.finish(actor, false, Some(manager_error(e)))
                    }
                    CommitOutcome::Applied => {
                        match inventory {
                            Ok(items) => self.installed = items,
                            Err(_) => {
                                self.locked = true;
                                self.state = AdminState::Uncertain;
                                self.error = Some("recovery_required");
                                return;
                            }
                        }
                        let active = self.active.as_ref().expect("commit owns activation");
                        if actor
                            .allow_plugin_activation(active.permit.as_ref().unwrap())
                            .is_err()
                        {
                            self.locked = true;
                            self.state = AdminState::Uncertain;
                            return;
                        }
                        host.retire(&active.id);
                        self.state = AdminState::Retiring;
                    }
                }
            }
            Reply::Spawned(prepared) => {
                let mut prepared = Some(*prepared);
                if host.attach(&mut prepared, now).is_err() {
                    // A rejected token may own a live process. Never discard it.
                    self.locked = true;
                    self.state = AdminState::Uncertain;
                    self.retained = prepared;
                    return;
                }
                if self.stopping
                    && let Some(active) = &self.active
                {
                    host.retire(&active.id);
                }
                self.state = AdminState::Handshake;
            }
        }
    }
}
