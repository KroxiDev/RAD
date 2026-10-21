//! Editor window: state, worker pump and layout. Views live in the submodules; the
//! runtime stays the only authority, so nothing here edits published state optimistically.
use crate::{
    Controller, Request,
    catalog::Entry,
    native::{self, Event, Job, OptionsEvent, OptionsJob, Reply, TransferKind},
    session::{Board, Inspection},
    theme::Palette,
    worker::{Polled, Pulse, SubmitError, Worker},
};
use eframe::egui;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Instant};

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
    gesture: String,
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
use crate::texts::tr;
pub struct App {
    preferences: crate::preferences::Preferences,
    preferences_path: Option<PathBuf>,
    worker: Worker<Job, Event>,
    /// Questions about plugin option lists, on a connection of their own: a slow plugin
    /// must not hold up Apply or the polling that `worker` does.
    options_worker: Worker<OptionsJob, OptionsEvent>,
    credential_worker: credentials::CredentialWorker,
    /// Replace a worker whose thread died, a bounded number of times in a row.
    worker_respawn: crate::worker::Respawner<Job, Event>,
    options_respawn: crate::worker::Respawner<OptionsJob, OptionsEvent>,
    credential_respawn: crate::worker::Respawner<credentials::Job, credentials::Event>,
    credentials: credentials::Panel,
    credential_selection: Option<String>,
    control: Controller,
    schedule: crate::schedule::Schedule,
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
    es: crate::preferences::Language,
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
    plugins: plugins::PluginUi,
}
impl App {
    pub fn new(runtime: PathBuf, theme: PathBuf) -> Self {
        Self::with_preferences_path(runtime, theme, crate::preferences::default_path())
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
            selected_slot: None,
            message: String::new(),
            message_is_error: false,
            message_code: String::new(),
            es,
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
            plugins: Default::default(),
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
    /// Says how a run of a key is going, in words; its execution id stays in the tooltip.
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
    /// A user command waits behind the in-flight poll; controls never depend on polling.
    fn command(&mut self, job: Job) -> bool {
        let refused = self.worker.enqueue(job).err();
        match refused {
            None => {}
            Some(SubmitError::Busy) => self.warn(tr(self.es, "Busy — not sent")),
            Some(SubmitError::Disconnected) => self.warn(tr(self.es, "Not applied — try again")),
        }
        refused.is_none()
    }
    /// The user asked to reconnect: a worker given up on is replaced first.
    /// The user asked to reconnect: every worker that gave up starts again with a fresh count.
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
    /// The worker's thread ended. A job in flight has an unknown outcome: it is not repeated,
    /// whatever waited for its answer stops waiting, and the state is read again. A command
    /// that never left is said not to have been applied. Replacement is bounded; once the
    /// limit is reached the bar offers Reconnect.
    fn worker_lost(&mut self, ctx: &egui::Context, in_flight: bool, unsent: bool) {
        ctx.request_repaint();
        self.connected = false;
        self.attempted = true;
        self.transfer_active = false;
        self.pending_action = None;
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
        self.plugins.dependency_credentials = None;
        self.plugins.clear_form();
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
                let position = (0..self.board.as_ref().map_or(0, |b| b.grid.key_count()))
                    .find(|&p| self.slot_id(p).as_deref() == Some(id.as_str()));
                if position.is_some() {
                    self.selected_slot = position;
                } else {
                    self.plugins.clear_form();
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
            NewAction::Wait => tr(self.es, "Wait").into(),
            NewAction::Plugin(entry) => entry.label.clone(),
        }
    }
    /// An action lands on a key: added to an existing one, or a key is created for it.
    fn place_action(&mut self, position: usize, action: &NewAction) {
        if let Some(id) = self.slot_id(position) {
            self.add_action(&id, action);
        } else if let Some(folder) = self.folder() {
            self.send(Request::CreateButton {
                folder_id: folder,
                position,
                name: self.action_name(action),
            });
            self.pending_action = Some((position, action.clone()));
        }
    }
    fn pump(&mut self, ctx: &egui::Context) {
        if let Some(notices) = &self.notices {
            let (connection, changed) = notices.take();
            if connection == Some(true) {
                // Even when disconnect/reconnect coalesced before this frame, the old
                // request socket belongs to the old session and must be replaced.
                self.reconnect_needed = true;
                self.connected = false;
            }
            if connection == Some(false) {
                self.connected = false;
            }
            for kind in changed {
                match kind {
                    rad_api::NoticeKind::Board => {
                        self.force = true;
                        self.plugins.visual_due = true;
                    }
                    rad_api::NoticeKind::Inspector => {
                        self.inspect_needed = self.selected().is_some()
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
        let polled = self.worker.poll();
        match &polled {
            Polled::Failed { unsent } => self.worker_lost(ctx, true, *unsent),
            Polled::Closed { unsent } => self.worker_lost(ctx, false, *unsent),
            Polled::Ready(_) | Polled::Pending => {}
        }
        if let Polled::Ready(event) = polled {
            // Some handlers finish this turn early; keep newly invalidated work runnable.
            ctx.request_repaint();
            self.palette = event.palette;
            // One recovery attempt per lost RPC, including a definitely-unsent mutation.
            // A failed Connect starts disconnected, so it cannot trigger a retry loop.
            let connection_lost = self.connected && !event.connected;
            // Connecting says nothing of the worker's health; any other answer does.
            if matches!(&event.result, Ok(reply) if !matches!(reply, Reply::Connected)) {
                self.worker_respawn.answered();
            }
            self.connected = event.connected;
            match event.result {
                Ok(Reply::PluginApplied(r)) => {
                    self.plugins.form_pending = false;
                    if r.status == rad_client::response::Status::Applied {
                        self.plugins.form = None;
                        self.force = true;
                        self.inspect_needed = self.selected().is_some();
                        self.say(tr(self.es, "Saved"));
                    } else {
                        self.warn(tr(self.es, "Not confirmed — draft retained"));
                    }
                }
                Ok(Reply::PluginCatalog(catalog)) => {
                    self.plugins.catalog = catalog;
                    self.plugins.catalog_loaded = true;
                }
                Ok(Reply::Plugins { catalog, status }) => {
                    self.plugins.catalog_loaded = true;
                    self.plugins.catalog = catalog;
                    let selection_changed = self.plugins.status["items"] != status["items"];
                    self.plugins.resources.retain(|id, (version, _)| {
                        status["items"].as_array().is_some_and(|items| {
                            items.iter().any(|item| {
                                item["id"] == *id
                                    && item["selected"]
                                        == serde_json::to_value(*version).expect("version")
                            })
                        })
                    });
                    self.plugins.status = status;
                    if selection_changed {
                        self.plugins.dependencies.reset();
                    }
                }
                Ok(Reply::PluginForm { target, loaded }) => {
                    self.plugins.accept_form(target, loaded);
                }
                Ok(Reply::PluginVisuals { revision, visuals }) => {
                    self.accept_plugin_visuals(ctx, &revision, visuals);
                }
                Err(e) => {
                    if connection_lost {
                        self.reconnect_needed = true;
                        self.force = true;
                        self.overview_due = true;
                        self.inspect_needed = self.selected().is_some();
                    }
                    if event.plugin_dependencies {
                        self.plugins.dependencies.fail(&e);
                        if e.code == "cursor_expired" {
                            return;
                        }
                    }
                    if e.unknown {
                        self.resolving_unknown = true;
                        self.force = true;
                        if !event.connected {
                            self.reconnect_needed = true;
                        }
                        self.plugins.dependencies.reset();
                        self.plugins.dependency_status_due = true;
                    }
                    if event.plugin_manifest {
                        self.plugins.manifest_pending = None;
                        self.plugins.manifest_invalidated = false;
                    }
                    if event.plugin_admin {
                        self.plugins.admin_pending = false;
                    }
                    if event.plugin_apply {
                        self.plugins.form_pending = false;
                        if e.unknown
                            && let Some((crate::plugins::FormTarget::Settings(id), _)) =
                                self.plugins.form.as_ref()
                        {
                            self.plugins.settings_reload = Some(id.clone());
                            self.plugins.form = None;
                        }
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
                        self.plugins.status = Value::Null;
                        self.plugins.dependencies.reset();
                        self.plugins.dependency_credentials = None;
                        self.credentials.clear();
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
                    let reason = crate::status::describe_failure(&e, self.es);
                    if e.unknown {
                        self.say(format!(
                            "{}: {reason}",
                            tr(self.es, "Checking what changed…")
                        ));
                    } else if e.code == "runtime_unavailable" {
                        // The bar already says so, next to the way out.
                        self.say("");
                    } else {
                        self.warn(reason);
                    }
                    self.message_code = match &e.cause {
                        Some(cause) => format!("{}: {cause}", e.code),
                        None => e.code.clone(),
                    };
                    if e.code == "runtime_unavailable" || e.code == "deadline_exceeded" {
                        self.connected = false;
                    }
                }
                Ok(Reply::Connected) => {
                    self.plugins.reconnect();
                    self.credentials.clear();
                    self.attempted = true;
                    self.connected = true;
                    self.force = true;
                    self.overview_due = true;
                    self.catalog_due = Some(0);
                    self.say("");
                }
                Ok(Reply::Refresh(r)) => {
                    if !r.status.is_null() {
                        if !self.settings_open {
                            self.settings = r.status["settings"].clone();
                        }
                        if self.status["view_revision"] != r.status["view_revision"] {
                            self.plugins.dependencies.reset();
                            self.plugins.dependency_status_due = true;
                            self.plugins.catalog_loaded = false;
                        }
                        self.status = r.status;
                    }
                    if self.resolving_unknown {
                        self.resolving_unknown = false;
                        self.say(tr(
                            self.es,
                            "State refreshed — check your change before trying again",
                        ));
                    }
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
                        if self
                            .inspection
                            .as_ref()
                            .is_some_and(|inspection| !crate::session::coherent(&board, inspection))
                        {
                            self.inspection = None;
                            self.inspect_needed = self.selected().is_some();
                        }
                        self.board = Some(board);
                        if changed_folder {
                            self.plugins.clear_form();
                            self.control.select(None);
                            self.selected_slot = None;
                            self.inspection = None;
                        } else {
                            self.follow_selection();
                        }
                        if let Some((position, action)) = self.pending_action.take()
                            && let Some(id) = self.slot_id(position)
                        {
                            self.select(position, Some(id.clone()));
                            self.add_action(&id, &action);
                        }
                    }
                }
                Ok(Reply::Inspection(v)) => {
                    if self
                        .board
                        .as_ref()
                        .is_some_and(|board| !crate::session::coherent(board, &v))
                    {
                        self.force = true;
                        self.inspect_needed = self.selected().is_some();
                        self.inspection = None;
                        return;
                    }
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
                        self.warn(crate::status::describe_failure(&e, self.es));
                    }
                }
                Ok(Reply::Response {
                    operation,
                    response: r,
                }) if operation == "result" => {
                    if let rad_api::ResponseData::Execution(result) = r.data {
                        let state = match result.state {
                            rad_api::ExecutionState::Running => "running",
                            rad_api::ExecutionState::Succeeded => "succeeded",
                            rad_api::ExecutionState::Failed => "failed",
                            rad_api::ExecutionState::TimedOut => "timed_out",
                        };
                        self.say_run(&json!(result.execution_id), state);
                        if result.state != rad_api::ExecutionState::Running {
                            self.control.terminal();
                        }
                    }
                }
                Ok(Reply::NoAppIcon) => {
                    if std::mem::take(&mut self.icon_asked) {
                        self.say(tr(
                            self.es,
                            "This application has no PNG or JPEG icon to use",
                        ));
                    }
                }
                Ok(Reply::Response {
                    operation,
                    response: r,
                }) => {
                    self.icon_asked = false;
                    if operation == "plugin_manifest" {
                        if let rad_api::ResponseData::PluginManifest(view) = r.data
                            && let Err(error) = self.plugins.accept_manifest(view)
                        {
                            self.warn(crate::status::describe_failure(&error, self.es));
                        }
                        return;
                    }
                    if operation == "plugin_log" {
                        if let rad_api::ResponseData::PluginLog(log) = r.data
                            && log.plugin_id == self.plugins.log_selected
                        {
                            self.plugins.log = Some(log);
                        }
                        return;
                    }
                    if operation == "plugin_dependencies" {
                        if let Err(error) = self.plugins.dependencies.accept(
                            serde_json::to_value(r.data).expect("typed dependency projection"),
                        ) && error.code != "cursor_expired"
                        {
                            self.warn(crate::status::describe_failure(&error, self.es));
                            self.message_code = error.code;
                        }
                        return;
                    }
                    if operation == "restore_commit" && self.plugins.restore_result(r.status) {
                        self.credentials.clear();
                        self.settings_open = true;
                        self.settings_tab = settings::Tab::Plugins;
                        self.plugins.section_open = true;
                        self.say(tr(self.es, "Configuration restored"));
                        self.force = true;
                        self.inspect_needed = self.selected().is_some();
                        return;
                    }
                    if operation == "plugin_retry" {
                        self.plugins.dependency_status_due = true;
                        self.say(tr(self.es, "Plugin will retry when needed"));
                        return;
                    }
                    if operation == "plugin_admin" {
                        self.plugins.admin_pending = false;
                        self.plugins.dependency_status_due = true;
                        self.plugins.status["state"] = json!("preparing");
                        self.say(tr(self.es, "Preparing — not yet activated"));
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
                        && let rad_api::ResponseData::Admitted(data) = &r.data
                    {
                        self.control.admitted(data.execution_id.clone());
                        self.result_due = true;
                    }
                    match r.status {
                        rad_client::response::Status::Admitted => {
                            if let rad_api::ResponseData::Admitted(data) = &r.data {
                                self.say_run(&json!(data.execution_id), "running")
                            }
                        }
                        rad_client::response::Status::Applied => self.say(tr(self.es, "Saved")),
                        _ => self.say(tr(self.es, "Updated")),
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
                    self.say(format!("{} {n}", tr(self.es, "Transferred bytes:")));
                }
                Ok(Reply::TransferFinished { kind, data }) => {
                    self.transfer_active = false;
                    match kind {
                        TransferKind::Image => {
                            self.draft.image = string(&data["media_ref"]);
                            self.image_path.clear();
                            self.say(tr(self.es, "Image prepared: save within 60 seconds"));
                        }
                        TransferKind::Restore => {
                            self.restore_token = data["transfer_id"].as_str().map(str::to_owned);
                            let count = data["summary"]["plugins_count"].as_u64();
                            self.restore_summary = match count {
                                Some(count) => format!(
                                    "{} · {count} plugins",
                                    tr(
                                        self.es,
                                        "This backup will replace your current configuration"
                                    )
                                ),
                                None => tr(
                                    self.es,
                                    "This backup will replace your current configuration",
                                )
                                .into(),
                            };
                        }
                        TransferKind::Export => self.say(tr(self.es, "Export finished")),
                    }
                }
            }
        }
        if self.reconnect_needed && !self.worker.busy() {
            self.reconnect_needed = false;
            let _ = self.worker.submit(Job::Connect);
        }
        if !self.worker.busy() && self.connected {
            let now = self.now_ms();
            let next_image = self.board.as_ref().and_then(|b| {
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
            match self.schedule.choose([
                self.force || self.overview_due,
                self.result_due && self.control.result_request().is_some(),
                self.transfer_active,
                self.inspect_needed,
                self.catalog_due.is_some_and(|due| now >= due),
                self.plugins.has_work(),
                next_image.is_some(),
            ]) {
                Some(0) => {
                    let force = std::mem::take(&mut self.force);
                    let overview = std::mem::take(&mut self.overview_due)
                        || self.status.is_null()
                        || self.resolving_unknown;
                    let _ = self.worker.submit(Job::Refresh {
                        board: force,
                        overview,
                    });
                }
                Some(1) => {
                    self.result_due = false;
                    let _ = self
                        .worker
                        .submit(Job::Request(self.control.result_request().unwrap()));
                }
                Some(2) => {
                    let _ = self.worker.submit(Job::TransferStep);
                }
                Some(3) => {
                    self.inspect_needed = false;
                    if let Some(id) = self.selected() {
                        let _ = self.worker.submit(Job::Inspect(id));
                    }
                }
                Some(4) => {
                    self.catalog_due = None;
                    let _ = self
                        .worker
                        .submit(Job::Request(self.catalog.request(false)));
                }
                Some(5) => {
                    self.poll_plugins();
                }
                Some(6) => {
                    let (id, reference, revision) = next_image.unwrap();
                    self.image_attempts.insert(reference.clone());
                    let _ = self.worker.submit(Job::Image {
                        id,
                        reference,
                        revision,
                    });
                }
                _ => {}
            }
        }
        if self.styled.as_ref() != Some(&self.palette) {
            Look::new(&self.palette).apply(ctx);
            self.styled = Some(self.palette.clone());
        }
    }
    /// Results of the credential and option-list workers; a dead one is replaced, within
    /// the same bound as the main worker.
    fn pump_auxiliary(&mut self, ctx: &egui::Context) {
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
            self.send(Request::Redo {});
        } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
            self.send(Request::Undo {});
        }
    }
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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
            let wake = ctx.clone();
            self.notices = Some(crate::notices::Notices::start(
                self.runtime.clone(),
                move || wake.request_repaint(),
            ));
            let wake = ctx.clone();
            self.search_wake = Some(crate::worker::WakeTimer::new(move || {
                wake.request_repaint()
            }));
            self.wake_registered = true;
        }
        self.pump(&ctx);
        self.pump_auxiliary(&ctx);
        if self.credential_selection != self.selected() {
            self.credentials.clear();
            self.credential_selection = self.selected();
        }
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
        if self.connected
            && !self.worker.busy()
            && (self.force
                || self.overview_due
                || self.inspect_needed
                || self.transfer_active
                || self.plugins.has_work()
                || self.catalog_due.is_some_and(|due| self.now_ms() >= due)
                || (self.result_due && self.control.result_request().is_some()))
        {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod connection_loss_tests {
    use super::*;
    #[test]
    fn overview_bar_shows_plugin_attention_and_inaccessible_deck() {
        let root =
            std::env::temp_dir().join(format!("rad-editor-bar-no-runtime-{}", std::process::id()));
        let mut app = App::with_preferences_path(root.clone(), root.join("no-theme"), None);
        app.es = crate::preferences::Language::En;
        app.connected = true;
        app.status = serde_json::json!({"deck_state":"inaccessible", "plugins":[
            {"id":"secret-technical-id", "code":"needs_credentials"},
            {"id":"legacy", "code":"needs_update"},
            {"id":"failed", "code":"failed"}
        ]});
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |ui| {
            app.bar(ui, &Look::new(&Default::default()))
        });
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::epaint::Shape::Text(t) = &shape.shape {
                    Some(t.galley.job.text.as_str())
                } else {
                    None
                }
            })
            .collect();
        for expected in [
            "Deck inaccessible — editor available",
            "Needs password",
            "Needs update",
            "Not working",
            "Plugins",
        ] {
            assert!(texts.contains(&expected), "missing {expected}: {texts:?}");
        }
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("secret-technical-id"))
        );
        let plugins_button = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::epaint::Shape::Text(t) = &shape.shape {
                    (t.galley.job.text == "Plugins").then(|| t.pos + t.galley.size() * 0.5)
                } else {
                    None
                }
            })
            .unwrap();
        let input = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(plugins_button),
                egui::Event::PointerButton {
                    pos: plugins_button,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos: plugins_button,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.bar(ui, &Look::new(&Default::default())));
        assert!(app.settings_open);
        assert!(app.settings_tab == settings::Tab::Plugins);
    }
    #[test]
    fn not_applied_disconnect_attempts_one_reconnect_and_never_replays_mutation() {
        let root =
            std::env::temp_dir().join(format!("rad-editor-no-runtime-{}", std::process::id()));
        let mut app = App::with_preferences_path(root.clone(), root.join("no-theme"), None);
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = sent.clone();
        app.worker = crate::worker::Worker::spawn(move |job| {
            record.lock().unwrap().push(match job {
                Job::Request(Request::Undo {}) => "undo",
                Job::Connect => "connect",
                _ => panic!("unexpected job: {job:?}"),
            });
            crate::native::Event {
                plugin_apply: false,
                plugin_admin: false,
                plugin_manifest: false,
                plugin_dependencies: false,
                plugin_form: false,
                plugin_visual: false,
                connected: false,
                transfer_job: false,
                palette: Palette::default(),
                result: Err(crate::session::Error {
                    code: "runtime_unavailable".into(),
                    cause: None,
                    unknown: false,
                }),
            }
        });
        app.connected = true;
        app.send(Request::Undo {});
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while app.worker.busy() {
            assert!(
                std::time::Instant::now() < deadline,
                "reconnection must be bounded"
            );
            app.pump(&ctx);
            std::thread::yield_now();
        }
        app.pump(&ctx);
        assert_eq!(*sent.lock().unwrap(), ["undo", "connect"]);
        assert!(!app.connected);
    }
}

#[cfg(test)]
mod worker_loss_tests;
