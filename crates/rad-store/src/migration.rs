//! The v1 source is preserved before any write-capable PRAGMA or migration.
use crate::{Store, StoreError, database, digest, ledger};
use rusqlite::{Connection, params};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

fn backup_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}
fn same_bytes(a: &Path, b: &Path) -> Result<bool, StoreError> {
    let mut a = File::open(a).map_err(|_| StoreError::Io)?;
    let mut b = File::open(b).map_err(|_| StoreError::Io)?;
    if a.metadata().map_err(|_| StoreError::Io)?.len()
        != b.metadata().map_err(|_| StoreError::Io)?.len()
    {
        return Ok(false);
    }
    let mut left = [0; 65536];
    let mut right = [0; 65536];
    loop {
        let count = a.read(&mut left).map_err(|_| StoreError::Io)?;
        if count == 0 {
            return Ok(true);
        }
        b.read_exact(&mut right[..count])
            .map_err(|_| StoreError::Io)?;
        if left[..count] != right[..count] {
            return Ok(false);
        }
    }
}
fn preserve(path: &Path, suffix: &str) -> Result<(), StoreError> {
    let backup = backup_path(path, suffix);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if backup.exists() {
        return if same_bytes(path, &backup)? {
            Ok(())
        } else {
            Err(StoreError::Corrupt)
        };
    }
    // Unique staging files are never mistaken for the published backup after a crash.
    let mut serial = 0u64;
    let (staging, mut output) = loop {
        let mut name = backup.as_os_str().to_owned();
        name.push(format!(".staging-{}-{serial}", std::process::id()));
        let staging = PathBuf::from(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
        {
            Ok(file) => break (staging, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                serial = serial.checked_add(1).ok_or(StoreError::Capacity)?
            }
            Err(_) => return Err(StoreError::Io),
        }
    };
    std::io::copy(
        &mut File::open(path).map_err(|_| StoreError::Io)?,
        &mut output,
    )
    .map_err(|_| StoreError::Io)?;
    output.flush().map_err(|_| StoreError::Io)?;
    output.sync_all().map_err(|_| StoreError::Io)?;
    #[cfg(test)]
    crate::fault_tests::checkpoint("migration-backup-staged");
    // hard_link is atomic and never replaces an existing destination.
    match std::fs::hard_link(&staging, &backup) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && same_bytes(path, &backup)? => (),
        Err(_) => return Err(StoreError::Io),
    }
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| StoreError::Io)?;
    // The complete, synced backup now owns these bytes; this redundant link is disposable.
    std::fs::remove_file(&staging).map_err(|_| StoreError::Io)?;
    #[cfg(test)]
    crate::fault_tests::checkpoint("migration-backup-published");
    Ok(())
}

