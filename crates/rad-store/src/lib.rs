//! Synchronous persistence only. The caller owns authorization and publication.
mod source;
pub use source::{MediaEntry, MediaReadError, MediaSource};
mod recovery;
pub use recovery::RecoveryHold;
mod commit;
mod database;
mod gc;
mod ledger;
mod media;
mod reconciliation;
use rad_core::{
    configuration::{Candidate, Configuration},
    identity::IdentityRegistry,
};
use reconciliation::{Pending, Stamp};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    Io,
    Sql,
    Invalid,
    Corrupt,
    Capacity,
    Uncertain,
    ReadOnly,
    StorageFull,
    Busy,
}
impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode::*;
        match error.sqlite_error_code() {
            Some(SystemIoFailure | CannotOpen) => Self::Io,
            Some(DiskFull) => Self::StorageFull,
            Some(ReadOnly) => Self::ReadOnly,
            Some(DatabaseBusy | DatabaseLocked) => Self::Busy,
            Some(DatabaseCorrupt | NotADatabase) => Self::Corrupt,
            _ => Self::Sql,
        }
    }
}
impl StoreError {
    pub(crate) fn may_have_effect(self) -> bool {
        matches!(self, Self::Io | Self::Sql | Self::StorageFull)
    }
}
pub struct Loaded {
    pub configuration: Configuration,
    pub registry: IdentityRegistry,
    pub revision: i64,
    pub recovered_previous: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitOutcome {
    Applied { revision: i64, changed: bool },
    NotApplied(StoreError),
    Uncertain { revision: i64, error: StoreError },
}
pub struct Store {
    connection: Connection,
    uncertain: bool,
    pending: Option<Pending>,
    #[cfg(test)]
    fail_return: bool,
}

pub(crate) fn digest(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}
impl Store {
    pub fn create(path: &Path, initial: &Candidate) -> Result<Self, StoreError> {
        Self::create_with_media(path, initial, &[])
    }
    pub fn create_with_media(
        path: &Path,
        initial: &Candidate,
        images: &[rad_media::NormalizedImage],
    ) -> Result<Self, StoreError> {
        Self::create_from_media(path, initial, &mut source::SliceSource::new(images)?)
    }
    /// Initialize from a reiterable local source without retaining its media batch.
    pub fn create_from_media(
        path: &Path,
        initial: &Candidate,
        source: &mut dyn MediaSource,
    ) -> Result<Self, StoreError> {
        let document =
            rad_codec::encode(initial.configuration()).map_err(|_| StoreError::Invalid)?;
        let bindings = ledger::bindings(initial.registry());
        if bindings.len() > ledger::CAP {
            return Err(StoreError::Capacity);
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| StoreError::Io)?;
        drop(file);
        #[cfg(test)]
        fault_tests::checkpoint("create-file");
        let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        database::configure(&connection)?;
        database::initialize_schema(&connection)?;
        #[cfg(test)]
        fault_tests::checkpoint("create-schema");
        let tx = connection.transaction()?;
        let work = (|| -> Result<(), StoreError> {
            let refs = media::prepare(&tx, initial.configuration(), source)?;
            ledger::insert(&tx, &bindings)?;
            tx.execute("INSERT INTO meta(singleton,installation,ledger_count,ledger_digest) VALUES(1,?1,?2,?3)",params![initial.registry().installation().to_be_bytes().as_slice(),bindings.len() as i64,ledger::checksum(initial.registry().installation(),&bindings)])?;
            tx.execute(
                "INSERT INTO revisions VALUES(1,?1,?2)",
                params![document, digest(&document)],
            )?;
            media::link(&tx, 1, &refs)?;
            tx.execute("INSERT INTO heads VALUES(1,1,NULL)", [])?;
            #[cfg(test)]
            fault_tests::checkpoint("create-tuple");
            Ok(())
        })();
        if let Err(error) = work {
            return match tx.rollback() {
                Ok(()) if !error.may_have_effect() => Err(error),
                _ => Err(StoreError::Uncertain),
            };
        }
        tx.commit().map_err(|_| StoreError::Uncertain)?;
        #[cfg(test)]
        fault_tests::checkpoint("create-committed");
        Ok(Self {
            connection,
            uncertain: false,
            pending: None,
            #[cfg(test)]
            fail_return: false,
        })
    }
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        let application: i64 = connection.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if application != 1380009025 || version != 1 {
            return Err(StoreError::Corrupt);
        }
        database::configure(&connection)?;
        let mut store = Self {
            connection,
            uncertain: false,
            pending: None,
            #[cfg(test)]
            fail_return: false,
        };
        store.load()?;
        // Startup/reopen cannot turn a previously failed commit return into a
        // durable assertion by reading alone. Force a distinct metadata write.
        let tx = store
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let registry = ledger::read(&tx)?;
        crate::recovery::validate(&tx)?;
        if Self::read_head(&tx, &registry)?.recovered_previous {
            tx.execute(
                "UPDATE recovery SET hold=(SELECT current FROM heads) WHERE singleton=1",
                [],
            )?;
        }
        let barrier: i64 = tx.query_row("SELECT barrier FROM meta WHERE singleton=1", [], |r| {
            r.get(0)
        })?;
        let barrier = barrier.checked_add(1).ok_or(StoreError::Capacity)?;
        tx.execute("UPDATE meta SET barrier=?1 WHERE singleton=1", [barrier])
            .map_err(|_| StoreError::Uncertain)?;
        tx.commit().map_err(|_| StoreError::Uncertain)?;
        Ok(store)
    }
    pub fn commit_with_media(
        &mut self,
        candidate: &Candidate,
        images: &[rad_media::NormalizedImage],
    ) -> CommitOutcome {
        let mut source = match source::SliceSource::new(images) {
            Ok(source) => source,
            Err(error) => return CommitOutcome::NotApplied(error),
        };
        self.commit_from_media(candidate, &mut source)
    }
    /// Two-pass local media consumption; source errors never imply publication.
    pub fn commit_from_media(
        &mut self,
        candidate: &Candidate,
        source: &mut dyn MediaSource,
    ) -> CommitOutcome {
        if self.uncertain {
            return CommitOutcome::NotApplied(StoreError::Uncertain);
        }
        let before = match Stamp::read(&self.connection) {
            Ok(v) => v,
            Err(e) => return CommitOutcome::NotApplied(e),
        };
        let loaded = match self.load() {
            Ok(v) => v,
            Err(e) => return CommitOutcome::NotApplied(e),
        };
        let valid_previous = loaded.revision;
        let document = match rad_codec::encode(candidate.configuration()) {
            Ok(v) => digest(&v),
            Err(_) => return CommitOutcome::NotApplied(StoreError::Invalid),
        };
        let result = commit::apply(&mut self.connection, candidate, source);
        #[cfg(test)]
        let result =
            if self.fail_return && matches!(result, CommitOutcome::Applied { changed: true, .. }) {
                self.fail_return = false;
                if let CommitOutcome::Applied { revision, .. } = result {
                    CommitOutcome::Uncertain {
                        revision,
                        error: StoreError::Io,
                    }
                } else {
                    unreachable!()
                }
            } else {
                result
            };
        if let CommitOutcome::Uncertain { revision, error } = result {
            self.uncertain = true;
            self.pending = Some(Pending {
                before: before.clone(),
                after: Stamp {
                    hold: if loaded.recovered_previous {
                        before.current
                    } else {
                        before.hold
                    },
                    current: revision,
                    previous: Some(valid_previous),
                    document,
                    ledger: ledger::checksum(
                        candidate.registry().installation(),
                        &ledger::bindings(candidate.registry()),
                    ),
                },
                error,
            });
        }
        result
    }
    /// Caller supplies all history, execution and export revision roots under the same worker.
    pub fn collect(&mut self, roots: &[i64]) -> Result<(), StoreError> {
        if self.uncertain {
            return Err(StoreError::Uncertain);
        }
        if roots.len() > 64 {
            return Err(StoreError::Capacity);
        }
        if self.load()?.recovered_previous {
            return Err(StoreError::Corrupt);
        }
        let before = Stamp::read(&self.connection)?;
        let result = gc::collect(&mut self.connection, roots);
        if result == Err(StoreError::Uncertain) {
            self.uncertain = true;
            self.pending = Some(Pending {
                before: before.clone(),
                after: before,
                error: StoreError::Sql,
            });
        }
        result
    }

