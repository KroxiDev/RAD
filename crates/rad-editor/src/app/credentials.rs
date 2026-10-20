//! Credential controls deliberately do not belong to the ordinary parameter draft.
use super::tr;
use eframe::egui;
use zeroize::Zeroizing;

#[derive(Default)]
struct SecretDraft(Zeroizing<String>);
impl std::fmt::Debug for SecretDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretDraft([redacted])")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Absent,
    Present,
    Unavailable,
}
#[derive(Default)]
struct Field {
    state: Option<State>,
    requested: bool,
    checking: bool,
    pending: bool,
    error: Option<crate::session::Error>,
    draft: SecretDraft,
    dirty: bool,
    confirm: bool,
}
#[derive(Debug)]
enum Intent {
    Save(SecretDraft),
    Remove,
}
fn field_ui(
    ui: &mut egui::Ui,
    field: &mut Field,
    es: impl Into<crate::preferences::Language> + Copy,
    available: bool,
) -> Option<Intent> {
    if field.pending {
        ui.label(tr(es, "Saving — awaiting durable confirmation"));
        return None;
    }
    if let Some(error) = &field.error {
        ui.label(if error.unknown {
            tr(es, "Outcome unknown — check status before retrying")
        } else {
            tr(es, "Not saved — operation failed")
        });
    }
    if field.state == Some(State::Unavailable) {
        ui.label(tr(es, "Stored credential unavailable — enter it again"));
    }
    let hint = if field.state == Some(State::Present) {
        "••••••••"
    } else {
        tr(es, "New credential")
    };
    let mut output = egui::TextEdit::singleline(&mut *field.draft.0)
        .password(true)
        .hint_text(hint)
        .char_limit(4096)
        .show(ui);
    // Password mode suppresses copying, but does not suppress egui's undo retention.
    output.state.clear_undoer();
    output.state.store(ui.ctx(), output.response.id);
    if output.response.changed() {
        field.dirty = true;
    }
    if output.response.lost_focus() && field.dirty {
        field.confirm = true;
    }
    if field.draft.0.len() > 4096 {
        ui.label(tr(es, "Too long — maximum 4096 UTF-8 bytes"));
    }
    let mut intent = None;
    if field.confirm {
        ui.label(tr(es, "Replace the saved credential?"));
        ui.horizontal(|ui| {
            if available
                && field.draft.0.len() <= 4096
                && ui.button(tr(es, "Save credential")).clicked()
            {
                intent = Some(Intent::Save(std::mem::take(&mut field.draft)));
                field.dirty = false;
                field.confirm = false;
            }
            if ui.button(tr(es, "Cancel")).clicked() {
                field.draft = SecretDraft::default();
                field.dirty = false;
                field.confirm = false;
            }
        });
    }
    if available
        && !field.dirty
        && matches!(field.state, Some(State::Present | State::Unavailable))
        && ui
            .button(tr(es, "Remove credential"))
            .on_hover_text(tr(es, "Cannot be undone"))
            .clicked()
    {
        intent = Some(Intent::Remove);
    }
    intent
}

#[derive(Debug)]
enum Operation {
    Status,
    Replace(SecretDraft),
    Remove,
}
#[derive(Debug)]
pub(super) struct Job {
    generation: u64,
    plugin: String,
    slot: String,
    operation: Operation,
}
pub(super) struct Event {
    generation: u64,
    slot: String,
    result: Result<State, crate::session::Error>,
}
pub(super) type CredentialWorker = crate::worker::Worker<Job, Event>;
pub(super) fn start(runtime: std::path::PathBuf) -> CredentialWorker {
    CredentialWorker::spawn(move |job: Job| {
        let result = (|| {
            use rad_client::connection::{Client, Endpoint};
            let client = Client::connect(&runtime, Endpoint::Editor).map_err(|error| {
                crate::session::Error {
                    code: error.code.into(),
                    cause: None,
                    unknown: false,
                }
            })?;
            let mut session = crate::session::Session::new(client);
            let request = match &job.operation {
                Operation::Status => crate::Request::CredentialStatus {
                    plugin_id: job.plugin,
                    slot: job.slot.clone(),
                },
                Operation::Remove => crate::Request::CredentialRemove {
                    plugin_id: job.plugin,
                    slot: job.slot.clone(),
                },
                Operation::Replace(secret) => crate::Request::CredentialReplace {
                    plugin_id: job.plugin,
                    slot: job.slot.clone(),
                    secret: secret.0.to_string(),
                },
            };
            let response = session.request(request)?;
            decode_state(
                response.data,
                !matches!(job.operation, Operation::Status),
                response.status,
            )
        })();
        Event {
            generation: job.generation,
            slot: job.slot,
            result,
        }
    })
}

