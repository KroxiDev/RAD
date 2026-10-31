//! Client projections and coalesced invalidations; no state authority lives here.
use super::*;
use rad_api::{
    Notice, NoticeKind, OverviewView, PluginAttention, PluginAttentionCode, RestartState,
};

#[derive(Default)]
pub(super) struct Notices {
    subscribers: BTreeSet<u64>,
    epoch: u64,
    revision: Option<String>,
    overview: Option<OverviewView>,
    plugins: Option<Value>,
    execution: u64,
}
impl Server {
    pub(super) fn overview(&self) -> OverviewView {
        let status = self.actor.status(Origin::Editor);
        let settings = self
            .actor
            .settings(Origin::Editor)
            .expect("editor settings");
        let p = self
            .actor
            .global_permissions(Origin::Editor)
            .expect("editor permissions");
        let mut plugins = BTreeMap::new();
        for (manifest, generation, _) in self.actor.plugin_manifests() {
            let code = if self
                .actor
                .plugin_credential_status(&manifest.id)
                .iter()
                .any(|(_, present)| !present)
            {
                Some(PluginAttentionCode::NeedsCredentials)
            } else if !self.actor.plugin_available(&manifest.id, generation) {
                Some(PluginAttentionCode::Unavailable)
            } else {
                None
            };
            if let Some(code) = code {
                plugins.insert(manifest.id.clone(), code);
            }
        }
        if let Some(admin) = &self.plugin_admin {
            for id in admin.plugins_needing_update() {
                plugins.insert(id.to_owned(), PluginAttentionCode::NeedsUpdate);
            }
        }
        OverviewView {
            view_revision: status
                .revision
                .map(|r| r.value().to_string())
                .unwrap_or_else(|| "0".into()),
            active_folder_id: status.active.map(|b| format_short(b.short())),
            storage: match self.actor.storage_state() {
                rad_authority::StorageState::Ready => rad_api::StorageState::Ready,
                rad_authority::StorageState::Uncertain => rad_api::StorageState::Uncertain,
                rad_authority::StorageState::RecoveryRequired => {
                    rad_api::StorageState::RecoveryRequired
                }
            },
            deck: self.deck_state(),
            plugins: plugins
                .into_iter()
                .map(|(id, code)| PluginAttention { id, code })
                .collect(),
            last_restart: if self.recovered_previous {
                RestartState::RecoveredPrevious
            } else {
                RestartState::Clean
            },
            settings: rad_api::Settings {
                long_press_threshold_ms: settings.long_press_threshold_ms.to_string(),
                brightness_percent: settings.brightness_percent,
                sleep_ms: settings.sleep_ms.to_string(),
                press_threshold_ms: settings.press_threshold_ms.to_string(),
            },
            global_permissions: rad_api::Permissions {
                consult: p.consult(),
                modify: p.modify(),
                execute: p.execute(),
            },
            history_operations: self
                .actor
                .editor_view(Origin::Editor)
                .expect("editor view")
                .history_operations,
            previous_exit: self.previous_exit.clone(),
        }
    }
    pub(super) fn client_api(
        &mut self,
        key: RequestKey,
        c: IncomingContext,
        r: &rad_wire::Request,
    ) -> Result<bool, RuntimeError> {
        if !matches!(
            r.operation(),
            "overview"
                | "subscribe"
                | "board"
                | "runtime_log"
                | "machine_settings"
                | "set_machine_settings"
        ) {
            return Ok(false);
        }
        // A refused change says so: a mutation's error always carries its outcome.
        let mutation = rad_api::Operation::parse(r.operation()).is_some_and(|o| o.is_mutation());
        if c.origin != Origin::Editor {
            self.respond(key, self.error(c, "permission_denied", mutation))?;
            return Ok(true);
        }
        let request = rad_api::Request::parse(r.operation(), Value::Object(r.args().clone()));
        let data:Result<rad_api::ResponseData,&'static str>=match request {
            Ok(rad_api::Request::Overview{})=>Ok(rad_api::ResponseData::Overview(self.overview())),
            Ok(rad_api::Request::RuntimeLog{})=>Ok(rad_api::ResponseData::RuntimeLog(self.events.view())),
            Ok(rad_api::Request::MachineSettings{})=>Ok(self.machine_settings_data()),
            Ok(rad_api::Request::SetMachineSettings{use_stream_deck,start_with_session})=>
                self.set_machine_settings(use_stream_deck,start_with_session)?,
            Ok(rad_api::Request::Subscribe{})=>{
                // A peer that left meanwhile has nothing to subscribe: its answer finds no request.
                if self.transport.subscribe(c.connection){self.notices.subscribers.insert(c.connection);}
                Ok(rad_api::ResponseData::Empty(rad_api::Empty {}))
            },
            Ok(rad_api::Request::Board{folder_id})=>self.protocol.board_view(&self.actor,c,folder_id.as_deref()).and_then(|mut board|{
                for key in board.keys.iter_mut().flatten() {
                    if key.appearance.visual_type.is_some() {
                        let id=parse_short(&key.entity.id).ok_or("invalid_configuration")?;
                        let value=self.plugin_visuals.as_ref().map(|v|v.ipc_view(&self.actor,id)).unwrap_or(json!({"availability":"unavailable","revision":"0","text":null,"text_position":"bottom","color":null,"image":false}));
                        key.visual=Some(serde_json::from_value(value).map_err(|_|"invalid_configuration")?);
                    }
                }
                Ok(rad_api::ResponseData::Board(board))
            }),
            _=>Err("invalid_request"),
        };
        let response = match data {
            Ok(data) => {
                let response = Response {
                    v: 1,
                    id: Some(c.request_id.to_string()),
                    session_id: Some(format!("{:032x}", self.session)),
                    status: Status::Ok,
                    data,
                    error: None,
                };
                if self.protocol.encode_response(&response).is_err() {
                    self.error(c, "resource_exhausted", false)
                } else {
                    response
                }
            }
            Err(code) => self.error(c, code, mutation),
        };
        self.respond(key, response)?;
        if r.operation() == "subscribe" && self.notices.subscribers.contains(&c.connection) {
            for kind in NoticeKind::ALL {
                self.send_notice(c.connection, kind)?;
            }
        }
        Ok(true)
    }
    fn machine_settings_data(&self) -> rad_api::ResponseData {
        rad_api::ResponseData::MachineSettings(rad_api::MachineSettingsView {
            use_stream_deck: self.machine.use_stream_deck,
            start_with_session: self.machine.start_with_session,
        })
    }
    /// Stores first and applies after: a change that could not be stored changes nothing.
    /// Only `use_stream_deck` acts on the runtime; `start_with_session` is applied by the
    /// editor, which owns the service unit.
    fn set_machine_settings(
        &mut self,
        use_stream_deck: Option<bool>,
        start_with_session: Option<bool>,
    ) -> Result<Result<rad_api::ResponseData, &'static str>, RuntimeError> {
        if use_stream_deck.is_none() && start_with_session.is_none() {
            return Ok(Err("invalid_request"));
        }
        let current = self.machine;
        let next = crate::machine_settings::MachineSettings {
            use_stream_deck: use_stream_deck.unwrap_or(current.use_stream_deck),
            start_with_session: start_with_session.unwrap_or(current.start_with_session),
        };
        if next != current {
            if next.save(&self.data_dir).is_err() {
                return Ok(Err("storage_error"));
            }
            self.machine = next;
            if next.use_stream_deck != current.use_stream_deck {
                self.apply_deck_setting(next.use_stream_deck)?;
            }
        }
        Ok(Ok(self.machine_settings_data()))
    }
    pub(super) fn send_notice(
        &mut self,
        connection: u64,
        kind: NoticeKind,
    ) -> Result<(), RuntimeError> {
        let notice = Notice {
            v: rad_api::VERSION,
            session_id: format!("{:032x}", self.session),
            kind,
            revision: self.notices.epoch.to_string(),
        };
        let payload = serde_json::to_vec(&notice).map_err(|_| RuntimeError::Protocol)?;
        // Notices coalesce and a missed one is recovered by the next: a refusal is not fatal.
        if self
            .transport
            .notify(connection, kind.key(), &payload)
            .is_err()
        {
            self.events.record(rad_api::EventCode::NoticeDropped, None);
        }
        Ok(())
    }
    pub(super) fn publish_notices(&mut self) -> Result<(), RuntimeError> {
        self.notices
            .subscribers
            .retain(|id| self.connections.contains(id));
        let overview = self.overview();
        let revision = overview.view_revision.clone();
        let config = self.notices.revision.as_ref() != Some(&revision);
        let overview_changed = self.notices.overview.as_ref() != Some(&overview);
        let plugins = self
            .plugin_admin
            .as_ref()
            .zip(self.native.as_ref())
            .map(|(a, n)| a.status(n.plugin_host()));
        let plugin_changed = self.notices.plugins != plugins
            || self.notices.overview.as_ref().map(|v| &v.plugins) != Some(&overview.plugins);
        let execution = self.actor.execution_revision();
        let mut kinds = Vec::new();
        if config || self.display_dirty {
            kinds.push(NoticeKind::Board);
        }
        if config {
            kinds.extend([NoticeKind::Inspector, NoticeKind::Dependencies]);
        }
        if overview_changed {
            kinds.push(NoticeKind::Overview);
        }
        if plugin_changed {
            kinds.extend([
                NoticeKind::Plugins,
                NoticeKind::Catalog,
                NoticeKind::Dependencies,
                NoticeKind::Inspector,
            ]);
        }
        if execution != self.notices.execution {
            kinds.push(NoticeKind::Execution);
        }
        self.notices.revision = Some(revision);
        self.notices.overview = Some(overview);
        self.notices.plugins = plugins;
        self.notices.execution = execution;
        if kinds.is_empty() {
            return Ok(());
        }
        self.notices.epoch = self.notices.epoch.wrapping_add(1);
        let clients: Vec<_> = self.notices.subscribers.iter().copied().collect();
        for client in clients {
            for kind in &kinds {
                self.send_notice(client, *kind)?;
            }
        }
        Ok(())
    }
}
fn format_short(id: rad_core::identity::ShortId) -> String {
    use rad_core::identity::EntityKind;
    format!(
        "{}{}",
        match id.kind() {
            EntityKind::Folder => 'f',
            EntityKind::Button => 'b',
            EntityKind::Action => 'a',
        },
        id.number()
    )
}
fn parse_short(value: &str) -> Option<rad_core::identity::ShortId> {
    use rad_core::identity::{EntityKind, ShortId};
    let kind = match value.as_bytes().first()? {
        b'f' => EntityKind::Folder,
        b'b' => EntityKind::Button,
        b'a' => EntityKind::Action,
        _ => return None,
    };
    ShortId::new(kind, value.get(1..)?.parse().ok()?).ok()
}