    /// Read a retained revision against the complete current ledger, not a backup ledger.
    pub fn revision(&self, revision: i64) -> Result<Loaded, StoreError> {
        if self.uncertain {
            return Err(StoreError::Uncertain);
        }
        let registry = ledger::read(&self.connection)?;
        Self::read_revision(&self.connection, revision, &registry)
    }
    pub fn lookup(
        &self,
        short: rad_core::identity::ShortId,
    ) -> Result<Option<rad_core::identity::IdentityRecord>, StoreError> {
        let loaded = self.load()?;
        Ok(loaded
            .registry
            .records()
            .find(|record| record.binding().short() == short))
    }
    pub fn image(&self, key: &str) -> Result<rad_media::NormalizedImage, StoreError> {
        media::read(&self.connection, key)
    }
    pub fn commit(&mut self, candidate: &Candidate) -> CommitOutcome {
        self.commit_with_media(candidate, &[])
    }
    pub fn reconcile(&mut self) -> CommitOutcome {
        let Some(pending) = &self.pending else {
            return CommitOutcome::NotApplied(StoreError::Invalid);
        };
        let outcome = reconciliation::reconcile(&mut self.connection, pending);
        if !matches!(outcome, CommitOutcome::Uncertain { .. }) {
            self.uncertain = false;
            self.pending = None;
        }
        outcome
    }
    pub fn load(&self) -> Result<Loaded, StoreError> {
        if self.uncertain {
            return Err(StoreError::Uncertain);
        }
        let registry = ledger::read(&self.connection)?;
        Self::read_head(&self.connection, &registry)
    }
    fn read_head(
        connection: &Connection,
        registry: &IdentityRegistry,
    ) -> Result<Loaded, StoreError> {
        let (current, previous): (i64, Option<i64>) = connection.query_row(
            "SELECT current,previous FROM heads WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        match Self::read_revision(connection, current, registry) {
            Ok(loaded) => Ok(loaded),
            Err(error @ (StoreError::Corrupt | StoreError::Invalid)) => match previous {
                Some(previous) => {
                    let mut loaded = Self::read_revision(connection, previous, registry)?;
                    loaded.recovered_previous = true;
                    Ok(loaded)
                }
                None => Err(error),
            },
            Err(error) => Err(error),
        }
    }
    fn read_revision(
        connection: &Connection,
        revision: i64,
        registry: &IdentityRegistry,
    ) -> Result<Loaded, StoreError> {
        let (bytes, hash): (Vec<u8>, Vec<u8>) = connection
            .query_row(
                "SELECT document,digest FROM revisions WHERE id=?1",
                [revision],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or(StoreError::Corrupt)?;
        if digest(&bytes) != hash {
            return Err(StoreError::Corrupt);
        }
        let data = rad_codec::decode_data(&bytes).map_err(|_| StoreError::Corrupt)?;
        let active: Vec<_> = data
            .folders
            .iter()
            .map(|f| f.binding)
            .chain(data.buttons.iter().flat_map(|b| {
                std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
            }))
            .collect();
        let known = ledger::bindings(registry);
        if active.iter().any(|b| !known.contains(b)) {
            return Err(StoreError::Corrupt);
        }
        let registry = registry.restore(&active).map_err(|_| StoreError::Corrupt)?;
        let configuration =
            Configuration::from_data(data, &registry).map_err(|_| StoreError::Corrupt)?;
        media::validate(connection, revision, &configuration)?;
        Ok(Loaded {
            configuration,
            registry,
            revision,
            recovered_previous: false,
        })
    }
}

pub fn media_key(image: &rad_media::NormalizedImage) -> String {
    digest(image.png_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[cfg(test)]
mod fault_tests;
