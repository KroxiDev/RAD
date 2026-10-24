//! Plugin presentation only; publication/activation belong to the runtime.
use super::{
    App, Job,
    look::{Icon, Look, icon_button},
    string, tr,
};
use crate::plugins::{CatalogEntry, FormDraft, FormTarget, LoadedForm, SourceChange, Visual};
use eframe::egui;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::collections::BTreeMap;
/// Width of the list of installed plugins; the chosen one takes the rest.
const LIST_WIDTH: f32 = 280.;
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
    // Only one load is admitted until its result is consumed, even if invalidated.
    form_load_current: bool,
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
    password_request: Option<(String, Vec<String>)>,
    update_request: bool,
    retry_request: Option<String>,
    /// Remove, reinstall, roll back or install from a link: clicked, not yet sent.
    operation_request: Option<&'static str>,
    /// The current release of an official plugin, clicked to install.
    official_install: Option<&'static str>,
    /// The user agreed to the origin change on display.
    confirmed: bool,
    /// The plugin whose settings were last asked for, so they are asked once per visit.
    pub settings_asked: Option<String>,
    /// An update or reinstall waiting for the user to agree to a different origin.
    confirmation: Option<Confirmation>,
    /// The last operation sent to the administration, to repeat it once confirmed.
    submitted: Option<&'static str>,
    schedule: crate::schedule::Schedule,
}
/// An origin change the user is looking at, for the plugin and link it was raised for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Confirmation {
    operation: &'static str,
    plugin: String,
    source: String,
    change: SourceChange,
}
impl PluginUi {
    /// Invalidating a load does not release its credit until the result is consumed.
    pub fn clear_form(&mut self) {
        self.form = None;
        self.form_load_current = false;
    }
    pub fn accept_form(&mut self, target: FormTarget, loaded: LoadedForm) {
        self.form_loading = false;
        if std::mem::take(&mut self.form_load_current) {
            self.form = Some((target, loaded));
        }
    }
    /// The worker died with these requests unanswered. None is repeated: whatever depended on
    /// an answer stops waiting for it, and the state is read again.
    pub fn abandon_pending(&mut self) {
        if self.form_pending
            && let Some((FormTarget::Settings(id), _)) = self.form.as_ref()
        {
            self.settings_reload = Some(id.clone());
            self.form = None;
        }
        self.form_pending = false;
        self.form_loading = false;
        self.form_load_current = false;
        self.admin_pending = false;
        self.manifest_pending = None;
        self.manifest_invalidated = false;
        self.dependencies.reset();
        self.dependency_status_due = true;
        for face in self.visuals.values_mut() {
            face.state = "stale".into();
        }
    }
    pub fn reconnect(&mut self) {
        self.dependencies.reset();
        self.dependency_status_due = true;
        self.status = Value::Null;
        self.catalog_loaded = false;
        self.trusted = false;
        self.confirmation = None;
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
    /// «Add plugin» instead of an installed one, with an empty link.
    fn add(&mut self) {
        self.confirmation = None;
        self.selected.clear();
        self.source.clear();
        self.source_edited = true;
        self.trusted = false;
    }
    fn select(&mut self, id: &str) {
        self.confirmation = None;
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
    /// What the administration should receive for `operation`, or `None` when the user must
    /// first agree to an update that comes from another origin than the registered one.
    fn next_admin_request(&mut self, operation: &'static str) -> Option<crate::Request> {
        if matches!(operation, "update" | "reinstall")
            && let Some(change) = SourceChange::between(
                self.selected_item()
                    .and_then(|item| item["source"].as_str()),
                &self.source,
            )
        {
            self.confirmation = Some(Confirmation {
                operation,
                plugin: self.selected.clone(),
                source: self.source.clone(),
                change,
            });
            return None;
        }
        Some(self.admin_request(operation, None))
    }
    /// The origin change on display, while the plugin and link it was raised for are still
    /// the ones selected and typed.
    fn pending_confirmation(&self) -> Option<&Confirmation> {
        self.confirmation
            .as_ref()
            .filter(|c| c.plugin == self.selected && c.source == self.source)
    }
    /// The user agreed to the origin change on display: the request that carries it.
    fn confirm(&mut self) -> Option<crate::Request> {
        let confirmation = self.pending_confirmation()?.clone();
        self.confirmation = None;
        Some(self.admin_request(confirmation.operation, Some(&confirmation.change)))
    }
    /// The runtime refused an update because the origin changed; ask about the two it names.
    pub(super) fn source_refused(&mut self, change: SourceChange) {
        if let Some(operation) = self.submitted {
            self.confirmation = Some(Confirmation {
                operation,
                plugin: self.selected.clone(),
                source: self.source.clone(),
                change,
            });
        }
    }
    fn admin_request(&self, operation: &str, confirmed: Option<&SourceChange>) -> crate::Request {
        let remote = matches!(operation, "install" | "update" | "reinstall");
        let operation_code = match operation {
            "install" => rad_api::PluginOperation::Install,
            "update" => rad_api::PluginOperation::Update,
            "reinstall" => rad_api::PluginOperation::Reinstall,
            "rollback" => rad_api::PluginOperation::Rollback,
            _ => rad_api::PluginOperation::Uninstall,
        };
        crate::plugins::admin_request(
            operation_code,
            (operation != "install").then(|| self.selected.clone()),
            remote.then(|| self.source.trim().to_owned()),
            !remote
                || self.trusted
                || (operation == "update" && self.trusted_update())
                || confirmed.is_some(),
            confirmed,
        )
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
/// What the list and the detail call a plugin: an official one by its name, any other by
/// the name it declares or the last part of its ID.
pub(super) fn plugin_name(
    item: &Value,
    es: impl Into<crate::preferences::Language> + Copy,
) -> String {
    let id = item["id"].as_str().unwrap_or("");
    if let Some(official) = crate::official::find(id) {
        return tr(es, official.name).to_owned();
    }
    item["name"]
        .as_str()
        .or_else(|| id.rsplit('.').next())
        .unwrap_or(id)
        .to_owned()
}
/// Where an update comes from: an official plugin's current release when it is newer or
/// needed, any other plugin's trusted registered origin when it needs one. `true` marks the
/// official release, which the click itself trusts.
fn update_source(item: &Value) -> Option<(String, bool)> {
    let needed = crate::status::PluginState::of(item) == crate::status::PluginState::NeedsUpdate;
    if let Some(official) = item["id"].as_str().and_then(crate::official::find) {
        return (needed || official.newer_than(&item["selected"]))
            .then(|| (official.source.to_owned(), true));
    }
    (needed && item["source_trusted"] == true)
        .then(|| item["source"].as_str().and_then(installation_source))
        .flatten()
        .map(|source| (source, false))
}
/// An operation under way: how far its download is, or only that it is working.
fn operation_progress(
    ui: &mut egui::Ui,
    status: &Value,
    es: impl Into<crate::preferences::Language> + Copy,
) {
    if matches!(status["state"].as_str(), None | Some("idle" | "failed")) {
        return;
    }
    let bar = match status["progress"].as_u64() {
        Some(percent) => egui::ProgressBar::new(percent.min(100) as f32 / 100.)
            .text(tr(es, "Downloading {percent} %").replace("{percent}", &percent.to_string())),
        None => egui::ProgressBar::new(0.)
            .animate(true)
            .text(tr(es, "Working…")),
    };
    ui.add(bar);
    ui.add_space(6.);
}
/// Left column: «Add plugin», then every installed plugin with its state and version. The
/// manager admits at most 32; the list scrolls, so none is out of reach.
fn plugin_list(
    ui: &mut egui::Ui,
    plugins: &mut PluginUi,
    es: impl Into<crate::preferences::Language> + Copy,
) -> bool {
    const MAX_PLUGINS: usize = 32;
    let items = plugins.status["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if items.len() > MAX_PLUGINS {
        ui.label(tr(es, "Invalid plugin status: more than 32 entries"));
        return false;
    }
    let row = egui::vec2(ui.available_width(), 30.);
    let adding = plugins.selected_item().is_none();
    if ui
        .add(egui::Button::selectable(adding, tr(es, "+ Add plugin")).min_size(row))
        .clicked()
    {
        plugins.add();
    }
    ui.add_space(4.);
    egui::ScrollArea::vertical()
        .id_salt("plugin-list")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for item in &items {
                let id = string(&item["id"]);
                let state = crate::status::PluginState::of(item);
                let detail = format!("{} · {}", state.label(es), version(&item["selected"]));
                let button = egui::Button::selectable(
                    !adding && plugins.selected == id,
                    plugin_name(item, es),
                )
                .right_text(egui::RichText::new(detail).weak())
                .min_size(row);
                if ui.add(button).clicked() {
                    plugins.select(&id);
                }
            }
        });
    true
}
/// Top of the chosen plugin: its name, state and version, the action its state asks for,
/// and the question when an update would come from another origin.
fn plugin_summary(
    ui: &mut egui::Ui,
    plugins: &mut PluginUi,
    idle: bool,
    es: impl Into<crate::preferences::Language> + Copy,
) {
    let Some(item) = plugins.selected_item().cloned() else {
        return;
    };
    let id = string(&item["id"]);
    let state = crate::status::PluginState::of(&item);
    ui.heading(plugin_name(&item, es));
    ui.label(
        egui::RichText::new(format!(
            "{} · {}",
            state.label(es),
            version(&item["selected"])
        ))
        .weak(),
    );
    if let Some(asked) = plugins.pending_confirmation().cloned() {
        ui.label(
            tr(
                es,
                "This plugin comes from {registered}, but the package you chose is from {requested}. Continue only if you trust the new source.",
            )
            .replace("{registered}", &asked.change.registered)
            .replace("{requested}", &asked.change.requested),
        );
        ui.horizontal(|ui| {
            if ui.button(tr(es, "Confirm")).clicked() {
                plugins.confirmed = true;
            }
            if ui.button(tr(es, "Cancel")).clicked() {
                plugins.confirmation = None;
            }
        });
        return;
    }
    if !idle {
        return;
    }
    ui.horizontal(|ui| {
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
            _ => {}
        }
        if let Some((source, official)) = update_source(&item)
            && ui.button(tr(es, "Update")).clicked()
        {
            plugins.source = source;
            plugins.source_edited = official;
            plugins.trusted = official;
            plugins.update_request = true;
        }
    });
}
/// Bottom of the chosen plugin: Remove, and its details with the rarely needed operations.
fn plugin_more(
    ui: &mut egui::Ui,
    look: &Look,
    plugins: &mut PluginUi,
    idle: bool,
    es: impl Into<crate::preferences::Language> + Copy,
) {
    let Some(item) = plugins.selected_item().cloned() else {
        return;
    };
    let id = string(&item["id"]);
    ui.add_space(8.);
    if idle && ui.add(look.dangerous(tr(es, "Remove"))).clicked() {
        plugins.operation_request = Some("uninstall");
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
        if let Some(source) = item["source"].as_str() {
            ui.label(source);
        }
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
        if !idle {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            let registered = item["source"].as_str().and_then(installation_source);
            if item["source_trusted"] == true
                && let Some(source) = registered
                && ui.button(tr(es, "Reinstall")).clicked()
            {
                plugins.source = source;
                plugins.trusted = true;
                plugins.operation_request = Some("reinstall");
            }
            if !item["previous"].is_null() && ui.button(tr(es, "Rollback")).clicked() {
                plugins.operation_request = Some("rollback");
            }
        });
    });
}
/// «Add plugin»: each official plugin with one button, or another one from its link.
fn add_plugin(
    ui: &mut egui::Ui,
    look: &Look,
    plugins: &mut PluginUi,
    idle: bool,
    es: impl Into<crate::preferences::Language> + Copy,
) {
    ui.heading(tr(es, "Add plugin"));
    ui.add_space(6.);
    ui.label(look.section(tr(es, "Official plugins")));
    for official in &crate::official::OFFICIAL {
        let installed = plugins.status["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["id"] == official.id));
        ui.horizontal(|ui| {
            ui.label(tr(es, official.name));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if installed {
                    ui.label(look.muted_text(tr(es, "Installed")));
                } else if idle && ui.add(look.primary(tr(es, "Install"))).clicked() {
                    plugins.official_install = Some(official.source);
                }
            });
        });
    }
    ui.add_space(12.);
    ui.label(look.section(tr(es, "Another plugin")));
    ui.label(look.muted_text(tr(es, "Public GitHub repository / release URL")));
    if ui
        .add(
            egui::TextEdit::singleline(&mut plugins.source)
                .char_limit(2048)
                .desired_width(f32::INFINITY),
        )
        .changed()
    {
        plugins.trusted = false;
        plugins.source_edited = true;
        plugins.confirmation = None;
    }
    ui.checkbox(
        &mut plugins.trusted,
        tr(es, "I trust this native code (not sandboxed)"),
    );
    let ready = idle && plugins.trusted && installation_source(plugins.source.trim()).is_some();
    if ui
        .add_enabled(ready, egui::Button::new(tr(es, "Install")))
        .clicked()
    {
        plugins.operation_request = Some("install");
    }
}
/// Installing an official plugin: its current release, trusted by the click itself.
fn official_request(source: &str) -> crate::Request {
    crate::plugins::admin_request(
        rad_api::PluginOperation::Install,
        None,
        Some(source.to_owned()),
        true,
        None,
    )
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
            self.plugins.form_load_current = true;
            self.form_leave.reset();
        }
    }
    /// The Plugins section of the settings: the installed plugins on the left, the chosen
    /// one (or «Add plugin») on the right.
    pub(super) fn plugin_settings(&mut self, ui: &mut egui::Ui, look: &Look) {
        operation_progress(ui, &self.plugins.status, self.es);
        let idle = self.plugins.can_administer(self.connected);
        if let Some((id, slots)) = dependency_list(ui, &mut self.plugins, idle, self.es) {
            // The password is typed next to the plugin that asks for it.
            self.plugins.select(&id);
            self.plugins.password_request = Some((id, slots));
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
        if self.plugins.status["locked"] == true {
            ui.label(tr(self.es, "Restart RAD to recover plugin administration"));
        }
        if let Some(code) = self.plugins.status["error"].as_str() {
            ui.label(crate::status::describe_error(code, self.es));
            ui.collapsing(tr(self.es, "View details"), |ui| {
                ui.label(code);
            });
        }
        let height = ui.available_height();
        let mut listed = true;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(LIST_WIDTH, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(LIST_WIDTH);
                    listed = plugin_list(ui, &mut self.plugins, self.es);
                },
            );
            ui.separator();
            ui.vertical(|ui| {
                egui::ScrollArea::vertical()
                    .id_salt("plugin-detail")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        if !listed {
                            return;
                        }
                        if self.plugins.selected_item().is_none() {
                            add_plugin(ui, look, &mut self.plugins, idle, self.es);
                        } else {
                            self.plugin_detail(ui, look, idle);
                        }
                    });
            });
        });
        self.plugin_intents();
    }
    /// The chosen plugin: summary, password or settings, then Remove and details.
    fn plugin_detail(&mut self, ui: &mut egui::Ui, look: &Look, idle: bool) {
        plugin_summary(ui, &mut self.plugins, idle, self.es);
        let id = self.plugins.selected.clone();
        // A plugin's settings belong to it: choosing another one closes them.
        let shown = match &self.plugins.form {
            Some((FormTarget::Settings(form), _)) => Some(form.clone()),
            _ => None,
        };
        if shown.as_ref().is_some_and(|form| *form != id) {
            self.plugins.clear_form();
            self.credentials.clear();
        }
        if self.plugins.dependency_credentials.as_ref() == Some(&id) {
            ui.add_space(8.);
            self.credentials
                .show(ui, &mut self.credential_worker, self.connected, self.es);
            if ui.button(tr(self.es, "Close credentials")).clicked() {
                self.credentials.clear();
                self.plugins.dependency_credentials = None;
                self.plugins.settings_asked = None;
            }
        } else if shown.as_ref() == Some(&id) {
            ui.add_space(8.);
            ui.label(look.section(tr(self.es, "Plugin settings")));
            self.plugin_form_ui(ui, look);
        } else if idle
            && self.connected
            && self.plugins.settings_asked.as_ref() != Some(&id)
            && self
                .plugins
                .selected_item()
                .is_some_and(|item| item["has_settings"] == true)
        {
            // Its settings are shown with it, asked for once per visit.
            self.plugins.settings_asked = Some(id.clone());
            self.open_plugin_form(&id, None, FormTarget::Settings(id.clone()));
        }
        plugin_more(ui, look, &mut self.plugins, idle, self.es);
    }
    /// What the clicks of this frame asked for, sent once drawing is over.
    fn plugin_intents(&mut self) {
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
        if let Some((id, slots)) = self.plugins.password_request.take() {
            self.plugins.clear_form();
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
        if let Some(operation) = self.plugins.operation_request.take() {
            self.submit_plugin_operation(operation);
        }
        if let Some(source) = self.plugins.official_install.take() {
            self.send_admin("install", official_request(source));
        }
        if std::mem::take(&mut self.plugins.confirmed)
            && let Some(operation) = self.plugins.pending_confirmation().map(|c| c.operation)
            && let Some(request) = self.plugins.confirm()
        {
            self.send_admin(operation, request);
        }
    }
    fn submit_plugin_operation(&mut self, operation: &'static str) {
        if let Some(request) = self.plugins.next_admin_request(operation) {
            self.send_admin(operation, request);
        }
    }
    fn send_admin(&mut self, operation: &'static str, request: crate::Request) {
        if self.command(Job::Request(request)) {
            self.plugins.admin_pending = true;
            self.plugins.trusted = false;
            self.plugins.submitted = Some(operation);
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
    /// The open plugin form, drawn in place: its fields, Apply and the credentials it declares.
    pub(super) fn plugin_form_ui(&mut self, ui: &mut egui::Ui, look: &Look) {
        let Some((target, mut loaded)) = self.plugins.form.take() else {
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
        let mut request = None;
        if !self.plugins.form_pending && self.connected {
            if let Some(form) = loaded.form.as_mut() {
                if form.compatible() {
                    form_ui(ui, look, form, self.es);
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
                        ui.colored_label(
                            look.warn,
                            tr(
                                self.es,
                                "Not saved: check required fields, types and limits",
                            ),
                        );
                    }
                }
            } else {
                ui.colored_label(
                    look.warn,
                    tr(
                        self.es,
                        "Plugin unavailable. Original configuration retained.",
                    ),
                );
            }
        }
        self.credentials
            .show(ui, &mut self.credential_worker, self.connected, self.es);
        if self.plugins.form_pending {
            ui.label(look.muted_text(tr(self.es, "Saving — awaiting durable confirmation")));
        }
        // What was kept untouched only matters when the form cannot show it.
        if loaded.form.as_ref().is_none_or(|form| !form.compatible()) {
            egui::CollapsingHeader::new(tr(self.es, "Preserved configuration (read only)")).show(
                ui,
                |ui| {
                    let mut raw = String::from_utf8_lossy(&loaded.original).into_owned();
                    ui.add(
                        egui::TextEdit::multiline(&mut raw)
                            .interactive(false)
                            .desired_rows(5),
                    );
                    ui.label(look.muted_text(format!(
                        "{}: {}",
                        tr(self.es, "Preserved bytes"),
                        loaded.original.len()
                    )));
                },
            );
        }
        ui.collapsing(tr(self.es, "View details"), |ui| {
            ui.label(look.muted_text(&loaded.type_key).small());
        });
        if let Some(request) = request
            && self.command(Job::PluginApply(request))
        {
            self.plugins.form_pending = true;
        }
        self.plugins.form = Some((target, loaded));
    }
    /// A form of the selected key (an action's parameters, a new action or who draws the
    /// key) is open in the key panel.
    pub(super) fn key_form_open(&self) -> bool {
        self.plugins
            .form
            .as_ref()
            .is_some_and(|(target, _)| !matches!(target, FormTarget::Settings(_)))
    }
    fn key_form_unapplied(&self) -> bool {
        self.plugins
            .form
            .as_ref()
            .is_some_and(|(_, loaded)| loaded.form.as_ref().is_some_and(|f| f.dirty))
            || self.credentials.editing()
    }
    /// Back or Esc in the key panel: to the key, unless something unapplied makes it ask.
    pub(super) fn leave_key_form(&mut self) {
        if self.form_leave.request(self.key_form_unapplied()) {
            self.close_form();
        }
    }
    fn close_form(&mut self) {
        self.plugins.clear_form();
        self.credentials.clear();
        self.form_leave.reset();
    }
    /// The key panel while a form of its key is open: Back, the form, and the question when
    /// leaving would lose something.
    pub(super) fn key_form(&mut self, ui: &mut egui::Ui, look: &Look) {
        let title = self
            .plugins
            .form
            .as_ref()
            .map(|(_, loaded)| self.plugin_label(&loaded.type_key))
            .unwrap_or_default();
        ui.horizontal(|ui| {
            if icon_button(ui, look, Icon::Back, tr(self.es, "Back (Esc)")).clicked() {
                self.leave_key_form();
            }
            ui.label(egui::RichText::new(title).heading());
        });
        if self.form_leave.asking(self.key_form_unapplied()) {
            ui.horizontal(|ui| {
                ui.colored_label(look.warn, tr(self.es, "Some changes are not applied"));
                if ui.add(look.dangerous(tr(self.es, "Discard"))).clicked() {
                    self.close_form();
                }
            });
        }
        ui.add_space(4.);
        self.plugin_form_ui(ui, look);
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
            false,
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

    fn click(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
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
        ]
    }
    /// One frame of the Plugins section without the App: the list, and the chosen plugin or
    /// «Add plugin». Returns every text drawn, with where.
    fn frame_with(
        ctx: &egui::Context,
        plugins: &mut PluginUi,
        events: Vec<egui::Event>,
        height: f32,
    ) -> Vec<(String, egui::Rect)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900., height),
            )),
            events,
            ..Default::default()
        };
        let look = Look::new(&Default::default());
        let output = ctx.run_ui(input, |ui| {
            operation_progress(ui, &plugins.status.clone(), false);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(LIST_WIDTH, height - 40.),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(LIST_WIDTH);
                        plugin_list(ui, plugins, false);
                    },
                );
                ui.vertical(|ui| {
                    if plugins.selected_item().is_none() {
                        add_plugin(ui, &look, plugins, true, false);
                    } else {
                        plugin_summary(ui, plugins, true, false);
                        plugin_more(ui, &look, plugins, true, false);
                    }
                });
            });
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
    fn frame(
        ctx: &egui::Context,
        plugins: &mut PluginUi,
        at: Option<egui::Pos2>,
    ) -> Vec<(String, egui::Rect)> {
        frame_with(ctx, plugins, at.map(click).unwrap_or_default(), 800.)
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
        plugins.select("fixture");
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
        plugins.select(&manifest.id);
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
        plugins.select("fixture");
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
    fn synthetic(count: usize) -> Value {
        json!({"items": (1..=count).map(|i| json!({
            "id":format!("plugin{i}"), "selected":{"major":i,"minor":0,"patch":0},
            "previous":{"major":0,"minor":0,"patch":1}, "host":"failed"
        })).collect::<Vec<_>>() })
    }
    #[test]
    fn every_installed_plugin_is_reachable_by_scrolling_the_list_with_8_and_32() {
        for count in [8, 32] {
            let ctx = egui::Context::default();
            ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.0);
            let mut plugins = PluginUi {
                status: synthetic(count),
                ..Default::default()
            };
            let name = |i: usize| format!("plugin{i}");
            let mut seen = std::collections::BTreeSet::new();
            let mut text = frame_with(&ctx, &mut plugins, vec![], 500.);
            for _ in 0..40 {
                seen.extend(
                    text.iter()
                        .filter(|(s, _)| s.starts_with("plugin"))
                        .map(|(s, _)| s.clone()),
                );
                if seen.contains(&name(count)) {
                    break;
                }
                let list = egui::pos2(LIST_WIDTH / 2., 250.);
                text = frame_with(
                    &ctx,
                    &mut plugins,
                    vec![
                        egui::Event::PointerMoved(list),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0., -120.),
                            phase: egui::TouchPhase::Move,
                            modifiers: Default::default(),
                        },
                    ],
                    500.,
                );
            }
            assert_eq!(seen.len(), count, "every plugin appears: {seen:?}");
            // Wheel scrolling is smooth: let the list come to rest before clicking.
            for _ in 0..60 {
                text = frame_with(&ctx, &mut plugins, vec![], 500.);
            }
            assert!(text.iter().any(|(s, _)| {
                s == "Not working · {count}.0.0"
                    .replace("{count}", &count.to_string())
                    .as_str()
            }));
            let last = text
                .iter()
                .find(|(s, _)| *s == name(count))
                .unwrap()
                .1
                .center();
            frame_with(&ctx, &mut plugins, click(last), 500.);
            assert_eq!(plugins.selected, name(count), "the last one can be chosen");
            assert!(!text.iter().any(|(s, _)| s == "Next"), "no pages");
        }
        let ctx = egui::Context::default();
        let mut plugins = PluginUi {
            status: synthetic(32),
            ..Default::default()
        };
        plugins.status["items"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"plugin33"}));
        let text = frame(&ctx, &mut plugins, None);
        assert!(
            text.iter()
                .any(|(s, _)| s == "Invalid plugin status: more than 32 entries")
        );
        assert!(!text.iter().any(|(s, _)| s.starts_with("plugin")));
    }
    #[test]
    fn installing_an_official_plugin_sends_its_current_release_trusted_by_the_click() {
        let ctx = egui::Context::default();
        let mut plugins = PluginUi {
            status: json!({"state":"idle","items":[{"id":"org.rad.media","host":"ready",
                "selected":{"major":0,"minor":2,"patch":0}}]}),
            ..Default::default()
        };
        let text = frame(&ctx, &mut plugins, None);
        assert!(
            text.iter().any(|(s, _)| s == "Installed"),
            "Media is installed"
        );
        let audio = text.iter().find(|(s, _)| s == "Audio").unwrap().1;
        let install = text
            .iter()
            .filter(|(s, r)| s == "Install" && (r.center().y - audio.center().y).abs() < 8.)
            .map(|(_, r)| r.center())
            .next()
            .expect("Audio has its own Install button");
        frame(&ctx, &mut plugins, Some(install));
        let source = plugins.official_install.take().unwrap();
        assert_eq!(source, crate::official::OFFICIAL[0].source);
        let request = serde_json::to_value(official_request(source)).unwrap();
        let text = request.to_string();
        assert!(text.contains(source), "{text}");
        assert!(text.contains("\"install\""), "{text}");
        assert!(
            text.contains("true"),
            "the click trusts the official release: {text}"
        );
        assert!(plugins.operation_request.is_none(), "nothing else is sent");
    }
    #[test]
    fn the_bar_follows_the_download_from_0_to_100_and_then_goes() {
        let ctx = egui::Context::default();
        let mut plugins = PluginUi {
            status: json!({"state":"preparing","items":[]}),
            ..Default::default()
        };
        let text = frame(&ctx, &mut plugins, None);
        assert!(
            text.iter().any(|(s, _)| s == "Working…"),
            "size still unknown"
        );
        for percent in [0, 50, 100] {
            plugins.status["progress"] = json!(percent);
            let text = frame(&ctx, &mut plugins, None);
            let shown = format!("Downloading {percent} %");
            assert!(text.iter().any(|(s, _)| *s == shown), "{shown}");
        }
        plugins.status = json!({"state":"idle","items":[]});
        let text = frame(&ctx, &mut plugins, None);
        assert!(!text.iter().any(|(s, _)| s.starts_with("Downloading")));
    }
    #[test]
    fn an_official_plugin_with_a_newer_release_offers_its_update_and_others_their_origin() {
        let official = json!({"id":"org.rad.audio","host":"ready","source_trusted":true,
            "source":"https://github.com/KroxiDev/rad-plugins/releases/tag/audio-v0.1.0",
            "selected":{"major":0,"minor":1,"patch":0}});
        assert_eq!(
            update_source(&official),
            Some((crate::official::OFFICIAL[0].source.into(), true))
        );
        let mut current = official.clone();
        current["selected"] = json!({"major":0,"minor":2,"patch":0});
        assert_eq!(update_source(&current), None, "already current");
        let third = json!({"id":"com.example.x","host":"ready","source_trusted":true,
            "source":"https://github.com/o/r","diagnostic":"incompatible_plugin",
            "selected":{"major":1,"minor":0,"patch":0}});
        assert_eq!(
            update_source(&third),
            Some(("https://github.com/o/r".into(), false))
        );
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

#[cfg(test)]
mod source_change_tests {
    use super::*;
    const REGISTERED: &str = "https://github.com/owner/repo/releases/tag/v1";
    const OTHER: &str = "https://github.com/other/repo/releases/tag/v2";
    fn ui(source: &str) -> PluginUi {
        let mut plugins = PluginUi {
            status: json!({"state":"idle","items":[{"id":"fixture","source":REGISTERED,"source_trusted":true}]}),
            ..Default::default()
        };
        plugins.select("fixture");
        plugins.source = source.into();
        plugins.source_edited = true;
        plugins.trusted = true;
        plugins
    }
    fn args(request: &crate::Request) -> Value {
        request.arguments()
    }
    #[test]
    fn another_repository_waits_for_the_user_and_then_sends_the_registered_origin() {
        for operation in ["update", "reinstall"] {
            let mut plugins = ui(OTHER);
            assert!(
                plugins.next_admin_request(operation).is_none(),
                "{operation} must not be sent before the user agrees"
            );
            let asked = plugins.confirmation.clone().expect("confirmation opens");
            assert_eq!(asked.change.registered, REGISTERED);
            assert_eq!(asked.change.requested, OTHER);
            let request = plugins.confirm().expect("the confirmed request");
            assert_eq!(
                args(&request),
                json!({"operation":operation,"plugin_id":"fixture","source":OTHER,
                       "trusted":true,"replaces_source":REGISTERED})
            );
            assert!(plugins.confirmation.is_none());
        }
    }
    #[test]
    fn same_repository_other_tag_or_case_is_sent_without_confirmation() {
        for source in [
            "https://github.com/owner/repo/releases/tag/v2",
            "https://github.com/OWNER/REPO",
        ] {
            let mut plugins = ui(source);
            let request = plugins.next_admin_request("update").unwrap();
            assert!(args(&request).get("replaces_source").is_none(), "{source}");
            assert!(plugins.confirmation.is_none());
        }
    }
    #[test]
    fn other_operations_and_plugins_without_origin_never_ask() {
        let mut plugins = ui(OTHER);
        for operation in ["install", "uninstall", "rollback"] {
            assert!(
                plugins.next_admin_request(operation).is_some(),
                "{operation}"
            );
        }
        plugins.status["items"][0]["source"] = Value::Null;
        plugins.select("fixture");
        plugins.source = OTHER.into();
        plugins.trusted = true;
        assert!(plugins.next_admin_request("update").is_some());
    }
    #[test]
    fn a_confirmation_does_not_outlive_the_plugin_or_the_link_it_was_raised_for() {
        let mut plugins = ui(OTHER);
        assert!(plugins.next_admin_request("update").is_none());
        assert!(plugins.pending_confirmation().is_some());
        plugins.source = "https://github.com/third/repo".into();
        assert!(plugins.pending_confirmation().is_none(), "another link");
        plugins.source = OTHER.into();
        assert!(plugins.pending_confirmation().is_some());
        plugins.status["items"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"second"}));
        plugins.selected = "second".into();
        assert!(plugins.pending_confirmation().is_none(), "another plugin");
        assert!(plugins.confirm().is_none(), "nothing valid to confirm");
    }
    #[test]
    fn a_refusal_from_the_runtime_opens_the_confirmation_with_the_origins_it_names() {
        let mut plugins = ui("https://github.com/owner/repo/releases/tag/v2");
        plugins.submitted = Some("update");
        // The registered origin changed meanwhile: the runtime names the current one.
        let change = SourceChange::from_details(&json!({
            "registered":"https://github.com/moved/repo","requested":"https://github.com/owner/repo/releases/tag/v2"
        }))
        .unwrap();
        plugins.source_refused(change.clone());
        assert_eq!(plugins.pending_confirmation().unwrap().change, change);
        let request = plugins.confirm().unwrap();
        assert_eq!(args(&request)["operation"], "update");
        assert_eq!(
            args(&request)["replaces_source"],
            "https://github.com/moved/repo"
        );
        assert_eq!(args(&request)["trusted"], true);
        // Without a remembered operation there is nothing to repeat.
        let mut idle = ui(OTHER);
        idle.source_refused(change);
        assert!(idle.pending_confirmation().is_none());
    }
}
