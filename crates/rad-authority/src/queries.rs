//! Sanitized typed views. JSON and transport encoding belong to the runtime adapter.
use crate::*;
use rad_core::{configuration::Mode, identity::Binding, permissions::Permissions};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Visible<T> {
    Visible(T),
    Restricted,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionType {
    Launcher,
    External(rad_core::configuration::ExternalType),
    EnterFolder,
    Return,
    Sleep,
    Wait,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionView {
    pub parent_binding: Option<Binding>,
    pub local_permissions: rad_core::permissions::LocalPermissions,
    pub desktop_entry: Option<rad_core::configuration::DesktopEntry>,
    pub wait_ms: Option<u64>,
    pub destination: Option<Binding>,
    pub executor_installed: bool,
    pub busy: bool,
    pub binding: Binding,
    pub parent: Option<EntityIdentity>,
    pub kind: ActionType,
    pub permissions: Permissions,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonView {
    pub folder: Option<Binding>,
    pub position: usize,
    pub local_permissions: rad_core::permissions::LocalPermissions,
    pub busy: bool,
    pub binding: Binding,
    pub name: String,
    pub mode: Mode,
    pub gesture: rad_core::configuration::Gesture,
    pub permissions: Permissions,
    pub actions: Vec<Visible<ActionView>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityView {
    Button(ButtonView),
    Action(ActionView),
}
impl Authority {
    pub fn get(&self, origin: Origin, id: rad_core::identity::ShortId) -> Option<EntityView> {
        let data = self.published.configuration().data();
        for b in &data.buttons {
            let permissions = b.permissions.resolve(data.global_permissions);
            let busy = self.runs.keys().any(|id| {
                self.engine
                    .snapshot(*id)
                    .is_some_and(|snapshot| snapshot.button().binding == b.binding)
            });
            let parent_visible = origin == Origin::Editor || permissions.consult();
            let action_view = |a: &rad_core::configuration::ActionData| {
                let permissions = a.permissions.resolve(permissions);
                if origin != Origin::Editor && !permissions.consult() {
                    return Visible::Restricted;
                }
                let kind = match &a.kind {
                    rad_core::configuration::ActionKind::Launcher(_) => ActionType::Launcher,
                    rad_core::configuration::ActionKind::External { type_key, .. } => {
                        ActionType::External(type_key.clone())
                    }
                    rad_core::configuration::ActionKind::EnterFolder(_) => ActionType::EnterFolder,
                    rad_core::configuration::ActionKind::Return => ActionType::Return,
                    rad_core::configuration::ActionKind::Sleep => ActionType::Sleep,
                    rad_core::configuration::ActionKind::Wait(_) => ActionType::Wait,
                };
                Visible::Visible(ActionView {
                    parent_binding: parent_visible.then_some(b.binding),
                    local_permissions: a.permissions,
                    desktop_entry: match &a.kind {
                        rad_core::configuration::ActionKind::Launcher(entry) => Some(entry.clone()),
                        _ => None,
                    },
                    wait_ms: match a.kind {
                        rad_core::configuration::ActionKind::Wait(wait) => {
                            Some(wait.milliseconds())
                        }
                        _ => None,
                    },
                    destination: {
                        let target = match a.kind {
                            rad_core::configuration::ActionKind::EnterFolder(id) => Some(id),
                            rad_core::configuration::ActionKind::Return => data
                                .folders
                                .iter()
                                .find(|f| f.slots.contains(&Some(b.binding.identity())))
                                .and_then(|f| f.parent),
                            _ => None,
                        };
                        target
                            .filter(|id| self.visible_folders(origin).contains(id))
                            .and_then(|id| {
                                data.folders
                                    .iter()
                                    .find(|f| f.binding.identity() == id)
                                    .map(|f| f.binding)
                            })
                    },
                    executor_installed: match &a.kind {
                        rad_core::configuration::ActionKind::External { type_key, .. } => {
                            self.installed.contains(type_key)
                        }
                        _ => true,
                    },
                    busy,
                    binding: a.binding,
                    parent: parent_visible.then_some(b.binding.identity()),
                    kind,
                    permissions,
                })
            };
            if b.binding.short() == id {
                return parent_visible.then(|| {
                    EntityView::Button(ButtonView {
                        folder: data
                            .folders
                            .iter()
                            .find(|f| f.slots.contains(&Some(b.binding.identity())))
                            .map(|f| f.binding),
                        position: data
                            .folders
                            .iter()
                            .find_map(|f| {
                                f.slots
                                    .iter()
                                    .position(|id| *id == Some(b.binding.identity()))
                            })
                            .expect("validated button position"),
                        local_permissions: b.permissions,
                        busy,
                        binding: b.binding,
                        name: b.name.clone(),
                        mode: b.mode,
                        gesture: b.gesture,
                        permissions,
                        actions: b.actions.iter().map(action_view).collect(),
                    })
                });
            }
            if let Some(a) = b.actions.iter().find(|a| a.binding.short() == id) {
                return match action_view(a) {
                    Visible::Visible(v) => Some(EntityView::Action(v)),
                    Visible::Restricted => None,
                };
            }
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultView {
    pub id: ExecutionId,
    pub status: rad_core::execution::ExecutionStatus,
    pub steps: Vec<Visible<rad_core::execution::StepResult>>,
}
impl Authority {
    pub fn result(&self, origin: Origin, id: ExecutionId) -> Option<ResultView> {
        let result = self.engine.result(id)?;
        let target = match result.target {
            Target::Button(id) | Target::Action(id) => id,
        };
        let data = self.published.configuration().data();
        let binding = data
            .buttons
            .iter()
            .map(|b| b.binding)
            .chain(
                data.buttons
                    .iter()
                    .flat_map(|b| b.actions.iter().map(|a| a.binding)),
            )
            .find(|b| b.identity() == target)?;
        self.get(origin, binding.short())?;
        let steps = result
            .steps
            .into_iter()
            .map(|step| {
                if self.get(origin, step.action.short()).is_some() {
                    Visible::Visible(step)
                } else {
                    Visible::Restricted
                }
            })
            .collect();
        Some(ResultView {
            id,
            status: result.status,
            steps,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewRevision(u64);
impl Authority {
    pub fn view_revision(&self, origin: Origin) -> Option<ViewRevision> {
        match origin {
            Origin::Editor => self.editor_revision.map(ViewRevision),
            Origin::Automation => self.automation_revision.map(ViewRevision),
        }
    }
    pub(super) fn refresh_views(&mut self) {
        self.observed_executions
            .retain(|id| self.engine.result(*id).is_some());
        let results: Vec<_> = self
            .observed_executions
            .iter()
            .filter_map(|id| self.result(Origin::Automation, *id))
            .collect();
        let editor = EditorProjection {
            revision: self.revision,
            active: self.active,
            results: self
                .observed_executions
                .iter()
                .filter_map(|id| self.result(Origin::Editor, *id))
                .collect(),
            installed: self.installed.clone(),
            storage: self.state,
        };
        if self.editor_projection.as_ref() != Some(&editor) {
            self.editor_revision = self.editor_revision.and_then(|v| v.checked_add(1));
            self.editor_projection = Some(editor);
        }
        let data = self.published.configuration().data();
        let visible: Vec<_> = data
            .buttons
            .iter()
            .flat_map(|b| {
                std::iter::once(b.binding.short())
                    .chain(b.actions.iter().map(|a| a.binding.short()))
            })
            .filter_map(|id| self.get(Origin::Automation, id))
            .collect();
        let appearance: Vec<_> = data
            .buttons
            .iter()
            .filter_map(|b| self.appearance(Origin::Automation, b.binding.short()))
            .collect();
        let folders: Vec<_> = data
            .folders
            .iter()
            .filter_map(|f| self.folder(Origin::Automation, f.binding.short()))
            .collect();
        let active = folders
            .iter()
            .find(|f| f.binding.identity() == self.active)
            .map(|f| f.binding);
        self.editor_active = data
            .folders
            .iter()
            .find(|f| f.binding.identity() == self.active)
            .map(|f| f.binding);
        if visible != self.automation_view
            || appearance != self.automation_appearance
            || folders != self.automation_folders
            || active != self.automation_active
            || results != self.automation_results
        {
            self.automation_revision = self.automation_revision.and_then(|v| v.checked_add(1));
            self.automation_view = visible;
            self.automation_appearance = appearance;
            self.automation_results = results;
            self.automation_folders = folders;
            self.automation_active = active;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    query: QueryKey,
    session: u128,
    connection: u64,
    origin: Origin,
    revision: ViewRevision,
    offset: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    NotFound,
    CursorExpired,
    Capacity,
}
#[derive(Debug)]
pub struct Page {
    pub entries: Vec<EntityView>,
    pub next: Option<Cursor>,
}
impl Authority {
    /// Adapter supplies measured encoded entry size; this port never defines JSON.
    pub fn list(
        &self,
        connection: u64,
        origin: Origin,
        cursor: Option<&Cursor>,
        mut encoded_size: impl FnMut(&EntityView) -> usize,
    ) -> Result<Page, QueryError> {
        let revision = self.view_revision(origin).ok_or(QueryError::Capacity)?;
        let offset = if let Some(cursor) = cursor {
            if cursor.query != QueryKey::List
                || cursor.session != self.session
                || cursor.connection != connection
                || cursor.origin != origin
                || cursor.revision != revision
            {
                return Err(QueryError::CursorExpired);
            }
            cursor.offset
        } else {
            0
        };
        let data = self.published.configuration().data();
        let mut source = data
            .buttons
            .iter()
            .flat_map(|b| {
                std::iter::once(b.binding.short())
                    .chain(b.actions.iter().map(|a| a.binding.short()))
            })
            .filter_map(|id| self.get(origin, id))
            .skip(offset)
            .peekable();
        let mut entries = vec![];
        let mut bytes = 0usize;
        while entries.len() < 32 {
            let Some(entry) = source.peek() else { break };
            let size = encoded_size(entry);
            if size > 48 * 1024 {
                return Err(QueryError::Capacity);
            }
            if bytes + size > 48 * 1024 {
                break;
            }
            bytes += size;
            entries.push(source.next().unwrap());
        }
        let next = source.peek().map(|_| Cursor {
            query: QueryKey::List,
            session: self.session,
            connection,
            origin,
            revision,
            offset: offset + entries.len(),
        });
        Ok(Page { entries, next })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisualView {
    Static,
    External(rad_core::configuration::ExternalType),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppearanceView {
    pub appearance: rad_core::configuration::Appearance,
    pub visual: VisualView,
}
impl Authority {
    pub fn appearance(
        &self,
        origin: Origin,
        id: rad_core::identity::ShortId,
    ) -> Option<AppearanceView> {
        if !matches!(self.get(origin, id), Some(EntityView::Button(_))) {
            return None;
        }
        let button = self
            .published
            .configuration()
            .data()
            .buttons
            .iter()
            .find(|b| b.binding.short() == id)?;
        let visual = match &button.visual {
            rad_core::configuration::VisualSource::Manual => VisualView::Static,
            rad_core::configuration::VisualSource::External { type_key, .. } => {
                VisualView::External(type_key.clone())
            }
        };
        Some(AppearanceView {
            appearance: button.appearance.clone(),
            visual,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderView {
    pub parent_binding: Option<Binding>,
    pub name: Option<String>,
    pub entry_button: Option<Binding>,
    pub binding: Binding,
    pub parent: Option<EntityIdentity>,
    pub slots: Vec<Option<Visible<Binding>>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusView {
    pub grid: rad_core::configuration::Grid,
    pub revision: Option<ViewRevision>,
    pub active: Option<Binding>,
}
impl Authority {
    pub fn status(&self, origin: Origin) -> StatusView {
        let active = if origin == Origin::Editor {
            self.editor_active
        } else {
            self.automation_active
        };
        StatusView {
            grid: self.published.configuration().data().grid,
            revision: self.view_revision(origin),
            active,
        }
    }
    pub fn folder(&self, origin: Origin, id: rad_core::identity::ShortId) -> Option<FolderView> {
        let data = self.published.configuration().data();
        let visible = self.visible_folders(origin);
        let f = data
            .folders
            .iter()
            .find(|f| f.binding.short() == id && visible.contains(&f.binding.identity()))?;
        let slots = f
            .slots
            .iter()
            .map(|id| {
                id.map(|id| {
                    let b = data
                        .buttons
                        .iter()
                        .find(|b| b.binding.identity() == id)
                        .expect("validated folder");
                    if self.get(origin, b.binding.short()).is_some() {
                        Visible::Visible(b.binding)
                    } else {
                        Visible::Restricted
                    }
                })
            })
            .collect();
        let entry = data.buttons.iter().find(|b| {
            b.actions.iter().any(|a| matches!(a.kind, rad_core::configuration::ActionKind::EnterFolder(id) if id == f.binding.identity()))
                && (origin == Origin::Editor || b.permissions.resolve(data.global_permissions).consult())
        });
        Some(FolderView {
            parent_binding: f.parent.filter(|p| visible.contains(p)).and_then(|p| {
                data.folders
                    .iter()
                    .find(|f| f.binding.identity() == p)
                    .map(|f| f.binding)
            }),
            name: entry.map(|b| b.name.clone()),
            entry_button: entry.map(|b| b.binding),
            binding: f.binding,
            parent: f.parent.filter(|p| visible.contains(p)),
            slots,
        })
    }
    fn visible_folders(&self, origin: Origin) -> std::collections::BTreeSet<EntityIdentity> {
        let data = self.published.configuration().data();
        let mut visible = std::collections::BTreeSet::new();
        for folder in &data.folders {
            let has_content = origin == Origin::Editor
                || folder.slots.iter().flatten().any(|id| {
                    let b = data
                        .buttons
                        .iter()
                        .find(|b| b.binding.identity() == *id)
                        .unwrap();
                    let permissions = b.permissions.resolve(data.global_permissions);
                    permissions.consult()
                        || b.actions
                            .iter()
                            .any(|a| a.permissions.resolve(permissions).consult())
                });
            if has_content {
                let mut current = Some(folder.binding.identity());
                while let Some(id) = current {
                    if !visible.insert(id) {
                        break;
                    }
                    current = data
                        .folders
                        .iter()
                        .find(|f| f.binding.identity() == id)
                        .and_then(|f| f.parent);
                }
            }
        }
        visible
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct EditorProjection {
    revision: i64,
    active: EntityIdentity,
    results: Vec<ResultView>,
    installed: Vec<rad_core::configuration::ExternalType>,
    storage: StorageState,
}

#[derive(Debug)]
pub struct ActionsPage {
    pub entries: Vec<Visible<ActionView>>,
    pub next: Option<Cursor>,
}
impl Authority {
    pub fn actions(
        &self,
        connection: u64,
        origin: Origin,
        button: rad_core::identity::ShortId,
        cursor: Option<&Cursor>,
        mut encoded_size: impl FnMut(&Visible<ActionView>) -> usize,
    ) -> Result<ActionsPage, QueryError> {
        let revision = self.view_revision(origin).ok_or(QueryError::Capacity)?;
        let query = QueryKey::Actions(button);
        let offset = if let Some(c) = cursor {
            if c.query != query
                || c.session != self.session
                || c.connection != connection
                || c.origin != origin
                || c.revision != revision
            {
                return Err(QueryError::CursorExpired);
            }
            c.offset
        } else {
            0
        };
        let Some(EntityView::Button(view)) = self.get(origin, button) else {
            return Err(QueryError::NotFound);
        };
        let mut source = view.actions.into_iter().skip(offset).peekable();
        let mut entries = vec![];
        let mut bytes = 0;
        while entries.len() < 32 {
            let Some(entry) = source.peek() else { break };
            let size = encoded_size(entry);
            if size > 48 * 1024 {
                return Err(QueryError::Capacity);
            }
            if bytes + size > 48 * 1024 {
                break;
            }
            bytes += size;
            entries.push(source.next().unwrap());
        }
        let next = source.peek().map(|_| Cursor {
            query,
            session: self.session,
            connection,
            origin,
            revision,
            offset: offset + entries.len(),
        });
        Ok(ActionsPage { entries, next })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum QueryKey {
    List,
    Actions(rad_core::identity::ShortId),
}

impl Authority {
    pub fn settings(&self, origin: Origin) -> Option<rad_core::configuration::SettingsData> {
        (origin == Origin::Editor).then(|| self.published.configuration().settings().data())
    }
}

impl ViewRevision {
    pub fn value(self) -> u64 {
        self.0
    }
}
impl Cursor {
    pub const ENCODED_LEN: usize = 51;
    pub fn to_bytes(&self) -> [u8; Self::ENCODED_LEN] {
        let mut bytes = [0; Self::ENCODED_LEN];
        bytes[..16].copy_from_slice(&self.session.to_be_bytes());
        bytes[16..24].copy_from_slice(&self.connection.to_be_bytes());
        bytes[24] = if self.origin == Origin::Editor { 0 } else { 1 };
        bytes[25..33].copy_from_slice(&self.revision.0.to_be_bytes());
        bytes[33..41].copy_from_slice(&(self.offset as u64).to_be_bytes());
        if let QueryKey::Actions(id) = self.query {
            bytes[41] = 1;
            bytes[42] = 2;
            bytes[43..51].copy_from_slice(&id.number().to_be_bytes());
        }
        bytes
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, QueryError> {
        let bad = QueryError::CursorExpired;
        if bytes.len() != Self::ENCODED_LEN {
            return Err(bad);
        }
        let session = u128::from_be_bytes(bytes[..16].try_into().unwrap());
        let connection = u64::from_be_bytes(bytes[16..24].try_into().unwrap());
        let origin = match bytes[24] {
            0 => Origin::Editor,
            1 => Origin::Automation,
            _ => return Err(bad),
        };
        let revision = u64::from_be_bytes(bytes[25..33].try_into().unwrap());
        let offset = usize::try_from(u64::from_be_bytes(bytes[33..41].try_into().unwrap()))
            .map_err(|_| QueryError::CursorExpired)?;
        if session == 0 || revision == 0 {
            return Err(bad);
        }
        let query = match bytes[41] {
            0 if bytes[42..].iter().all(|b| *b == 0) => QueryKey::List,
            1 if bytes[42] == 2 => QueryKey::Actions(
                rad_core::identity::ShortId::new(
                    rad_core::identity::EntityKind::Button,
                    u64::from_be_bytes(bytes[43..51].try_into().unwrap()),
                )
                .map_err(|_| QueryError::CursorExpired)?,
            ),
            _ => return Err(bad),
        };
        Ok(Self {
            query,
            session,
            connection,
            origin,
            revision: ViewRevision(revision),
            offset,
        })
    }
}

impl Authority {
    pub fn result_id(&self, origin: Origin, session: u128, counter: u64) -> Option<ResultView> {
        let id = self
            .observed_executions
            .iter()
            .find(|id| id.session() == session && id.counter() == counter)?;
        self.result(origin, *id)
    }
}

impl Authority {
    /// Whether the global Consulta is on. Unlike `global_permissions`, any origin may ask.
    pub fn global_consult(&self) -> bool {
        self.published
            .configuration()
            .data()
            .global_permissions
            .consult()
    }
    pub fn global_permissions(&self, origin: Origin) -> Option<Permissions> {
        (origin == Origin::Editor)
            .then_some(self.published.configuration().data().global_permissions)
    }
}

impl Authority {
    /// Portable references, not installed packages. Editor only: opaque plugin settings stay private.
    pub fn plugin_dependencies(
        &self,
        origin: Origin,
    ) -> Option<BTreeMap<String, rad_core::configuration::PluginDependency>> {
        if origin != Origin::Editor {
            return None;
        }
        let data = self.published.configuration().data();
        let mut ids: std::collections::BTreeSet<String> =
            data.plugin_settings.keys().cloned().collect();
        for button in &data.buttons {
            if let rad_core::configuration::VisualSource::External { type_key, .. } = &button.visual
            {
                ids.insert(type_key.plugin().into());
            }
            for action in &button.actions {
                if let rad_core::configuration::ActionKind::External { type_key, .. } = &action.kind
                {
                    ids.insert(type_key.plugin().into());
                }
            }
        }
        Some(
            ids.into_iter()
                .map(|id| {
                    let dependency = data.plugin_dependencies.get(&id).cloned().unwrap_or(
                        rad_core::configuration::PluginDependency {
                            version: None,
                            origin: None,
                        },
                    );
                    (id, dependency)
                })
                .collect(),
        )
    }
}

impl Authority {
    /// Editor-only access to retained non-secret plugin-owned settings. Bytes stay
    /// opaque even when the corresponding package is missing or incompatible.
    pub fn plugin_settings(&self, origin: Origin, plugin: &str) -> Option<&[u8]> {
        (origin == Origin::Editor)
            .then(|| {
                self.published
                    .configuration()
                    .data()
                    .plugin_settings
                    .get(plugin)
                    .map(|settings| settings.bytes())
            })
            .flatten()
    }
}
