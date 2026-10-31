//! Apply replies from the main worker without changing the order of UI effects.
use super::{App, Draft, string, tr, visual_type};
use crate::{
    native::{Job, Reply, TransferKind},
    worker::Polled,
};
use eframe::egui;
use serde_json::{Value, json};
use std::{ops::ControlFlow, time::Instant};

impl App {
    /// A handler can end this pump turn before service, reads, or styling run.
    pub(super) fn pump_replies(&mut self, ctx: &egui::Context) -> ControlFlow<()> {
        let polled = self.worker.poll();
        match &polled {
            Polled::Failed { unsent } => self.worker_lost(ctx, true, *unsent),
            Polled::Closed { unsent } => self.worker_lost(ctx, false, *unsent),
            Polled::Ready(_) | Polled::Pending => {}
        }
        if let Polled::Ready(event) = polled {
            self.finish_application_catalog(&event.result);
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
                        // A plugin's settings come back with what was just saved.
                        self.plugins.settings_asked = None;
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
                // A read ahead that was refused is not asked again until the board changes.
                Err(_) if event.background && event.connected => {}
                Err(e) => {
                    // The runtime did not explain: nothing is retried until something changes.
                    self.recovery_due = false;
                    if matches!(self.recovery, crate::recovery::Mode::Asking) {
                        self.recovery = Default::default();
                    }
                    if connection_lost {
                        self.reconnect_needed = true;
                        self.force = true;
                        self.overview_due = true;
                        self.inspect_needed = self.selected().is_some();
                    }
                    if event.plugin_dependencies {
                        self.plugins.dependencies.fail(&e);
                        if e.code == "cursor_expired" {
                            return ControlFlow::Break(());
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
                        if let Some(change) = e.source_change.clone() {
                            self.plugins.source_refused(change);
                        }
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
                Ok(Reply::RecoveryRequired) => self.enter_recovery(),
                Ok(Reply::Connected) => {
                    self.recovery = Default::default();
                    self.recovery_due = false;
                    self.machine = None;
                    self.machine_due = true;
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
                        if self.navigation.screen() == crate::screens::Screen::Keys
                            || self.settings.is_null()
                        {
                            self.settings = r.status["settings"].clone();
                        } else if let Some(settings) = r.status["settings"].as_object() {
                            // A native action may publish brightness while Settings is open.
                            // Follow untouched fields without discarding the user's drafts.
                            for (field, value) in settings {
                                if self.settings.get(field) == self.status["settings"].get(field) {
                                    self.settings[field] = value.clone();
                                }
                            }
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
                        let changed_revision = self.board.as_ref().is_none_or(|old| {
                            old.folder["view_revision"] != board.folder["view_revision"]
                        });
                        self.pictures.changed();
                        if changed_revision || changed_folder {
                            self.board_since = Instant::now();
                            // A face stays until its new one arrives, while its key still
                            // shows the same visual.
                            let old = self.board.as_ref();
                            self.plugins.visuals.retain(|id, _| {
                                old.is_some_and(|old| {
                                    visual_type(old, id) == visual_type(&board, id)
                                })
                            });
                        }
                        self.ahead.follow(&board);
                        // The selected key's inspector came with the board: both change at once.
                        let fresh = r.inspection.filter(|inspection| {
                            self.ahead.keep_inspection(&board, inspection);
                            crate::session::coherent(&board, inspection)
                                && inspection.button["id"].as_str() == self.control.selected()
                        });
                        if let Some(inspection) = fresh {
                            self.draft = Draft::load(&inspection);
                            self.inspection = Some(inspection);
                        } else if self
                            .inspection
                            .as_ref()
                            .is_some_and(|inspection| !crate::session::coherent(&board, inspection))
                        {
                            self.inspection = None;
                            self.inspect_needed = self.selected().is_some();
                        }
                        self.board = Some(board);
                        if changed_folder {
                            self.icon_library.reset();
                            self.app_picker.cancel();
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
                    let Some(board) = self.board.as_ref() else {
                        return ControlFlow::Break(());
                    };
                    if !crate::session::coherent(board, &v) {
                        // Newer than the board: the two are read again together, and the
                        // pair on screen stays until then.
                        self.force = true;
                        return ControlFlow::Break(());
                    }
                    self.ahead.keep_inspection(board, &v);
                    if v.button["id"].as_str() == self.control.selected() {
                        self.adopt_inspection(v);
                    }
                }
                Ok(Reply::InspectedAhead(v)) => {
                    if let Some(board) = &self.board {
                        self.ahead.keep_inspection(board, &v);
                        // A key selected while it was being read takes it at once.
                        if self.inspection.is_none()
                            && crate::session::coherent(board, &v)
                            && v.button["id"].as_str() == self.control.selected()
                        {
                            self.draft = Draft::load(&v);
                            self.inspection = Some(v);
                            self.inspect_needed = false;
                        }
                    }
                }
                Ok(Reply::FolderBoard(folder)) => self.ahead.keep_folder(folder),
                Ok(Reply::Response { operation, .. }) if operation == "launcher_catalog" => {} // Handled by finish_application_catalog.
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
                        return ControlFlow::Break(());
                    }
                    if operation == "plugin_log" {
                        if let rad_api::ResponseData::PluginLog(log) = r.data
                            && log.plugin_id == self.plugins.log_selected
                        {
                            self.plugins.log = Some(log);
                        }
                        return ControlFlow::Break(());
                    }
                    if operation == "plugin_dependencies" {
                        if let Err(error) = self.plugins.dependencies.accept(
                            serde_json::to_value(r.data).expect("typed dependency projection"),
                        ) && error.code != "cursor_expired"
                        {
                            self.warn(crate::status::describe_failure(&error, self.es));
                            self.message_code = error.code;
                        }
                        return ControlFlow::Break(());
                    }
                    if self.recovery_response(&operation, &r)
                        || self.machine_response(&operation, &r)
                    {
                        return ControlFlow::Break(());
                    }
                    if operation == "restore_commit" && self.plugins.restore_result(r.status) {
                        self.credentials.clear();
                        self.navigation.open(crate::screens::Section::Plugins);
                        self.say(tr(self.es, "Configuration restored"));
                        self.force = true;
                        self.inspect_needed = self.selected().is_some();
                        return ControlFlow::Break(());
                    }
                    if operation == "plugin_retry" {
                        self.plugins.dependency_status_due = true;
                        self.say(tr(self.es, "Plugin will retry when needed"));
                        return ControlFlow::Break(());
                    }
                    if operation == "plugin_admin" {
                        self.plugins.admin_pending = false;
                        self.plugins.dependency_status_due = true;
                        self.plugins.status["state"] = json!("preparing");
                        self.say(tr(self.es, "Preparing — not yet activated"));
                        return ControlFlow::Break(());
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
                Ok(Reply::Pictures(read)) => {
                    let now = ctx.input(|i| i.time);
                    for (reference, pixels) in read {
                        let Some(pixels) = pixels else {
                            continue;
                        };
                        // One that arrives with its key on screen fades in.
                        let on_show = self.board.as_ref().is_some_and(|board| {
                            board
                                .buttons
                                .iter()
                                .any(|key| key.appearance["image"] == reference.as_str())
                        });
                        let image =
                            egui::ColorImage::from_rgba_unmultiplied(pixels.size, &pixels.rgba);
                        let texture =
                            ctx.load_texture(reference.clone(), image, Default::default());
                        self.pictures
                            .arrived(reference, texture, on_show.then_some(now));
                    }
                }
                Ok(Reply::IconSaved {
                    button,
                    ticket,
                    result,
                }) => self.icon_saved(&button, ticket, result),
                Ok(Reply::Theme) => {}
                Ok(Reply::TransferProgress(n)) => {
                    self.transfer_active = true;
                    self.say(format!("{} {n}", tr(self.es, "Transferred bytes:")));
                }
                Ok(Reply::TransferFinished { kind, data, path }) => {
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
                        TransferKind::Export => {
                            self.say(tr(self.es, "Export finished"));
                            if let Some(path) = path {
                                self.backup_path = path.display().to_string();
                                self.message_code = self.backup_path.clone();
                            }
                        }
                    }
                }
            }
        }
        ControlFlow::Continue(())
    }
}
