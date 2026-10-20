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
    worker::Worker,
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
    ) {
        if let Some(event) = worker.poll()
            && let Some(form) = self
                .form
                .as_mut()
                .and_then(|(_, loaded)| loaded.form.as_mut())
        {
            form.accept_answer(event.ticket, event.result);
        }
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
    es: bool,
) -> bool {
    let state = form.list(index).cloned();
    let options = match &state {
        Some(ListState::Answered(ChoiceList::Available { options })) => options.as_slice(),
        _ => &[],
    };
    let unlisted = form.is_unavailable(index);
    let mark = |value: &str| format!("{value} — {}", tr(es, "unavailable", "no disponible"));
    let shown = match options.iter().find(|option| option.value == *value) {
        _ if value.is_empty() => tr(es, "Choose…", "Elige…").to_owned(),
        Some(option) => option.label.clone(),
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
                    ui.selectable_value(value, option.value.clone(), &option.label);
                }
            });
    });
    // A question in flight is not asked again: the control appears once it has an answer.
    if matches!(state, Some(ListState::Answered(_) | ListState::Failed(_)))
        && icon_button_with_id(
            ui,
            look,
            Icon::Refresh,
            tr(es, "Ask again", "Volver a consultar"),
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
    es: bool,
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
    let with_kept = |en: &'static str, es_text: &'static str| {
        let text = tr(es, en, es_text);
        if kept {
            format!("{text} — {}", tr(es, "value kept", "se conserva el valor"))
        } else {
            text.to_owned()
        }
    };
    // Before anything is asked, say what the list is waiting for: not a state to worry about.
    if let Some(dependency) = form.waiting_for(index) {
        let text = if es {
            format!("Elige primero {dependency}")
        } else {
            format!("Choose {dependency} first")
        };
        line(ui, egui::RichText::new(text).size(12.).color(look.muted));
        return;
    }
    let (text, alert) = match form.list(index) {
        None => return,
        Some(ListState::Due | ListState::Asking(_)) => {
            (tr(es, "Loading…", "Cargando…").to_owned(), false)
        }
        Some(ListState::Answered(ChoiceList::Available { options })) if options.is_empty() => {
            (tr(es, "No options", "Sin opciones").to_owned(), false)
        }
        Some(ListState::Answered(ChoiceList::Available { .. })) => return,
        Some(ListState::Answered(ChoiceList::Unavailable)) => (
            with_kept("Source unavailable", "Fuente no disponible"),
            true,
        ),
        Some(ListState::Failed(Failure::NoAnswer)) => {
            (describe_error("options_timeout", es).to_owned(), true)
        }
        Some(ListState::Failed(Failure::PluginUnavailable)) => (
            with_kept("Plugin not available", "Plugin no disponible"),
            true,
        ),
        Some(ListState::Failed(Failure::Busy)) => (
            tr(
                es,
                "Plugin busy or list too large",
                "Plugin ocupado o lista demasiado grande",
            )
            .to_owned(),
            true,
        ),
        Some(ListState::Failed(Failure::Other(code))) => {
            (describe_error(code, es).to_owned(), true)
        }
    };
    let color = if alert { look.warn } else { look.muted };
    line(ui, egui::RichText::new(text).size(12.).color(color));
}

#[cfg(test)]
mod tests {
    use super::super::plugins::form_ui;
    use super::*;
    use crate::theme::Palette;
    use rad_plugin_contract::{Definition, Field, FieldKind, Schema};
    use serde_json::json;

    const TYPE: &str = "org.rad.test.options:pick";