fn decode_state(
    data: rad_api::ResponseData,
    mutating: bool,
    status: rad_client::response::Status,
) -> Result<State, crate::session::Error> {
    let error = || crate::session::Error {
        code: "invalid_request".into(),
        cause: None,
        unknown: mutating,
    };
    if status != rad_client::response::Status::Ok {
        return Err(error());
    }
    match data {
        rad_api::ResponseData::Credential(data) => Ok(match data.state {
            rad_api::CredentialState::Absent => State::Absent,
            rad_api::CredentialState::Present => State::Present,
            rad_api::CredentialState::Unavailable => State::Unavailable,
        }),
        _ => Err(error()),
    }
}

#[derive(Default)]
pub(super) struct Panel {
    context: String,
    plugin: String,
    generation: u64,
    fields: std::collections::BTreeMap<String, Field>,
}
impl Panel {
    pub(super) fn editing(&self) -> bool {
        self.fields
            .values()
            .any(|field| field.dirty || field.pending)
    }

    pub(super) fn bind(&mut self, context: &str, type_key: &str, slots: &[String]) {
        if self.context == context {
            return;
        }
        self.clear();
        let Some((plugin, _)) = type_key.split_once([':', '#']) else {
            return;
        };
        if slots.len() > 32 || self.generation == u64::MAX {
            return;
        }
        self.context = context.into();
        self.plugin = plugin.into();
        self.fields = slots
            .iter()
            .map(|slot| (slot.clone(), Field::default()))
            .collect();
    }
    pub(super) fn clear(&mut self) {
        if self.context.is_empty() && self.fields.is_empty() {
            return;
        }
        self.context.clear();
        self.plugin.clear();
        self.fields.clear();
        self.generation = self.generation.saturating_add(1);
    }
    pub(super) fn pump(&mut self, worker: &mut CredentialWorker) {
        if let Some(event) = worker.poll()
            && event.generation == self.generation
            && let Some(field) = self.fields.get_mut(&event.slot)
        {
            field.pending = false;
            field.checking = false;
            match event.result {
                Ok(state) => {
                    field.state = Some(state);
                    field.error = None;
                }
                Err(error) => {
                    if error.unknown {
                        field.state = None;
                        // Reconcile by a read; never replay the uncertain write.
                        field.requested = false;
                    }
                    field.error = Some(error);
                }
            }
        }
    }
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        worker: &mut CredentialWorker,
        connected: bool,
        es: impl Into<crate::preferences::Language> + Copy,
    ) {
        if self.fields.is_empty() {
            return;
        }
        ui.separator();
        ui.label(egui::RichText::new(tr(es, "Credentials — saved separately")).strong());
        ui.label(
            egui::RichText::new(tr(
                es,
                "Shared by this plugin; not included in exports or Undo.",
            ))
            .small()
            .weak(),
        );
        for (slot, field) in &mut self.fields {
            ui.push_id((self.generation, slot), |ui| {
                ui.label(slot);
                if !connected {
                    ui.label(tr(es, "Runtime unavailable"));
                    return;
                }
                let mut operation = None;
                if !field.requested && !worker.busy() {
                    operation = Some(Operation::Status);
                }
                if !field.requested || field.checking {
                    ui.label(tr(es, "Checking credential status…"));
                } else {
                    operation =
                        field_ui(ui, field, es, !worker.busy()).map(|intent| match intent {
                            Intent::Save(secret) => Operation::Replace(secret),
                            Intent::Remove => Operation::Remove,
                        });
                    if !worker.busy()
                        && !field.dirty
                        && operation.is_none()
                        && ui.button(tr(es, "Check credential status")).clicked()
                    {
                        operation = Some(Operation::Status);
                    }
                }
                if let Some(operation) = operation {
                    let checking = matches!(operation, Operation::Status);
                    let job = Job {
                        generation: self.generation,
                        plugin: self.plugin.clone(),
                        slot: slot.clone(),
                        operation,
                    };
                    match worker.submit(job) {
                        Ok(()) => {
                            field.requested = true;
                            field.checking = checking;
                            field.pending = !checking;
                        }
                        Err(_) => {
                            field.requested = true;
                            field.error = Some(crate::session::Error {
                                code: "runtime_unavailable".into(),
                                cause: None,
                                unknown: false,
                            });
                        }
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests;
