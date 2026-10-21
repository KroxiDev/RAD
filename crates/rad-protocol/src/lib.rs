//! Pure IPC adapter; trusted endpoint origin, no I/O or second authority.
mod external;
mod plugins;
pub use external::{Capabilities, External, TransferKind};
pub use plugins::{OptionsSubject, PluginAdmin, PluginOperation, PluginOptions};
mod admin;
mod edits;
mod execution;
mod queries;
mod views;
use rad_api::response::{Error, Outcome, Response, Status};
use rad_authority::{Authority, Command, Effect, StorageState};
use rad_core::{
    configuration::Origin,
    identity::{EntityKind, ShortId},
    permissions::Permissions,
};
use serde_json::{Map, Value, json};
use std::num::NonZeroU128;
#[derive(Debug, Clone, Copy)]
/// Runtime supplies these values; never construct origin from JSON.
pub struct IncomingContext {
    pub request_id: u64,
    pub connection: u64,
    pub origin: Origin,
}
/// Runtime must process every Immediate effect through its normal effect sink.
pub enum Resolution {
    Immediate {
        response: Box<Response>,
        effects: Vec<Effect>,
    },
    Command(Command),
    External(External),
}
pub struct Protocol {
    session: NonZeroU128,
    capabilities: Capabilities,
}
type Checked<T> = Result<T, &'static str>;
impl Protocol {
    pub fn new(session: NonZeroU128) -> Self {
        Self {
            session,
            capabilities: Capabilities::default(),
        }
    }
    pub fn with_capabilities(session: NonZeroU128, capabilities: Capabilities) -> Self {
        Self {
            session,
            capabilities,
        }
    }
    pub fn handle(
        &self,
        a: &mut Authority,
        c: IncomingContext,
        r: &rad_wire::Request,
    ) -> Resolution {
        let mutation =
            rad_api::Operation::parse(r.operation()).is_some_and(rad_api::Operation::is_mutation);
        if c.request_id != r.id() {
            return self.error(c, "invalid_request", mutation);
        }
        if !r.args().iter().all(|(key, value)| {
            (r.operation() == "transfer_write" && key == "base64")
                || (r.operation() == "credential_replace" && key == "secret")
                || scalars_fit(value)
        }) {
            return self.error(c, "invalid_request", mutation);
        }
        let request = match rad_api::Request::parse(r.operation(), Value::Object(r.args().clone()))
        {
            Ok(request) => request,
            Err(_) => return self.error(c, "invalid_request", mutation),
        };
        self.handle_request(a, c, &request)
    }
    /// Structural decoding is performed once by rad-api. This layer only validates
    /// domain identifiers, capabilities, permissions and bounded domain values.
    pub fn handle_request(
        &self,
        a: &mut Authority,
        c: IncomingContext,
        request: &rad_api::Request,
    ) -> Resolution {
        use rad_api::Request as R;
        let mutation = request.operation().is_mutation();
        let result = (|| -> Checked<Resolution> {
            match request {
                R::Board { folder_id } => {
                    return Ok(self.immediate(
                        c,
                        rad_api::ResponseData::Board(views::board(a, c, folder_id.as_deref())?),
                        vec![],
                    ));
                }
                R::Inspector { id } => {
                    return Ok(self.immediate(
                        c,
                        rad_api::ResponseData::Inspector(views::inspector(a, c, id)?),
                        vec![],
                    ));
                }
                R::CredentialStatus { .. }
                | R::CredentialReplace { .. }
                | R::CredentialRemove { .. } => {
                    return Ok(Resolution::External(External::Credential(
                        credentials::convert(c, request)?,
                    )));
                }
                R::Options {
                    id,
                    type_key,
                    field,
                    cursor,
                    context,
                } => {
                    if let Some(type_key) = type_key {
                        if id.is_some() {
                            return Err("invalid_request");
                        }
                        return Ok(Resolution::External(External::PluginOptions(Box::new(
                            plugins::type_options(
                                a,
                                c.origin,
                                type_key,
                                field,
                                cursor.as_deref(),
                                context.as_ref(),
                            )?,
                        ))));
                    }
                    let target = id.as_deref().ok_or("invalid_request")?;
                    let entity = parse_id(target)?;
                    if a.get(c.origin, entity).is_none() {
                        return Err("not_found");
                    }
                    if a.plugin_configuration(c.origin, entity).is_some() {
                        return Ok(
                            match plugins::options(
                                a,
                                c,
                                target,
                                field,
                                cursor.as_deref(),
                                context.as_ref(),
                            )? {
                                plugins::Listing::Fixed(data) => self.immediate(
                                    c,
                                    rad_api::ResponseData::StaticOptions(
                                        serde_json::from_value(data)
                                            .map_err(|_| "invalid_configuration")?,
                                    ),
                                    vec![],
                                ),
                                plugins::Listing::Dynamic(query) => {
                                    Resolution::External(External::PluginOptions(query))
                                }
                            },
                        );
                    }
                    return Ok(Resolution::External(external::parse(
                        a,
                        c,
                        request,
                        self.capabilities,
                    )?));
                }
                R::PluginVisual { id } | R::PluginVisualRead { id, .. } => {
                    let id = parse_id(id)?;
                    if !matches!(
                        a.get(c.origin, id),
                        Some(rad_authority::EntityView::Button(_))
                    ) {
                        return Err("not_found");
                    }
                    return Ok(Resolution::External(match request {
                        R::PluginVisualRead {
                            revision, offset, ..
                        } => {
                            let offset = decimal(offset)?;
                            if offset > 256 * 1024 {
                                return Err("invalid_request");
                            }
                            External::PluginVisualRead {
                                id,
                                revision: decimal(revision)?,
                                offset: offset as usize,
                            }
                        }
                        _ => External::PluginVisual { id },
                    }));
                }
                R::PluginAdmin { .. } => {
                    return Ok(Resolution::External(External::PluginAdmin(plugins::admin(
                        c, request,
                    )?)));
                }
                R::PluginLog { plugin_id } => {
                    if c.origin != Origin::Editor {
                        return Err("permission_denied");
                    }
                    return Ok(Resolution::External(External::PluginLog {
                        plugin_id: plugin_id.clone(),
                    }));
                }
                R::PluginRetry { plugin_id } => {
                    if c.origin != Origin::Editor {
                        return Err("permission_denied");
                    }
                    return Ok(Resolution::External(External::PluginRetry {
                        plugin_id: plugin_id.clone(),
                    }));
                }
                R::PluginStatus {} => {
                    if c.origin != Origin::Editor {
                        return Err("permission_denied");
                    }
                    return Ok(Resolution::External(External::PluginStatus));
                }
                R::TransferBegin { .. }
                | R::TransferWrite { .. }
                | R::TransferFinish { .. }
                | R::TransferRead { .. }
                | R::TransferDrop { .. }
                | R::RestoreCommit { .. }
                | R::LauncherCatalog { .. }
                | R::MediaRead { .. } => {
                    return Ok(Resolution::External(external::parse(
                        a,
                        c,
                        request,
                        self.capabilities,
                    )?));
                }
                R::Run { id } => {
                    let id = parse_id(id)?;
                    if id.kind() == EntityKind::Folder {
                        return Err("invalid_request");
                    }
                    return Ok(Resolution::Command(Command::RunId { id }));
                }
                R::Navigate { folder_id } => {
                    if c.origin != Origin::Editor {
                        return Err("permission_denied");
                    }
                    let effects = a
                        .navigate_id(c.origin, parse_id(folder_id)?)
                        .map_err(|_| "not_found")?;
                    return Ok(self.immediate(
                        c,
                        rad_api::ResponseData::Empty(rad_api::Empty {}),
                        effects,
                    ));
                }
                R::SetPluginSettings { .. }
                | R::CreateButton { .. }
                | R::CreateFolder { .. }
                | R::AddAction { .. }
                | R::Edit { .. }
                | R::Duplicate { .. }
                | R::Delete { .. }
                | R::MoveFolder { .. }
                | R::SetPermissions { .. }
                | R::SetSettings { .. }
                | R::Undo {}
                | R::Redo {} => {
                    if c.origin != Origin::Editor && !matches!(request, R::Edit { .. }) {
                        return Err("permission_denied");
                    }
                    return Ok(Resolution::Command(edits::command(request)?));
                }
                _ => {}
            }
            Ok(self.immediate(
                c,
                rad_api::ResponseData::decode(
                    request.operation(),
                    Status::Ok,
                    self.query(a, c, request)?,
                )
                .map_err(|_| "invalid_configuration")?,
                vec![],
            ))
        })();
        result.unwrap_or_else(|code| self.error(c, code, mutation))
    }
    fn query(
        &self,
        a: &Authority,
        c: IncomingContext,
        request: &rad_api::Request,
    ) -> Checked<Value> {
        use rad_api::Request as R;
        match request {
            R::PluginManifest { plugin_id } => {
                if c.origin != Origin::Editor {
                    return Err("permission_denied");
                }
                let manifest = a
                    .plugin_manifests()
                    .find(|(m, _, _)| m.id == *plugin_id)
                    .ok_or("not_found")?
                    .0;
                Ok(json!({"plugin_id":plugin_id,"document":manifest}))
            }
            R::PluginSettings { plugin_id } => {
                if c.origin != Origin::Editor {
                    return Err("permission_denied");
                }
                Ok(
                    json!({"plugin_id":plugin_id,"encoding":"hex","hex":a.plugin_settings(c.origin,plugin_id).map(|bytes|bytes.iter().map(|b|format!("{b:02x}")).collect::<String>())}),
                )
            }
            R::PluginDependencies { cursor } => plugins::dependencies(a, c, cursor.as_deref()),
            R::PluginConfiguration { id, cursor } => {
                plugins::configuration(a, c, id, cursor.as_deref())
            }
            R::PluginCatalog { cursor } => plugins::catalog(a, c, cursor.as_deref()),
            R::Schema { type_key, cursor } => {
                let items = match type_key.as_str() {
                    "launcher" => {
                        json!([{"name":"desktop_entry","type":"string","required":true,"options":self.capabilities.launcher}])
                    }
                    "wait" => {
                        json!([{"name":"milliseconds","type":"integer","required":true,"minimum":0,"maximum":rad_core::configuration::MAX_WAIT_MS}])
                    }
                    _ => return plugins::schema(a, c, type_key, cursor.as_deref()),
                };
                if cursor.is_some() {
                    return Err("cursor_expired");
                }
                Ok(json!({"type":type_key,"items":items,"next_cursor":null}))
            }
            R::Result { execution_id } => execution::result(a, c, execution_id),
            R::Get { .. } | R::List { .. } | R::GetSection { .. } => queries::read(a, c, request),
            R::Hello {} => Ok(
                json!({"protocol":1,"session_id":format!("{:032x}",self.session),"capabilities":self.operations(c.origin),"limits":{"frame_bytes":65536,"page_items":32,"page_bytes":49152,"value_bytes":8192,
                "json_depth":16,"string_bytes":16384,"array_items":256,
                "connections":8,"automation_connections":6,"requests_per_connection":4,"requests_global":32,
                "output_frames_per_connection":16,"output_bytes_per_connection":1048576,
                "hello_ms":2000,"partial_frame_ms":2000,"idle_ms":60000,"write_ms":2000,
                "request_ms":10000,"queued_request_ms":5000,"active_executions":16,"live_jobs":32,
                "terminal_results":256,"terminal_bytes":4194304,"terminal_ttl_ms":600000,
                "result_bytes":12288,"error_message_bytes":256}}),
            ),
            R::Status {} => {
                let s = a.status(c.origin);
                let mut v = json!({"storage_state":match a.storage_state(){StorageState::Ready=>"ready",StorageState::Uncertain=>"uncertain",StorageState::RecoveryRequired=>"recovery_required"},"view_revision":s.revision.map(|r|r.value().to_string()),"active_folder_id":s.active.map(|b|short(b.short()))});
                v["grid"] = json!({"columns":s.grid.columns(),"rows":s.grid.rows()});
                if let Some(settings) = a.settings(c.origin) {
                    v["settings"] = json!({"brightness_percent":settings.brightness_percent,"sleep_ms":settings.sleep_ms.to_string(),"press_threshold_ms":settings.press_threshold_ms.to_string(),"long_press_threshold_ms":settings.long_press_threshold_ms.to_string()});
                    v["global_permissions"] =
                        permissions(a.global_permissions(c.origin).expect("Editor settings"));
                    v["history_operations"] = json!(
                        a.editor_view(c.origin)
                            .expect("Editor settings")
                            .history_operations
                    );
                }
                Ok(v)
            }
            _ => Err("invalid_request"),
        }
    }
    fn envelope(
        &self,
        c: IncomingContext,
        status: Status,
        data: rad_api::ResponseData,
        error: Option<Error>,
    ) -> Response {
        Response {
            v: 1,
            id: Some(c.request_id.to_string()),
            session_id: Some(format!("{:032x}", self.session)),
            status,
            data,
            error,
        }
    }
    fn immediate(
        &self,
        c: IncomingContext,
        data: rad_api::ResponseData,
        effects: Vec<Effect>,
    ) -> Resolution {
        let response = self.envelope(c, Status::Ok, data, None);
        if self.encode_response(&response).is_err() {
            return self.error(c, "resource_exhausted", false);
        }
        Resolution::Immediate {
            response: Box::new(response),
            effects,
        }
    }
    fn error(&self, c: IncomingContext, code: &str, mutation: bool) -> Resolution {
        Resolution::Immediate {
            response: Box::new(self.rejection(c, code, mutation)),
            effects: vec![],
        }
    }
    fn rejection(&self, c: IncomingContext, code: &str, mutation: bool) -> Response {
        self.envelope(
            c,
            Status::Error,
            rad_api::ResponseData::None,
            Some(Error {
                code: rad_api::ErrorCode::parse(code).expect("defined response code"),
                message: "Request rejected".into(),
                details: json!({}),
                outcome: mutation.then_some(Outcome::NotApplied),
            }),
        )
    }
}
fn short(id: ShortId) -> String {
    format!(
        "{}{}",
        match id.kind() {
            EntityKind::Folder => "f",
            EntityKind::Button => "b",
            EntityKind::Action => "a",
        },
        id.number()
    )
}
fn permissions(p: Permissions) -> Value {
    json!({"consult":p.consult(),"modify":p.modify(),"execute":p.execute()})
}

