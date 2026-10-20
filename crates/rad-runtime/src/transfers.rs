//! Blocking transfer services for the single Store worker, never the actor.
use rad_core::configuration::{Candidate, ConfigurationData, Origin};
use rad_package::{PreparedMedia, PreparedRestore, RestoreSummary, StagingArea};
use rad_store::{CommitOutcome, Store, StoreError};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    time::Instant,
};
mod deadline;
mod export;
mod files;
pub const CHUNK: usize = 8192;
const PACKAGE: u64 = 256 * 1024 * 1024;
const IMAGE: u64 = 8 * 1024 * 1024;
const TOTAL: u64 = 512 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotAllowed,
    Invalid,
    NotFound,
    Capacity,
    Expired,
    Io,
    Busy,
    ConfirmationRequired,
}
#[derive(Debug, Clone, Copy)]
pub struct Context {
    pub connection: u64,
    pub origin: Origin,
    pub now_ms: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    ImageImport,
    Restore,
    Export,
}
pub enum Operation {
    Begin {
        kind: Kind,
        size: Option<u64>,
        export_revision: Option<i64>,
    },
    Write {
        transfer_id: String,
        offset: u64,
        bytes: Vec<u8>,
    },
    Finish {
        transfer_id: String,
    },
    Read {
        transfer_id: String,
        offset: u64,
    },
    Drop {
        transfer_id: String,
    },
    RestoreData {
        transfer_id: String,
        confirmed: bool,
    },
    MediaRead {
        resolved_key: String,
        offset: u64,
    },
    LoadVisual {
        resolved_key: String,
    },
    /// Internal runtime operation, not an IPC command.
    RetainImage {
        media_ref: String,
    },
    Disconnect,
    Reap,
}
#[derive(Debug)]
pub enum Output {
    Visual {
        image: rad_media::NormalizedImage,
    },
    Begun {
        transfer_id: String,
    },
    Retained {
        transfer_id: String,
    },
    Written,
    Dropped,
    ImagePrepared {
        media_ref: String,
    },
    RestorePrepared {
        transfer_id: String,
        summary: RestoreSummary,
    },
    ExportReady {
        transfer_id: String,
        size: u64,
    },
    Chunk {
        bytes: Vec<u8>,
        eof: bool,
    },
    RestoreData {
        data: ConfigurationData,
    },
    Reaped,
}
#[derive(Debug, Clone)]
pub struct Handle {
    pub transfer_id: String,
    pub connection: u64,
    pub revision: Option<i64>,
    pub pending: bool,
    pub reserved_bytes: u64,
}
#[derive(Debug, Clone)]
pub struct Metadata {
    pub handles: Vec<Handle>,
    pub reserved_bytes: u64,
    pub next_deadline_ms: Option<u64>,
}
enum Body {
    Upload,
    Image(rad_media::NormalizedImage),
    Restore(PreparedRestore),
    Verified(PreparedMedia),
    Export(u64),
    Empty,
}
struct Transfer {
    connection: u64,
    kind: Kind,
    declared: Option<u64>,
    limit: u64,
    reserved: u64,
    written: u64,
    created: u64,
    touched: u64,
    prepared: Option<u64>,
    pending: bool,
    started: bool,
    revision: Option<i64>,
    file: files::Spool,
    body: Body,
}
pub struct Transfers {
    files: files::Area,
    area: StagingArea,
    session: u128,
    next: u64,
    now: u64,
    entries: BTreeMap<String, Transfer>,
    released: Vec<String>,
}
impl Transfers {
    /// Startup only, with the exclusive runtime lock and Store recovery already complete.
    pub fn recover(root: &Path, session: u128) -> Result<Self, Error> {
        let files = files::Area::recover(root)?;
        let area = StagingArea::recover_abandoned(&files.path().join("packages"))
            .map_err(|_| Error::Io)?;
        Ok(Self {
            files,
            area,
            session,
            next: 1,
            now: 0,
            entries: BTreeMap::new(),
            released: vec![],
        })
    }
    pub fn create(root: &Path, session: u128) -> Result<Self, Error> {
        let files = files::Area::create(root)?;
        let area =
            StagingArea::create_new(&files.path().join("packages")).map_err(|_| Error::Io)?;
        Ok(Self {
            files,
            area,
            session,
            next: 1,
            now: 0,
            entries: BTreeMap::new(),
            released: vec![],
        })
    }
    pub fn execute(
        &mut self,
        store: &mut Store,
        context: Context,
        op: Operation,
    ) -> Result<Output, Error> {
        if context.origin != Origin::Editor && !matches!(&op, Operation::MediaRead { .. }) {
            return Err(Error::NotAllowed);
        }
        if context.now_ms < self.now {
            return Err(Error::Invalid);
        }
        let id = match &op {
            Operation::Write { transfer_id, .. }
            | Operation::Finish { transfer_id }
            | Operation::Read { transfer_id, .. }
            | Operation::RestoreData { transfer_id, .. } => Some(transfer_id.clone()),
            _ => None,
        }
        .filter(|id| {
            self.entries
                .get(id)
                .is_some_and(|t| t.connection == context.connection && !t.pending)
        });
        let remaining = id
            .as_ref()
            .and_then(|id| self.entries.get(id))
            .map_or(30_000, |t| Self::deadline(t).saturating_sub(context.now_ms));
        let end = Instant::now() + std::time::Duration::from_millis(remaining.min(120_000));
        let result = self.execute_inner(store, context, op, end);
        if Instant::now() >= end {
            let cleanup = id.or_else(|| match &result {
                Ok(Output::Begun { transfer_id }) => Some(transfer_id.clone()),
                _ => None,
            });
            if let Some(id) = cleanup
                && let Some(t) = self.entries.get_mut(&id)
                && !t.started
            {
                t.pending = false;
                self.release(&id)?;
            }
            return Err(Error::Expired);
        }
        result
    }
    fn execute_inner(
        &mut self,
        store: &mut Store,
        context: Context,
        op: Operation,
        end: Instant,
    ) -> Result<Output, Error> {
        if context.origin != Origin::Editor && !matches!(&op, Operation::MediaRead { .. }) {
            return Err(Error::NotAllowed);
        }
        if context.now_ms < self.now {
            return Err(Error::Invalid);
        }
        self.now = context.now_ms;
        self.reap()?;
        match op {
            Operation::Begin {
                kind,
                size,
                export_revision,
            } => {
                if kind == Kind::Export && (export_revision.is_none() || size.is_some()) {
                    return Err(Error::Invalid);
                }
                let out = self.begin(context, kind, size, export_revision)?;
                if let Output::Begun { transfer_id } = &out
                    && kind == Kind::Export
                {
                    let t = self.entries.get_mut(transfer_id).unwrap();
                    match export::write(
                        store,
                        export_revision.unwrap(),
                        &mut deadline::Timed {
                            inner: &mut t.file.file,
                            end,
                        },
                    ) {
                        Ok(size) => {
                            t.body = Body::Export(size);
                            t.written = size;
                            t.reserved = size;
                        }
                        Err(e) => {
                            self.release(transfer_id)?;
                            return Err(e);
                        }
                    }
                }
                Ok(out)
            }
            Operation::Write {
                transfer_id,
                offset,
                bytes,
            } => {
                let t = self.lookup(&transfer_id, context.connection)?;
                if t.pending
                    || t.kind == Kind::Export
                    || !matches!(t.body, Body::Upload)
                    || bytes.is_empty()
                    || bytes.len() > CHUNK
                    || offset != t.written
                    || offset
                        .checked_add(bytes.len() as u64)
                        .is_none_or(|n| n > t.limit)
                {
                    return Err(Error::Invalid);
                }
                if (deadline::Timed {
                    inner: &mut t.file.file,
                    end,
                })
                .write_all(&bytes)
                .is_err()
                {
                    t.body = Body::Empty;
                    return Err(Error::Io);
                };
                t.written += bytes.len() as u64;
                t.touched = context.now_ms;
                Ok(Output::Written)
            }
            Operation::Finish { transfer_id } => self.finish(context, &transfer_id, end),
            Operation::Read {
                transfer_id,
                offset,
            } => {
                let t = self.lookup(&transfer_id, context.connection)?;
                let Body::Export(size) = t.body else {
                    return Err(Error::Invalid);
                };
                if offset > size {
                    return Err(Error::Invalid);
                }
                t.file
                    .file
                    .seek(SeekFrom::Start(offset))
                    .map_err(|_| Error::Io)?;
                let mut bytes = vec![0; (size - offset).min(CHUNK as u64) as usize];
                deadline::Timed {
                    inner: &mut t.file.file,
                    end,
                }
                .read_exact(&mut bytes)
                .map_err(|_| Error::Io)?;
                let eof = offset + bytes.len() as u64 == size;
                t.touched = context.now_ms;
                if eof {
                    self.release(&transfer_id)?;
                }
                Ok(Output::Chunk { bytes, eof })
            }
            Operation::RestoreData {
                transfer_id,
                confirmed,
            } => {
                if !confirmed {
                    return Err(Error::ConfirmationRequired);
                }
                let t = self.lookup(&transfer_id, context.connection)?;
                if t.pending || !matches!(t.body, Body::Restore(_)) {
                    return Err(Error::Invalid);
                }
                let Body::Restore(prepared) = std::mem::replace(&mut t.body, Body::Empty) else {
                    unreachable!()
                };
                let (data, media) = prepared.into_verified_data().map_err(|_| Error::Invalid)?;
                t.body = Body::Verified(media);
                t.pending = true;
                Ok(Output::RestoreData { data })
            }
            Operation::RetainImage { media_ref } => {
                let (id,t)=self.entries.iter_mut().find(|(_,t)|t.connection==context.connection&&!t.pending&&matches!(&t.body,Body::Image(image) if rad_store::media_key(image)==media_ref)).ok_or(Error::NotFound)?;
                t.pending = true;
                Ok(Output::Retained {
                    transfer_id: id.clone(),
                })
            }
            Operation::LoadVisual { resolved_key } => store
                .image(&resolved_key)
                .map(|image| Output::Visual { image })
                .map_err(|_| Error::NotFound),
            Operation::MediaRead {
                resolved_key,
                offset,
            } => {
                let image = store.image(&resolved_key).map_err(|_| Error::NotFound)?;
                let offset = usize::try_from(offset).map_err(|_| Error::Invalid)?;
                if offset > image.png_bytes().len() {
                    return Err(Error::Invalid);
                }
                let end = (offset + CHUNK).min(image.png_bytes().len());
                Ok(Output::Chunk {
                    bytes: image.png_bytes()[offset..end].to_vec(),
                    eof: end == image.png_bytes().len(),
                })
            }
            Operation::Drop { transfer_id } => {
                self.lookup(&transfer_id, context.connection)?;
                self.release(&transfer_id)?;
                Ok(Output::Dropped)
            }
            Operation::Disconnect => {
                let ids: Vec<_> = self
                    .entries
                    .iter()
                    .filter(|(_, t)| t.connection == context.connection && !t.pending)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in ids {
                    self.release(&id)?;
                }
                Ok(Output::Reaped)
            }
            Operation::Reap => Ok(Output::Reaped),
        }
    }
    fn begin(
        &mut self,
        c: Context,
        kind: Kind,
        size: Option<u64>,
        revision: Option<i64>,
    ) -> Result<Output, Error> {
        if self.entries.len() >= 2
            || self.released.len() >= 2
            || self.entries.values().any(|t| t.connection == c.connection)
        {
            return Err(Error::Capacity);
        }
        let limit = if kind == Kind::ImageImport {
            IMAGE
        } else {
            PACKAGE
        };
        if size.is_some_and(|n| n == 0 || n > limit) || (kind != Kind::Export && revision.is_some())
        {
            return Err(Error::Invalid);
        }
        let max = size.unwrap_or(limit);
        let reserved = match kind {
            Kind::Restore => max * 2,
            Kind::ImageImport => max + 512 * 1024,
            Kind::Export => PACKAGE,
        };
        if self.entries.values().map(|t| t.reserved).sum::<u64>() + reserved > TOTAL {
            return Err(Error::Capacity);
        }
        let n = self.next;
        self.next = self.next.checked_add(1).ok_or(Error::Capacity)?;
        let id = format!("{:032x}-{n:016x}", self.session);
        let file = self.files.spool(n)?;
        self.entries.insert(
            id.clone(),
            Transfer {
                connection: c.connection,
                kind,
                declared: size,
                limit: max,
                reserved,
                written: 0,
                created: c.now_ms,
                touched: c.now_ms,
                prepared: None,
                pending: false,
                started: false,
                revision,
                file,
                body: Body::Upload,
            },
        );
        Ok(Output::Begun { transfer_id: id })
    }
    fn lookup(&mut self, id: &str, connection: u64) -> Result<&mut Transfer, Error> {
        self.entries
            .get_mut(id)
            .filter(|t| t.connection == connection)
            .ok_or(Error::NotFound)
    }
    fn deadline(t: &Transfer) -> u64 {
        t.prepared.map_or_else(
            || {
                t.created
                    .saturating_add(120_000)
                    .min(t.touched.saturating_add(30_000))
            },
            |n| n.saturating_add(60_000),
        )
    }
    fn reap(&mut self) -> Result<(), Error> {
        let ids: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, t)| !t.pending && Self::deadline(t) <= self.now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.release(&id)?;
        }
        Ok(())
    }
    fn release(&mut self, id: &str) -> Result<(), Error> {
        let Some(t) = self.entries.get_mut(id) else {
            return Err(Error::NotFound);
        };
        if t.pending {
            return Err(Error::Busy);
        }
        let body = std::mem::replace(&mut t.body, Body::Empty);
        match body {
            Body::Restore(p) => p.discard().map_err(|_| Error::Io)?,
            Body::Verified(p) => p.discard().map_err(|_| Error::Io)?,
            _ => {}
        }
        self.area.reap().map_err(|_| Error::Io)?;
        t.file.remove()?;
        self.entries.remove(id);
        self.released.push(id.to_owned());
        Ok(())
    }
    pub fn metadata(&self) -> Metadata {
        Metadata {
            handles: self
                .entries
                .iter()
                .map(|(id, t)| Handle {
                    transfer_id: id.clone(),
                    connection: t.connection,
                    revision: t.revision,
                    pending: t.pending,
                    reserved_bytes: t.reserved,
                })
                .collect(),
            reserved_bytes: self.entries.values().map(|t| t.reserved).sum(),
            next_deadline_ms: self
                .entries
                .values()
                .filter(|t| !t.pending)
                .map(Self::deadline)
                .min(),
        }
    }
    pub fn take_released(&mut self) -> Vec<String> {
        std::mem::take(&mut self.released)
    }
    fn finish(&mut self, c: Context, id: &str, end: Instant) -> Result<Output, Error> {
        let area = self.area.clone();
        let t = self.lookup(id, c.connection)?;
        if let Body::Export(size) = t.body {
            t.touched = c.now_ms;
            return Ok(Output::ExportReady {
                transfer_id: id.to_owned(),
                size,
            });
        }
        if t.pending
            || !matches!(t.body, Body::Upload)
            || t.written == 0
            || t.declared.is_some_and(|n| n != t.written)
        {
            return Err(Error::Invalid);
        }
        if t.kind == Kind::Restore {
            t.file
                .file
                .seek(SeekFrom::Start(0))
                .map_err(|_| Error::Io)?;
            let prepared = match rad_package::prepare(
                &mut deadline::Timed {
                    inner: &mut t.file.file,
                    end,
                },
                &area,
            ) {
                Ok(p) => p,
                Err(_) => {
                    t.body = Body::Empty;
                    return Err(Error::Invalid);
                }
            };
            let summary = prepared.summary();
            t.body = Body::Restore(prepared);
            t.file.remove()?;
            t.file.file.set_len(0).map_err(|_| Error::Io)?;
            t.reserved = t.written;
            t.prepared = Some(c.now_ms);
            return Ok(Output::RestorePrepared {
                transfer_id: id.to_owned(),
                summary,
            });
        }
        if t.kind != Kind::ImageImport {
            return Err(Error::Invalid);
        }
        t.file
            .file
            .seek(SeekFrom::Start(0))
            .map_err(|_| Error::Io)?;
        let mut bytes = vec![0; t.written as usize];
        deadline::Timed {
            inner: &mut t.file.file,
            end,
        }
        .read_exact(&mut bytes)
        .map_err(|_| Error::Io)?;
        let image = rad_media::normalize(&bytes).map_err(|_| Error::Invalid)?;
        let key = rad_store::media_key(&image);
        t.file.remove()?;
        t.file.file.set_len(0).map_err(|_| Error::Io)?;
        t.reserved = image.png_bytes().len() as u64;
        t.prepared = Some(c.now_ms);
        t.body = Body::Image(image);
        Ok(Output::ImagePrepared { media_ref: key })
    }
    pub fn commit(
        &mut self,
        store: &mut Store,
        candidate: &Candidate,
        connection: u64,
        transfer_id: &str,
    ) -> CommitOutcome {
        let Ok(t) = self.lookup(transfer_id, connection) else {
            return CommitOutcome::NotApplied(StoreError::Invalid);
        };
        if t.started {
            return CommitOutcome::NotApplied(StoreError::Invalid);
        }
        t.pending = true;
        t.started = true;
        match &mut t.body {
            Body::Image(image) => store.commit_with_media(candidate, std::slice::from_ref(image)),
            Body::Verified(media) => store.commit_from_media(candidate, media),
            _ => {
                t.pending = false;
                t.started = false;
                CommitOutcome::NotApplied(StoreError::Invalid)
            }
        }
    }
    pub fn resolved(&mut self, transfer_id: &str, outcome: &CommitOutcome) -> Result<(), Error> {
        let t = self.entries.get_mut(transfer_id).ok_or(Error::NotFound)?;
        if matches!(outcome, CommitOutcome::Uncertain { .. }) {
            t.pending = true;
            return Ok(());
        }
        t.pending = false;
        self.release(transfer_id)
    }
}
