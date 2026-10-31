//! The dynamic option fields of a plugin form as the user sees them. The rules (what is asked,
//! what an answer means) live in `crate::options`; here only drawing and the click on "ask again".
use super::{
    look::{Icon, Look, icon_button_with_id},
    plugins::PluginUi,
    tr,
};
use crate::{
    native::{OptionsEvent, OptionsJob},
    options::{Failure, ListState, Target},
    plugins::FormDraft,
    session::Error,
    status::describe_error,
    worker::{Polled, Pulse, Worker},
};
use eframe::egui;
use rad_plugin_contract::ChoiceList;

/// Id of the "ask again" control of a field, so that it can be found again whatever the layout.
pub(super) fn ask_again_id(index: usize) -> egui::Id {
    egui::Id::new(("ask-again", index))
}

impl PluginUi {
    /// One turn of the conversation about lists: takes the answer that arrived, then asks the
    /// next list due. The worker holds one question at a time, so nothing queues behind a
    /// slow plugin, and an answer for a context that has since changed finds no one waiting.
    pub(super) fn pump_options(
        &mut self,
        worker: &mut Worker<OptionsJob, OptionsEvent>,
        connected: bool,
    ) -> Pulse {
        let pulse = match worker.poll() {
            Polled::Ready(event) => {
                if let Some(form) = self
                    .form
                    .as_mut()
                    .and_then(|(_, loaded)| loaded.form.as_mut())
                {
                    form.accept_answer(event.ticket, event.result);
                }
                Pulse::Answered
            }
            Polled::Failed { .. } | Polled::Closed { .. } => {
                // The question was a read: its list says so and can be asked again.
                if let Some(form) = self
                    .form
                    .as_mut()
                    .and_then(|(_, loaded)| loaded.form.as_mut())
                {
                    let lost = Error {
                        code: "runtime_unavailable".into(),
                        cause: None,
                        source_change: None,
                        unknown: false,
                    };
                    form.accept_answer(self.tickets, Err(lost));
                }
                // The list failed; it is asked for again, on a replacement worker, when needed.
                return Pulse::Lost;
            }
            Polled::Pending => Pulse::Quiet,
        };
        self.ask_next(worker, connected);
        pulse
    }
    fn ask_next(&mut self, worker: &mut Worker<OptionsJob, OptionsEvent>, connected: bool) {
        if !connected || worker.busy() {
            return;
        }
        let Some((_, loaded)) = self.form.as_mut() else {
            return;
        };
        let target = Target {
            type_key: &loaded.type_key,
            entity: loaded.id.as_deref(),
        };
        let Some(form) = loaded.form.as_mut().filter(|form| form.compatible()) else {
            return;
        };
        if form.settings {
            return;
        }
        let ticket = self.tickets + 1;
        let Some(query) = form.next_query(target, ticket) else {
            return;
        };
        self.tickets = ticket;
        let job = OptionsJob {
            ticket,
            request: query.request,
        };
        if worker.submit(job).is_err() {
            // The worker is gone: say so on the list rather than leave it loading forever.
            let lost = Error {
                code: "runtime_unavailable".into(),
                cause: None,
                source_change: None,
                unknown: false,
            };
            form.accept_answer(ticket, Err(lost));
        }
    }
}

