//! Plugin presentation only; publication/activation belong to the runtime.
use super::{App, Job, look::Look, string, tr};
use crate::plugins::{CatalogEntry, FormDraft, FormTarget, LoadedForm, Visual};
use eframe::egui;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::collections::BTreeMap;
#[derive(Default)]
pub(super) struct PluginUi {
    pub catalog: Vec<CatalogEntry>,
    pub status: Value,
    pub dependencies: crate::dependencies::Dependencies,
    dependency_page: usize,
    pub dependency_credentials: Option<String>,
    pub dependency_status_due: bool,
    pub section_open: bool,
    pub catalog_loaded: bool,
    pub visual_due: bool,
    pub form: Option<(FormTarget, LoadedForm)>,
    pub visuals: BTreeMap<String, Face>,
    pub admin_pending: bool,
    pub form_pending: bool,
    pub form_loading: bool,
    pub settings_reload: Option<String>,
    pub log_selected: String,
    pub log: Option<rad_api::PluginLogView>,
    pub resources: BTreeMap<
        String,
        (
            rad_plugin_contract::Version,
            Vec<rad_plugin_contract::Resource>,
        ),
    >,
    pub manifest_pending: Option<String>,
    pub manifest_invalidated: bool,
    manifest_request: Option<String>,
    /// Last ticket given to a question about a list, so no answer is taken for another's.
    pub tickets: u64,
    source: String,
    trusted: bool,
    source_edited: bool,
    selected: String,
    status_page: usize,
    password_request: Option<(String, Vec<String>)>,
    update_request: bool,
    retry_request: Option<String>,
    schedule: crate::schedule::Schedule,
}
impl PluginUi {
    pub fn reconnect(&mut self) {
        self.dependencies.reset();
        self.dependency_status_due = true;
        self.status = Value::Null;
        self.catalog_loaded = false;
        self.trusted = false;
        self.source.clear();
        self.selected.clear();
        self.source_edited = true;
        self.dependency_credentials = None;
        self.log = None;
        self.resources.clear();
        self.manifest_pending = None;
        self.manifest_invalidated = false;
    }
    pub fn accept_manifest(
        &mut self,
        view: rad_api::PluginManifestView,
    ) -> Result<(), crate::session::Error> {
        self.manifest_pending = None;
        if std::mem::take(&mut self.manifest_invalidated) {
            return Ok(());
        }
        let manifest: rad_plugin_contract::Manifest = serde_json::from_value(
            serde_json::to_value(view.document).map_err(|_| crate::session::Error::protocol())?,
        )
        .map_err(|_| crate::session::Error::protocol())?;
        if manifest.id != view.plugin_id
            || manifest.resources.len() > 5
            || manifest
                .resources
                .iter()
                .enumerate()
                .any(|(index, resource)| manifest.resources[..index].contains(resource))
        {
            return Err(crate::session::Error::protocol());
        }
        let current = self.status["items"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item["id"] == manifest.id
                    && item["selected"] == serde_json::to_value(manifest.version).expect("version")
            })
        });
        if current {
            self.resources
                .insert(manifest.id, (manifest.version, manifest.resources));
        }
        Ok(())
    }
    pub fn restore_result(&mut self, status: rad_client::response::Status) -> bool {
        if status != rad_client::response::Status::Applied {
            return false;
        }
        self.reconnect();
        true
    }
    pub fn has_work(&self) -> bool {
        self.settings_reload.is_some()
            || self.dependency_status_due
            || self.dependencies.request().is_some()
            || !self.catalog_loaded
            || self.visual_due
    }

    fn can_administer(&self, connected: bool) -> bool {
        connected
            && !self.admin_pending
            && self.status["locked"] != true
            && matches!(self.status["state"].as_str(), Some("idle" | "failed"))
    }
    fn select(&mut self, id: &str) {
        self.selected = id.into();
        self.source_edited = false;
        self.source = self
            .selected_item()
            .and_then(|v| v["source"].as_str())
            .unwrap_or("")
            .into();
        self.trusted = false;
    }
    fn selected_item(&self) -> Option<&Value> {
        self.status["items"]
            .as_array()?
            .iter()
            .find(|v| v["id"] == self.selected)
    }
    fn trusted_update(&self) -> bool {
        self.trusted
            || (!self.source_edited
                && self.selected_item().is_some_and(|v| {
                    v["source_trusted"] == true && v["source"].as_str() == Some(self.source.trim())
                }))
    }
}
pub(super) struct Face {
    pub revision: String,
    pub state: String,
    pub text: Option<String>,
    pub position: String,
    pub color: Option<[u8; 4]>,
    pub texture: Option<egui::TextureHandle>,
}
fn label(code: &str, es: impl Into<crate::preferences::Language> + Copy) -> &str {
    match code {
        "starting" => tr(es, "Starting"),
        "idle" => tr(es, "Idle"),
        "preparing" => tr(es, "Preparing"),
        "committing" => tr(es, "Saving selection"),
        "activating" => tr(es, "Activating"),
        "failed" => tr(es, "Failed"),
        "uncertain" => tr(es, "Outcome uncertain"),
        "ready" => tr(es, "Ready"),
        "residual" => tr(es, "Residual resources"),
        "absent" => tr(es, "Not running"),
        "plugin_in_use" => tr(es, "Plugin in use — retry manually later"),
        "plugin_residual" => tr(es, "Waiting for real termination"),
        "current" => tr(es, "Current"),
        "stale" => tr(es, "Stale"),
        "unavailable" => tr(es, "Unavailable"),
        _ => crate::status::describe_error(code, es),
    }
}
fn resource_details(
    ui: &mut egui::Ui,
    resources: &[rad_plugin_contract::Resource],
    language: impl Into<crate::preferences::Language> + Copy,
) {
    use rad_plugin_contract::Resource;
    ui.label(tr(language, "Declared resources"));
    if resources.is_empty() {
        ui.label(tr(language, "No resources declared"));
    }
    for resource in resources {
        ui.label(tr(
            language,
            match resource {
                Resource::Internet => "Internet",
                Resource::SystemPrograms => "System programs",
                Resource::ExternalFiles => "Files outside the plugin folder",
                Resource::Devices => "Devices",
                Resource::DesktopServices => "Desktop services",
            },
        ));
    }
    ui.label(tr(
        language,
        "Declared by the author; informational, not enforced permissions",
    ));
}
fn version(v: &Value) -> String {
    if v.is_null() {
        return "—".into();
    }
    format!("{}.{}.{}", v["major"], v["minor"], v["patch"])
}
/// The manager admits at most 32 installed plugins. Page controls never hide a tail.
fn plugin_status_list(
    ui: &mut egui::Ui,
    plugins: &mut PluginUi,
    idle: bool,
    es: impl Into<crate::preferences::Language> + Copy,
) -> bool {
    const PAGE_SIZE: usize = 8;
    const MAX_PLUGINS: usize = 32;
    let items = plugins.status["items"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if items.len() > MAX_PLUGINS {
        ui.label(tr(es, "Invalid plugin status: more than 32 entries"));
        return false;
    }
    let pages = items.len().div_ceil(PAGE_SIZE).max(1);
    plugins.status_page = plugins.status_page.min(pages - 1);
    let mut selected_id = None;
    for item in items
        .iter()
        .skip(plugins.status_page * PAGE_SIZE)
        .take(PAGE_SIZE)
    {
        let id = string(&item["id"]);
        let name = item["name"]
            .as_str()
            .or_else(|| id.rsplit('.').next())
            .unwrap_or(&id);
        if idle {
            if ui.selectable_label(plugins.selected == id, name).clicked() {
                selected_id = Some(id.clone());
            }
        } else {
            ui.label(name);
        }
        let state = crate::status::PluginState::of(item);
        ui.label(state.label(es));
        if idle {
            match state {
                crate::status::PluginState::NotWorking if item["can_retry"] == true => {
                    if ui.button(tr(es, "Retry")).clicked() {
                        plugins.retry_request = Some(id.clone());
                    }
                }
                crate::status::PluginState::NeedsPassword => {
                    let slots: Vec<String> = plugins
                        .dependencies
                        .items()
                        .into_iter()
                        .flatten()
                        .find(|dependency| dependency["id"] == id)
                        .into_iter()
                        .flat_map(|dependency| {
                            dependency["credential_slots"]
                                .as_array()
                                .into_iter()
                                .flatten()
                        })
                        .filter_map(|slot| slot.as_str().map(str::to_owned))
                        .collect();
                    if !slots.is_empty() && ui.button(tr(es, "Enter password")).clicked() {
                        plugins.password_request = Some((id.clone(), slots));
                    }
                }
                crate::status::PluginState::NeedsUpdate
                    if item["source_trusted"] == true
                        && item["source"]
                            .as_str()
                            .and_then(installation_source)
                            .is_some()
                        && ui.button(tr(es, "Update")).clicked() =>
                {
                    selected_id = Some(id.clone());
                    plugins.update_request = true;
                }
                _ => {}
            }
        }
        ui.collapsing(tr(es, "View details"), |ui| {
            ui.label(format!(
                "{} {} · {} {} · {}",
                tr(es, "Selected"),
                version(&item["selected"]),
                tr(es, "Previous"),
                version(&item["previous"]),
                item["host"].as_str().unwrap_or("absent")
            ));
            if let Some(code) = item["diagnostic"].as_str() {
                ui.label(code);
            }
            if let Some((_, resources)) = plugins.resources.get(&id) {
                resource_details(ui, resources, es);
            } else if idle
                && plugins.manifest_pending.is_none()
                && ui.button(tr(es, "Declared resources")).clicked()
            {
                plugins.manifest_request = Some(id.clone());
            }
        });
    }
    if pages > 1 {
        ui.horizontal(|ui| {
            ui.label(
                tr(es, "Page {page}/{pages} · {count} plugins")
                    .replace("{page}", &(plugins.status_page + 1).to_string())
                    .replace("{pages}", &pages.to_string())
                    .replace("{count}", &items.len().to_string()),
            );
            if plugins.status_page > 0 && ui.button(tr(es, "Previous")).clicked() {
                plugins.status_page -= 1;
            }
            if plugins.status_page + 1 < pages && ui.button(tr(es, "Next")).clicked() {
                plugins.status_page += 1;
            }
        });
    }
    if let Some(id) = selected_id {
        plugins.select(&id);
    }
    true
}
/// Conservative public-source projection; the runtime still validates the package and source.
fn installation_source(origin: &str) -> Option<String> {
    if origin.len() > 1024 {
        return None;
    }
    let path = origin
        .strip_prefix("https://github.com/")?
        .trim_end_matches('/');
    let parts: Vec<_> = path.split('/').collect();
    let component = |s: &str| {
        !s.is_empty()
            && s.len() <= 128
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    if !matches!(parts.len(), 2..=5)
        || !parts.iter().all(|p| component(p))
        || parts.len() >= 3 && parts[2] != "releases"
        || parts.len() == 4 && parts[3] != "latest"
        || parts.len() == 5 && parts[3] != "tag"
    {
        return None;
    }
    let repository = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    if !component(repository) {
        return None;
    }
    let base = format!("https://github.com/{}/{repository}", parts[0]);
    Some(
        parts
            .get(4)
            .map_or(base.clone(), |tag| format!("{base}/releases/tag/{tag}")),
    )
}

/// Shows eight published dependencies per frame, including an explicit tail navigator.
/// Imported metadata is only a suggestion; preparing never submits an install or trusts it.
fn dependency_list(
    ui: &mut egui::Ui,
    plugins: &mut PluginUi,
    idle: bool,
    es: impl Into<crate::preferences::Language> + Copy,
) -> Option<(String, Vec<String>)> {
    let Some(items) = plugins.dependencies.items() else {
        if plugins.dependencies.failed() {
            ui.label(tr(es, "Could not read required plugins — retry"));
            if ui.button(tr(es, "Retry")).clicked() {
                plugins.dependencies.reset();
            }
        } else {
            ui.label(tr(es, "Checking required plugins…"));
        }
        return None;
    };
    if items.is_empty() {
        return None;
    }
    let Some(installed) = plugins.status["items"]
        .as_array()
        .filter(|items| items.len() <= 32)
    else {
        ui.label(tr(es, "Checking installed plugins…"));
        return None;
    };
    ui.heading(tr(es, "Plugins used by this configuration"));
    let pages = items.len().div_ceil(8);
    plugins.dependency_page = plugins.dependency_page.min(pages - 1);
    let mut source = None;
    let mut credentials = None;
    for item in items.iter().skip(plugins.dependency_page * 8).take(8) {
        let id = item["id"].as_str().unwrap_or("");
        ui.push_id(("dependency", id), |ui| {
            ui.label(id);
            ui.label(if item["version"].is_null() {
                tr(es, "Version unknown").to_owned()
            } else {
                version(&item["version"])
            });
            if let Some(local) = installed.iter().find(|local| local["id"] == id) {
                if local["needs_credentials"] == true {
                    ui.label(tr(es, "Needs password"));
                    let slots: Vec<String> = item["credential_slots"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect();
                    if idle && !slots.is_empty() && ui.button(tr(es, "Enter password")).clicked() {
                        credentials = Some((id.to_owned(), slots));
                    }
                }
            } else {
                ui.label(tr(es, "Not installed"));
                if let Some(origin) = item["origin"].as_str().and_then(installation_source) {
                    ui.label(&origin);
                    if idle && ui.button(tr(es, "Prepare installation")).clicked() {
                        source = Some(origin);
                    }
                } else {
                    ui.label(tr(
                        es,
                        "Source unknown or unsupported — obtain a trusted public GitHub link",
                    ));
                }
            }
        });
        ui.add_space(4.);
    }
    if pages > 1 {
        ui.horizontal(|ui| {
            ui.label(
                tr(es, "Page {page}/{pages} · {count} plugins")
                    .replace("{page}", &(plugins.dependency_page + 1).to_string())
                    .replace("{pages}", &pages.to_string())
                    .replace("{count}", &items.len().to_string()),
            );
            if plugins.dependency_page > 0 && ui.button(tr(es, "Previous")).clicked() {
                plugins.dependency_page -= 1;
            }
            if plugins.dependency_page + 1 < pages && ui.button(tr(es, "Next")).clicked() {
                plugins.dependency_page += 1;
            }
        });
    }
    if let Some(source) = source {
        plugins.source = source;
        plugins.selected.clear();
        plugins.source_edited = true;
        plugins.trusted = false;
    }
    ui.separator();
    credentials
}
impl App {
    pub(super) fn poll_plugins(&mut self) -> bool {
        match self.plugins.schedule.choose([
            self.plugins.dependency_status_due,
            self.plugins.dependencies.request().is_some(),
            !self.plugins.catalog_loaded,
            self.plugins.visual_due,
            self.plugins.settings_reload.is_some(),
        ]) {
            Some(0) => {
                self.plugins.dependency_status_due = false;
                let _ = self.worker.submit(Job::Plugins);
                return true;
            }
            Some(1) => {
                if let Some(request) = self.plugins.dependencies.request() {
                    let _ = self.worker.submit(Job::Request(request));
                }
                return true;
            }
            Some(2) => {
                self.plugins.catalog_loaded = true;
                let _ = self.worker.submit(Job::PluginCatalog);
                return true;
            }
            Some(4) => {
                let id = self
                    .plugins
                    .settings_reload
                    .take()
                    .expect("pending settings read");
                self.open_plugin_form(&id, None, FormTarget::Settings(id.clone()));
                return true;
            }
            Some(3) => self.plugins.visual_due = false,
            _ => return false,
        }
        let ids: Vec<_> = self
            .board
            .iter()
            .flat_map(|b| &b.buttons)
            .filter(|b| b.appearance["visual_type"].is_string())
            .map(|b| string(&b.entity["id"]))
            .collect();
        if ids.is_empty() {
            return false;
        }
        let _ = self.worker.submit(Job::PluginVisuals {
            ids,
            revision: self
                .board
                .as_ref()
                .map(|board| string(&board.folder["view_revision"]))
                .unwrap_or_default(),
        });
        true
    }

    pub(super) fn open_plugin_form(
        &mut self,
        key: &str,
        entity: Option<String>,
        target: FormTarget,
    ) {
        if !self.connected || self.plugins.form_pending || self.plugins.form_loading {
            return;
        }
        if self.command(Job::PluginForm {
            key: key.into(),
            entity,
            target,
        }) {
            self.credentials.clear();
            self.plugins.dependency_credentials = None;
            self.plugins.form = None;
            self.plugins.form_loading = true;
        }
    }
    /// The Plugins tab of the settings: installed plugins and their administration.
    pub(super) fn plugin_settings(&mut self, ui: &mut egui::Ui) {
        // Nothing happening is not worth a line; an operation under way is.
        let state = string(&self.plugins.status["state"]);
        if !state.is_empty() && state != "idle" {
            ui.label(tr(self.es, "Working…"));
        }
        let idle = self.plugins.can_administer(self.connected);
        if let Some((id, slots)) = dependency_list(ui, &mut self.plugins, idle, self.es) {
            self.plugins.form = None;
            self.credentials.bind(
                &format!("dependency:{id}"),
                &format!("{id}:credentials"),
                &slots,
            );
            self.plugins.dependency_credentials = Some(id);
        }
        if self
            .plugins
            .dependency_credentials
            .as_ref()
            .is_some_and(|id| {
                self.plugins.status["items"]
                    .as_array()
                    .is_none_or(|items| !items.iter().any(|item| item["id"] == *id))
            })
        {
            self.plugins.dependency_credentials = None;
            self.credentials.clear();
        }
        if self.plugins.dependency_credentials.is_some() {
            self.credentials
                .show(ui, &mut self.credential_worker, self.connected, self.es);
            if ui.button(tr(self.es, "Close credentials")).clicked() {
                self.credentials.clear();
                self.plugins.dependency_credentials = None;
            }
        }
        if self.plugins.status["locked"] == true {
            ui.label(tr(self.es, "Restart RAD to recover plugin administration"));
        }
        if let Some(code) = self.plugins.status["error"].as_str() {
            ui.label(crate::status::describe_error(code, self.es));
            ui.collapsing(tr(self.es, "View details"), |ui| {
                ui.label(code);
            });
        }
        // Known only once the status has arrived; until then nothing is claimed.
        let installed = self.plugins.status["items"].as_array().map(Vec::len);
        if installed == Some(0) {
            ui.label(egui::RichText::new(tr(self.es, "No plugins installed yet")).weak());
        }
        if !plugin_status_list(ui, &mut self.plugins, idle, self.es) {
            return;
        }
        if let Some(plugin_id) = self.plugins.manifest_request.take()
            && self.command(Job::Request(crate::Request::PluginManifest {
                plugin_id: plugin_id.clone(),
            }))
        {
            self.plugins.manifest_pending = Some(plugin_id);
        }
        if let Some(plugin_id) = self.plugins.retry_request.take() {
            self.send(crate::Request::PluginRetry { plugin_id });
        }
        if idle
            && self
                .plugins
                .selected_item()
                .is_some_and(|item| item["has_settings"] == true)
            && ui.button(tr(self.es, "Plugin settings")).clicked()
        {
            let id = self.plugins.selected.clone();
            self.open_plugin_form(&id, None, FormTarget::Settings(id.clone()));
        }
        if let Some((id, slots)) = self.plugins.password_request.take() {
            self.credentials.bind(
                &format!("dependency:{id}"),
                &format!("{id}:credentials"),
                &slots,
            );
            self.plugins.dependency_credentials = Some(id);
        }
        if std::mem::take(&mut self.plugins.update_request) {
            self.submit_plugin_operation("update");
        }
        if !idle {
            return;
        }
        // A line between the installed plugins and the form to install another.
        if installed.is_some_and(|count| count > 0) {
            ui.add_space(6.);
            ui.separator();
        }
        self.plugin_admin_controls(ui);
    }
    fn plugin_admin_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(tr(self.es, "Public GitHub repository / release URL"));
        if ui
            .add(egui::TextEdit::singleline(&mut self.plugins.source).char_limit(2048))
            .changed()
        {
            self.plugins.trusted = false;
            self.plugins.source_edited = true;
        }
        ui.checkbox(
            &mut self.plugins.trusted,
            tr(self.es, "I trust this native code (not sandboxed)"),
        );
        let selected = self.plugins.selected_item();
        let source_ok =
            self.plugins.trusted && installation_source(self.plugins.source.trim()).is_some();
        let operations = [
            ("install", "Install", source_ok),
            (
                "update",
                "Update",
                selected.is_some()
                    && self.plugins.trusted_update()
                    && installation_source(self.plugins.source.trim()).is_some(),
            ),
            ("reinstall", "Reinstall", selected.is_some() && source_ok),
            ("uninstall", "Uninstall", selected.is_some()),
            (
                "rollback",
                "Rollback",
                selected.is_some_and(|item| !item["previous"].is_null()),
            ),
        ];
        let mut chosen = None;
        ui.horizontal_wrapped(|ui| {
            for (operation, en, allowed) in operations {
                if allowed && ui.button(tr(self.es, en)).clicked() {
                    chosen = Some(operation);
                }
            }
        });
        if let Some(operation) = chosen {
            self.submit_plugin_operation(operation);
        }
    }
    fn submit_plugin_operation(&mut self, operation: &str) {
        let remote = matches!(operation, "install" | "update" | "reinstall");
        let operation_code = match operation {
            "install" => rad_api::PluginOperation::Install,
            "update" => rad_api::PluginOperation::Update,
            "reinstall" => rad_api::PluginOperation::Reinstall,
            "uninstall" => rad_api::PluginOperation::Uninstall,
            "rollback" => rad_api::PluginOperation::Rollback,
            _ => return,
        };
        let request = crate::Request::PluginAdmin {
            operation: operation_code,
            plugin_id: if operation == "install" {
                None
            } else {
                Some(self.plugins.selected.clone())
            },
            source: if remote {
                Some(self.plugins.source.trim().to_owned())
            } else {
                None
            },
            trusted: Some(
                !remote
                    || self.plugins.trusted
                    || (operation == "update" && self.plugins.trusted_update()),
            ),
        };
        if self.command(Job::Request(request)) {
            self.plugins.admin_pending = true;
            self.plugins.trusted = false;
            // Keep the selected origin visible; backend status confirms persisted consent.
        }
    }

    /// What the list of actions calls the plugin type `type_key`; the key itself when the
    /// plugin no longer offers it.
    pub(super) fn plugin_label(&self, type_key: &str) -> String {
        self.plugins
            .catalog
            .iter()
            .find(|entry| entry.type_key == type_key)
            .map(|entry| entry.label.clone())
            .unwrap_or_else(|| type_key.to_owned())
    }
    fn current_visual(&self) -> Option<String> {
        self.inspection
            .as_ref()
            .and_then(|i| i.appearance["visual_type"].as_str())
            .map(str::to_owned)
    }
    /// The appearance row has something to offer: a plugin that can draw keys, or one
    /// already drawing this key.
    pub(super) fn appearance_visible(&self) -> bool {
        self.connected
            && !self.plugins.form_pending
            && !self.plugins.form_loading
            && (self.current_visual().is_some()
                || self
                    .plugins
                    .catalog
                    .iter()
                    .any(|entry| entry.visual && entry.ready))
    }
    /// Who draws the key: Manual (its own text and image) or a plugin, chosen from one list;
    /// a plugin's settings open next to it.
    pub(super) fn appearance_control(&mut self, ui: &mut egui::Ui, id: &str) {
        let current = self.current_visual();
        let manual = tr(self.es, "Manual");
        let shown = current
            .as_deref()
            .map(|key| self.plugin_label(key))
            .unwrap_or_else(|| manual.to_owned());
        egui::ComboBox::from_id_salt("visual-source")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), manual).clicked() && current.is_some() {
                    self.send(crate::Request::Edit {
                        changes: vec![rad_api::Change::Set {
                            id: id.into(),
                            edit: rad_api::FieldEdit::Visual(rad_api::VisualSource {
                                type_key: "manual".into(),
                                parameters: None,
                            }),
                        }],
                    });
                }
                for entry in self
                    .plugins
                    .catalog
                    .clone()
                    .into_iter()
                    .filter(|entry| entry.visual && entry.ready)
                {
                    let chosen = current.as_deref() == Some(entry.type_key.as_str());
                    if ui.selectable_label(chosen, &entry.label).clicked() {
                        self.open_plugin_form(&entry.type_key, None, FormTarget::Visual(id.into()));
                    }
                }
            });
        if current.is_some() && ui.button(tr(self.es, "Configure…")).clicked() {
            self.open_plugin_form("", Some(id.into()), FormTarget::Visual(id.into()));
        }
    }
    pub(super) fn plugin_form_window(&mut self, ctx: &egui::Context) {
        let Some((target, mut loaded)) = self.plugins.form.take() else {
            if self.plugins.dependency_credentials.is_none() {
                self.credentials.clear();
            }
            return;
        };
        if let Some(form) = &loaded.form {
            self.credentials.bind(
                &format!("{target:?}:{}", loaded.type_key),
                &loaded.type_key,
                &form.definition.credential_slots,
            );
        } else {
            self.credentials.clear();
        }
        let look = Look::new(&self.palette);
        let mut open = true;
        let mut request = None;
        // Named after what is being set up, as the list of actions calls it.
        let title = if matches!(target, FormTarget::Settings(_)) {
            tr(self.es, "Plugin settings").to_owned()
        } else {
            self.plugin_label(&loaded.type_key)
        };
        egui::Window::new(title)
            .id(egui::Id::new("plugin-form"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .vscroll(true)
            .default_width(420.)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.collapsing(tr(self.es,"View details"), |ui| { ui.label(look.muted_text(&loaded.type_key).small()); });
                ui.add_space(4.);
                if !self.plugins.form_pending && self.connected {
                    if let Some(form) = loaded.form.as_mut() {
                        if form.compatible() {
                            form_ui(ui, &look, form, self.es);
                        } else {
                            ui.colored_label(look.warn, tr(self.es, "Incompatible parameters preserved; no migration. Remove explicitly if needed."));
                        }
                        ui.add_space(4.);
                        match form.publish() {
                            Ok(Some(value)) => {
                                if ui.add(look.primary(tr(self.es, "Apply"))).clicked() {
                                    request = Some(target.request(&loaded.type_key, value));
                                }
                            }
                            Ok(None) => {
                                ui.label(look.muted_text(tr(self.es, "Unchanged — nothing to publish")));
                            }
                            Err(_) => {
                                ui.colored_label(look.warn, tr(self.es, "Not saved: check required fields, types and limits"));
                            }
                        }
                    } else {
                        ui.colored_label(look.warn, tr(self.es, "Plugin unavailable. Original configuration retained."));
                    }
                }
                self.credentials.show(ui, &mut self.credential_worker, self.connected, self.es);
                if self.plugins.form_pending {
                    ui.label(look.muted_text(tr(self.es, "Saving — awaiting durable confirmation")));
                }
                // What was kept untouched only matters when the form cannot show it.
                if loaded.form.as_ref().is_none_or(|form| !form.compatible()) {
                    egui::CollapsingHeader::new(tr(self.es, "Preserved configuration (read only)"))
                        .show(ui, |ui| {
                            let mut raw = String::from_utf8_lossy(&loaded.original).into_owned();
                            ui.add(egui::TextEdit::multiline(&mut raw).interactive(false).desired_rows(5));
                            ui.label(look.muted_text(format!("{}: {}", tr(self.es, "Preserved bytes"), loaded.original.len())));
                        });
                }
            });
        if let Some(request) = request
            && self.command(Job::PluginApply(request))
        {
            self.plugins.form_pending = true;
        }
        if open {
            self.plugins.form = Some((target, loaded));
        } else {
            self.credentials.clear();
        }
    }
    pub(super) fn accept_plugin_visuals(
        &mut self,
        ctx: &egui::Context,
        revision: &str,
        visuals: Vec<Visual>,
    ) {
        if self
            .board
            .as_ref()
            .and_then(|board| board.folder["view_revision"].as_str())
            != Some(revision)
        {
            return;
        }
        let ids: std::collections::BTreeSet<_> = self
            .board
            .iter()
            .flat_map(|b| b.buttons.iter())
            .filter(|b| b.appearance["visual_type"].is_string())
            .map(|b| string(&b.entity["id"]))
            .collect();
        self.plugins.visuals.retain(|id, _| ids.contains(id));
        for v in visuals.into_iter().filter(|v| ids.contains(&v.id)) {
            if v.fault {
                if let Some(previous) = self.plugins.visuals.get_mut(&v.id) {
                    previous.state = "stale".into();
                }
                continue;
            }
            let texture = v.pixels.map(|p| {
                ctx.load_texture(
                    format!("plugin:{}:{}", v.id, v.revision),
                    egui::ColorImage::from_rgba_unmultiplied(p.size, &p.rgba),
                    Default::default(),
                )
            });
            let texture = texture.or_else(|| {
                if v.has_image && v.availability != "unavailable" {
                    self.plugins
                        .visuals
                        .get(&v.id)
                        .filter(|old| old.revision == v.revision)
                        .and_then(|old| old.texture.clone())
                } else {
                    None
                }
            });
            self.plugins.visuals.insert(
                v.id,
                Face {
                    revision: v.revision,
                    state: v.availability,
                    text: v.text,
                    position: v.text_position,
                    color: v.color,
                    texture,
                },
            );
        }
    }
    pub(super) fn plugin_face(
        &self,
        ui: &egui::Ui,
        _look: &super::look::Look,
        id: &str,
        rect: egui::Rect,
    ) -> bool {
        let external = self
            .board
            .iter()
            .flat_map(|b| &b.buttons)
            .any(|b| b.entity["id"] == id && b.appearance["visual_type"].is_string());
        if !external {
            return false;
        }
        let face = self.plugins.visuals.get(id);
        let state = face.map(|f| f.state.as_str()).unwrap_or("unavailable");
        let text = face.and_then(|f| f.text.as_deref()).unwrap_or("");
        let mut look = super::look::Look::new(&self.palette);
        if let Some(c) = face.and_then(|f| f.color) {
            look.key = egui::Color32::from_rgb(
                (u16::from(c[0]) * u16::from(c[3]) / 255) as u8,
                (u16::from(c[1]) * u16::from(c[3]) / 255) as u8,
                (u16::from(c[2]) * u16::from(c[3]) / 255) as u8,
            );
        }
        super::deck::paint_key(
            ui,
            &look,
            rect,
            text,
            face.map(|f| f.position.as_str()).unwrap_or("middle"),
            face.and_then(|f| f.texture.as_ref()),
        );
        if state != "current" {
            // A small tag in the corner: the face is not live, whatever it still shows.
            let painter = ui.painter().with_clip_rect(rect);
            let galley = painter.layout_no_wrap(
                label(state, self.es).to_owned(),
                egui::FontId::proportional(10.),
                look.warn,
            );
            let tag = egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(4., 4.),
                galley.size() + egui::vec2(8., 4.),
            );
            painter.rect_filled(
                tag,
                egui::CornerRadius::same(4),
                egui::Color32::from_black_alpha(170),
            );
            painter.galley(tag.min + egui::vec2(4., 2.), galley, look.warn);
        }
        true
    }
}
pub(super) fn form_ui(
    ui: &mut egui::Ui,
    look: &Look,
    form: &mut FormDraft,
    es: impl Into<crate::preferences::Language> + Copy,
) {
    use rad_plugin_contract::FieldKind;
    // Labels share one column so the controls line up.
    const LABEL: f32 = 120.;
    for (index, field) in form.definition.schema.fields.clone().iter().enumerate() {
        if form.settings && matches!(field.kind, FieldKind::DynamicChoice { .. }) {
            ui.label(format!(
                "{} — {}",
                field.label,
                tr(es, "This field is unavailable; its value is preserved")
            ));
            continue;
        }
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(LABEL, ui.spacing().interact_size.y),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(LABEL);
                        ui.label(look.muted_text(format!(
                            "{}{}",
                            field.label,
                            if field.required { " *" } else { "" }
                        )));
                    },
                );
                let mut present = form.fields[index].is_some();
                if !field.required && ui.checkbox(&mut present, tr(es, "Set")).changed() {
                    form.set(
                        index,
                        present.then(|| {
                            if matches!(field.kind, FieldKind::Boolean) {
                                "false".into()
                            } else {
                                String::new()
                            }
                        }),
                    );
                }
                if let Some(mut value) = form.fields[index].clone() {
                    let changed = match &field.kind {
                        FieldKind::Boolean => {
                            let mut v = value == "true";
                            let r = ui.checkbox(&mut v, tr(es, "Enabled"));
                            if r.changed() {
                                value = v.to_string();
                            }
                            r.changed()
                        }
                        FieldKind::Choice { options } => {
                            let old = value.clone();
                            egui::ComboBox::from_id_salt("choice")
                                .selected_text(if value.is_empty() {
                                    tr(es, "Choose…")
                                } else {
                                    &value
                                })
                                .show_ui(ui, |ui| {
                                    for option in options {
                                        ui.selectable_value(&mut value, option.clone(), option);
                                    }
                                });
                            old != value
                        }
                        FieldKind::Integer { min, max } => {
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut value)
                                    .char_limit(21)
                                    .desired_width(100.),
                            );
                            ui.label(look.muted_text(format!("{min}…{max}")).small());
                            r.changed()
                        }
                        FieldKind::Text { max_bytes } => {
                            let r = ui
                                .add(egui::TextEdit::singleline(&mut value).char_limit(*max_bytes))
                                .on_hover_text(
                                    tr(es, "≤{count} bytes")
                                        .replace("{count}", &max_bytes.to_string()),
                                );
                            r.changed()
                        }
                        FieldKind::DynamicChoice { .. } => {
                            super::options::dynamic_choice(ui, look, form, index, &mut value, es)
                        }
                    };
                    if changed {
                        form.set(index, Some(value));
                    }
                }
            });
            if matches!(field.kind, FieldKind::DynamicChoice { .. }) {
                let indent = LABEL + ui.spacing().item_spacing.x;
                super::options::list_status(ui, look, form, index, es, indent);
            }
        });
    }
}

