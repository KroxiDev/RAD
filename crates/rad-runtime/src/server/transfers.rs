use super::*;
use crate::{
    storage::JobId,
    transfers::{Context, Kind, Operation, Output},
};
use rad_authority::{Command, ExportLease};
use rad_core::identity::ShortId;

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
                    (
                        Operation::Begin {
                            kind,
                            size,
                            export_revision: Some(lease.revision()),
                            export_snapshot: Some(
                                crate::transfers::export_snapshot(
                                    lease.snapshot(),
                                    self.plugin_admin
                                        .as_ref()
                                        .map(|admin| admin.export_dependencies())
                                        .unwrap_or_default(),
                                )
                                .map_err(|_| RuntimeError::Protocol)?,
                            ),
                        },
                        FlightKind::Export(lease),
                    )
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
                    self.failed_images.insert(key.clone());
                }
                if let FlightKind::Export(lease) = kind {
                    self.actor.release_export(lease);
                }
                if let Some((key, c)) = flight.target {
                    let mut response = self.error(c, code(error), mutation);
                    if error == crate::transfers::Error::DocumentLimit
                        && let Some(error) = response.error.as_mut()
                    {
                        error.details = json!({"cause":"document_limit"});
                    }
                    return self.respond(key, response);
                }
                return Ok(());
            }
        };
        if let FlightKind::Visual(key) = kind {
            if let Output::Visual { image } = output {
                self.images.insert(key, Arc::new(image));
                self.display_dirty = true;
                return Ok(());
            }
            return Err(RuntimeError::Storage);
        }
        let Some((key, c)) = flight.target else {
            return Ok(());
        };
        use rad_api::{
            Empty, ExportReady, ImagePrepared, PluginVersion, ResponseData as D, RestorePlugin,
            RestorePrepared, RestoreSummary, Transfer, TransferChunk,
        };
        let data = match output {
            Output::Begun { transfer_id } => {
                if let FlightKind::Export(lease) = kind {
                    self.exports
                        .insert(transfer_id.clone(), (c.connection, lease));
                }
                D::Transfer(Transfer { transfer_id })
            }
            Output::Written | Output::Dropped => D::Empty(Empty {}),
            Output::ImagePrepared { media_ref } => {
                if let FlightKind::Finish(token) = kind {
                    self.prepared
                        .insert((c.connection, media_ref.clone()), token);
                }
                D::ImagePrepared(ImagePrepared { media_ref })
            }
            Output::RestorePrepared {
                transfer_id,
                summary,
            } => {
                let plugins: Vec<_> = summary
                    .plugins
                    .iter()
                    .map(|p| RestorePlugin {
                        id: p.id.clone(),
                        version: p.version.map(|v| PluginVersion {
                            major: v.major,
                            minor: v.minor,
                            patch: v.patch,
                        }),
                        origin: p.origin.clone(),
                    })
                    .collect();
                let complete = plugins.len() <= 32
                    && serde_json::to_vec(&plugins).is_ok_and(|bytes| bytes.len() <= 32 * 1024);
                D::RestorePrepared(RestorePrepared {
                    transfer_id,
                    summary: RestoreSummary {
                        entities: summary.entities,
                        media_count: summary.media_count,
                        media_bytes: summary.media_bytes,
                        document_bytes: summary.document_bytes as u64,
                        plugins_count: plugins.len(),
                        plugins_complete: complete,
                        plugins: if complete { plugins } else { vec![] },
                    },
                })
            }
            Output::ExportReady { transfer_id, size } => D::ExportReady(ExportReady {
                transfer_id,
                size: size.to_string(),
            }),
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
            _ => return Err(RuntimeError::Storage),
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
            let required = self.visual_keys();
            self.images.retain(|key, _| required.contains(key));
            self.failed_images.retain(|key| required.contains(key));
            if let Some(key) = required
                .into_iter()
                .find(|key| !self.images.contains_key(key) && !self.failed_images.contains(key))
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
    fn visual_keys(&self) -> BTreeSet<String> {
        self.actor
            .status(Origin::Editor)
            .active
            .and_then(|b| self.actor.folder(Origin::Editor, b.short()))
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
fn code(e: crate::transfers::Error) -> &'static str {
    use crate::transfers::Error::*;
    match e {
        NotAllowed => "permission_denied",
        NotFound => "not_found",
        Capacity | Busy | DocumentLimit => "resource_exhausted",
        Expired => "deadline_exceeded",
        Io => "storage_error",
        UnsupportedVersion => "unsupported_version",
        Invalid | ConfirmationRequired => "invalid_request",
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

#[cfg(test)]
mod tests;