fn parse_id(s: &str) -> Checked<ShortId> {
    let kind = match s.as_bytes().first() {
        Some(b'f') => EntityKind::Folder,
        Some(b'b') => EntityKind::Button,
        Some(b'a') => EntityKind::Action,
        _ => return Err("invalid_request"),
    };
    let n = decimal(&s[1..])?;
    ShortId::new(kind, n).map_err(|_| "invalid_request")
}
fn decimal(s: &str) -> Checked<u64> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("invalid_request");
    };
    s.parse().map_err(|_| "invalid_request")
}
impl Protocol {
    /// Encode only the correlated response effect. Does not release ticket/transport credits.
    pub fn response_for(
        &self,
        a: &Authority,
        c: IncomingContext,
        expected: rad_authority::RequestId,
        e: &Effect,
    ) -> Option<Response> {
        use rad_authority::Rejection;
        let (request, status, data, error) = match e {
            Effect::Reply { request, changed } => {
                let revision = if c.origin == Origin::Editor {
                    a.editor_view(c.origin)?.revision.to_string()
                } else {
                    a.view_revision(c.origin)?.value().to_string()
                };
                (
                    *request,
                    Status::Applied,
                    rad_api::ResponseData::Applied(rad_api::Applied {
                        changed: *changed,
                        revision,
                    }),
                    None,
                )
            }
            Effect::Accepted { request, execution } => (
                *request,
                Status::Admitted,
                rad_api::ResponseData::Admitted(rad_api::Admitted {
                    execution_id: format!("{:032x}:{}", execution.session(), execution.counter()),
                }),
                None,
            ),
            Effect::Unknown { request } => (
                *request,
                Status::Error,
                rad_api::ResponseData::None,
                Some(("storage_uncertain", Outcome::Unknown)),
            ),
            Effect::Rejected { request, reason } => {
                let code = match reason {
                    Rejection::Execution(e) => execution::error_code(*e),
                    Rejection::Persistence => "storage_error",
                    Rejection::ResourceExhausted(_) => "resource_exhausted",
                    Rejection::Invalid => "invalid_request",
                    Rejection::Configuration(e) => configuration_error(e),
                    Rejection::StorageUncertain => "storage_uncertain",
                    Rejection::Expired => "deadline_exceeded",
                    Rejection::Capacity => "resource_exhausted",
                    Rejection::PublicationPending => "publication_pending",
                };
                (
                    *request,
                    Status::Error,
                    rad_api::ResponseData::None,
                    Some((code, Outcome::NotApplied)),
                )
            }
            _ => return None,
        };
        if request != expected {
            return None;
        }
        Some(self.envelope(
            c,
            status,
            data,
            error.map(|(code, outcome)| Error {
                code: rad_api::ErrorCode::parse(code).expect("defined response code"),
                message: "Request rejected".into(),
                details: rejection_details(e),
                outcome: Some(outcome),
            }),
        ))
    }
}

