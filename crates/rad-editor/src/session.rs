//! Socket projection adapter. No Configuration or identity allocation lives here.
use crate::Request;
use rad_client::{
    connection::{Client, Failure},
    response::{Outcome, Response, Status},
};
use serde_json::{Value, json};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: String,
    pub unknown: bool,
}
impl Error {
    pub fn protocol() -> Self {
        Self {
            code: "invalid_request".into(),
            unknown: false,
        }
    }
}
pub trait Port {
    fn request(&mut self, op: &str, args: Value) -> Result<Response, Failure>;
}
impl Port for Client {
    fn request(&mut self, op: &str, args: Value) -> Result<Response, Failure> {
        Client::request(self, op, args)
    }
}
pub struct Session<P> {
    port: P,
    closed: bool,
    revision: Option<String>,
}
impl<P: Port> Session<P> {
    pub fn new(port: P) -> Self {
        Self {
            port,
            closed: false,
            revision: None,
        }
    }
    pub fn is_open(&self) -> bool {
        !self.closed
    }
    pub fn request(&mut self, request: Request) -> Result<Response, Error> {
        if self.closed {
            return Err(Error {
                code: "runtime_unavailable".into(),
                unknown: false,
            });
        }
        match self.port.request(&request.operation, request.arguments) {
            Err(e) => {
                self.closed = true;
                Err(Error {
                    code: e.code.into(),
                    unknown: e.outcome == Some(Outcome::Unknown),
                })
            }
            Ok(r) if r.status == Status::Error => {
                let e = r.error.ok_or_else(Error::protocol)?;
                Err(Error {
                    code: e.code,
                    unknown: e.outcome == Some(Outcome::Unknown),
                })
            }
            Ok(r) => Ok(r),
        }
    }
}
#[derive(Debug, Clone)]
pub struct KeyView {
    pub entity: Value,
    pub appearance: Value,
}
#[derive(Debug, Clone)]
pub struct Board {
    pub folder: Value,
    pub buttons: Vec<KeyView>,
}
#[derive(Debug, Clone)]
pub struct Refresh {
    pub status: Value,
    pub board: Option<Board>,
}
impl<P: Port> Session<P> {
    pub fn refresh(&mut self, force: bool) -> Result<Refresh, Error> {
        let status = self.request(Request::new("status", json!({})))?.data;
        let revision = status["view_revision"]
            .as_str()
            .ok_or_else(Error::protocol)?
            .to_owned();
        if !force && self.revision.as_ref() == Some(&revision) {
            return Ok(Refresh {
                status,
                board: None,
            });
        }
        let id = status["active_folder_id"]
            .as_str()
            .ok_or_else(Error::protocol)?;
        let folder = self.request(Request::new("get", json!({"id":id})))?.data;
        same_revision(&folder, &revision)?;
        let slots = folder["fields"]["slots"]
            .as_array()
            .filter(|s| s.len() == 32)
            .ok_or_else(Error::protocol)?;
        let mut buttons = Vec::new();
        for slot in slots {
            if let Some(id) = slot["id"].as_str() {
                let b = self.request(Request::new("get", json!({"id":id})))?.data;
                same_revision(&b, &revision)?;
                let section = self
                    .request(Request::new(
                        "get_section",
                        json!({"id":id,"section":"appearance","cursor":null}),
                    ))?
                    .data;
                same_revision(&section, &revision)?;
                if !section["next_cursor"].is_null() {
                    return Err(Error::protocol());
                }
                let mut appearance = serde_json::Map::new();
                for item in section["items"]
                    .as_array()
                    .filter(|v| v.len() <= 3)
                    .ok_or_else(Error::protocol)?
                {
                    let name = item["name"].as_str().ok_or_else(Error::protocol)?;
                    if appearance
                        .insert(name.into(), item["value"].clone())
                        .is_some()
                    {
                        return Err(Error::protocol());
                    }
                }
                if let Some(v) = section.get("visual_type") {
                    appearance.insert("visual_type".into(), v.clone());
                }
                buttons.push(KeyView {
                    entity: b,
                    appearance: Value::Object(appearance),
                });
            }
        }
        self.revision = Some(revision);
        Ok(Refresh {
            status,
            board: Some(Board { folder, buttons }),
        })
    }
}