pub(crate) fn migrate(path: &Path, connection: &mut Connection) -> Result<(), StoreError> {
    // Read transaction prevents a concurrent writer from changing the bytes being copied.
    let read = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)?;
    let legacy = ledger::read_legacy(&read)?;
    crate::recovery::validate(&read)?;
    preserve(path, ".v1-backup")?;
    read.rollback()?;
    database::configure(connection)?;
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)?;
    let work = (|| -> Result<(), StoreError> {
        // Configuring connection pragmas requires leaving the read transaction.
        // Refuse to migrate if another writer changed any byte in that gap.
        if !same_bytes(path, &backup_path(path, ".v1-backup"))?
            || ledger::read_legacy(&tx)? != legacy
        {
            return Err(StoreError::Corrupt);
        }
        let ids: Vec<i64> = tx
            .prepare("SELECT id FROM revisions ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        // A usable head is required; corrupt evidence is retained verbatim under hold.
        Store::read_head(&tx, &legacy)?;
        for id in ids {
            let loaded = match Store::read_revision(&tx, id, &legacy) {
                Ok(loaded) => loaded,
                Err(StoreError::Corrupt | StoreError::Invalid) => {
                    tx.execute(
                        "UPDATE recovery SET hold=?1 WHERE singleton=1 AND hold=0",
                        [id],
                    )?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let bytes =
                rad_codec::encode(&loaded.configuration).map_err(|_| StoreError::Invalid)?;
            tx.execute(
                "UPDATE revisions SET document=?1,digest=?2 WHERE id=?3",
                params![bytes, digest(&bytes), id],
            )?;
        }
        tx.execute_batch("ALTER TABLE meta RENAME TO meta_v1;
            CREATE TABLE meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),installation BLOB NOT NULL CHECK(length(installation)=16),ledger_count INTEGER NOT NULL CHECK(ledger_count>=1),ledger_digest BLOB NOT NULL CHECK(length(ledger_digest)=32),barrier INTEGER NOT NULL DEFAULT 0) STRICT;
            INSERT INTO meta SELECT * FROM meta_v1;
            DROP TABLE meta_v1;")?;
        tx.execute_batch(include_str!("counters.sql"))?;
        crate::counters::write(&tx, legacy.installation(), legacy.counters())?;
        tx.execute_batch("PRAGMA user_version=2")?;
        #[cfg(test)]
        crate::fault_tests::checkpoint("migration-tuple");
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
    crate::fault_tests::checkpoint("migration-committed");
    Ok(())
}

/// Upgrade embedded documents while keeping the unchanged SQLite v2 envelope.
pub(crate) fn migrate_documents(
    path: &Path,
    connection: &mut Connection,
) -> Result<(), StoreError> {
    const BACKUP: &str = ".document-v2-backup";
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)?;
    let registry = ledger::read(&tx)?;
    crate::recovery::validate(&tx)?;
    Store::read_head(&tx, &registry)?;
    let documents: Vec<(i64, Vec<u8>)> = tx
        .prepare("SELECT id,document FROM revisions ORDER BY id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut migrate = Vec::new();
    let mut corrupt = Vec::new();
    for (id, bytes) in documents {
        match rad_codec::document_version(&bytes) {
            Ok(version) if version > 3 => return Err(StoreError::UnsupportedVersion),
            Ok(1 | 2) => match Store::read_revision(&tx, id, &registry) {
                Ok(_) => migrate.push(id),
                Err(StoreError::Corrupt | StoreError::Invalid) => corrupt.push(id),
                Err(error) => return Err(error),
            },
            _ => {}
        }
    }
    if migrate.is_empty() {
        return Ok(());
    }
    preserve(path, BACKUP)?;
    tx.rollback()?;
    database::configure(connection)?;
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)?;
    let work = (|| -> Result<(), StoreError> {
        if !same_bytes(path, &backup_path(path, BACKUP))? || ledger::read(&tx)? != registry {
            return Err(StoreError::Corrupt);
        }
        for id in corrupt {
            tx.execute(
                "UPDATE recovery SET hold=?1 WHERE singleton=1 AND hold=0",
                [id],
            )?;
        }
        for id in migrate {
            let loaded = match Store::read_revision(&tx, id, &registry) {
                Ok(loaded) => loaded,
                Err(StoreError::Corrupt | StoreError::Invalid) => {
                    tx.execute(
                        "UPDATE recovery SET hold=?1 WHERE singleton=1 AND hold=0",
                        [id],
                    )?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let bytes =
                rad_codec::encode(&loaded.configuration).map_err(|_| StoreError::Invalid)?;
            tx.execute(
                "UPDATE revisions SET document=?1,digest=?2 WHERE id=?3",
                params![bytes, digest(&bytes), id],
            )?;
        }
        Ok(())
    })();
    if let Err(error) = work {
        return match tx.rollback() {
            Ok(()) if !error.may_have_effect() => Err(error),
            _ => Err(StoreError::Uncertain),
        };
    }
    tx.commit().map_err(|_| StoreError::Uncertain)
}
