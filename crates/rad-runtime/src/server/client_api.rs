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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixStream},
    };

    #[test]
    fn incompatible_inventory_enters_and_leaves_overview_with_notices_without_activation() {
        let root =
            std::env::temp_dir().join(format!("rad-overview-inventory-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut server =
            Server::new(&root, &root.join("data"), NonZeroU128::new(811).unwrap()).unwrap();
        let package_root = root.join("inventory");
        std::fs::create_dir(&package_root).unwrap();
        std::fs::set_permissions(&package_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut manager =
            rad_plugin_manager::Manager::create(&package_root, 1, rad_plugin_host::TARGET).unwrap();
        let mut manifest: Value = serde_json::from_slice(include_bytes!(
            "../../../rad-plugin-host/fixture/manifest-v1.json"
        ))
        .unwrap();
        manifest["id"] = json!("org.rad.audio");
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let executable = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let candidate = rad_plugin_package::Candidate::from_parts(
            &manifest,
            &executable,
            1,
            rad_plugin_host::TARGET,
        )
        .unwrap();
        let package = rad_plugin_package::encode(&candidate);
        let mut change = Some(
            manager
                .prepare(rad_plugin_manager::Operation::Install, Some(&package))
                .unwrap(),
        );
        assert_eq!(
            manager.commit(&mut change).unwrap(),
            rad_plugin_manager::CommitOutcome::Applied
        );
        drop(manager);
        std::fs::write(
            package_root.join("v0000000000000001/manifest.json"),
            include_bytes!("../../../rad-plugin-manager/tests/fixtures/audio-prerelease.json"),
        )
        .unwrap();
        let mut admin = crate::plugin_admin::Admin::new(
            package_root,
            Arc::new(|| {}),
            Box::new(|_, _| panic!("no downloads")),
        );
        let mut host = rad_plugin_host::Host::default();
        let end = std::time::Instant::now() + Duration::from_secs(3);
        while admin.status(&host)["items"].as_array().unwrap().is_empty() {
            admin.poll(&mut server.actor, &mut host, 0);
            assert!(std::time::Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        // Let the normal startup registration reject the incompatible package.
        admin.poll(&mut server.actor, &mut host, 0);
        assert_eq!(server.actor.plugin_manifests().count(), 0);
        assert!(host.state("org.rad.audio").is_none());
        server.publish_notices().unwrap();
        let mut socket = UnixStream::connect(root.join("rad/editor.sock")).unwrap();
        socket.set_nonblocking(true).unwrap();
        let hello = br#"{"v":1,"id":"1","op":"hello","args":{}}"#;
        socket
            .write_all(&(hello.len() as u32).to_be_bytes())
            .unwrap();
        socket.write_all(hello).unwrap();
        server.transport.drive(1).unwrap();
        let incoming = server.transport.take_next().unwrap();
        let connection = incoming.key.connection();
        server.connections.insert(connection);
        assert!(server.transport.subscribe(connection));
        server.notices.subscribers.insert(connection);
        server.plugin_admin = Some(admin);
        for present in [true, false] {
            if !present {
                server.plugin_admin = None;
            }
            let overview = server.overview();
            let value = serde_json::to_value(&overview).unwrap();
            if present {
                assert_eq!(
                    value["plugins"],
                    json!([{"id":"org.rad.audio","code":"needs_update"}])
                );
            } else {
                assert!(overview.plugins.is_empty());
            }
            assert!(serde_json::to_vec(&overview).unwrap().len() < 48 * 1024);
            assert!(
                rad_api::ResponseData::decode(rad_api::Operation::Overview, Status::Ok, value)
                    .is_ok()
            );
            server.publish_notices().unwrap();
            server.transport.drive(if present { 2 } else { 3 }).unwrap();
            let mut bytes = [0; 8192];
            let n = socket.read(&mut bytes).unwrap();
            let mut rest = &bytes[..n];
            let mut kinds = BTreeSet::new();
            while !rest.is_empty() {
                let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
                let notice =
                    rad_api::Notice::decode(serde_json::from_slice(&rest[4..4 + len]).unwrap())
                        .unwrap();
                kinds.insert(notice.kind);
                rest = &rest[4 + len..];
            }
            assert!(kinds.contains(&NoticeKind::Overview));
            assert!(kinds.contains(&NoticeKind::Plugins));
        }
        drop(server);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn subscribing_from_a_connection_that_already_left_is_ignored() {
        let root = std::env::temp_dir().join(format!("rad-subscribe-gone-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut server =
            Server::new(&root, &root.join("data"), NonZeroU128::new(812).unwrap()).unwrap();
        let mut socket = UnixStream::connect(root.join("rad/editor.sock")).unwrap();
        let frame = |value: Value| {
            let bytes = serde_json::to_vec(&value).unwrap();
            let mut framed = (bytes.len() as u32).to_be_bytes().to_vec();
            framed.extend(bytes);
            framed
        };
        let hello = frame(json!({"v":1,"id":"1","op":"hello","args":{}}));
        socket.write_all(&hello).unwrap();
        // The accept and the read may each take a readiness round of their own: drive until
        // the hello is taken, on a clock that only moves forward.
        let mut now = 0;
        let key = loop {
            now += 1;
            server.transport.drive(now).unwrap();
            if let Some(incoming) = server.transport.take_next() {
                break incoming.key;
            }
            assert!(now < 1000, "the hello never arrived");
            server
                .transport
                .wait_readiness(Duration::from_millis(1))
                .unwrap();
        };
        // The peer leaves before its request is looked at.
        drop(socket);
        while server.transport.connection_ids().next().is_some() {
            now += 1;
            assert!(now < 2000, "the closed connection was never noticed");
            server
                .transport
                .wait_readiness(Duration::from_millis(1))
                .unwrap();
            server.transport.drive(now).unwrap();
        }
        let mut wire = rad_wire::Session::default();
        wire.accept(rad_wire::read_frame(&hello).unwrap().unwrap())
            .unwrap();
        let subscribe = frame(json!({"v":1,"id":"2","op":"subscribe","args":{}}));
        let request = wire
            .accept(rad_wire::read_frame(&subscribe).unwrap().unwrap())
            .unwrap();
        let context = IncomingContext {
            request_id: key.id(),
            connection: key.connection(),
            origin: Origin::Editor,
        };
        assert_eq!(server.client_api(key, context, &request), Ok(true));
        assert!(server.notices.subscribers.is_empty());
        drop(server);
        // Workers may still be ending: a leftover file must not fail the test.
        let _ = std::fs::remove_dir_all(root);
    }
}
