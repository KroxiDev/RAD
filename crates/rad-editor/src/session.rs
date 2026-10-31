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
    pub cause: Option<String>,
    /// Set when the runtime refused an update from another source until it is confirmed.
    pub source_change: Option<crate::plugins::SourceChange>,
    pub unknown: bool,
}
impl Error {
    pub fn protocol() -> Self {
        Self {
            code: "invalid_request".into(),
            cause: None,
            source_change: None,
            unknown: false,
        }
    }
}
pub trait Port {
    fn call(&mut self, request: Request) -> Result<Response, Failure>;
}
impl Port for Client {
    fn call(&mut self, request: Request) -> Result<Response, Failure> {
        Client::call(self, request)
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
                cause: None,
                source_change: None,
                unknown: false,
            });
        }
        match self.port.call(request) {
            Err(e) => {
                self.closed = true;
                Err(Error {
                    code: e.code.into(),
                    cause: None,
                    source_change: None,
                    unknown: e.outcome == Some(Outcome::Unknown),
                })
            }
            Ok(r) if r.status == Status::Error => {
                let e = r.error.ok_or_else(Error::protocol)?;
                Err(Error {
                    code: e.code.to_string(),
                    cause: e.details["cause"].as_str().map(str::to_owned),
                    source_change: (e.code == "source_changed")
                        .then(|| crate::plugins::SourceChange::from_details(&e.details))
                        .flatten(),
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
/// Client projection of the only currently supported model, never domain state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    columns: usize,
    rows: usize,
}
impl Grid {
    pub const XL: Self = Self {
        columns: 8,
        rows: 4,
    };
    pub fn from_status(status: &Value) -> Result<Self, Error> {
        let grid = &status["grid"];
        let columns = grid["columns"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(Error::protocol)?;
        let rows = grid["rows"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(Error::protocol)?;
        if columns == 0 || rows == 0 || columns.checked_mul(rows).is_none_or(|n| n > 256) {
            return Err(Error::protocol());
        }
        Ok(Self { columns, rows })
    }
    pub const fn columns(self) -> usize {
        self.columns
    }
    pub const fn rows(self) -> usize {
        self.rows
    }
    pub const fn key_count(self) -> usize {
        self.columns * self.rows
    }
}
#[derive(Debug, Clone)]
pub struct Board {
    pub grid: Grid,
    pub folder: Value,
    pub buttons: Vec<KeyView>,
}
#[derive(Debug, Clone)]
pub struct Refresh {
    pub status: Value,
    pub board: Option<Board>,
    /// The selected key's inspector, read with a board of a newer revision.
    pub inspection: Option<Inspection>,
}
/// The key selected in the editor, and the revision its inspector on screen was read at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    pub id: String,
    pub revision: Option<String>,
}
impl<P: Port> Session<P> {
    pub fn refresh(&mut self, force: bool) -> Result<Refresh, Error> {
        self.refresh_views(force || self.revision.is_none(), true, None)
    }
    /// Notices invalidate independent views; a board-only update never fetches the overview.
    /// A board of another revision brings the selected key's inspector with it, so the two
    /// change on screen together instead of the inspector going blank in between.
    pub fn refresh_views(
        &mut self,
        board: bool,
        overview: bool,
        selected: Option<&Selected>,
    ) -> Result<Refresh, Error> {
        let status = if overview {
            self.overview()?
        } else {
            Value::Null
        };
        let board = if board { Some(self.board()?) } else { None };
        let inspection = match (&board, selected) {
            (Some(board), Some(selected))
                if board.folder["view_revision"].as_str() != selected.revision.as_deref()
                    && board
                        .buttons
                        .iter()
                        .any(|key| key.entity["id"] == selected.id.as_str()) =>
            {
                match self.inspect(&selected.id) {
                    Ok(inspection) => Some(inspection),
                    // Refused or already newer: it is read again on its own.
                    Err(_) if self.is_open() => None,
                    Err(error) => return Err(error),
                }
            }
            _ => None,
        };
        Ok(Refresh {
            status,
            board,
            inspection,
        })
    }
    pub fn overview(&mut self) -> Result<Value, Error> {
        let rad_api::ResponseData::Overview(overview) = self.request(Request::Overview {})?.data
        else {
            return Err(Error::protocol());
        };
        let mut status = serde_json::to_value(&overview).map_err(|_| Error::protocol())?;
        status["storage_state"] = status["storage"].clone();
        status["deck_state"] = status["deck"].clone();
        status["recovered_previous"] =
            json!(overview.last_restart == rad_api::RestartState::RecoveredPrevious);
        Ok(status)
    }
    pub fn board(&mut self) -> Result<Board, Error> {
        let board = self.read_board(None)?;
        self.revision = board.folder["view_revision"].as_str().map(str::to_owned);
        Ok(board)
    }
    /// A folder other than the one on show, read ahead so changing to it shows it complete.
    pub fn folder_board(&mut self, folder: &str) -> Result<Board, Error> {
        self.read_board(Some(folder.into()))
    }
    fn read_board(&mut self, folder_id: Option<String>) -> Result<Board, Error> {
        let rad_api::ResponseData::Board(view) = self.request(Request::Board { folder_id })?.data
        else {
            return Err(Error::protocol());
        };
        let grid = Grid::from_status(&json!({"grid":view.grid}))?;
        if view.keys.len() != grid.key_count()
            || view.folder.view_revision != view.view_revision
            || view
                .keys
                .iter()
                .flatten()
                .any(|key| key.entity.view_revision != view.view_revision)
        {
            return Err(Error::protocol());
        }
        Ok(Board {
            grid,
            folder: serde_json::to_value(view.folder).map_err(|_| Error::protocol())?,
            buttons: view
                .keys
                .into_iter()
                .flatten()
                .map(|key| {
                    Ok(KeyView {
                        entity: serde_json::to_value(key.entity).map_err(|_| Error::protocol())?,
                        appearance: serde_json::to_value(key.appearance)
                            .map_err(|_| Error::protocol())?,
                    })
                })
                .collect::<Result<_, Error>>()?,
        })
    }
}

fn same_revision(data: &Value, revision: &str) -> Result<(), Error> {
    if data["view_revision"].as_str() != Some(revision) {
        return Err(Error {
            code: "cursor_expired".into(),
            cause: None,
            source_change: None,
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
        let rad_api::ResponseData::Inspector(view) =
            self.request(Request::Inspector { id: id.into() })?.data
        else {
            return Err(Error::protocol());
        };
        if view.actions.len() > MAX_ACTIONS {
            return Err(Error::protocol());
        }
        let button = serde_json::to_value(view.entity).map_err(|_| Error::protocol())?;
        same_revision(&button, &view.view_revision)?;
        let actions = view
            .actions
            .into_iter()
            .map(|action| serde_json::to_value(action).map_err(|_| Error::protocol()))
            .collect::<Result<Vec<_>, _>>()?;
        for action in &actions {
            if action["visibility"] != "restricted" {
                same_revision(action, &view.view_revision)?;
            }
        }
        Ok(Inspection {
            button,
            appearance: serde_json::to_value(view.appearance).map_err(|_| Error::protocol())?,
            actions,
        })
    }
}

impl<P: Port> Session<P> {
    pub fn media_bytes(&mut self, id: &str, revision: &str) -> Result<Vec<u8>, Error> {
        same_entity_revision(
            &self.request(Request::Get { id: id.into() })?.data,
            revision,
        )?;
        let mut bytes = Vec::new();
        loop {
            let data = self
                .request(Request::MediaRead {
                    id: id.into(),
                    offset: bytes.len().to_string(),
                })?
                .data;
            let rad_api::ResponseData::TransferChunk(data) = data else {
                return Err(Error::protocol());
            };
            let chunk = crate::transfer::decode(&data.base64)?;
            let eof = data.eof;
            if bytes.len() + chunk.len() > 512 * 1024 || (chunk.is_empty() && !eof) {
                return Err(Error::protocol());
            }
            bytes.extend(chunk);
            if eof {
                break;
            }
        }
        same_entity_revision(
            &self.request(Request::Get { id: id.into() })?.data,
            revision,
        )?;
        Ok(bytes)
    }
}

/// Separate composed responses may be shown together only at the same revision.
pub fn coherent(board: &Board, inspection: &Inspection) -> bool {
    board.folder["view_revision"]
        .as_str()
        .is_some_and(|revision| inspection.button["view_revision"].as_str() == Some(revision))
}

fn same_entity_revision(data: &rad_api::ResponseData, revision: &str) -> Result<(), Error> {
    let rad_api::ResponseData::Entity(entity) = data else {
        return Err(Error::protocol());
    };
    if entity.view_revision != revision {
        return Err(Error {
            code: "cursor_expired".into(),
            cause: None,
            source_change: None,
            unknown: false,
        });
    }
    Ok(())
}
