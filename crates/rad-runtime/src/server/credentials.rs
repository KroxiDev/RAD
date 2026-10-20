use super::*;
use crate::credential_worker::{Binding, Event, Task, Worker};
use rad_credentials::{Error as VaultError, State as VaultState, Status as VaultStatus};
use rad_protocol::CredentialOperation;
pub(super) struct Credentials {
    worker: Worker,
    pending: Option<(RequestKey, IncomingContext, bool)>,
    responses: VecDeque<(RequestKey, Response)>,
    loaded: BTreeMap<String, u64>,
    revisions: BTreeMap<(String, u64), u64>,
    reconciled: bool,
}
impl Credentials {
    pub fn new(root: std::path::PathBuf, wake: Arc<mio::Waker>) -> Result<Self, RuntimeError> {
        Ok(Self {
            worker: Worker::spawn(root, wake).map_err(|_| RuntimeError::Startup)?,
            pending: None,
            responses: VecDeque::new(),
            loaded: BTreeMap::new(),
            revisions: BTreeMap::new(),
            reconciled: false,
        })
    }
    pub fn shutdown(&mut self) {
        self.worker.shutdown();
    }
    pub fn stopped(&self) -> bool {
        self.worker.stopped()
    }
}
fn bindings(actor: &Authority) -> Vec<Binding> {
    actor
        .plugin_manifests()
        .filter(|(manifest, _, _)| !manifest.credential_slots().is_empty())
        .map(|(manifest, generation, _)| Binding {
            plugin: manifest.id.clone(),
            generation,
            slots: manifest
                .credential_slots()
                .into_iter()
                .map(str::to_owned)
                .collect(),
        })
        .collect()
}
fn state_name(status: VaultStatus) -> &'static str {
    match status {
        VaultStatus::Absent => "absent",
        VaultStatus::Present => "present",
        VaultStatus::Unavailable => "unavailable",
    }
}
fn error_code(error: VaultError) -> &'static str {
    match error {
        VaultError::InvalidInput => "invalid_request",
        VaultError::Capacity => "resource_exhausted",
        VaultError::UnsafePath => "storage_error",
        VaultError::Unavailable => "executor_unavailable",
        VaultError::Io => "storage_error",
        VaultError::Uncertain => "storage_uncertain",
        VaultError::Busy => "busy",
    }
}
impl Server {
    pub(super) fn credential_request(
        &mut self,
        key: RequestKey,
        context: IncomingContext,
        operation: CredentialOperation,
    ) -> Result<(), RuntimeError> {
        let mutation = operation.mutation();
        if context.origin != Origin::Editor {
            return self.respond(key, self.error(context, "permission_denied", mutation));
        }
        let refresh = if self.credentials.worker.state == Some(VaultState::Unknown) {
            bindings(&self.actor)
        } else if mutation {
            bindings(&self.actor)
                .into_iter()
                .filter(|b| b.plugin == operation.plugin_id())
                .collect()
        } else {
            vec![]
        };
        match self
            .credentials
            .worker
            .submit(Task::Request(operation, refresh))
        {
            Ok(()) => self.credentials.pending = Some((key, context, mutation)),
            Err(error) => {
                return self.respond(key, self.error(context, error_code(error), mutation));
            }
        }
        Ok(())
    }
    pub(super) fn respond_credentials(&mut self) -> Result<(), RuntimeError> {
        while let Some((key, response)) = self.credentials.responses.pop_front() {
            self.respond(key, response)?;
        }
        Ok(())
    }
    pub(super) fn poll_credentials(&mut self) -> Result<(), RuntimeError> {
        while let Some(event) = self.credentials.worker.poll() {
            let Event::Finished(done) = event else {
                continue;
            };
            if done.state == VaultState::Unknown {
                self.credentials.loaded.clear();
                for binding in bindings(&self.actor) {
                    let _ = self
                        .actor
                        .invalidate_plugin_credentials(&binding.plugin, binding.generation);
                }
            }
            for map in done.maps {
                let b = map.binding;
                let key = (b.plugin.clone(), b.generation);
                if let Ok(values) = map.values
                    && done.state != VaultState::Unknown
                {
                    let next = self
                        .credentials
                        .revisions
                        .get(&key)
                        .copied()
                        .unwrap_or(0)
                        .checked_add(1);
                    if let Some(next) = next
                        && self
                            .actor
                            .publish_plugin_credentials(
                                &b.plugin,
                                b.generation,
                                next,
                                Arc::new(values),
                            )
                            .is_ok()
                    {
                        self.credentials.revisions.insert(key, next);
                    } else {
                        let _ = self
                            .actor
                            .invalidate_plugin_credentials(&b.plugin, b.generation);
                    }
                } else {
                    let _ = self
                        .actor
                        .invalidate_plugin_credentials(&b.plugin, b.generation);
                }
                self.credentials.loaded.insert(b.plugin, b.generation);
            }
            if let Some(result) = done.result
                && let Some((key, context, mutation)) = self.credentials.pending.take()
            {
                let response = match result {
                    Ok(state) => Response {
                        v: 1,
                        id: Some(context.request_id.to_string()),
                        session_id: Some(format!("{:032x}", self.session)),
                        status: Status::Ok,
                        data: json!({"state":state_name(state)}),
                        error: None,
                    },
                    Err(error) => {
                        let mut response = self.error(context, error_code(error), mutation);
                        // Unknown readiness is not proof that this request wrote anything.
                        // The vault (or lost worker) explicitly classifies uncertain effects.
                        if mutation && error == VaultError::Uncertain {
                            response.error.as_mut().unwrap().outcome = Some(Outcome::Unknown);
                        }
                        response
                    }
                };
                self.credentials.responses.push_back((key, response));
            }
        }
        if self.shutting_down {
            return Ok(());
        }
        let current = bindings(&self.actor);
        self.credentials.loaded.retain(|id, generation| {
            current
                .iter()
                .any(|b| &b.plugin == id && &b.generation == generation)
        });
        self.credentials.revisions.retain(|(id, generation), _| {
            current
                .iter()
                .any(|b| &b.plugin == id && &b.generation == generation)
        });
        if self.credentials.worker.available() && self.credentials.worker.state.is_some() {
            if self.credentials.worker.state == Some(VaultState::Unknown) {
                if !self.credentials.reconciled {
                    self.credentials.reconciled = true;
                    let _ = self.credentials.worker.submit(Task::Reconcile(current));
                }
            } else {
                self.credentials.reconciled = false;
                let missing: Vec<_> = current
                    .into_iter()
                    .filter(|b| self.credentials.loaded.get(&b.plugin) != Some(&b.generation))
                    .collect();
                if !missing.is_empty() {
                    let _ = self.credentials.worker.submit(Task::Refresh(missing));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vault_errors_use_the_existing_closed_wire_codes() {
        for error in [
            VaultError::InvalidInput,
            VaultError::Capacity,
            VaultError::UnsafePath,
            VaultError::Unavailable,
            VaultError::Io,
            VaultError::Uncertain,
            VaultError::Busy,
        ] {
            let response = Response {
                v: 1,
                id: Some("2".into()),
                session_id: Some(format!("{:032x}", 1)),
                status: Status::Error,
                data: Value::Null,
                error: Some(Error {
                    code: error_code(error).into(),
                    message: "Request rejected".into(),
                    details: json!({}),
                    outcome: Some(if error == VaultError::Uncertain {
                        Outcome::Unknown
                    } else {
                        Outcome::NotApplied
                    }),
                }),
            };
            assert!(
                Response::decode(&serde_json::to_vec(&response).unwrap(), 2, None).is_ok(),
                "{error:?}"
            );
        }
    }
    #[test]
    fn disconnected_editor_does_not_kill_runtime_when_vault_work_finishes() {
        use rad_client::connection::{Client, Endpoint};
        use std::{
            os::unix::fs::DirBuilderExt,
            time::{Duration, Instant},
        };
        let root =
            std::env::temp_dir().join(format!("rad-credential-disconnect-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let mut server = Server::new(
            &root,
            &root.join("data"),
            std::num::NonZeroU128::new(0xc430).unwrap(),
        )
        .unwrap();
        let (worker, tasks, results) = Worker::controlled();
        server.credentials.worker = worker;
        let path = root.clone();
        let client = std::thread::spawn(move || {
            let mut client = Client::connect(&path, Endpoint::Editor).unwrap();
            let outcome = client.request_until(
                "credential_replace",
                json!({"plugin_id":"fixture","slot":"password","secret":"disconnect-canary"}),
                Instant::now() + Duration::from_millis(40),
            );
            assert_eq!(outcome.unwrap_err().code, "deadline_exceeded");
        });
        let end = Instant::now() + Duration::from_secs(3);
        while !client.is_finished() {
            server.turn().unwrap();
            server.wait(Duration::from_millis(2)).unwrap();
            assert!(Instant::now() < end);
        }
        client.join().unwrap();
        assert!(matches!(
            tasks.try_recv().unwrap(),
            Task::Request(CredentialOperation::Replace { .. }, _)
        ));
        for _ in 0..3 {
            server.turn().unwrap();
        }
        assert_eq!(
            server.pending_requests(),
            1,
            "accepted worker work remains pending after peer closes"
        );
        results
            .send(Event::Finished(crate::credential_worker::Finished {
                state: VaultState::Ready,
                result: Some(Ok(VaultStatus::Present)),
                maps: vec![],
            }))
            .unwrap_or_else(|_| panic!("worker result channel closed"));
        server
            .turn()
            .expect("late durable result must not kill runtime");
        assert_eq!(server.pending_requests(), 0);
        let path = root.clone();
        let other = std::thread::spawn(move || {
            let mut c = Client::connect(&path, Endpoint::Editor).unwrap();
            assert!(c.request("status", json!({})).unwrap().error.is_none());
        });
        while !other.is_finished() {
            server.turn().unwrap();
            server.wait(Duration::from_millis(2)).unwrap();
            assert!(Instant::now() < end);
        }
        other.join().unwrap();
        server.request_shutdown();
        drop(server);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn uncertain_completion_and_lost_worker_remain_unknown_over_ipc() {
        use rad_client::connection::{Client, Endpoint};
        use std::{
            os::unix::fs::DirBuilderExt,
            time::{Duration, Instant},
        };
        for lost in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "rad-credential-uncertain-{}-{lost}",
                std::process::id()
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .unwrap();
            let mut server = Server::new(
                &root,
                &root.join("data"),
                std::num::NonZeroU128::new(0xc452).unwrap(),
            )
            .unwrap();
            let (worker, tasks, results) = Worker::controlled();
            server.credentials.worker = worker;
            let mut results = Some(results);
            let path = root.clone();
            let client = std::thread::spawn(move || {
                let mut c = Client::connect(&path, Endpoint::Editor).unwrap();
                c.request(
                    "credential_replace",
                    json!({"plugin_id":"fixture","slot":"password","secret":"uncertain-fixture"}),
                )
                .unwrap()
            });
            let end = Instant::now() + Duration::from_secs(3);
            while !client.is_finished() {
                server.turn().unwrap();
                if tasks.try_recv().is_ok() {
                    let sender = results.take().expect("one accepted request");
                    if !lost {
                        sender
                            .send(Event::Finished(crate::credential_worker::Finished {
                                state: VaultState::Unknown,
                                result: Some(Err(VaultError::Uncertain)),
                                maps: vec![],
                            }))
                            .unwrap_or_else(|_| panic!("result receiver lost"));
                        results = Some(sender);
                    }
                }
                server.wait(Duration::from_millis(2)).unwrap();
                assert!(Instant::now() < end);
            }
            let error = client.join().unwrap().error.unwrap();
            assert_eq!(error.code, "storage_uncertain");
            assert_eq!(error.outcome, Some(Outcome::Unknown));
            server.request_shutdown();
            drop(server);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