/// The control of a dynamic choice: a drop-down over the labels of its list, and a way to ask
/// again. Returns whether the user picked another value (the caller stores it in the draft).
pub(super) fn dynamic_choice(
    ui: &mut egui::Ui,
    look: &Look,
    form: &mut FormDraft,
    index: usize,
    value: &mut String,
    es: impl Into<crate::preferences::Language> + Copy,
) -> bool {
    let state = form.list(index).cloned();
    let options = match &state {
        Some(ListState::Answered(ChoiceList::Available { options })) => options.as_slice(),
        _ => &[],
    };
    let unlisted = form.is_unavailable(index);
    let mark = |value: &str| format!("{value} — {}", tr(es, "unavailable"));
    // A plugin may translate the fixed texts of its lists, such as its default device.
    let texts = form.texts.clone();
    let language = es.into();
    let shown = match options.iter().find(|option| option.value == *value) {
        _ if value.is_empty() => tr(es, "Choose…").to_owned(),
        Some(option) => texts.get(language, &option.label).to_owned(),
        None if unlisted => mark(value),
        None => value.clone(),
    };
    let before = value.clone();
    // With nothing to pick from the control stays, quiet, and the line under it says why.
    ui.add_enabled_ui(!options.is_empty(), |ui| {
        egui::ComboBox::from_id_salt(("dynamic", index))
            .width(220.)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if unlisted && !before.is_empty() {
                    ui.selectable_value(value, before.clone(), mark(&before));
                }
                for option in options {
                    ui.selectable_value(
                        value,
                        option.value.clone(),
                        texts.get(language, &option.label),
                    );
                }
            });
    });
    // A question in flight is not asked again: the control appears once it has an answer.
    if matches!(state, Some(ListState::Answered(_) | ListState::Failed(_)))
        && icon_button_with_id(
            ui,
            look,
            Icon::Refresh,
            tr(es, "Ask again"),
            Some(ask_again_id(index)),
        )
        .clicked()
    {
        form.refresh_list(index);
    }
    *value != before
}

/// What is known about the list of a dynamic choice, in one quiet line under its control,
/// indented by `indent` points so that it lines up with the control.
pub(super) fn list_status(
    ui: &mut egui::Ui,
    look: &Look,
    form: &FormDraft,
    index: usize,
    es: impl Into<crate::preferences::Language> + Copy,
    indent: f32,
) {
    let line = |ui: &mut egui::Ui, text: egui::RichText| {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            ui.label(text);
        });
    };
    // A field that is not set has no control, so nothing to say about its list.
    let Some(value) = form.fields[index].as_deref() else {
        return;
    };
    let kept = !value.is_empty();
    let with_kept = |en: &'static str| {
        let text = tr(es, en);
        if kept {
            format!("{text} — {}", tr(es, "value kept"))
        } else {
            text.to_owned()
        }
    };
    // Before anything is asked, say what the list is waiting for: not a state to worry about.
    if let Some(dependency) = form.waiting_for(index) {
        let text = tr(es, "Choose {field} first")
            .replace("{field}", form.texts.get(es.into(), dependency));
        line(ui, egui::RichText::new(text).size(12.).color(look.muted));
        return;
    }
    let (text, alert) = match form.list(index) {
        None => return,
        Some(ListState::Due | ListState::Asking(_)) => (tr(es, "Loading…").to_owned(), false),
        Some(ListState::Answered(ChoiceList::Available { options })) if options.is_empty() => {
            (tr(es, "No options").to_owned(), false)
        }
        Some(ListState::Answered(ChoiceList::Available { .. })) => return,
        Some(ListState::Answered(ChoiceList::Unavailable)) => {
            (with_kept("Source unavailable"), true)
        }
        Some(ListState::Failed(Failure::NoAnswer)) => {
            (describe_error("options_timeout", es).to_owned(), true)
        }
        Some(ListState::Failed(Failure::PluginUnavailable)) => {
            (with_kept("Plugin not available"), true)
        }
        Some(ListState::Failed(Failure::Busy)) => {
            (tr(es, "Plugin busy or list too large").to_owned(), true)
        }
        Some(ListState::Failed(Failure::Other(code))) => {
            (describe_error(code, es).to_owned(), true)
        }
    };
    let color = if alert { look.warn } else { look.muted };
    line(ui, egui::RichText::new(text).size(12.).color(color));
}

#[cfg(test)]
mod conversation_tests {
    use super::*;

    use crate::plugins::{FormTarget, LoadedForm};
    use rad_plugin_contract::{Definition, Field, FieldKind, Schema};
    use serde_json::json;
    use std::{
        sync::{
            Arc, Mutex,
            mpsc::{Receiver, Sender, channel},
        },
        time::{Duration, Instant},
    };

