use crate::{
    Controller, Request,
    catalog::Entry,
    native::{self, Event, Job, OptionsEvent, OptionsJob, Reply},
    session::{Board, Inspection},
    theme::Palette,
    worker::{Pulse, SubmitError, Worker},
};
use eframe::egui;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Instant};

mod actions;
mod ahead;
mod application_icons;
mod bar;
mod clipboard;
mod credentials;
mod deck;
mod icon_library;
mod inspector;
mod launcher;
mod look;
mod machine;
mod options;
mod pacing;
mod pictures;
mod plugins;
mod reads;
mod recovery;
mod replies;
mod service;
mod settings;
mod text_style;
mod updates;

use look::Look;

const SEARCH_DEBOUNCE_MS: u64 = 250;

/// Pause after starting the service so the runtime can open its socket before connecting.
const SERVICE_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);

const DEFAULT_WAIT_MS: u64 = 1000;

const LOADING_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

#[derive(Clone)]
enum NewAction {
    OpenApp,
    Launcher(Entry),
    Wait,
    Sleep,
    Brightness,
    Plugin(crate::plugins::CatalogEntry),
}
#[derive(Clone)]
enum Drag {
    Button { id: String, face: egui::Rect },
    Add { action: NewAction, label: String },
    Folder,
    Reorder(usize),
}
#[derive(Clone, Default)]
struct Draft {
    source: Option<Inspection>,
    visual_type: Option<String>,
    state: Option<String>,
    state_initialized: bool,
    states: std::collections::BTreeMap<String, rad_api::StateFace>,
    base_face: rad_api::StateFace,
    published_states: std::collections::BTreeMap<String, rad_api::StateFace>,
    id: String,
    name: String,
    text: String,
    image: String,
    /// Published reference when this draft was read; never a local prepared image.
    published_image: String,
    style: rad_api::TextStyle,
    mode: String,
    gesture: String,
    order: Vec<usize>,
    waits: Vec<String>,
    entries: Vec<String>,
    brightness_directions: Vec<rad_api::BrightnessDirection>,
    permissions: Value,
    action_permissions: Vec<Value>,
    action_ids: Vec<String>,
}
impl Draft {
    fn initial_state(&mut self, observed: Option<&str>, resting: &str, protect: bool) {
        let published = self.state.as_ref().and_then(|id| self.states.get(id));
        let edited = self.text != published.map(|face| face.text.as_str()).unwrap_or_default()
            || self.image
                != published
                    .and_then(|face| face.image.as_deref())
                    .unwrap_or_default()
            || self.style != published.map(text_style::from_face).unwrap_or_default();
        if self.state.is_some() && (protect || edited) {
            self.state_initialized = true;
        }
        if self.state.is_none() || (!self.state_initialized && observed.is_some()) {
            self.select_state(observed.unwrap_or(resting).to_owned());
        }
        self.state_initialized |= observed.is_some();
    }
    fn state_faces(&self) -> std::collections::BTreeMap<String, rad_api::StateFace> {
        let mut states = self.states.clone();
        if let Some(id) = &self.state {
            let face = text_style::face(
                self.text.clone(),
                (!self.image.is_empty()).then(|| self.image.clone()),
                self.style,
            );
            if face == rad_api::StateFace::default() && states.get(id) != Some(&face) {
                states.remove(id);
            } else {
                states.insert(id.clone(), face);
            }
        }
        states
    }
    fn select_state(&mut self, id: String) {
        self.states = self.state_faces();
        let face = self.states.get(&id).cloned().unwrap_or_default();
        self.style = text_style::from_face(&face);
        self.text = face.text;
        self.image = face.image.unwrap_or_default();
        self.published_image = self
            .published_states
            .get(&id)
            .and_then(|face| face.image.clone())
            .unwrap_or_default();
        self.state = Some(id);
    }
    fn retain_appearance_edits(&mut self, before: &Self, edited: &Self) {
        let published_image = self.published_image.clone();
        if let Some(state) = &edited.state {
            let local = edited.state_faces();
            for id in before.states.keys().chain(local.keys()) {
                let previous = before.states.get(id).cloned().unwrap_or_default();
                let edited = local.get(id).cloned().unwrap_or_default();
                if previous != edited {
                    let face = self.states.entry(id.clone()).or_default();
                    if previous.text != edited.text {
                        face.text = edited.text.clone();
                    }
                    let mut style = text_style::from_face(face);
                    text_style::retain(
                        &mut style,
                        text_style::from_face(&previous),
                        text_style::from_face(&edited),
                    );
                    *face = text_style::face(face.text.clone(), face.image.clone(), style);
                    if previous.image != edited.image {
                        face.image = edited.image;
                    }
                    if *face == rad_api::StateFace::default() && !local.contains_key(id) {
                        self.states.remove(id);
                    }
                }
            }
            self.state = None;
            self.select_state(state.clone());
            self.state_initialized = edited.state_initialized;
        } else {
            text_style::retain(&mut self.style, before.style, edited.style);
            if edited.text != before.text {
                self.text = edited.text.clone();
            }
            if edited.image != before.image {
                self.image = edited.image.clone();
            }
        }
        self.published_image = published_image;
        fn retain<T: Clone + PartialEq>(fresh: &mut T, before: &T, edited: &T) {
            if edited != before {
                *fresh = edited.clone();
            }
        }
        retain(&mut self.name, &before.name, &edited.name);
        retain(&mut self.mode, &before.mode, &edited.mode);
        retain(&mut self.gesture, &before.gesture, &edited.gesture);
        if self.action_ids == before.action_ids {
            retain(&mut self.order, &before.order, &edited.order);
        }
        for (index, id) in self.action_ids.iter().enumerate() {
            let Some(previous) = before.action_ids.iter().position(|old| old == id) else {
                continue;
            };
            let Some(local) = edited.action_ids.iter().position(|old| old == id) else {
                continue;
            };
            let typed = edited.waits[local].trim().parse::<u64>().ok();
            if typed.is_none() || typed != self.waits[index].parse::<u64>().ok() {
                retain(
                    &mut self.waits[index],
                    &before.waits[previous],
                    &edited.waits[local],
                );
            }
            retain(
                &mut self.entries[index],
                &before.entries[previous],
                &edited.entries[local],
            );
            retain(
                &mut self.brightness_directions[index],
                &before.brightness_directions[previous],
                &edited.brightness_directions[local],
            );
        }
    }
    fn load(v: &Inspection) -> Self {
        Self {
            source: Some(v.clone()),
            visual_type: v.appearance["visual_type"].as_str().map(str::to_owned),
            state: None,
            state_initialized: false,
            states: text_style::states(&v.appearance["states"]),
            published_states: text_style::states(&v.appearance["states"]),
            base_face: text_style::face_from_view(&v.appearance),
            action_ids: v.actions.iter().map(|a| string(&a["id"])).collect(),
            id: string(&v.button["id"]),
            name: string(&v.button["fields"]["name"]),
            text: string(&v.appearance["text"]),
            image: string(&v.appearance["image"]),
            published_image: string(&v.appearance["image"]),
            style: text_style::from_view(&v.appearance),
            mode: string(&v.button["fields"]["mode"]),
            gesture: v.button["fields"]["gesture"]
                .as_str()
                .unwrap_or("press")
                .into(),
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
            brightness_directions: v
                .actions
                .iter()
                .map(|a| match a["fields"]["parameters"]["direction"].as_str() {
                    Some("decrease") => rad_api::BrightnessDirection::Decrease,
                    _ => rad_api::BrightnessDirection::Increase,
                })
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
fn visual_type<'a>(board: &'a Board, id: &str) -> Option<&'a Value> {
    board
        .buttons
        .iter()
        .find(|key| key.entity["id"] == id)
        .map(|key| &key.appearance["visual_type"])
        .filter(|visual| visual.is_string())
}
use crate::texts::tr;
pub struct App {
    preferences: crate::preferences::Preferences,
    preferences_path: Option<PathBuf>,
    worker: Worker<Job, Event>,
    options_worker: Worker<OptionsJob, OptionsEvent>,
    credential_worker: credentials::CredentialWorker,
    worker_respawn: crate::worker::Respawner<Job, Event>,
    options_respawn: crate::worker::Respawner<OptionsJob, OptionsEvent>,
    credential_respawn: crate::worker::Respawner<credentials::Job, credentials::Event>,
    credentials: credentials::Panel,
    credential_selection: Option<String>,
    control: Controller,
    schedule: crate::schedule::Schedule,
    frame_pacer: pacing::FramePacer,
    result_due: bool,
    wake_registered: bool,
    search_wake: Option<crate::worker::WakeTimer>,
    runtime: PathBuf,
    notices: Option<crate::notices::Notices>,
    overview_due: bool,
    resolving_unknown: bool,
    reconnect_needed: bool,
    started: Instant,
    connected: bool,
    attempted: bool,
    force: bool,
    inspect_needed: bool,
    board: Option<Board>,
    status: Value,
    inspection: Option<Inspection>,
    draft: Draft,
    save_ticket: u64,
    saves: std::collections::BTreeMap<u64, (u64, Draft)>,
    saved_draft: Option<(u64, Draft)>,
    unknown_saves: Vec<(u64, Draft)>,
    icon_library: icon_library::Library,
    selected_slot: Option<usize>,
    message: String,
    message_is_error: bool,
    message_code: String,
    es: crate::preferences::Language,
    palette: Palette,
    styled: Option<Palette>,
    navigation: crate::screens::Navigation,
    updates: crate::updates::Controller,
    update_outcome_asked: bool,
    update_reopen_ack: bool,
    form_leave: crate::screens::Leave,
    settings: Value,
    new_name: String,
    move_folder: String,
    move_position: usize,
    transfer_active: bool,
    image_target: Option<(String, Option<String>)>,
    appearance_retry: Option<Inspection>,
    prepared_imports: Vec<(String, String)>,
    discarded_imports: Vec<String>,
    image_cleanup_attempt: u8,
    image_cleanup_disconnect: bool,
    image_cleanup_due: Instant,
    image_path: String,
    backup_path: String,
    restore_token: Option<String>,
    restore_summary: String,
    global_permissions: Value,
    catalog: crate::catalog::Catalog,
    catalog_due: Option<u64>,
    catalog_more: bool,
    catalog_read: Option<launcher::CatalogRead>,
    app_picker: launcher::ApplicationPicker,
    /// Action dropped on an empty key: added once the new key is published.
    pending_action: Option<(usize, NewAction)>,
    copied_key: Option<String>,
    pending_paste: Option<clipboard::PendingPaste>,
    /// Key that was just sent an application, and that application's desktop entry: the
    /// key may take its icon once the action is applied.
    icon_candidate: Option<(String, String)>,
    /// The user asked for the application icon, so a missing one deserves a word.
    icon_asked: bool,
    application_icons: application_icons::ApplicationIcons,
    folded: std::collections::BTreeSet<actions::Category>,
    pictures: pictures::Pictures,
    ahead: ahead::Ahead,
    selected_at: Instant,
    selection: u64,
    board_since: Instant,
    plugins: plugins::PluginUi,
    /// Owner of every call to systemd; absent where the editor must not touch the system.
    service_worker: Option<Worker<crate::service::Job, crate::service::Outcome>>,
    service: service::ServiceUi,
    machine: Option<crate::machine::Settings>,
    machine_due: bool,
    machine_pending: Option<machine::Pending>,
    recovery: crate::recovery::Mode,
    recovery_due: bool,
    recovery_reset_confirm: bool,
    previous_exit_seen: Option<(String, String)>,
    runtime_log: Option<rad_api::RuntimeLogView>,
}
impl App {
    pub fn new(runtime: PathBuf, theme: PathBuf) -> Self {
        let mut app =
            Self::with_preferences_path(runtime, theme, crate::preferences::default_path());
        app.service_worker = Some(crate::service::worker(
            crate::service::user_service(),
            SERVICE_SETTLE,
        ));
        app.bootstrap_service();
        app
    }
    fn with_preferences_path(
        runtime: PathBuf,
        theme: PathBuf,
        preferences_path: Option<PathBuf>,
    ) -> Self {
        let loaded = preferences_path
            .as_deref()
            .map(crate::preferences::Preferences::load)
            .transpose();
        let preference_error = loaded.as_ref().err().map(ToString::to_string);
        let preferences = loaded.ok().flatten().unwrap_or_default();
        let es = preferences.language(&crate::preferences::system_locale());
        let options_respawn = {
            let runtime = runtime.clone();
            crate::worker::Respawner::new(move || native::start_options(runtime.clone()))
        };
        let credential_respawn = {
            let runtime = runtime.clone();
            crate::worker::Respawner::new(move || credentials::start(runtime.clone()))
        };
        let worker_respawn = {
            let runtime = runtime.clone();
            let theme = theme.clone();
            crate::worker::Respawner::new(move || native::start(runtime.clone(), theme.clone()))
        };
        let options_worker = native::start_options(runtime.clone());
        let credential_worker = credentials::start(runtime.clone());
        let mut worker = native::start(runtime.clone(), theme.clone());
        let _ = worker.submit(Job::Connect);
        let mut app = Self {
            preferences,
            preferences_path,
            worker,
            options_worker,
            credential_worker,
            worker_respawn,
            options_respawn,
            credential_respawn,
            credentials: Default::default(),
            credential_selection: None,
            control: Controller::default(),
            schedule: Default::default(),
            frame_pacer: Default::default(),
            result_due: false,
            wake_registered: false,
            search_wake: None,
            runtime,
            notices: None,
            overview_due: false,
            resolving_unknown: false,
            reconnect_needed: false,
            started: Instant::now(),
            connected: false,
            attempted: false,
            force: true,
            inspect_needed: false,
            board: None,
            status: Value::Null,
            inspection: None,
            draft: Draft::default(),
            save_ticket: 0,
            saves: Default::default(),
            saved_draft: None,
            unknown_saves: Vec::new(),
            icon_library: Default::default(),
            selected_slot: None,
            message: String::new(),
            message_is_error: false,
            message_code: String::new(),
            es,
            palette: Palette::default(),
            styled: None,
            navigation: Default::default(),
            updates: Default::default(),
            update_outcome_asked: false,
            update_reopen_ack: false,
            form_leave: Default::default(),
            settings: Value::Null,
            new_name: String::new(),
            move_folder: String::new(),
            move_position: 0,
            transfer_active: false,
            image_target: None,
            appearance_retry: None,
            prepared_imports: vec![],
            discarded_imports: vec![],
            image_cleanup_attempt: 0,
            image_cleanup_disconnect: false,
            image_cleanup_due: Instant::now(),
            image_path: String::new(),
            backup_path: String::new(),
            restore_token: None,
            restore_summary: String::new(),
            global_permissions: Value::Null,
            catalog: Default::default(),
            catalog_due: None,
            catalog_more: false,
            catalog_read: None,
            app_picker: Default::default(),
            pending_action: None,
            copied_key: None,
            pending_paste: None,
            icon_candidate: None,
            icon_asked: false,
            application_icons: Default::default(),
            folded: [actions::Category::Applications].into(),
            pictures: Default::default(),
            ahead: Default::default(),
            selected_at: Instant::now(),
            selection: 0,
            board_since: Instant::now(),
            plugins: Default::default(),
            service_worker: None,
            service: Default::default(),
            machine: None,
            machine_due: false,
            machine_pending: None,
            recovery: Default::default(),
            recovery_due: false,
            recovery_reset_confirm: false,
            previous_exit_seen: None,
            runtime_log: None,
        };
        if let Some(error) = preference_error {
            app.warn(tr(
                es,
                "Could not read preferences — select your language again",
            ));
            app.message_code = error;
        }
        app
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
    fn say_run(&mut self, execution: &Value, state: &str) {
        let text = match state {
            "running" => tr(self.es, "Running…"),
            "succeeded" => tr(self.es, "Done"),
            "failed" => tr(self.es, "Failed"),
            _ => tr(self.es, "Something went wrong"),
        }
        .to_owned();
        if state == "failed" {
            self.warn(text);
        } else {
            self.say(text);
        }
        self.message_code = string(execution);
    }
    fn command(&mut self, job: Job) -> bool {
        let refused = self.worker.enqueue(job).err();
        match refused {
            None => {}
            Some(SubmitError::Busy) => self.warn(tr(self.es, "Busy — not sent")),
            Some(SubmitError::Disconnected) => self.warn(tr(self.es, "Not applied — try again")),
        }
        refused.is_none()
    }
    fn reconnect(&mut self, ctx: &egui::Context) {
        if self.worker_respawn.gave_up() {
            let wake = ctx.clone();
            self.worker_respawn
                .revive(&mut self.worker, move || wake.request_repaint());
        }
        if self.credential_respawn.gave_up() {
            let wake = ctx.clone();
            self.credential_respawn
                .revive(&mut self.credential_worker, move || wake.request_repaint());
        }
        if self.options_respawn.gave_up() {
            let wake = ctx.clone();
            self.options_respawn
                .revive(&mut self.options_worker, move || wake.request_repaint());
        }
        self.command(Job::Connect);
    }
    fn worker_lost(&mut self, ctx: &egui::Context, in_flight: bool, unsent: bool) {
        self.unknown_saves
            .extend(std::mem::take(&mut self.saves).into_values());
        self.machine_request_failed();
        ctx.request_repaint();
        self.connected = false;
        self.icon_library.reset();
        self.attempted = true;
        self.transfer_active = false;
        self.pending_action = None;
        self.pending_paste = None;
        self.catalog_read = None;
        self.app_picker = Default::default();
        self.icon_candidate = None;
        self.icon_asked = false;
        self.plugins.abandon_pending();
        self.plugins.visuals.clear();
        self.plugins.status = Value::Null;
        self.plugins.dependency_credentials = None;
        self.credentials.clear();
        if in_flight {
            self.resolving_unknown = true;
            self.force = true;
            self.overview_due = true;
            self.inspect_needed = self.selected().is_some();
        }
        if unsent {
            self.warn(tr(self.es, "Not applied — try again"));
        } else if in_flight {
            self.say(tr(self.es, "Checking what changed…"));
        }
        let wake = ctx.clone();
        self.reconnect_needed = self
            .worker_respawn
            .replace(&mut self.worker, move || wake.request_repaint());
    }
    fn send(&mut self, request: Request) {
        if matches!(request, Request::Navigate { .. }) {
            self.pending_paste = None;
        }
        self.command(Job::Request(request));
    }
    fn selected(&self) -> Option<String> {
        self.control.selected().map(str::to_owned)
    }
    /// Runs the selected key now, through the same admission as a physical press.
    fn try_selected(&mut self) {
        if self.inspection.as_ref().is_some_and(|inspection| {
            inspection.button["id"].as_str() == self.control.selected()
                && inspection.actions.iter().any(|action| {
                    matches!(
                        action["fields"]["type"].as_str(),
                        Some("enter_folder" | "return")
                    )
                })
        }) {
            self.pending_paste = None;
        }
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
        self.abandon_appearance();
        self.app_picker.cancel();
        self.credentials.clear();
        self.plugins.dependency_credentials = None;
        self.plugins.clear_form();
        self.selected_slot = Some(position);
        self.control.select(id);
        self.selected_at = Instant::now();
        self.selection += 1;
        let ready = self
            .control
            .selected()
            .zip(self.board.as_ref())
            .and_then(|(id, board)| self.ahead.inspection(board, id))
            .cloned();
        self.draft = ready.as_ref().map(Draft::load).unwrap_or_default();
        self.inspection = ready;
        self.inspect_needed = self.control.selected().is_some() && self.inspection.is_none();
    }
    /// Deletes the selected key, or the folder it enters, with no question: undo, a
    /// keystroke away, is the safety net. Its inspector must be read to tell which.
    fn delete_selected(&mut self) {
        let Some(inspection) = self
            .inspection
            .as_ref()
            .filter(|inspection| inspection.button["id"].as_str() == self.control.selected())
        else {
            return;
        };
        let request = crate::commands::delete_selection(
            &string(&inspection.button["id"]),
            &inspection.actions,
        );
        self.send(request);
    }
    /// Keeps the selection on what the runtime just published: it follows a moved key,
    /// falls back to the slot when the key is gone, and adopts a key created in the slot.
    fn follow_selection(&mut self) {
        match self.selected() {
            Some(id) => {
                let position = (0..self.board.as_ref().map_or(0, |b| b.grid.key_count()))
                    .find(|&p| self.slot_id(p).as_deref() == Some(id.as_str()));
                if position.is_some() {
                    self.selected_slot = position;
                } else {
                    self.app_picker.cancel();
                    self.plugins.clear_form();
                    self.abandon_appearance();
                    self.control.select(None);
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
    fn creation_name(&self, folder: bool) -> String {
        let typed = self.new_name.trim();
        if !typed.is_empty() {
            return typed.to_owned();
        }
        if folder {
            tr(self.es, "New folder").into()
        } else {
            tr(self.es, "New button").into()
        }
    }
    fn create(&mut self, position: usize, folder: bool) {
        let Some(parent) = self.folder() else {
            return;
        };
        let name = self.creation_name(folder);
        self.send(if folder {
            Request::CreateFolder {
                parent_id: parent,
                position,
                name,
            }
        } else {
            Request::CreateButton {
                folder_id: parent,
                position,
                name,
            }
        });
        self.new_name.clear();
    }
    fn add_action(&mut self, button: &str, action: &NewAction) {
        match action {
            NewAction::Launcher(entry) => {
                self.add_application(button, entry);
            }
            NewAction::OpenApp => {
                if let Some(position) = (0..self.board.as_ref().map_or(0, |b| b.grid.key_count()))
                    .find(|&p| self.slot_id(p).as_deref() == Some(button))
                {
                    self.begin_application_choice(position);
                }
            }
            NewAction::Plugin(entry) => self.open_plugin_form(
                &entry.type_key,
                None,
                if entry.visual {
                    crate::plugins::FormTarget::Visual(button.into())
                } else {
                    crate::plugins::FormTarget::AddAction(button.into())
                },
            ),
            NewAction::Wait => self.send(crate::commands::add_action(
                button,
                "wait",
                json!({"milliseconds":DEFAULT_WAIT_MS}),
            )),
            NewAction::Sleep => {
                self.send(crate::commands::add_action(button, "sleep", Value::Null))
            }
            NewAction::Brightness => self.send(crate::commands::add_action(
                button,
                "brightness",
                json!({"direction":"increase"}),
            )),
        }
    }
    fn action_name(&self, action: &NewAction) -> String {
        match action {
            NewAction::OpenApp => tr(self.es, "Open App").into(),
            NewAction::Launcher(entry) => entry.name.clone(),
            NewAction::Wait => tr(self.es, "Wait").into(),
            NewAction::Sleep => tr(self.es, "Sleep").into(),
            NewAction::Brightness => tr(self.es, "Brightness").into(),
            NewAction::Plugin(entry) => entry.shown(self.es).to_owned(),
        }
    }
    fn place_action(&mut self, position: usize, action: &NewAction) {
        if matches!(action, NewAction::OpenApp) {
            self.begin_application_choice(position);
        } else if let Some(id) = self.slot_id(position) {
            self.add_action(&id, action);
        } else if let Some(folder) = self.folder() {
            self.create_action_button(folder, position, action);
        }
    }
    fn create_action_button(
        &mut self,
        folder: String,
        position: usize,
        action: &NewAction,
    ) -> bool {
        let sent = self.command(Job::Request(Request::CreateButton {
            folder_id: folder,
            position,
            name: self.action_name(action),
        }));
        if sent {
            self.pending_action = Some((position, action.clone()));
        }
        sent
    }
    fn pump(&mut self, ctx: &egui::Context) {
        self.pump_notices();
        if self.pump_replies(ctx).is_break() {
            return;
        }
        if matches!(
            self.updates.installation(),
            crate::updates::Installation::Ready
                | crate::updates::Installation::HandingOff
                | crate::updates::Installation::Relayed
        ) {
            return;
        }
        if self.reconnect_needed && !self.worker.busy() {
            self.reconnect_needed = false;
            let _ = self.worker.submit(Job::Connect);
        }
        self.pump_service(ctx);
        self.pump_reads();
        if self.connected && (self.image_cleanup_disconnect || !self.discarded_imports.is_empty()) {
            let delay = self
                .image_cleanup_due
                .saturating_duration_since(Instant::now());
            if !delay.is_zero() || !self.worker.busy() {
                ctx.request_repaint_after(delay);
            }
        }
        if self.styled.as_ref() != Some(&self.palette) {
            Look::new(&self.palette).apply(ctx);
            self.styled = Some(self.palette.clone());
        }
    }
    fn pump_notices(&mut self) {
        if let Some(notices) = &self.notices {
            let (connection, changed) = notices.take();
            if connection == Some(true) {
                self.reconnect_needed = true;
                self.connected = false;
            }
            if connection == Some(false) {
                self.connected = false;
            }
            for kind in changed {
                if !self.recovery.is_normal() {
                    break;
                }
                match kind {
                    rad_api::NoticeKind::Board => {
                        self.force = true;
                        self.plugins.visual_due = true;
                    }
                    rad_api::NoticeKind::Inspector => {
                        self.inspect_needed = self.selected().is_some();
                        self.ahead.forget_inspections();
                    }
                    rad_api::NoticeKind::Overview => self.overview_due = true,
                    rad_api::NoticeKind::Execution => self.result_due = true,
                    rad_api::NoticeKind::Plugins => {
                        self.plugins.dependency_status_due = true;
                        self.plugins.resources.clear();
                        self.plugins.manifest_invalidated |=
                            self.plugins.manifest_pending.is_some();
                    }
                    rad_api::NoticeKind::Catalog => self.plugins.catalog_loaded = false,
                    rad_api::NoticeKind::Dependencies => self.plugins.dependencies.reset(),
                }
            }
        }
    }
    fn reads_due(&self) -> bool {
        self.force
            || self.overview_due
            || self.inspect_needed
            || self.transfer_active
            || self.plugins.has_work()
            || self.catalog_due.is_some_and(|due| self.now_ms() >= due)
            || self.app_picker.is_due(self.now_ms())
            || (self.result_due && self.control.result_request().is_some())
    }
    fn pump_auxiliary(&mut self, ctx: &egui::Context) {
        if !self.connected {
            self.icon_library.reset();
        }
        if self.icon_library.open
            && (self.key_form_open() || self.navigation.screen() != crate::screens::Screen::Keys)
        {
            self.icon_library.cancel();
        }
        self.icon_library.pump(ctx);
        match self.credentials.pump(&mut self.credential_worker) {
            Pulse::Answered => self.credential_respawn.answered(),
            Pulse::Lost => {
                let wake = ctx.clone();
                self.credential_respawn
                    .replace(&mut self.credential_worker, move || wake.request_repaint());
            }
            Pulse::Quiet => {}
        }
        match self
            .plugins
            .pump_options(&mut self.options_worker, self.connected)
        {
            Pulse::Answered => self.options_respawn.answered(),
            Pulse::Lost => {
                let wake = ctx.clone();
                self.options_respawn
                    .replace(&mut self.options_worker, move || wake.request_repaint());
            }
            Pulse::Quiet => {}
        }
    }
    /// Ctrl+Z / Ctrl+Shift+Z (or Ctrl+Y) reach the runtime unless a text field wants them;
    /// Delete removes the selected key; Copy/Paste duplicate it into the selected place.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if !self.connected
            || !self.recovery.is_normal()
            || self.credentials.editing()
            || ctx.egui_wants_keyboard_input()
        {
            return;
        }
        use egui::{Key, Modifiers};
        let redo = ctx.input_mut(|i| {
            i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::COMMAND, Key::Y)
        });
        if redo {
            self.send(Request::Redo {});
        } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
            self.send(Request::Undo {});
        } else if self.navigation.screen() == crate::screens::Screen::Keys
            && !self.key_form_open()
            && !egui::Popup::is_any_open(ctx)
        {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete)) {
                self.delete_selected();
            } else {
                self.clipboard_shortcuts(ctx);
            }
        }
    }
    fn keys_screen(&mut self, ui: &mut egui::Ui, look: &Look, editable: bool) {
        egui::Panel::top("bar")
            .frame(look.panel(look.surface, 10))
            .show(ui, |ui| self.bar(ui, look));
        egui::Panel::bottom("status")
            .frame(
                look.panel(look.surface, 4)
                    .inner_margin(egui::Margin::symmetric(14, 5)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.status_line(ui, look));
        // The cap keeps content that outgrows the column from ever covering the editor.
        egui::Panel::right("actions")
            .default_size(260.)
            .max_size(420.)
            .frame(look.panel(look.surface, 12))
            .show(ui, |ui| {
                ui.add_enabled_ui(editable, |ui| {
                    if self.icon_library.open && !self.key_form_open() {
                        self.icon_library_panel(ui, look, editable);
                    } else {
                        self.actions(ui, look);
                    }
                });
            });
        // The keys come first: the inspector takes what a comfortable grid leaves over,
        // and in a short tile never more than what still shows all four rows.
        let room = ui.available_height();
        let comfortable = self
            .board
            .as_ref()
            .map_or(0., |b| deck::comfortable_height(b.grid));
        let minimum = self.board.as_ref().map_or(0., |b| deck::min_height(b.grid));
        let inspector = (room - comfortable).clamp(176., 420.).min(room - minimum);
        egui::Panel::bottom("inspector")
            .exact_size(inspector.max(120.))
            .resizable(false)
            .frame(look.panel(look.surface, 14))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.add_enabled_ui(editable && !self.icon_library.pending(), |ui| {
                            self.inspector(ui, look)
                        });
                    });
            });
        egui::CentralPanel::default()
            .frame(look.panel(look.background, 0))
            .show(ui, |ui| {
                ui.add_enabled_ui(!self.transfer_active, |ui| self.deck(ui, look));
            });
        self.drag_ghost(ui.ctx(), look);
    }
    /// Esc goes back, as the Back button does, once no text field or menu holds it.
    fn escape(&mut self, ctx: &egui::Context) {
        if self.icon_library.open
            && !egui::Popup::is_any_open(ctx)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.icon_library.cancel();
            return;
        }
        let keys = self.navigation.screen() == crate::screens::Screen::Keys;
        if (keys && !self.key_form_open())
            || ctx.egui_wants_keyboard_input()
            || egui::Popup::is_any_open(ctx)
            || !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            return;
        }
        if keys {
            self.leave_key_form();
        } else {
            self.leave_settings();
        }
    }
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.application_icons
            .begin(ui.ctx(), self.started.elapsed());
        let wait = self.frame_pacer.wait(Instant::now());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        let ctx = ui.ctx().clone();
        if !self.wake_registered {
            let wake = ctx.clone();
            self.worker.set_waker(move || wake.request_repaint());
            let wake = ctx.clone();
            self.options_worker
                .set_waker(move || wake.request_repaint());
            let wake = ctx.clone();
            self.credential_worker
                .set_waker(move || wake.request_repaint());
            if let Some(service) = &self.service_worker {
                let wake = ctx.clone();
                service.set_waker(move || wake.request_repaint());
            }
            let wake = ctx.clone();
            self.notices = Some(crate::notices::Notices::start(
                self.runtime.clone(),
                move || wake.request_repaint(),
            ));
            let wake = ctx.clone();
            self.search_wake = Some(crate::worker::WakeTimer::new(move || {
                wake.request_repaint()
            }));
            let wake = ctx.clone();
            self.updates.set_waker(move || wake.request_repaint());
            self.wake_registered = true;
        }
        self.updates.poll();
        if self.updates.installation() == &crate::updates::Installation::Ready
            && !self.update_drafts()
            && !self.update_pending()
        {
            self.updates.handoff();
        }
        if self.updates.installation() == &crate::updates::Installation::Relayed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if !self.updates.relaying() {
            self.pump(&ctx);
            self.pump_auxiliary(&ctx);
        }
        if self.credential_selection != self.selected() {
            self.credentials.clear();
            self.credential_selection = self.selected();
        }
        if !matches!(
            self.updates.installation(),
            crate::updates::Installation::Preparing
                | crate::updates::Installation::Ready
                | crate::updates::Installation::HandingOff
                | crate::updates::Installation::Relayed
        ) {
            self.shortcuts(&ctx);
        }
        let look = Look::new(&self.palette);
        let editable = self.connected && !self.transfer_active;
        if !self.recovery.is_normal()
            && self.navigation.screen()
                != crate::screens::Screen::Settings(crate::screens::Section::Updates)
        {
            self.recovery_view(ui, &look);
            return;
        }
        self.escape(&ctx);
        if self.navigation.screen() == crate::screens::Screen::Keys {
            self.keys_screen(ui, &look, editable);
        } else {
            self.settings_screen(ui, &look);
        }
        if self.updates.installation() == &crate::updates::Installation::Ready {
            ctx.request_repaint();
        }
        if self.update_reopen_ack
            && self.navigation.screen()
                == crate::screens::Screen::Settings(crate::screens::Section::Updates)
        {
            self.update_reopen_ack = false;
            if let Err(error) = rad_update::coordinator::acknowledge_reopen() {
                eprintln!("RAD update reopen acknowledgement: {error}");
            }
        }
        if self.plugins.form.is_none() && self.plugins.dependency_credentials.is_none() {
            self.credentials.clear();
        }
        if self.connected && !self.worker.busy() && (self.machine_due || self.reads_due()) {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod capture;
#[cfg(test)]
mod clipboard_tests;
#[cfg(test)]
mod image_cleanup_tests;
#[cfg(test)]
mod reload_tests;
#[cfg(test)]
mod worker_loss_tests;

#[cfg(test)]
mod state_draft_tests {
    use super::Draft;
    #[test]
    fn explicit_empty_face_is_not_human_edit_before_first_observation() {
        let mut draft = Draft {
            states: [("off".into(), rad_api::StateFace::default())].into(),
            ..Default::default()
        };
        draft.initial_state(None, "off", false);
        assert_eq!(
            draft.state_faces(),
            draft.states,
            "an explicit empty face is unchanged"
        );
        draft.initial_state(Some("on"), "off", false);
        assert_eq!(draft.state.as_deref(), Some("on"));
        assert_eq!(
            draft.states.get("off"),
            Some(&rad_api::StateFace::default())
        );
    }
    #[test]
    fn first_observation_selects_real_tab_without_overriding_choice_or_draft() {
        let mut draft = Draft::default();
        draft.initial_state(None, "off", false);
        assert_eq!(draft.state.as_deref(), Some("off"));
        draft.initial_state(Some("on"), "off", false);
        assert_eq!(draft.state.as_deref(), Some("on"));
        draft.initial_state(Some("off"), "off", false);
        assert_eq!(draft.state.as_deref(), Some("on"));
        let mut chosen = Draft::default();
        chosen.initial_state(None, "off", false);
        chosen.state_initialized = true;
        chosen.initial_state(Some("on"), "off", false);
        assert_eq!(chosen.state.as_deref(), Some("off"));
        let mut edited = Draft::default();
        edited.initial_state(None, "off", false);
        edited.text = "My draft".into();
        edited.initial_state(Some("on"), "off", false);
        assert_eq!(edited.state.as_deref(), Some("off"));
        assert_eq!(edited.text, "My draft");
        let mut library = Draft::default();
        library.initial_state(None, "off", false);
        library.initial_state(Some("on"), "off", true);
        assert_eq!(library.state.as_deref(), Some("off"));
    }
    #[test]
    fn tabs_preserve_both_faces_with_independent_style_without_replacing_the_base() {
        let mut draft = Draft {
            base_face: rad_api::StateFace {
                text: "Base".into(),
                image: Some("base".into()),
                ..Default::default()
            },
            style: rad_api::TextStyle {
                position: Some(rad_api::TextPosition::Top),
                ..Default::default()
            },
            ..Default::default()
        };
        draft.select_state("muted".into());
        draft.text = "Quiet".into();
        draft.image = "first".into();
        draft.select_state("unmuted".into());
        assert!(draft.text.is_empty());
        draft.text = "Live".into();
        draft.image = "second".into();
        draft.select_state("muted".into());
        assert_eq!((&*draft.text, &*draft.image), ("Quiet", "first"));
        let faces = draft.state_faces();
        assert_eq!(faces.len(), 2);
        assert_eq!(faces["unmuted"].image.as_deref(), Some("second"));
        assert_eq!(draft.style.position, None, "new state starts automatic");
        assert_eq!(draft.base_face.image.as_deref(), Some("base"));
        draft.text.clear();
        draft.image.clear();
        assert!(!draft.state_faces().contains_key("muted"));
    }
    #[test]
    fn style_edits_protect_the_tab_and_rebase_each_field_independently() {
        let before = crate::session::Inspection {
            button: serde_json::json!({"id":"b1","fields":{}}),
            actions: vec![],
            appearance: serde_json::json!({"text":"Base","image":null,"text_position":"middle","text_position_automatic":true,"states":{
                "off":{"text":"Quiet","image":null,"text_position":"bottom","text_position_automatic":true,"text_size":13,"text_color":"#FFFFFF","text_band":true},
                "on":{"text":"Live","image":null,"text_position":"top","text_position_automatic":false,"text_size":20,"text_color":"#FF8040","text_band":false}
            }}),
        };
        let mut edited = Draft::load(&before);
        edited.initial_state(None, "off", false);
        edited.style.size = 8;
        edited.style.band = false;
        edited.initial_state(Some("on"), "off", false);
        assert_eq!(edited.state.as_deref(), Some("off"));
        let mut changed = before.clone();
        changed.appearance["states"]["off"]["text_color"] = serde_json::json!("#123456");
        changed.appearance["states"]["on"]["text_size"] = serde_json::json!(24);
        let mut fresh = Draft::load(&changed);
        fresh.retain_appearance_edits(&Draft::load(&before), &edited);
        assert_eq!(fresh.style.size, 8);
        assert!(!fresh.style.band);
        assert_eq!(fresh.style.color, [0x12, 0x34, 0x56]);
        assert_eq!(fresh.style.position, None);
        fresh.select_state("on".into());
        assert_eq!(fresh.style.size, 24);
        assert_eq!(fresh.style.position, Some(rad_api::TextPosition::Top));
    }
}
