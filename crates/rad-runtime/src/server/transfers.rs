use super::*;
use crate::{
    storage::JobId,
    transfers::{Context, Kind, Operation, Output},
};
use rad_authority::{Command, EntityView, ExportLease, Visible};
use rad_core::{
    configuration::{Candidate, ConfigurationError},
    identity::ShortId,
};

/// The folder on show and the revision it was read at.
pub(super) type Shown = (
    Option<rad_authority::ViewRevision>,
    rad_core::identity::Binding,
);
pub(super) struct Flight {
    job: JobId,
    target: Option<(RequestKey, IncomingContext)>,
    kind: FlightKind,
}
pub(super) enum FlightKind {
    Reply {
        mutation: bool,
        media: Option<(ShortId, String)>,
    },
    Export(ExportLease),
    Finish(String),
    Restore(String),
    Retain(Command),
    Internal,
    Visual(String),
}
impl Flight {
    pub(super) fn prepares(&self) -> bool {
        matches!(self.kind, FlightKind::Restore(_) | FlightKind::Retain(_))
    }
}
impl Server {
    fn start_transfer(
        &mut self,
        target: Option<(RequestKey, IncomingContext)>,
        operation: Operation,
        kind: FlightKind,
    ) -> Result<(), RuntimeError> {
        let context = target.map(|(_, c)| c).unwrap_or(IncomingContext {
            request_id: 0,
            connection: 0,
            origin: Origin::Editor,
        });
        let submitted = if self.transfer.is_none() {
            self.persistence.submit_transfer(
                Context {
                    connection: context.connection,
                    origin: context.origin,
                    now_ms: monotonic_ms().map_err(|_| RuntimeError::Clock)?,
                },
                operation,
            )
        } else {
            Err(crate::storage::WorkerError::Busy)
        };
        match submitted {
            Ok(job) => {
                self.transfer = Some(Flight { job, target, kind });
                Ok(())
            }
            Err(_) => {
                let mutation = !matches!(
                    &kind,
                    FlightKind::Reply {
                        mutation: false,
                        ..
                    }
                );
                if let FlightKind::Export(lease) = kind {
                    self.actor.release_export(lease);
                }
                if let Some((key, context)) = target {
                    self.respond(key, self.error(context, "resource_exhausted", mutation))?;
                }
                Ok(())
            }
        }
    }
    /// Starts an export from its pinned lease and the snapshot made from it. A snapshot that
    /// cannot be made fails this export only: the lease goes back and the client is told.
    pub(super) fn export_begin(
        &mut self,
        c: IncomingContext,
        size: Option<u64>,
        lease: ExportLease,
        snapshot: Result<Candidate, ConfigurationError>,
    ) -> Result<(Operation, FlightKind), Box<Response>> {
        let Ok(snapshot) = snapshot else {
            self.actor.release_export(lease);
            self.events.record(EventCode::ExportFailed, None);
            return Err(Box::new(self.error(c, "resource_exhausted", true)));
        };
        Ok((
            Operation::Begin {
                kind: Kind::Export,
                size,
                export_revision: Some(lease.revision()),
                export_snapshot: Some(snapshot),
            },
            FlightKind::Export(lease),
        ))
    }
    pub(super) fn transfer_external(
        &mut self,
        key: RequestKey,
        c: IncomingContext,
        external: External,
    ) -> Result<(), RuntimeError> {
        let reply = |mutation| FlightKind::Reply {
            mutation,
            media: None,
        };
        let (op, kind) = match external {
            External::TransferBegin { kind, size } => {
                let kind = match kind {
                    rad_protocol::TransferKind::ImageImport => Kind::ImageImport,
                    rad_protocol::TransferKind::Restore => Kind::Restore,
                    rad_protocol::TransferKind::Export => Kind::Export,
                };
                if kind == Kind::Export {
                    let lease = match self.actor.pin_export(c.connection, c.origin) {
                        Ok(p) => p,
                        Err(_) => {
                            return self.respond(key, self.error(c, "resource_exhausted", true));
                        }
                    };
                    let snapshot = crate::transfers::export_snapshot(
                        lease.snapshot(),
                        self.plugin_admin
                            .as_ref()
                            .map(|admin| admin.export_dependencies())
                            .unwrap_or_default(),
                    );
                    match self.export_begin(c, size, lease, snapshot) {
                        Ok(begun) => begun,
                        Err(response) => return self.respond(key, *response),
                    }
                } else {
                    (
                        Operation::Begin {
                            kind,
                            size,
                            export_revision: None,
                            export_snapshot: None,
                        },
                        reply(true),
                    )
                }
            }
            External::TransferWrite {
                transfer_id,
                offset,
                bytes,
            } => (
                Operation::Write {
                    transfer_id,
                    offset,
                    bytes,
                },
                reply(true),
            ),
            External::TransferFinish { transfer_id } => (
                Operation::Finish {
                    transfer_id: transfer_id.clone(),
                },
                FlightKind::Finish(transfer_id),
            ),
            External::TransferRead {
                transfer_id,
                offset,
            } => {
                if let Some((owner, lease)) = self.exports.get(&transfer_id)
                    && *owner == c.connection
                {
                    let _ = self.actor.touch_export(lease);
                }
                (
                    Operation::Read {
                        transfer_id,
                        offset,
                    },
                    reply(false),
                )
            }
            External::TransferDrop { transfer_id } => {
                (Operation::Drop { transfer_id }, reply(true))
            }
            External::RestoreCommit {
                transfer_id,
                confirmed,
            } => (
                Operation::RestoreData {
                    transfer_id: transfer_id.clone(),
                    confirmed,
                },
                FlightKind::Restore(transfer_id),
            ),
            External::MediaRead {
                id,
                media_ref,
                offset,
            } => (
                Operation::MediaRead {
                    resolved_key: media_ref.clone(),
                    offset,
                },
                FlightKind::Reply {
                    mutation: false,
                    media: Some((id, media_ref)),
                },
            ),
            _ => return Err(RuntimeError::Protocol),
        };
        self.start_transfer(Some((key, c)), op, kind)
    }
    pub(super) fn command(
        &mut self,
        key: RequestKey,
        c: IncomingContext,
        command: Command,
        media: Option<String>,
    ) -> Result<(), RuntimeError> {
        if let Some(media_ref) = media {
            return self.start_transfer(
                Some((key, c)),
                Operation::RetainImage { media_ref },
                FlightKind::Retain(command),
            );
        }
        self.submit_command(key, c, command, None)
    }
    fn submit_command(
        &mut self,
        key: RequestKey,
        c: IncomingContext,
        command: Command,
        token: Option<String>,
    ) -> Result<(), RuntimeError> {
        match self.actor.reserve(c.connection, c.origin) {
            Ok(ticket) => {
                if let Some(token) = token
                    && self
                        .persistence
                        .bind_media(ticket.request(), c.connection, token.clone())
                        .is_err()
                {
                    self.persistence
                        .release_unbound(token)
                        .map_err(|_| RuntimeError::Storage)?;
                    // Release the reserved credit through the actor's normal rejected-command path.
                    let effects = self.actor.submit(
                        ticket,
                        Command::PrepareEdits(vec![Box::new(|_| {
                            Err(rad_core::configuration::ConfigurationError::InvalidValue)
                        })]),
                    );
                    self.effects(effects)?;
                    return self.respond(key, self.error(c, "resource_exhausted", true));
                }
                self.requests.insert(
                    ticket.request(),
                    ReplyTarget {
                        key,
                        context: c,
                        sent: false,
                    },
                );
                let effects = self.actor.submit(ticket, command);
                self.effects(effects)
            }
            Err(_) => {
                if let Some(token) = token {
                    self.persistence
                        .release_unbound(token)
                        .map_err(|_| RuntimeError::Storage)?;
                }
                self.respond(key, self.error(c, "resource_exhausted", true))
            }
        }
    }
    pub(super) fn prepared_image(
        &self,
        c: IncomingContext,
        request: &rad_wire::Request,
    ) -> Option<String> {
        if request.operation() != "edit" {
            return None;
        }
        request
            .args()
            .get("changes")?
            .as_array()?
            .iter()
            .filter(|v| v["field"] == "appearance")
            .filter_map(|v| v["value"]["image"].as_str())
            .find(|key| {
                self.prepared
                    .contains_key(&(c.connection, (*key).to_owned()))
            })
            .map(str::to_owned)
    }
    pub(super) fn poll_transfers(&mut self) -> Result<(), RuntimeError> {
        for token in self.persistence.take_released() {
            if let Some((_, lease)) = self.exports.remove(&token) {
                self.actor.release_export(lease);
            }
            self.prepared.retain(|_, value| *value != token);
        }
        let Some((job, result)) = self.persistence.take_transfer() else {
            return Ok(());
        };
        let flight = self.transfer.take().ok_or(RuntimeError::Storage)?;
        if job != flight.job {
            return Err(RuntimeError::Storage);
        }
        let kind = flight.kind;
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                let mutation = !matches!(
                    kind,
                    FlightKind::Reply {
                        mutation: false,
                        ..
                    }
                );
                if let FlightKind::Visual(key) = &kind {
                    self.picture_arrived(key.clone(), None);
                }
                if let FlightKind::Export(lease) = kind {
                    self.actor.release_export(lease);
                }
                if let Some((key, c)) = flight.target {
                    let response =
                        self.error_with_details(c, error.wire_code(), error.details(), mutation);
                    return self.respond(key, response);
                }
                return Ok(());
            }
        };
        if let FlightKind::Visual(key) = kind {
            if let Output::Visual { image } = output {
                self.picture_arrived(key, Some(Arc::new(image)));
                return Ok(());
            }
            return Err(RuntimeError::Storage);
        }
        let Some((key, c)) = flight.target else {
            return Ok(());
        };
        use rad_api::{ResponseData as D, TransferChunk};
        let data = match output {
            Output::Begun { transfer_id } => {
                if let FlightKind::Export(lease) = kind {
                    self.exports
                        .insert(transfer_id.clone(), (c.connection, lease));
                }
                D::Transfer(rad_api::Transfer { transfer_id })
            }
            Output::ImagePrepared { media_ref } => {
                if let FlightKind::Finish(token) = kind {
                    self.prepared
                        .insert((c.connection, media_ref.clone()), token);
                }
                D::ImagePrepared(rad_api::ImagePrepared { media_ref })
            }
            Output::Chunk { bytes, eof } => {
                if let FlightKind::Reply {
                    media: Some((id, ref media_ref)),
                    ..
                } = kind
                    && !self
                        .actor
                        .appearance(c.origin, id)
                        .is_some_and(|v| v.appearance.image.as_ref() == Some(media_ref))
                {
                    return self.respond(key, self.error(c, "not_found", false));
                }
                D::TransferChunk(TransferChunk {
                    base64: encode(&bytes),
                    eof,
                })
            }
            Output::RestoreData { data } => {
                let FlightKind::Restore(token) = kind else {
                    return Err(RuntimeError::Protocol);
                };
                return self.submit_command(
                    key,
                    c,
                    Command::RestorePortable {
                        installation: data.identity.installation,
                        counters: data.identity.counters,
                        data: data.data,
                        confirmed: true,
                    },
                    Some(token),
                );
            }
            Output::Retained { transfer_id } => {
                let FlightKind::Retain(command) = kind else {
                    return Err(RuntimeError::Protocol);
                };
                return self.submit_command(key, c, command, Some(transfer_id));
            }
            other => other.into_data().ok_or(RuntimeError::Storage)?,
        };
        self.respond(
            key,
            Response {
                v: 1,
                id: Some(c.request_id.to_string()),
                session_id: Some(format!("{:032x}", self.session)),
                status: Status::Ok,
                data,
                error: None,
            },
        )
    }
    pub(super) fn maintain_transfers(&mut self) -> Result<(), RuntimeError> {
        if self.transfer.is_some() || !self.persistence.can_submit_transfer() {
            return Ok(());
        }
        let now = monotonic_ms().map_err(|_| RuntimeError::Clock)?;
        if let Some(metadata) = self.persistence.transfer_metadata() {
            if let Some(connection) = metadata
                .handles
                .iter()
                .find(|h| !h.pending && !self.connections.contains(&h.connection))
                .map(|h| h.connection)
            {
                let job = self
                    .persistence
                    .submit_transfer(
                        Context {
                            connection,
                            origin: Origin::Editor,
                            now_ms: now,
                        },
                        Operation::Disconnect,
                    )
                    .map_err(|_| RuntimeError::Storage)?;
                self.transfer = Some(Flight {
                    job,
                    target: None,
                    kind: FlightKind::Internal,
                });
                return Ok(());
            }
            if metadata.next_deadline_ms.is_some_and(|t| t <= now) {
                return self.start_transfer(None, Operation::Reap, FlightKind::Internal);
            }
        }
        if self.deck.is_some() {
            let shown = self.visual_keys();
            self.follow_nearby_pictures();
            let nearby = self.nearby_pictures.as_ref().map(|(_, nearby)| nearby);
            if let Some(key) = self
                .pictures
                .next(&shown, nearby.unwrap_or(&BTreeSet::new()))
            {
                return self.start_transfer(
                    None,
                    Operation::LoadVisual {
                        resolved_key: key.clone(),
                    },
                    FlightKind::Visual(key),
                );
            }
        }
        Ok(())
    }
    /// A picture arrived, or could not be loaded. The folder on show is repainted once its
    /// pictures are all in, so they appear together; one for later repaints nothing.
    fn picture_arrived(&mut self, key: String, image: Option<Arc<rad_media::NormalizedImage>>) {
        let shown = self.visual_keys();
        let on_show = shown.contains(&key);
        match image {
            Some(image) => self.pictures.loaded(key, image),
            None => self.pictures.fail(key),
        }
        self.display_dirty |= on_show && !self.pictures.loading(&shown);
    }
    fn visual_keys(&self) -> BTreeSet<String> {
        self.actor
            .status(Origin::Editor)
            .active
            .map(|active| self.folder_pictures(active))
            .unwrap_or_default()
    }
    /// Pictures of the folders one press away from the one on show: its parent and the
    /// folders its keys enter. Worked out again only when the folder or revision changes.
    fn follow_nearby_pictures(&mut self) {
        let status = self.actor.status(Origin::Editor);
        let Some(active) = status.active else {
            self.nearby_pictures = None;
            return;
        };
        let shown = (status.revision, active);
        if self
            .nearby_pictures
            .as_ref()
            .is_some_and(|(at, _)| *at == shown)
        {
            return;
        }
        let mut folders = Vec::new();
        if let Some(folder) = self.actor.folder(Origin::Editor, active.short()) {
            let entered = folder.slots.iter().flatten().flat_map(|slot| match slot {
                Visible::Visible(button) => match self.actor.get(Origin::Editor, button.short()) {
                    Some(EntityView::Button(button)) => button.actions,
                    _ => Vec::new(),
                },
                Visible::Restricted => Vec::new(),
            });
            let destinations = entered.filter_map(|action| match action {
                Visible::Visible(action) => action.destination,
                Visible::Restricted => None,
            });
            for destination in folder.parent_binding.into_iter().chain(destinations) {
                if destination != active && !folders.contains(&destination) {
                    folders.push(destination);
                }
            }
        }
        let pictures = folders
            .into_iter()
            .flat_map(|folder| self.folder_pictures(folder))
            .collect();
        self.nearby_pictures = Some((shown, pictures));
    }
    fn folder_pictures(&self, folder: rad_core::identity::Binding) -> BTreeSet<String> {
        self.actor
            .folder(Origin::Editor, folder.short())
            .into_iter()
            .flat_map(|f| f.slots)
            .flatten()
            .filter_map(|v| match v {
                rad_authority::Visible::Visible(b) => self
                    .actor
                    .appearance(Origin::Editor, b.short())
                    .and_then(|v| v.appearance.image),
                _ => None,
            })
            .collect()
    }
}
fn encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let a = c[0];
        let b = c.get(1).copied().unwrap_or(0);
        let d = c.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[(a >> 2) as usize] as char);
        out.push(ALPHABET[((a & 3) << 4 | b >> 4) as usize] as char);
        out.push(if c.len() > 1 {
            ALPHABET[((b & 15) << 2 | d >> 6) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            ALPHABET[(d & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