    fn pick() -> Definition {
        let dynamic = |id: &str, depends_on: &[&str]| Field {
            id: id.into(),
            label: id.into(),
            required: true,
            kind: FieldKind::DynamicChoice {
                depends_on: depends_on.iter().map(|d| d.to_string()).collect(),
            },
        };
        Definition {
            gestures: rad_plugin_contract::default_gestures(),
            response_timeout: None,
            id: "pick".into(),
            label: "pick".into(),
            schema: Schema {
                version: 1,
                fields: vec![
                    Field {
                        id: "scenario".into(),
                        label: "scenario".into(),
                        required: true,
                        kind: FieldKind::Choice {
                            options: vec!["normal".into(), "empty".into()],
                        },
                    },
                    dynamic("group", &["scenario"]),
                    dynamic("member", &["scenario", "group"]),
                ],
            },
            credential_slots: vec![],
        }
    }

    /// An editor with the form of a saved action open, and a worker standing in for the
    /// connection: it reports each question on `asked` and holds it until `release`.
    struct Conversation {
        ui: PluginUi,
        worker: Worker<OptionsJob, OptionsEvent>,
        asked: Receiver<Request>,
        release: Sender<()>,
    }

    use crate::Request;

    impl Conversation {
        fn new() -> Self {
            let (asked_tx, asked) = channel();
            let (release, released) = channel::<()>();
            let released = Arc::new(Mutex::new(released));
            let worker = Worker::spawn(move |job: OptionsJob| {
                let field = job.request.arguments()["field"].clone();
                asked_tx.send(job.request).unwrap();
                released.lock().unwrap().recv().unwrap();
                OptionsEvent {
                    ticket: job.ticket,
                    result: Ok(json!({
                        "type": "p:a", "field": field, "availability": "available",
                        "items": [{"value":"x","label":"X"}], "next_cursor": null,
                    })),
                }
            });
            let saved =
                br#"{"schema_version":1,"values":{"scenario":"normal","group":"a","member":"a1"}}"#;
            let loaded = LoadedForm {
                type_key: "p:a".into(),
                id: Some("action-1".into()),
                form: Some(FormDraft::load(pick(), saved.to_vec())),
                original: saved.to_vec(),
            };
            let mut ui = PluginUi::default();
            ui.form = Some((FormTarget::Action("action-1".into()), loaded));
            Self {
                ui,
                worker,
                asked,
                release,
            }
        }
        fn form(&mut self) -> &mut FormDraft {
            self.ui.form.as_mut().unwrap().1.form.as_mut().unwrap()
        }
        fn question(&self) -> Request {
            self.asked
                .recv_timeout(Duration::from_secs(5))
                .expect("a question was sent")
        }
        /// Pumps until the next question reaches the worker.
        fn pumped_question(&mut self) -> Request {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                self.ui.pump_options(&mut self.worker, true);
                if let Ok(request) = self.asked.try_recv() {
                    return request;
                }
                assert!(Instant::now() < deadline, "no question was sent");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        /// Pumps until the list at `index` is answered: the worker answers on its own thread.
        fn until_answered(&mut self, index: usize) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !matches!(self.form().list(index), Some(ListState::Answered(_))) {
                assert!(Instant::now() < deadline, "list {index} was never answered");
                self.ui.pump_options(&mut self.worker, true);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    fn field_of(request: &Request) -> String {
        request.arguments()["field"].as_str().unwrap().to_owned()
    }

    #[test]
    fn an_answer_for_a_context_that_changed_meanwhile_is_dropped_and_the_list_is_asked_again() {
        let mut talk = Conversation::new();
        talk.ui.pump_options(&mut talk.worker, true);
        assert_eq!(field_of(&talk.question()), "group");

        talk.form().set(0, Some("empty".into())); // the user changes the scenario meanwhile
        talk.release.send(()).unwrap(); // the old question is answered
        let again = talk.pumped_question();
        assert_eq!(field_of(&again), "group");
        assert_eq!(
            again.arguments()["context"],
            json!({"scenario":"empty"}),
            "the new context"
        );
        assert!(
            !matches!(talk.form().list(1), Some(ListState::Answered(_))),
            "the answer of the old scenario was not taken"
        );
        talk.release.send(()).unwrap();
        talk.until_answered(1);
    }
}