impl Protocol {
    fn operations(&self, origin: Origin) -> Vec<&'static str> {
        let mut ops = vec![
            "hello",
            "status",
            "get",
            "list",
            "get_section",
            "schema",
            "plugin_catalog",
            "plugin_configuration",
            "plugin_visual",
            "plugin_visual_read",
            "edit",
            "run",
            "result",
        ];
        if origin == Origin::Editor {
            ops.extend([
                "board",
                "inspector",
                "overview",
                "subscribe",
                "plugin_admin",
                "plugin_status",
                "plugin_retry",
                "plugin_log",
                "runtime_log",
                "plugin_dependencies",
                "plugin_manifest",
                "plugin_settings",
                "set_plugin_settings",
                "credential_status",
                "credential_replace",
                "credential_remove",
                "create_folder",
                "create_button",
                "add_action",
                "duplicate",
                "delete",
                "move_folder",
                "set_permissions",
                "navigate",
                "undo",
                "redo",
                "set_settings",
            ]);
            if self.capabilities.transfers {
                ops.extend([
                    "transfer_begin",
                    "transfer_write",
                    "transfer_finish",
                    "transfer_read",
                    "transfer_drop",
                    "restore_commit",
                ])
            }
            if self.capabilities.launcher {
                ops.push("launcher_catalog")
            }
        }
        if self.capabilities.launcher {
            ops.push("options")
        }
        if self.capabilities.media {
            ops.push("media_read")
        }
        ops
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseTooLarge;
impl Protocol {
    pub fn encode_response(&self, r: &Response) -> Result<Vec<u8>, ResponseTooLarge> {
        {
            let bytes = serde_json::to_vec(r).map_err(|_| ResponseTooLarge)?;
            if bytes.len() > 48 * 1024 {
                Err(ResponseTooLarge)
            } else {
                Ok(bytes)
            }
        }
    }
}

fn scalars_fit(value: &Value) -> bool {
    match value {
        Value::String(_) => serde_json::to_vec(value).is_ok_and(|b| b.len() <= 8192),
        Value::Array(v) => v.iter().all(scalars_fit),
        Value::Object(m) => m.values().all(scalars_fit),
        _ => true,
    }
}
fn configuration_error(e: &rad_core::configuration::ConfigurationError) -> &'static str {
    use rad_core::{configuration::ConfigurationError as E, identity::IdentityError as I};
    match e {
        E::PermissionDenied => "permission_denied",
        E::NotFound => "not_found",
        E::Occupied => "busy",
        E::NotAllowed | E::ProtectedNavigation | E::ConfirmationRequired => "operation_not_allowed",
        E::ResourceExhausted => "resource_exhausted",
        E::Identity(I::ShortIdExhausted | I::SerialExhausted) => "resource_exhausted",
        E::Identity(I::ShortIdCollision | I::IdentityCollision | I::ConflictingRetirement) => {
            "identity_collision"
        }
        _ => "invalid_configuration",
    }
}

mod credentials;
pub use credentials::CredentialOperation;

fn rejection_details(effect: &Effect) -> Value {
    use rad_authority::Rejection;
    use rad_core::{configuration::ConfigurationError as E, identity::IdentityError as I};
    let cause = match effect {
        Effect::Rejected {
            reason: Rejection::Configuration(E::ResourceExhausted),
            ..
        } => "entity_limit",
        Effect::Rejected {
            reason: Rejection::Configuration(E::Identity(I::ShortIdExhausted | I::SerialExhausted)),
            ..
        } => "identity_counter_exhausted",
        Effect::Rejected {
            reason: Rejection::ResourceExhausted(rad_authority::ResourceLimit::DocumentBytes),
            ..
        } => "document_limit",
        Effect::Rejected {
            reason: Rejection::ResourceExhausted(rad_authority::ResourceLimit::StorageCapacity),
            ..
        } => "storage_capacity",
        _ => return json!({}),
    };
    json!({"cause":cause})
}