#[cfg(test)]
mod administration_tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        plugins: &mut PluginUi,
        click: Option<egui::Pos2>,
    ) -> Vec<(String, egui::Rect)> {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900., 800.),
            )),
            ..Default::default()
        };
        if let Some(pos) = click {
            input.events = vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ];
        }
        let output = ctx.run_ui(input, |ui| {
            plugin_status_list(ui, plugins, true, false);
        });
        output
            .shapes
            .into_iter()
            .filter_map(|s| {
                if let egui::epaint::Shape::Text(text) = s.shape {
                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                    Some((text.galley.job.text.clone(), rect))
                } else {
                    None
                }
            })
            .collect()
    }
    #[test]
    fn resource_metadata_is_requested_only_from_details() {
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.0);
        ctx.style_mut_of(egui::Theme::Light, |style| style.animation_time = 0.0);
        let mut plugins = PluginUi {
            status: json!({"items":[{"id":"fixture","host":"ready"}]}),
            ..Default::default()
        };
        let text = frame(&ctx, &mut plugins, None);
        assert!(!text.iter().any(|(text, _)| text == "Declared resources"));
        let at = text
            .iter()
            .find(|(text, _)| text == "View details")
            .unwrap()
            .1
            .center();
        frame(&ctx, &mut plugins, Some(at));
        let text = frame(&ctx, &mut plugins, None);
        assert!(text.iter().any(|(text, _)| text == "Declared resources"));
    }
    #[test]
    fn resource_details_are_bounded_read_only_and_exclude_other_manifest_fields() {
        use rad_plugin_contract::Resource;
        let mut manifest: rad_plugin_contract::Manifest = serde_json::from_str(include_str!(
            "../../../rad-plugin-host/fixture/manifest-v1.json"
        ))
        .unwrap();
        manifest.resources = vec![
            Resource::Internet,
            Resource::SystemPrograms,
            Resource::ExternalFiles,
            Resource::Devices,
            Resource::DesktopServices,
        ];
        manifest.actions[0].credential_slots = vec!["PRIVATE_SENTINEL".into()];
        let mut plugins = PluginUi {
            status: json!({"items":[{"id":manifest.id,"selected":manifest.version,"host":"ready"}]}),
            ..Default::default()
        };
        let view = |manifest: &rad_plugin_contract::Manifest| rad_api::PluginManifestView {
            plugin_id: manifest.id.clone(),
            document: serde_json::from_value(serde_json::to_value(manifest).unwrap()).unwrap(),
        };
        plugins.accept_manifest(view(&manifest)).unwrap();
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.0);
        ctx.style_mut_of(egui::Theme::Light, |style| style.animation_time = 0.0);
        let text = frame(&ctx, &mut plugins, None);
        let at = text
            .iter()
            .find(|(text, _)| text == "View details")
            .unwrap()
            .1
            .center();
        frame(&ctx, &mut plugins, Some(at));
        let text = frame(&ctx, &mut plugins, None);
        assert!(text.iter().any(|(text, _)| text == "Internet"));
        assert!(
            text.iter().any(|(text, _)| text
                == "Declared by the author; informational, not enforced permissions")
        );
        assert!(
            !text
                .iter()
                .any(|(text, _)| text.contains("PRIVATE_SENTINEL"))
        );
        assert!(plugins.manifest_request.is_none());
        manifest.resources.push(Resource::Internet);
        assert!(plugins.accept_manifest(view(&manifest)).is_err());
        plugins.resources.clear();
        manifest.resources.pop();
        manifest.version.patch += 1;
        plugins.accept_manifest(view(&manifest)).unwrap();
        assert!(
            plugins.resources.is_empty(),
            "a stale selection cannot populate the cache"
        );
    }
    #[test]
    fn retry_is_shown_only_after_runtime_allows_it_and_click_is_explicit() {
        let ctx = egui::Context::default();
        let mut plugins = PluginUi {
            status: json!({"items":[{"id":"fixture","host":"failed","can_retry":false}]}),
            ..Default::default()
        };
        assert!(
            !frame(&ctx, &mut plugins, None)
                .iter()
                .any(|(text, _)| text == "Retry")
        );
        plugins.status["items"][0]["can_retry"] = json!(true);
        let text = frame(&ctx, &mut plugins, None);
        assert!(plugins.retry_request.is_none());
        let at = text
            .iter()
            .find(|(text, _)| text == "Retry")
            .unwrap()
            .1
            .center();
        frame(&ctx, &mut plugins, Some(at));
        assert_eq!(plugins.retry_request.as_deref(), Some("fixture"));
        assert!(!plugins.update_request);
        assert!(!plugins.admin_pending);
    }
    #[test]
    fn locked_admin_hides_mutations_and_reuses_only_selected_source_trust() {
        let mut p = PluginUi {
            status: json!({"state":"failed","locked":true,"items":[{"id":"p","source":"https://github.com/o/r","source_trusted":true}]}),
            ..Default::default()
        };
        assert!(!p.can_administer(true));
        p.status["locked"] = json!(false);
        assert!(p.can_administer(true));
        p.select("p");
        assert_eq!(p.source, "https://github.com/o/r");
        assert!(p.trusted_update());
        p.source.push_str("/releases/tag/v2");
        assert!(!p.trusted_update());
        assert!(!p.trusted);
    }
    #[test]
    fn every_installed_plugin_state_and_selector_is_reachable_with_bounded_pages() {
        let ctx = egui::Context::default();
        let mut plugins = PluginUi {
            status: json!({"items": (1..=32).map(|i| json!({
            "id":format!("plugin{i}"), "selected":{"major":i,"minor":0,"patch":0},
            "previous":{"major":0,"minor":0,"patch":1}, "host":"failed"
        })).collect::<Vec<_>>() }),
            ..Default::default()
        };
        let mut text = frame(&ctx, &mut plugins, None);
        for first in [1, 9, 17, 25] {
            for id in first..first + 8 {
                assert!(
                    text.iter().any(|(s, _)| s == &format!("plugin{id}")),
                    "missing plugin{id}"
                );
                assert!(text.iter().any(|(s, _)| { s == "Not working" }));
            }
            assert!(!text.iter().any(|(s, _)| s.starts_with("Selected ")));
            assert_eq!(
                text.iter().filter(|(s, _)| s.starts_with("plugin")).count(),
                8
            );
            let at = text
                .iter()
                .find(|(s, _)| s == &format!("plugin{first}"))
                .unwrap()
                .1
                .center();
            text = frame(&ctx, &mut plugins, Some(at));
            assert_eq!(plugins.selected, format!("plugin{first}"));
            if first != 25 {
                let next = text
                    .iter()
                    .find(|(s, _)| s == "Next")
                    .expect("all installed plugins must be reachable, not truncated")
                    .1
                    .center();
                frame(&ctx, &mut plugins, Some(next));
                text = frame(&ctx, &mut plugins, None);
            }
        }
        assert!(!text.iter().any(|(s, _)| s == "Next"));
        plugins.status["items"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"plugin33"}));
        text = frame(&ctx, &mut plugins, None);
        assert!(
            text.iter()
                .any(|(s, _)| s == "Invalid plugin status: more than 32 entries")
        );
        assert!(!text.iter().any(|(s, _)| s.starts_with("plugin")));
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;
    #[test]
    fn declared_credentials_are_visible_separately_from_ordinary_fields() {
        let definition = rad_plugin_contract::Definition {
            gestures: rad_plugin_contract::default_gestures(),
            id: "authenticate".into(),
            label: "Authenticate".into(),
            schema: rad_plugin_contract::Schema {
                version: 1,
                fields: vec![],
            },
            credential_slots: vec!["password".into()],
            response_timeout: None,
        };
        let mut form = FormDraft::new(definition);
        let ctx = egui::Context::default();
        let mut panel = super::super::credentials::Panel::default();
        panel.bind("first", "p:authenticate", &form.definition.credential_slots);
        let mut worker = super::super::credentials::start(std::path::PathBuf::from("/unused"));
        let output = ctx.run_ui(Default::default(), |ui| {
            form_ui(ui, &Look::new(&Default::default()), &mut form, false);
            panel.show(ui, &mut worker, false, false);
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
        assert!(texts.contains(&"Credentials — saved separately"));
        assert!(texts.contains(&"password"));
        assert_eq!(form.publish().unwrap().unwrap()["values"], json!({}));
    }
}

#[cfg(test)]
mod dependency_tests {
    use super::*;
    fn dependency_frame(
        ctx: &egui::Context,
        plugins: &mut PluginUi,
        click: Option<egui::Pos2>,
    ) -> Vec<(String, egui::Rect)> {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900., 800.),
            )),
            ..Default::default()
        };
        if let Some(pos) = click {
            input.events = vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ];
        }
        let output = ctx.run_ui(input, |ui| {
            dependency_list(ui, plugins, true, false);
        });
        output
            .shapes
            .into_iter()
            .filter_map(|s| {
                if let egui::epaint::Shape::Text(text) = s.shape {
                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                    Some((text.galley.job.text.clone(), rect))
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn password_badge_requires_installed_status_and_slots_never_imported_hints() {
        let ctx = egui::Context::default();
        let mut plugins = PluginUi {
            status: json!({"items":[],"state":"idle"}),
            ..Default::default()
        };
        plugins.dependencies.accept(json!({"view_revision":"1","items":[{"id":"p","version":null,"origin":null,"credential_slots":["password"],"needs_credentials":true}],"next_cursor":null})).unwrap();
        let text = dependency_frame(&ctx, &mut plugins, None);
        assert!(
            !text
                .iter()
                .any(|(text, _)| text == "Needs password" || text == "Enter password")
        );
        plugins.status = json!({"items":[{"id":"p","needs_credentials":false}],"state":"idle"});
        let text = dependency_frame(&ctx, &mut plugins, None);
        assert!(!text.iter().any(|(text, _)| text == "Needs password"));
        plugins.status["items"][0]["needs_credentials"] = json!(true);
        let text = dependency_frame(&ctx, &mut plugins, None);
        assert!(text.iter().any(|(text, _)| text == "Needs password"));
        assert!(text.iter().any(|(text, _)| text == "Enter password"));
    }

    #[test]
    fn every_missing_dependency_is_offered_with_explicit_untrusted_source() {
        let ctx = egui::Context::default();
        let mut plugins = PluginUi {
            status: json!({"items":[],"state":"idle"}),
            trusted: true,
            ..Default::default()
        };
        let items: Vec<_> = (0..33).map(|i|json!({"id":format!("missing{i}"),"version":null,"origin":if i == 0 {Some("https://github.com/example/plugin")}else{None},"credential_slots":[],"needs_credentials":null})).collect();
        plugins
            .dependencies
            .accept(json!({"view_revision":"1","items":items[..32],"next_cursor":"next"}))
            .unwrap();
        plugins
            .dependencies
            .accept(json!({"view_revision":"1","items":items[32..],"next_cursor":null}))
            .unwrap();
        let mut text = dependency_frame(&ctx, &mut plugins, None);
        assert!(
            text.iter().any(|(s, _)| s == "missing0"),
            "missing plugin not offered"
        );
        let install = text
            .iter()
            .find(|(s, _)| s == "Prepare installation")
            .unwrap()
            .1
            .center();
        dependency_frame(&ctx, &mut plugins, Some(install));
        assert_eq!(plugins.source, "https://github.com/example/plugin");
        assert!(!plugins.trusted);
        assert!(plugins.source_edited);
        for first in [0, 8, 16, 24, 32] {
            text = dependency_frame(&ctx, &mut plugins, None);
            for i in first..(first + 8).min(33) {
                assert!(text.iter().any(|(s, _)| s == &format!("missing{i}")));
            }
            assert!(text.iter().any(|(s, _)| s.contains("Version unknown")));
            assert!(!text.iter().any(|(s, _)| s == "Needs password"));
            if first < 32 {
                let next = text.iter().find(|(s, _)| s == "Next").unwrap().1.center();
                dependency_frame(&ctx, &mut plugins, Some(next));
            }
        }
        assert!(!text.iter().any(|(s, _)| s == "Prepare installation"));
        assert!(text.iter().any(|(s, _)| s.contains("Source unknown")));
    }
}

#[cfg(test)]
mod restored_tests {
    use super::*;
    #[test]
    fn only_applied_restore_and_reconnect_reload_published_dependencies_without_reusing_trust() {
        let mut p = PluginUi {
            source: "https://github.com/example/old".into(),
            trusted: true,
            ..Default::default()
        };
        p.dependencies
            .accept(json!({"view_revision":"1","items":[],"next_cursor":null}))
            .unwrap();
        assert!(!p.restore_result(rad_client::response::Status::Ok));
        assert!(!p.restore_result(rad_client::response::Status::Error));
        assert!(p.dependencies.request().is_none());
        assert!(p.restore_result(rad_client::response::Status::Applied));
        assert_eq!(
            p.dependencies.request().unwrap().operation().as_str(),
            "plugin_dependencies"
        );
        assert!(!p.trusted);
        assert!(p.source.is_empty());
        p.dependencies
            .accept(json!({"view_revision":"2","items":[],"next_cursor":null}))
            .unwrap();
        p.reconnect();
        assert_eq!(
            p.dependencies.request().unwrap().arguments(),
            json!({"cursor":null})
        );
        assert!(p.dependency_status_due);
    }
    #[test]
    fn imported_sources_are_only_supported_public_links_normalized_without_execution() {
        for unsupported in [
            "",
            "file:///tmp/plugin",
            "https://evil.test/o/r",
            "https://github.com/o/r?secret=x",
            "https://github.com/o/../releases",
            "https://github.com/o/r/releases/tag/x/y",
        ] {
            assert!(installation_source(unsupported).is_none());
        }
        assert_eq!(
            installation_source("https://github.com/o/r.git/releases/latest/").as_deref(),
            Some("https://github.com/o/r")
        );
        assert_eq!(
            installation_source("https://github.com/o/r/releases/tag/v2/").as_deref(),
            Some("https://github.com/o/r/releases/tag/v2")
        );
    }
}
