//! Pure IPC adapter; trusted endpoint origin, no I/O or second authority.
mod external;
mod plugins;
pub use external::{Capabilities, External, TransferKind};
pub use plugins::{OptionsSubject, PluginAdmin, PluginOperation, PluginOptions};
mod admin;
mod edits;
mod execution;
mod queries;
use rad_authority::{Authority, Command, Effect, StorageState};
use rad_core::{
    configuration::Origin,
    identity::{EntityKind, ShortId},
    permissions::Permissions,
};
use rad_wire::response::{Error, Outcome, Response, Status};
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
        response: Response,
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
        let mutation = is_mutation(r.operation());
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
        if matches!(
            r.operation(),
            "credential_status" | "credential_replace" | "credential_remove"
        ) {
            return match credentials::parse(c, r.operation(), r.args()) {
                Ok(operation) => Resolution::External(External::Credential(operation)),
                Err(code) => self.error(c, code, mutation),
            };
        }
        if r.operation() == "options" {
            if r.args().contains_key("type") {
                return match plugins::type_options(a, c.origin, r.args()) {
                    Ok(query) => Resolution::External(External::PluginOptions(Box::new(query))),
                    Err(e) => self.error(c, e, false),
                };
            }
            let target = id(r.args(), "id").ok().and_then(|id| a.get(c.origin, id));
            if target.is_none() {
                return self.error(c, "not_found", false);
            }
            if id(r.args(), "id")
                .ok()
                .and_then(|id| a.plugin_configuration(c.origin, id))
                .is_some()
            {
                return match plugins::options(a, c, r.args()) {
                    Ok(plugins::Listing::Fixed(data)) => self.immediate(c, data, vec![]),
                    Ok(plugins::Listing::Dynamic(query)) => {
                        Resolution::External(External::PluginOptions(query))
                    }
                    Err(e) => self.error(c, e, false),
                };
            }
        }
        if matches!(r.operation(), "plugin_visual" | "plugin_visual_read") {
            let result = (|| {
                let id = id(r.args(), "id")?;
                if !matches!(
                    a.get(c.origin, id),
                    Some(rad_authority::EntityView::Button(_))
                ) {
                    return Err("not_found");
                }
                if r.operation() == "plugin_visual" {
                    closed(r.args(), &["id"])?;
                    Ok(External::PluginVisual { id })
                } else {
                    closed(r.args(), &["id", "revision", "offset"])?;
                    let offset = decimal(string(r.args(), "offset")?)?;
                    if offset > 256 * 1024 {
                        return Err("invalid_request");
                    }
                    Ok(External::PluginVisualRead {
                        id,
                        revision: decimal(string(r.args(), "revision")?)?,
                        offset: offset as usize,
                    })
                }
            })();
            return match result {
                Ok(op) => Resolution::External(op),
                Err(e) => self.error(c, e, false),
            };
        }
        if r.operation() == "plugin_admin" {
            return match plugins::admin(c, r.args()) {
                Ok(request) => Resolution::External(External::PluginAdmin(request)),
                Err(e) => self.error(c, e, true),
            };
        }
        if r.operation() == "plugin_status" {
            return if c.origin != Origin::Editor {
                self.error(c, "permission_denied", false)
            } else if closed(r.args(), &[]).is_err() {
                self.error(c, "invalid_request", false)
            } else {
                Resolution::External(External::PluginStatus)
            };
        }
        if matches!(
            r.operation(),
            "transfer_begin"
                | "transfer_write"
                | "transfer_finish"
                | "transfer_read"
                | "transfer_drop"
                | "restore_commit"
                | "launcher_catalog"
                | "options"
                | "media_read"
        ) {
            return match external::parse(a, c, r.operation(), r.args(), self.capabilities) {
                Ok(x) => Resolution::External(x),
                Err(e) => self.error(
                    c,
                    e,
                    r.operation().starts_with("transfer_") || r.operation() == "restore_commit",
                ),
            };
        }
        if r.operation() == "run" {
            let parsed = (|| {
                closed(r.args(), &["id"])?;
                let id = id(r.args(), "id")?;
                if id.kind() == EntityKind::Folder {
                    return Err("invalid_request");
                };
                Ok(Command::RunId { id })
            })();
            return match parsed {
                Ok(cmd) => Resolution::Command(cmd),
                Err(e) => self.error(c, e, true),
            };
        }
        if r.operation() == "navigate" {
            if c.origin != Origin::Editor {
                return self.error(c, "permission_denied", true);
            }
            let parsed = (|| {
                closed(r.args(), &["folder_id"])?;
                let id = id(r.args(), "folder_id")?;
                a.navigate_id(c.origin, id).map_err(|_| "not_found")
            })();
            return match parsed {
                Ok(effects) => self.immediate(c, json!({}), effects),
                Err(e) => self.error(c, e, true),
            };
        }
        if matches!(
            r.operation(),
            "create_button"
                | "create_folder"
                | "add_action"
                | "edit"
                | "duplicate"
                | "delete"
                | "move_folder"
                | "set_permissions"
                | "set_settings"
                | "undo"
                | "redo"
        ) {
            if c.origin != Origin::Editor && r.operation() != "edit" {
                return self.error(c, "permission_denied", true);
            }
            return match edits::command(r.operation(), r.args()) {
                Ok(cmd) => Resolution::Command(cmd),
                Err(e) => self.error(c, e, true),
            };
        }
        match self.query(a, c, r.operation(), r.args()) {
            Ok(data) => self.immediate(c, data, vec![]),
            Err(code) => self.error(c, code, false),
        }
    }
    fn query(
        &self,
        a: &Authority,
        c: IncomingContext,
        op: &str,
        args: &Map<String, Value>,
    ) -> Checked<Value> {
        if op == "plugin_dependencies" {
            return plugins::dependencies(a, c, args);
        }
        if op == "plugin_configuration" {
            return plugins::configuration(a, c, args);
        }
        if op == "plugin_catalog" {
            return plugins::catalog(a, c, args);
        }
        if op == "schema" {
            closed(args, &["type", "cursor"])?;
            let items = match string(args, "type")? {
                "launcher" => {
                    json!([{"name":"desktop_entry","type":"string","required":true,"options":self.capabilities.launcher}])
                }
                "wait" => {
                    json!([{"name":"milliseconds","type":"integer","required":true,"minimum":0,"maximum":rad_core::configuration::MAX_WAIT_MS}])
                }
                _ => return plugins::schema(a, c, args),
            };
            if args.get("cursor") != Some(&Value::Null) {
                return Err("cursor_expired");
            }
            return Ok(json!({"type":args["type"],"items":items,"next_cursor":null}));
        }
        if op == "result" {
            return execution::result(a, c, args);
        }
        if matches!(op, "get" | "list" | "get_section") {
            return queries::read(a, c, op, args);
        }
        closed(args, &[])?;
        match op {
            "hello" => Ok(
                json!({"protocol":1,"session_id":format!("{:032x}",self.session),"capabilities":self.operations(c.origin),"limits":{"frame_bytes":65536,"page_items":32,"page_bytes":49152,"value_bytes":8192,
                "json_depth":16,"string_bytes":16384,"array_items":256,
                "connections":8,"automation_connections":6,"requests_per_connection":4,"requests_global":32,
                "output_frames_per_connection":16,"output_bytes_per_connection":1048576,
                "hello_ms":2000,"partial_frame_ms":2000,"idle_ms":60000,"write_ms":2000,
                "request_ms":10000,"queued_request_ms":5000,"active_executions":16,"live_jobs":32,
                "terminal_results":256,"terminal_bytes":4194304,"terminal_ttl_ms":600000,
                "result_bytes":12288,"error_message_bytes":256}}),
            ),
            "status" => {
                let s = a.status(c.origin);
                let mut v = json!({"storage_state":match a.storage_state(){StorageState::Ready=>"ready",StorageState::Uncertain=>"uncertain",StorageState::RecoveryRequired=>"recovery_required"},"view_revision":s.revision.map(|r|r.value().to_string()),"active_folder_id":s.active.map(|b|short(b.short()))});
                v["grid"] = json!({"columns":s.grid.columns(),"rows":s.grid.rows()});
                if let Some(settings) = a.settings(c.origin) {
                    v["settings"] = json!({"brightness_percent":settings.brightness_percent,"sleep_ms":settings.sleep_ms.to_string(),"press_threshold_ms":settings.press_threshold_ms.to_string()});
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
        data: Value,
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
    fn immediate(&self, c: IncomingContext, data: Value, effects: Vec<Effect>) -> Resolution {
        let response = self.envelope(c, Status::Ok, data, None);
        if self.encode_response(&response).is_err() {
            return self.error(c, "resource_exhausted", false);
        }
        Resolution::Immediate { response, effects }
    }
    fn error(&self, c: IncomingContext, code: &str, mutation: bool) -> Resolution {
        Resolution::Immediate {
            response: self.rejection(c, code, mutation),
            effects: vec![],
        }
    }
    fn rejection(&self, c: IncomingContext, code: &str, mutation: bool) -> Response {
        self.envelope(
            c,
            Status::Error,
            Value::Null,
            Some(Error {
                code: code.into(),
                message: "Request rejected".into(),
                details: json!({}),
                outcome: mutation.then_some(Outcome::NotApplied),
            }),
        )
    }
}
fn closed(m: &Map<String, Value>, keys: &[&str]) -> Checked<()> {
    if m.keys().all(|k| keys.contains(&k.as_str())) {
        Ok(())
    } else {
        Err("invalid_request")
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

fn string<'a>(m: &'a Map<String, Value>, key: &str) -> Checked<&'a str> {
    m.get(key).and_then(Value::as_str).ok_or("invalid_request")
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
fn id(m: &Map<String, Value>, key: &str) -> Checked<ShortId> {
    parse_id(string(m, key)?)
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
                    json!({"changed":changed,"revision":revision}),
                    None,
                )
            }
            Effect::Accepted { request, execution } => (
                *request,
                Status::Admitted,
                json!({"execution_id":format!("{:032x}:{}",execution.session(),execution.counter())}),
                None,
            ),
            Effect::Unknown { request } => (
                *request,
                Status::Error,
                Value::Null,
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
                    Value::Null,
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
                code: code.into(),
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
                "plugin_admin",
                "plugin_status",
                "plugin_dependencies",
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
fn is_mutation(op: &str) -> bool {
    matches!(
        op,
        "credential_replace"
            | "credential_remove"
            | "plugin_admin"
            | "run"
            | "edit"
            | "create_folder"
            | "create_button"
            | "add_action"
            | "duplicate"
            | "delete"
            | "move_folder"
            | "set_permissions"
            | "navigate"
            | "undo"
            | "redo"
            | "set_settings"
            | "restore_commit"
    ) || op.starts_with("transfer_")
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