    fn target() -> Target<'static> {
        Target {
            type_key: TYPE,
            entity: Some("action-1"),
        }
    }

    /// The `pick` action of the options test plugin: `scenario`, then `group`, then `member`.
    fn pick() -> Definition {
        let field = |id: &str, required, kind| Field {
            id: id.into(),
            label: id.into(),
            required,
            kind,
        };
        let dynamic = |depends_on: &[&str]| FieldKind::DynamicChoice {
            depends_on: depends_on.iter().map(|d| d.to_string()).collect(),
        };
        Definition {
            response_timeout: None,
            id: "pick".into(),
            label: "pick".into(),
            schema: Schema {
                version: 1,
                fields: vec![
                    field(
                        "scenario",
                        true,
                        FieldKind::Choice {
                            options: vec!["normal".into(), "empty".into()],
                        },
                    ),
                    field("group", true, dynamic(&["scenario"])),
                    field("member", false, dynamic(&["scenario", "group"])),
                ],
            },
            credential_slots: vec![],
        }
    }

    fn saved_form() -> FormDraft {
        let saved =
            br#"{"schema_version":1,"values":{"scenario":"normal","group":"a","member":"a1"}}"#;
        FormDraft::load(pick(), saved.to_vec())
    }

    /// Asks the next due question and gives it `answer`.
    fn answer(form: &mut FormDraft, ticket: u64, answer: Given) {
        let query = form
            .next_query(target(), ticket)
            .expect("a question is due");
        let field = form.definition.schema.fields[query.field].id.clone();
        let result = match answer {
            Ok((availability, items)) => Ok(json!({
                "type": TYPE, "field": field, "availability": availability,
                "items": items.iter().map(|(v, l)| json!({"value":v,"label":l})).collect::<Vec<_>>(),
                "next_cursor": null,
            })),
            Err(code) => Err(Error {
                code: code.into(),
                unknown: false,
            }),
        };
        assert!(form.accept_answer(ticket, result));
    }

    const GROUPS: &[(&str, &str)] = &[("a", "Group A"), ("b", "Group B")];

    /// Both lists answered: the saved group is listed, the saved member `a1` is not.
    fn answered_form() -> FormDraft {
        let mut form = saved_form();
        answer(&mut form, 1, Ok(("available", GROUPS)));
        answer(&mut form, 2, Ok(("available", &[("b1", "Member B1")])));
        form
    }

    type Texts = Vec<(String, egui::Rect)>;
    /// What a question gets back: an availability with its (value, label) items, or an error code.
    type Given<'a> = Result<(&'a str, &'a [(&'a str, &'a str)]), &'a str>;

    struct Screen {
        ctx: egui::Context,
        look: Look,
        tick: u32,
        /// Colour of each text drawn in the last frame.
        colors: std::collections::HashMap<String, egui::Color32>,
    }

    impl Screen {
        fn new() -> Self {
            Self {
                ctx: egui::Context::default(),
                look: Look::new(&Palette::default()),
                tick: 0,
                colors: Default::default(),
            }
        }
        fn frame(&mut self, form: &mut FormDraft, es: bool, events: Vec<egui::Event>) -> Texts {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900., 400.),
                )),
                time: Some(f64::from(self.tick) * 0.05),
                events,
                ..Default::default()
            };
            self.tick += 1;
            let look = self.look.clone();
            let output = self.ctx.run_ui(input, |ui| form_ui(ui, &look, form, es));
            self.colors.clear();
            output
                .shapes
                .into_iter()
                .filter_map(|s| match s.shape {
                    egui::epaint::Shape::Text(text) => {
                        let text_color = text.galley.job.sections.first().map(|s| s.format.color);
                        self.colors
                            .extend(text_color.map(|c| (text.galley.job.text.clone(), c)));
                        Some((
                            text.galley.job.text.clone(),
                            text.galley.rect.translate(text.pos.to_vec2()),
                        ))
                    }
                    _ => None,
                })
                .collect()
        }
        /// Two quiet frames: what is drawn once the form has reacted to the last event.
        fn settle(&mut self, form: &mut FormDraft, es: bool) -> Texts {
            self.frame(form, es, vec![]);
            self.frame(form, es, vec![])
        }
        fn click(&mut self, form: &mut FormDraft, es: bool, at: egui::Pos2) -> Texts {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            self.frame(form, es, vec![egui::Event::PointerMoved(at)]);
            self.frame(form, es, vec![press(true)]);
            self.frame(form, es, vec![press(false)]);
            self.settle(form, es)
        }
        fn ask_again(&self, index: usize) -> Option<egui::Pos2> {
            self.ctx
                .read_response(ask_again_id(index))
                .map(|r| r.rect.center())
        }
    }

    fn drawn(texts: &Texts, wanted: &str) -> bool {
        texts.iter().any(|(text, _)| text == wanted)
    }

    fn place(texts: &Texts, wanted: &str) -> egui::Pos2 {
        texts
            .iter()
            .find(|(text, _)| text == wanted)
            .unwrap_or_else(|| {
                panic!(
                    "`{wanted}` not drawn; drawn: {:?}",
                    texts.iter().map(|t| &t.0).collect::<Vec<_>>()
                )
            })
            .1
            .center()
    }

    #[test]
    fn a_list_still_being_asked_reads_as_loading_and_offers_nothing_to_ask_again() {
        for (es, loading) in [(false, "Loading…"), (true, "Cargando…")] {
            let mut screen = Screen::new();
            let mut form = saved_form();
            let texts = screen.settle(&mut form, es);
            assert!(drawn(&texts, loading), "{loading}");
            assert!(
                drawn(&texts, "a"),
                "the saved value shows as it is until the list arrives"
            );
            assert!(screen.ask_again(1).is_none() && screen.ask_again(2).is_none());
            form.next_query(target(), 1);
            let texts = screen.settle(&mut form, es);
            assert!(drawn(&texts, loading), "in flight reads the same");
            assert!(screen.ask_again(1).is_none());
        }
    }

    #[test]
    fn an_answered_list_shows_labels_marks_the_value_it_lacks_and_offers_to_ask_again() {
        let mut screen = Screen::new();
        let mut form = answered_form();
        let texts = screen.settle(&mut form, false);
        assert!(drawn(&texts, "Group A"), "the saved group shows its label");
        assert!(
            drawn(&texts, "a1 — unavailable"),
            "the saved member is kept and marked"
        );
        assert!(!drawn(&texts, "Loading…"));
        assert!(screen.ask_again(1).is_some() && screen.ask_again(2).is_some());
        assert!(
            screen.ask_again(0).is_none(),
            "a fixed choice has no list to ask again"
        );

        let texts = screen.settle(&mut form, true);
        assert!(drawn(&texts, "a1 — no disponible"));
    }

    #[test]
    fn empty_unavailable_and_unanswered_lists_are_told_apart_in_both_languages() {
        // (group's answer, English line, Spanish line, text the control shows for the saved "a")
        let cases: [(Given, &str, &str, &str); 6] = [
            (
                Ok(("available", &[])),
                "No options",
                "Sin opciones",
                "a — unavailable",
            ),
            (
                Ok(("unavailable", &[])),
                "Source unavailable — value kept",
                "Fuente no disponible — se conserva el valor",
                "a — unavailable",
            ),
            (
                Err("options_timeout"),
                "The plugin did not answer in time; try again",
                "El plugin no respondió a tiempo; vuelve a intentarlo",
                "a",
            ),
            (
                Err("executor_unavailable"),
                "Plugin not available — value kept",
                "Plugin no disponible — se conserva el valor",
                "a",
            ),
            (
                Err("resource_exhausted"),
                "Plugin busy or list too large",
                "Plugin ocupado o lista demasiado grande",
                "a",
            ),
            (
                Err("some_future_code"),
                "some_future_code",
                "some_future_code",
                "a",
            ),
        ];
        let mut lines = [vec![], vec![]];
        for (given, en, es_line, shown) in cases {
            for (es, line) in [(false, en), (true, es_line)] {
                let mut screen = Screen::new();
                let mut form = saved_form();
                answer(&mut form, 1, given);
                answer(&mut form, 2, Ok(("available", &[("a1", "Member A1")])));
                let texts = screen.settle(&mut form, es);
                assert!(drawn(&texts, line), "{line}");
                let shown = if es {
                    shown.replace("unavailable", "no disponible")
                } else {
                    shown.to_owned()
                };
                assert!(drawn(&texts, &shown), "{line}: the control shows `{shown}`");
                assert!(
                    screen.ask_again(1).is_some(),
                    "{line}: the user can ask again"
                );
                assert!(!drawn(&texts, if es { "Cargando…" } else { "Loading…" }));
                lines[usize::from(es)].push(line);
            }
        }
        for mut lines in lines {
            lines.sort_unstable();
            lines.dedup();
            assert_eq!(lines.len(), 6, "every state has its own words");
        }
    }

    #[test]
    fn a_list_that_waits_for_a_required_choice_says_which_in_a_quiet_line_and_offers_nothing() {
        for (es, line, loading) in [
            (false, "Choose scenario first", "Loading…"),
            (true, "Elige primero scenario", "Cargando…"),
        ] {
            let mut screen = Screen::new();
            let mut form = FormDraft::new(pick());
            form.set(2, Some(String::new())); // the user sets the optional member as well
            let texts = screen.settle(&mut form, es);
            assert!(drawn(&texts, line), "{line}");
            assert_eq!(
                screen.colors.get(line),
                Some(&screen.look.muted),
                "quiet, not a warning"
            );
            assert!(!drawn(&texts, loading), "nothing is being asked");
            assert!(screen.ask_again(1).is_none() && screen.ask_again(2).is_none());

            form.set(0, Some("normal".into()));
            let texts = screen.settle(&mut form, es);
            assert!(
                drawn(&texts, loading),
                "the group is asked as soon as the scenario is chosen"
            );
            let waiting = if es {
                "Elige primero group"
            } else {
                "Choose group first"
            };
            assert!(drawn(&texts, waiting), "the member now waits for the group");
        }
    }

    #[test]
    fn picking_an_item_stores_its_value_and_the_lists_that_depend_on_it_load_again() {
        let mut screen = Screen::new();
        let mut form = answered_form();
        let texts = screen.settle(&mut form, false);
        let texts = screen.click(&mut form, false, place(&texts, "Group A"));
        let texts = screen.click(&mut form, false, place(&texts, "Group B"));
        assert_eq!(
            form.fields[1].as_deref(),
            Some("b"),
            "the value, not the label"
        );
        assert_eq!(
            form.list(2),
            Some(&ListState::Due),
            "the member list is asked again"
        );
        assert!(drawn(&texts, "Group B") && drawn(&texts, "Loading…"));
        assert_eq!(
            form.fields[2].as_deref(),
            Some("a1"),
            "its value is kept meanwhile"
        );
    }

    #[test]
    fn the_kept_value_stays_in_the_drop_down_marked_and_another_value_replaces_it() {
        let mut screen = Screen::new();
        let mut form = answered_form();
        let texts = screen.settle(&mut form, false);
        let opened = screen.click(&mut form, false, place(&texts, "a1 — unavailable"));
        assert!(drawn(&opened, "Member B1"));
        assert_eq!(
            opened
                .iter()
                .filter(|(t, _)| t == "a1 — unavailable")
                .count(),
            2,
            "once on the control, once as the entry that keeps it"
        );
        screen.click(&mut form, false, place(&opened, "Member B1"));
        assert_eq!(form.fields[2].as_deref(), Some("b1"));
        assert_eq!(form.publish().unwrap().unwrap()["values"]["member"], "b1");
    }

    #[test]
    fn asking_again_puts_that_list_back_to_loading_and_hides_the_control_meanwhile() {
        let mut screen = Screen::new();
        let mut form = answered_form();
        screen.settle(&mut form, false);
        let at = screen
            .ask_again(1)
            .expect("offered once the list is answered");
        let texts = screen.click(&mut form, false, at);
        assert_eq!(form.list(1), Some(&ListState::Due));
        assert!(drawn(&texts, "Loading…"));
        assert!(screen.ask_again(1).is_none());
        assert!(screen.ask_again(2).is_some(), "the other list is untouched");
    }

    #[test]
    fn an_optional_field_that_is_not_set_shows_no_list_and_no_line() {
        let mut screen = Screen::new();
        let without_member = br#"{"schema_version":1,"values":{"scenario":"normal","group":"a"}}"#;
        let mut form = FormDraft::load(pick(), without_member.to_vec());
        answer(&mut form, 1, Ok(("available", GROUPS)));
        let texts = screen.settle(&mut form, false);
        assert!(
            !drawn(&texts, "Loading…"),
            "member is not set, so nothing to wait for"
        );
        assert!(screen.ask_again(2).is_none());
    }
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
                let field = job.request.arguments["field"].clone();
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

    fn field_of(request: &Request) -> &str {
        request.arguments["field"].as_str().unwrap()
    }

    #[test]
    fn the_lists_are_asked_one_at_a_time_and_nothing_queues_behind_a_slow_plugin() {
        let mut talk = Conversation::new();
        talk.ui.pump_options(&mut talk.worker, true);
        let first = talk.question();
        assert_eq!(field_of(&first), "group");
        assert_eq!(
            first.arguments["id"], "action-1",
            "asked by entity while editing"
        );
        for _ in 0..20 {
            talk.ui.pump_options(&mut talk.worker, true);
        }
        assert!(
            talk.asked.try_recv().is_err(),
            "no second question while the first is out"
        );

        // The user changes the scenario while that question is out: the group is due again,
        // but nothing is sent, and nothing fails, until the worker is free.
        talk.form().set(0, Some("empty".into()));
        for _ in 0..20 {
            talk.ui.pump_options(&mut talk.worker, true);
        }
        assert!(
            talk.asked.try_recv().is_err(),
            "nothing queues behind the open question"
        );
        assert_eq!(
            talk.form().list(1),
            Some(&ListState::Due),
            "waiting, not failed"
        );

        talk.release.send(()).unwrap();
        let again = talk.pumped_question();
        assert_eq!(field_of(&again), "group");
        assert_eq!(again.arguments["context"], json!({"scenario":"empty"}));
        talk.release.send(()).unwrap();
        talk.until_answered(1);
        let second = talk.question();
        assert_eq!(field_of(&second), "member");
        assert_eq!(
            second.arguments["context"],
            json!({"scenario":"empty","group":"a"})
        );
        talk.release.send(()).unwrap();
        talk.until_answered(2);
        talk.ui.pump_options(&mut talk.worker, true);
        assert!(
            talk.asked.try_recv().is_err(),
            "everything answered, nothing more to ask"
        );
        assert!(!talk.worker.busy());
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
            again.arguments["context"],
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

    #[test]
    fn nothing_is_asked_without_a_connection_a_form_or_a_form_the_plugin_cannot_use() {
        let mut talk = Conversation::new();
        talk.ui.pump_options(&mut talk.worker, false);
        assert!(talk.asked.try_recv().is_err(), "not connected");

        talk.ui.form.as_mut().unwrap().1.form = None; // the plugin is unavailable
        talk.ui.pump_options(&mut talk.worker, true);
        talk.ui.form = None; // the form was closed
        talk.ui.pump_options(&mut talk.worker, true);
        assert!(talk.asked.try_recv().is_err());
        assert!(!talk.worker.busy());
    }

    #[test]
    fn an_answer_that_arrives_after_the_form_closed_is_dropped() {
        let mut talk = Conversation::new();
        talk.ui.pump_options(&mut talk.worker, true);
        talk.question();
        talk.ui.form = None;
        talk.release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while talk.worker.busy() {
            assert!(Instant::now() < deadline);
            talk.ui.pump_options(&mut talk.worker, true);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(talk.ui.form.is_none());
    }
}
