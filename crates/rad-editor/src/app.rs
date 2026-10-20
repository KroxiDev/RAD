//! Editor window: state, worker pump and layout. Views live in the submodules; the
//! runtime stays the only authority, so nothing here edits published state optimistically.
use crate::{
    Controller, PollSchedule, Request,
    catalog::Entry,
    native::{self, Event, Job, OptionsEvent, OptionsJob, Reply, TransferKind},
    session::{Board, Inspection},
    theme::Palette,
    worker::Worker,
};
use eframe::egui;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

mod actions;
mod bar;
mod credentials;
mod deck;
mod inspector;
mod look;
mod options;
mod plugins;
mod settings;

use look::Look;

/// Pause after the last keystroke before the application list is searched again.
const SEARCH_DEBOUNCE_MS: u64 = 250;

/// Duration a wait starts with when it is put on a key.
const DEFAULT_WAIT_MS: u64 = 1000;

/// Something from the list that becomes an action on a key.
#[derive(Clone)]
enum NewAction {
    Launcher(Entry),
    Wait,
    Plugin(crate::plugins::CatalogEntry),
}
#[derive(Clone)]
enum Drag {
    Button {
        id: String,
        label: String,
    },
    /// An action from the list, with the name shown while it is dragged.
    Add {
        action: NewAction,
        label: String,
    },
    Folder,
    /// A row of the selected key's actions, by its place in the draft order.
    Reorder(usize),
}
#[derive(Default)]
struct Draft {
    id: String,
    name: String,
    text: String,
    image: String,
    /// `top`, `middle` or `bottom`, as the runtime resolved it or as just chosen.
    text_position: String,
    mode: String,
    order: Vec<usize>,
    /// Milliseconds typed for each action; only wait actions use theirs.
    waits: Vec<String>,
    entries: Vec<String>,
    permissions: Value,
    action_permissions: Vec<Value>,
}
impl Draft {
    fn load(v: &Inspection) -> Self {
        Self {
            id: string(&v.button["id"]),
            name: string(&v.button["fields"]["name"]),
            text: string(&v.appearance["text"]),
            image: string(&v.appearance["image"]),
            text_position: string(&v.appearance["text_position"]),
            mode: string(&v.button["fields"]["mode"]),
            order: (0..v.actions.len()).collect(),
            waits: v
                .actions
                .iter()
                .map(|a| {
                    a["fields"]["parameters"]["milliseconds"]
                        .as_u64()
                        .map(|n| n.to_string())
                        .unwrap_or_default()
                })
                .collect(),
            entries: v
                .actions
                .iter()
                .map(|a| string(&a["fields"]["parameters"]["desktop_entry"]))
                .collect(),
            permissions: v.button["fields"]["permissions"].clone(),
            action_permissions: v
                .actions
                .iter()
                .map(|a| a["fields"]["permissions"].clone())
                .collect(),
        }
    }
}
fn string(v: &Value) -> String {
    v.as_str().unwrap_or_default().into()
}
fn tr(es: bool, en: &'static str, es_text: &'static str) -> &'static str {
    if es { es_text } else { en }
}
pub struct App {
    worker: Worker<Job, Event>,
    /// Questions about plugin option lists, on a connection of their own: a slow plugin
    /// must not hold up Apply or the polling that `worker` does.
    options_worker: Worker<OptionsJob, OptionsEvent>,
    credential_worker: credentials::CredentialWorker,
    credentials: credentials::Panel,
    credential_selection: Option<String>,
    control: Controller,
    poll: PollSchedule,
    started: Instant,
    connected: bool,
    /// A connection attempt has finished, so "not connected" is a fact worth showing.
    attempted: bool,
    force: bool,
    inspect_needed: bool,
    board: Option<Board>,
    status: Value,
    inspection: Option<Inspection>,
    draft: Draft,
    selected_slot: Option<usize>,
    message: String,
    message_is_error: bool,
    /// Raw code behind an error message, kept for the tooltip.
    message_code: String,
    es: bool,
    palette: Palette,
    styled: Option<Palette>,
    settings_open: bool,
    settings_tab: settings::Tab,
    settings: Value,
    new_name: String,
    move_folder: String,
    move_position: usize,
    transfer_active: bool,
    image_path: String,
    backup_path: String,
    restore_token: Option<String>,
    restore_summary: String,
    global_permissions: Value,
    catalog: crate::catalog::Catalog,
    /// When the application list must be asked again, in milliseconds since start.
    catalog_due: Option<u64>,
    /// Action dropped on an empty key: added once the new key is published.
    pending_action: Option<(usize, NewAction)>,
    /// Key that was just sent an application, and that application's desktop entry: the
    /// key may take its icon once the action is applied.
    icon_candidate: Option<(String, String)>,
    /// The user asked for the application icon, so a missing one deserves a word.
    icon_asked: bool,
    /// Action categories the user folded away in the list.
    folded: std::collections::BTreeSet<actions::Category>,
    textures: std::collections::BTreeMap<String, egui::TextureHandle>,
    image_attempts: std::collections::BTreeSet<String>,
    result_poll: PollSchedule,
    plugins: plugins::PluginUi,
}
impl App {
    pub fn new(runtime: PathBuf, theme: PathBuf) -> Self {
        let options_worker = native::start_options(runtime.clone());
        let credential_worker = credentials::start(runtime.clone());
        let mut worker = native::start(runtime, theme);
        let _ = worker.submit(Job::Connect);
        Self {
            worker,
            options_worker,
            credential_worker,
            credentials: Default::default(),
            credential_selection: None,
            control: Controller::default(),
            poll: PollSchedule::default(),
            started: Instant::now(),
            connected: false,
            attempted: false,
            force: true,
            inspect_needed: false,
            board: None,
            status: Value::Null,
            inspection: None,
            draft: Draft::default(),
            selected_slot: None,
            message: String::new(),
            message_is_error: false,
            message_code: String::new(),
            es: false,
            palette: Palette::default(),
            styled: None,
            settings_open: false,
            settings_tab: Default::default(),
            settings: Value::Null,
            new_name: String::new(),
            move_folder: String::new(),
            move_position: 0,
            transfer_active: false,
            image_path: String::new(),
            backup_path: String::new(),
            restore_token: None,
            restore_summary: String::new(),
            global_permissions: Value::Null,
            catalog: Default::default(),
            catalog_due: None,
            pending_action: None,
            icon_candidate: None,
            icon_asked: false,
            folded: Default::default(),
            textures: Default::default(),
            image_attempts: Default::default(),
            result_poll: PollSchedule::default(),
            plugins: Default::default(),
        }
    }
    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
    fn say(&mut self, text: impl Into<String>) {
        self.message = text.into();
        self.message_is_error = false;
        self.message_code.clear();
    }
    fn warn(&mut self, text: impl Into<String>) {
        self.message = text.into();
        self.message_is_error = true;
        self.message_code.clear();
    }
    /// Says how a run of a key is going, in words; its execution id stays in the tooltip.
    fn say_run(&mut self, execution: &Value, state: &str) {
        let text = match state {
            "running" => tr(self.es, "Running…", "Ejecutando…"),
            "succeeded" => tr(self.es, "Done", "Hecho"),
            "failed" => tr(self.es, "Failed", "Falló"),
            other => other,
        }
        .to_owned();
        if state == "failed" {
            self.warn(text);
        } else {
            self.say(text);
        }
        self.message_code = string(execution);
    }
    /// A user command waits behind the in-flight poll; controls never depend on polling.
    fn command(&mut self, job: Job) -> bool {
        let accepted = self.worker.enqueue(job).is_ok();
        if !accepted {
            self.warn(tr(self.es, "Busy — not sent", "Ocupado — no enviado"));
        }
        accepted
    }
    fn send(&mut self, request: Request) {
        self.command(Job::Request(request));
    }
    fn selected(&self) -> Option<String> {
        self.control.selected().map(str::to_owned)
    }
    /// Runs the selected key now, through the same admission as a physical press.
    fn try_selected(&mut self) {
        if let Some(request) = self.control.try_selected() {
            self.send(request);
        }
    }
    fn folder(&self) -> Option<String> {
        self.board
            .as_ref()
            .and_then(|b| b.folder["id"].as_str().map(str::to_owned))
    }
    fn slot_id(&self, position: usize) -> Option<String> {
        self.board.as_ref()?.folder["fields"]["slots"].get(position)?["id"]
            .as_str()
            .map(str::to_owned)
    }
    fn select(&mut self, position: usize, id: Option<String>) {
        self.credentials.clear();
        self.plugins.form = None;
        self.selected_slot = Some(position);
        self.control.select(id);
        self.inspection = None;
        self.draft = Draft::default();
        self.inspect_needed = self.control.selected().is_some();
    }
    /// Keeps the selection on what the runtime just published: it follows a moved key,
    /// falls back to the slot when the key is gone, and adopts a key created in the slot.
    fn follow_selection(&mut self) {
        match self.selected() {
            Some(id) => {
                let position = (0..32).find(|&p| self.slot_id(p).as_deref() == Some(id.as_str()));
                if position.is_some() {
                    self.selected_slot = position;
                } else {
                    self.control.select(None);
                    self.inspection = None;
                    self.draft = Draft::default();
                }
            }
            None => {
                if let Some(position) = self.selected_slot
                    && let Some(id) = self.slot_id(position)
                {
                    self.select(position, Some(id));
                }
            }
        }
    }
    /// Name for a new key or folder: what the user typed, else a default for its kind.
    fn creation_name(&self, folder: bool) -> String {
        let typed = self.new_name.trim();
        if !typed.is_empty() {
            return typed.to_owned();
        }
        if folder {
            tr(self.es, "New folder", "Nueva carpeta").into()
        } else {
            tr(self.es, "New button", "Nuevo botón").into()
        }
    }
    fn create(&mut self, position: usize, folder: bool) {
        let Some(parent) = self.folder() else {
            return;
        };
        let name = self.creation_name(folder);
        self.send(if folder {
            Request::new(
                "create_folder",
                json!({"parent_id":parent,"position":position,"name":name}),
            )
        } else {
            Request::new(
                "create_button",
                json!({"folder_id":parent,"position":position,"name":name}),
            )
        });
        self.new_name.clear();
    }
    fn add_action(&mut self, button: &str, action: &NewAction) {
        if let NewAction::Plugin(entry) = action {
            self.open_plugin_form(
                &entry.type_key,
                None,
                crate::plugins::FormTarget::AddAction(button.into()),
            );
            return;
        }
        let (kind, parameters) = match action {
            NewAction::Launcher(entry) => {
                ("launcher", json!({"desktop_entry":entry.desktop_entry}))
            }
            NewAction::Wait => ("wait", json!({"milliseconds":DEFAULT_WAIT_MS})),
            NewAction::Plugin(_) => unreachable!(),
        };
        let sent = self.command(Job::Request(crate::commands::add_action(
            button, kind, parameters,
        )));
        if sent && let NewAction::Launcher(entry) = action {
            self.icon_candidate = Some((button.to_owned(), entry.desktop_entry.clone()));
        }
    }
    /// What the list calls `action`; also the name of a key created just for it.
    fn action_name(&self, action: &NewAction) -> String {
        match action {
            NewAction::Launcher(entry) => entry.name.clone(),
            NewAction::Wait => tr(self.es, "Wait", "Esperar").into(),
            NewAction::Plugin(entry) => entry.label.clone(),
        }
    }
    /// An action lands on a key: added to an existing one, or a key is created for it.
    fn place_action(&mut self, position: usize, action: &NewAction) {
        if let Some(id) = self.slot_id(position) {
            self.add_action(&id, action);
        } else if let Some(folder) = self.folder() {
            self.send(Request::new(
                "create_button",
                json!({"folder_id":folder,"position":position,"name":self.action_name(action)}),
            ));
            self.pending_action = Some((position, action.clone()));
        }
    }
    fn pump(&mut self, ctx: &egui::Context) {
        if let Some(event) = self.worker.poll() {
            self.palette = event.palette;
            self.connected = event.connected;
            match event.result {
                Ok(Reply::PluginApplied(r)) => {
                    self.plugins.form_pending = false;
                    if r.status == rad_client::response::Status::Applied {
                        self.plugins.form = None;
                        self.force = true;
                        self.inspect_needed = self.selected().is_some();
                        self.say(tr(self.es, "Saved", "Guardado"));
                    } else {
                        self.warn(tr(
                            self.es,
                            "Not confirmed — draft retained",
                            "Sin confirmar — borrador conservado",
                        ));
                    }
                }
                Ok(Reply::PluginCatalog(catalog)) => {
                    self.plugins.catalog = catalog;
                    self.plugins.catalog_loaded = true;
                }
                Ok(Reply::Plugins { catalog, status }) => {
                    self.plugins.catalog_loaded = true;
                    self.plugins.catalog = catalog;
                    self.plugins.status = status;
                }
                Ok(Reply::PluginForm { target, loaded }) => {
                    self.plugins.form_loading = false;
                    self.plugins.form = Some((target, loaded));
                }
                Ok(Reply::PluginVisuals { revision, visuals }) => {
                    self.accept_plugin_visuals(ctx, &revision, visuals);
                }
                Err(e) => {
                    if event.plugin_admin {
                        self.plugins.admin_pending = false;
                    }
                    if event.plugin_apply {
                        self.plugins.form_pending = false;
                    }
                    if event.plugin_form {
                        self.plugins.form_loading = false;
                    }
                    if event.plugin_visual {
                        for face in self.plugins.visuals.values_mut() {
                            face.state = "stale".into();
                        }
                    }
                    if !event.connected {
                        self.plugins.visuals.clear();
                    }
                    self.attempted = true;
                    self.pending_action = None;
                    self.icon_candidate = None;
                    self.icon_asked = false;
                    if event.transfer_job || !event.connected {
                        self.transfer_active = false;
                    }
                    if matches!(e.code.as_str(), "not_found" | "result_expired") {
                        self.control.terminal();
                    }
                    let reason = crate::status::describe_error(&e.code, self.es);
                    if e.unknown {
                        self.warn(format!(
                            "{}: {reason}",
                            tr(
                                self.es,
                                "Outcome unknown; not retried",
                                "Resultado incierto; no reintentado"
                            )
                        ));
                    } else if e.code == "runtime_unavailable" {
                        // The bar already says so, next to the way out.
                        self.say("");
                    } else {
                        self.warn(reason);
                    }
                    self.message_code = e.code.clone();
                    if e.code == "runtime_unavailable" || e.code == "deadline_exceeded" {
                        self.connected = false;
                    }
                }
                Ok(Reply::Connected) => {
                    self.plugins.catalog_loaded = false;
                    self.attempted = true;
                    self.connected = true;
                    self.force = true;
                    self.catalog_due = Some(0);
                    self.say("");
                }
                Ok(Reply::Refresh(r)) => {
                    self.es = r.status["settings"]["gui_language"] == "es";
                    if !self.settings_open {
                        self.settings = r.status["settings"].clone();
                    }
                    if self.status["view_revision"] != r.status["view_revision"] {
                        self.plugins.catalog_loaded = false;
                    }
                    self.status = r.status;
                    if let Some(board) = r.board {
                        let changed_folder = self
                            .board
                            .as_ref()
                            .is_none_or(|old| old.folder["id"] != board.folder["id"]);
                        let refs: std::collections::BTreeSet<_> = board
                            .buttons
                            .iter()
                            .filter_map(|b| b.appearance["image"].as_str())
                            .map(str::to_owned)
                            .collect();
                        self.textures.retain(|key, _| refs.contains(key));
                        self.image_attempts.clear();
                        if self.board.as_ref().is_none_or(|old| {
                            old.folder["view_revision"] != board.folder["view_revision"]
                        }) {
                            self.plugins.visuals.clear();
                        }
                        self.board = Some(board);
                        if changed_folder {
                            self.control.select(None);
                            self.selected_slot = None;
                            self.inspection = None;
                        } else {
                            self.follow_selection();
                        }
                        if let Some((position, action)) = self.pending_action.take()
                            && let Some(id) = self.slot_id(position)
                        {
                            self.add_action(&id, &action);
                            self.select(position, Some(id));
                        }
                    }
                    self.force = false;
                }
                Ok(Reply::Inspection(v)) => {
                    if v.button["id"].as_str() == self.control.selected() {
                        self.draft = Draft::load(&v);
                        self.inspection = Some(v);
                    }
                }
                Ok(Reply::Response {
                    operation,
                    response: r,
                }) if operation == "launcher_catalog" => {
                    if let Err(e) = self.catalog.accept(&r.data) {
                        self.warn(crate::status::describe_error(&e.code, self.es));
                    }
                }
                Ok(Reply::Response {
                    operation,
                    response: r,
                }) if operation == "result" => {
                    self.say_run(&r.data["execution_id"], &string(&r.data["state"]));
                    if r.data["state"] != "running" {
                        self.control.terminal();
                    }
                }
                Ok(Reply::NoAppIcon) => {
                    if std::mem::take(&mut self.icon_asked) {
                        self.say(tr(
                            self.es,
                            "This application has no PNG or JPEG icon to use",
                            "Esta aplicación no trae un icono PNG o JPEG que usar",
                        ));
                    }
                }
                Ok(Reply::Response {
                    operation,
                    response: r,
                }) => {
                    self.icon_asked = false;
                    if operation == "plugin_admin" {
                        self.plugins.admin_pending = false;
                        self.plugins.due = 0;
                        self.plugins.status["state"] = json!("preparing");
                        self.say(tr(
                            self.es,
                            "Preparing — not yet activated",
                            "Preparando — aún no activado",
                        ));
                        return;
                    }
                    // The worker decides against what the runtime now holds: the key takes
                    // the icon only if that application is its top action and it has no image.
                    if operation == "add_action"
                        && let Some((button, application)) = self.icon_candidate.take()
                    {
                        let _ = self.worker.enqueue(Job::AppIcon {
                            button,
                            application: Some(application),
                        });
                    }
                    if r.status == rad_client::response::Status::Admitted
                        && let Some(id) = r.data["execution_id"].as_str()
                    {
                        self.control.admitted(id.into());
                    }
                    match r.status {
                        rad_client::response::Status::Admitted => {
                            self.say_run(&r.data["execution_id"], "running")
                        }
                        rad_client::response::Status::Applied => {
                            self.say(tr(self.es, "Saved", "Guardado"))
                        }
                        _ => self.say(tr(self.es, "Updated", "Actualizado")),
                    };
                    self.force = true;
                    self.inspect_needed = self.selected().is_some();
                }
                Ok(Reply::Image { reference, pixels }) => {
                    if self.board.as_ref().is_some_and(|b| {
                        b.buttons.iter().any(|b| b.appearance["image"] == reference)
                    }) {
                        let image =
                            egui::ColorImage::from_rgba_unmultiplied(pixels.size, &pixels.rgba);
                        self.textures.insert(
                            reference.clone(),
                            ctx.load_texture(reference, image, Default::default()),
                        );
                    }
                }
                Ok(Reply::Theme) => {}
                Ok(Reply::TransferProgress(n)) => {
                    self.transfer_active = true;
                    self.say(format!(
                        "{} {n}",
                        tr(self.es, "Transferred bytes:", "Bytes transferidos:")
                    ));
                }
                Ok(Reply::TransferFinished { kind, data }) => {
                    self.transfer_active = false;
                    match kind {
                        TransferKind::Image => {
                            self.draft.image = string(&data["media_ref"]);
                            self.image_path.clear();
                            self.say(tr(
                                self.es,
                                "Image prepared: save within 60 seconds",
                                "Imagen preparada: guardar antes de 60 segundos",
                            ));
                        }
                        TransferKind::Restore => {
                            self.restore_token = data["transfer_id"].as_str().map(str::to_owned);
                            self.restore_summary = data["summary"].to_string();
                        }
                        TransferKind::Export => {
                            self.say(tr(self.es, "Export finished", "Exportación terminada"))
                        }
                    }
                }
            }
        }
        if !self.worker.busy() {
            let now = self.now_ms();
            let visible = ctx.input(|i| {
                i.viewport().focused.unwrap_or(true) && !i.viewport().minimized.unwrap_or(false)
            });
            if self.connected && (self.force || self.poll.due(now, visible, false)) {
                let _ = self.worker.submit(Job::Refresh(self.force));
            } else if self.connected
                && self.control.result_request().is_some()
                && self.result_poll.due(now, visible, false)
            {
                let _ = self
                    .worker
                    .submit(Job::Request(self.control.result_request().unwrap()));
            } else if self.connected && self.transfer_active {
                let _ = self.worker.submit(Job::TransferStep);
            } else if self.connected && self.inspect_needed {
                if let Some(id) = self.selected() {
                    let _ = self.worker.submit(Job::Inspect(id));
                }
                self.inspect_needed = false;
            } else if self.connected && self.catalog_due.is_some_and(|due| now >= due) {
                let _ = self
                    .worker
                    .submit(Job::Request(self.catalog.request(false)));
                self.catalog_due = None;
            } else if self.connected && self.poll_plugins(now, visible) {
                // Plugin work was submitted by its bounded scheduler.
            } else if self.connected {
                let next = self.board.as_ref().and_then(|b| {
                    b.buttons.iter().find_map(|button| {
                        let reference = button.appearance["image"].as_str()?;
                        if self.textures.contains_key(reference)
                            || self.image_attempts.contains(reference)
                        {
                            return None;
                        }
                        Some((
                            string(&button.entity["id"]),
                            reference.to_owned(),
                            string(&button.entity["view_revision"]),
                        ))
                    })
                });
                if let Some((id, reference, revision)) = next {
                    self.image_attempts.insert(reference.clone());
                    let _ = self.worker.submit(Job::Image {
                        id,
                        reference,
                        revision,
                    });
                }
            } else if !self.connected && self.poll.due(now, visible, false) {
                let _ = self.worker.submit(Job::Theme);
            }
        }
        if self.styled.as_ref() != Some(&self.palette) {
            Look::new(&self.palette).apply(ctx);
            self.styled = Some(self.palette.clone());
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    /// Ctrl+Z / Ctrl+Shift+Z (or Ctrl+Y) reach the runtime unless a text field wants them.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if !self.connected || self.credentials.editing() || ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::{Key, Modifiers};
        let redo = ctx.input_mut(|i| {
            i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::COMMAND, Key::Y)
        });
        if redo {
            self.send(Request::new("redo", json!({})));
        } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
            self.send(Request::new("undo", json!({})));
        }
    }
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump(&ctx);
        self.credentials.pump(&mut self.credential_worker);
        if self.credential_selection != self.selected() {
            self.credentials.clear();
            self.plugins.form = None;
            self.credential_selection = self.selected();
        }
        self.plugins
            .pump_options(&mut self.options_worker, self.connected);
        self.shortcuts(&ctx);
        let look = Look::new(&self.palette);
        // Background polling must not gate the controls: disabling them every poll made
        // the panels blink and took keyboard focus away from their text fields.
        let editable = self.connected && !self.transfer_active;
        egui::Panel::top("bar")
            .frame(look.panel(look.surface, 10))
            .show(ui, |ui| self.bar(ui, &look));
        egui::Panel::bottom("status")
            .frame(
                look.panel(look.surface, 4)
                    .inner_margin(egui::Margin::symmetric(14, 5)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.status_line(ui, &look));
        egui::Panel::right("actions")
            .default_size(260.)
            .frame(look.panel(look.surface, 12))
            .show(ui, |ui| {
                ui.add_enabled_ui(editable, |ui| self.actions(ui, &look));
            });
        // The keys come first: the inspector takes what a comfortable grid leaves over,
        // and in a short tile never more than what still shows all four rows.
        let room = ui.available_height();
        let inspector = (room - deck::COMFORTABLE_HEIGHT)
            .clamp(176., 420.)
            .min(room - deck::MIN_HEIGHT);
        egui::Panel::bottom("inspector")
            .exact_size(inspector.max(120.))
            .resizable(false)
            .frame(look.panel(look.surface, 14))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.add_enabled_ui(editable, |ui| self.inspector(ui, &look));
                    });
            });
        egui::CentralPanel::default()
            .frame(look.panel(look.background, 0))
            .show(ui, |ui| {
                ui.add_enabled_ui(!self.transfer_active, |ui| self.deck(ui, &look));
            });
        self.settings_window(&ctx, &look);
        self.plugin_form_window(&ctx);
        deck::drag_ghost(&ctx, &look);
    }
}