fn same_revision(data: &Value, revision: &str) -> Result<(), Error> {
    if data["view_revision"].as_str() != Some(revision) {
        return Err(Error {
            code: "cursor_expired".into(),
            unknown: false,
        });
    }
    Ok(())
}
/// The runtime's per-button action limit; a longer list comes from a broken peer.
const MAX_ACTIONS: usize = 20;
#[derive(Debug, Clone)]
pub struct Inspection {
    pub button: Value,
    pub appearance: Value,
    pub actions: Vec<Value>,
}
impl<P: Port> Session<P> {
    pub fn inspect(&mut self, id: &str) -> Result<Inspection, Error> {
        let button = self.request(Request::new("get", json!({"id":id})))?.data;
        let revision = button["view_revision"]
            .as_str()
            .ok_or_else(Error::protocol)?;
        let mut appearance = serde_json::Map::new();
        let mut actions = vec![];
        let mut bytes = serde_json::to_vec(&button)
            .map_err(|_| Error::protocol())?
            .len();
        for section in ["appearance", "actions"] {
            let mut cursor = Value::Null;
            let mut seen = std::collections::BTreeSet::new();
            loop {
                let data = self
                    .request(Request::new(
                        "get_section",
                        json!({"id":id,"section":section,"cursor":cursor}),
                    ))?
                    .data;
                same_revision(&data, revision)?;
                if section == "appearance"
                    && let Some(v) = data.get("visual_type")
                {
                    appearance.insert("visual_type".into(), v.clone());
                }
                bytes += serde_json::to_vec(&data)
                    .map_err(|_| Error::protocol())?
                    .len();
                if bytes > 16 * 1024 * 1024 {
                    return Err(Error::protocol());
                }
                let items = data["items"]
                    .as_array()
                    .filter(|i| i.len() <= 32)
                    .ok_or_else(Error::protocol)?;
                for item in items {
                    if section == "appearance" {
                        let name = item["name"].as_str().ok_or_else(Error::protocol)?;
                        if appearance
                            .insert(name.into(), item["value"].clone())
                            .is_some()
                        {
                            return Err(Error::protocol());
                        }
                    } else {
                        actions.push(item.clone());
                    }
                }
                if actions.len() > MAX_ACTIONS {
                    return Err(Error::protocol());
                }
                cursor = data["next_cursor"].clone();
                if cursor.is_null() {
                    break;
                }
                let c = cursor.as_str().ok_or_else(Error::protocol)?;
                if !seen.insert(c.to_owned()) || seen.len() > 32 {
                    return Err(Error::protocol());
                }
            }
        }
        Ok(Inspection {
            button,
            appearance: Value::Object(appearance),
            actions,
        })
    }
}
impl<P: Port> Session<P> {
    pub fn media_bytes(&mut self, id: &str, revision: &str) -> Result<Vec<u8>, Error> {
        same_revision(
            &self.request(Request::new("get", json!({"id":id})))?.data,
            revision,
        )?;
        let mut bytes = Vec::new();
        loop {
            let data = self
                .request(Request::new(
                    "media_read",
                    json!({"id":id,"offset":bytes.len().to_string()}),
                ))?
                .data;
            let chunk =
                crate::transfer::decode(data["base64"].as_str().ok_or_else(Error::protocol)?)?;
            let eof = data["eof"].as_bool().ok_or_else(Error::protocol)?;
            if bytes.len() + chunk.len() > 512 * 1024 || (chunk.is_empty() && !eof) {
                return Err(Error::protocol());
            }
            bytes.extend(chunk);
            if eof {
                break;
            }
        }
        same_revision(
            &self.request(Request::new("get", json!({"id":id})))?.data,
            revision,
        )?;
        Ok(bytes)
    }
}
