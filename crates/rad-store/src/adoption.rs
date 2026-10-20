//! The sole rebinding exception: portable restore into an untouched installation.
use crate::{Store, StoreError, digest, ledger};
use rad_core::{configuration::Candidate, identity::*};
use rusqlite::{Connection, params};

pub(crate) fn prepare(
    connection: &Connection,
    old: &IdentityRegistry,
    candidate: &Candidate,
) -> Result<Vec<Binding>, StoreError> {
    let known = ledger::bindings(old);
    let Some(adoption) = candidate.root_adoption() else {
        return Ok(known);
    };
    let pristine = IdentityCounters {
        folders: 1,
        buttons: 0,
        actions: 0,
        serial: 1,
    };
    if old.counters() != pristine
        || known != [adoption.previous]
        || adoption.previous.short().kind() != EntityKind::Folder
        || adoption.previous.short().number() != 1
        || adoption.previous.identity().installation() != old.installation()
        || adoption.previous.identity().serial() != 1
        || adoption.replacement.short().kind() != EntityKind::Folder
        || adoption.replacement.short().number() <= 1
        || adoption.replacement.identity().installation() != old.installation()
        || adoption.replacement.identity().serial() <= 1
        || !candidate
            .registry()
            .records()
            .any(|r| r.retired() && r.binding() == adoption.replacement)
    {
        return Err(StoreError::Invalid);
    }
    let hold: i64 =
        connection.query_row("SELECT hold FROM recovery WHERE singleton=1", [], |r| {
            r.get(0)
        })?;
    if hold != 0 {
        return Err(StoreError::Corrupt);
    }
    let ids: Vec<i64> = connection
        .prepare("SELECT id FROM revisions ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for id in ids {
        let loaded = Store::read_revision(connection, id, old)?;
        let mut data = loaded.configuration.data().clone();
        if !data.buttons.is_empty()
            || data.folders.len() != 1
            || data.folders[0].binding != adoption.previous
            || data.folders[0].parent.is_some()
            || data.folders[0].slots.iter().any(Option::is_some)
        {
            return Err(StoreError::Invalid);
        }
        data.principal = adoption.replacement.identity();
        data.folders[0].binding = adoption.replacement;
        let registry = candidate
            .registry()
            .restore(&[adoption.replacement])
            .map_err(|_| StoreError::Invalid)?;
        let configuration = rad_core::configuration::Configuration::from_data(data, &registry)
            .map_err(|_| StoreError::Invalid)?;
        let bytes = rad_codec::encode(&configuration).map_err(|_| StoreError::Invalid)?;
        connection.execute(
            "UPDATE revisions SET document=?1,digest=?2 WHERE id=?3",
            params![bytes, digest(&bytes), id],
        )?;
    }
    connection.execute("DELETE FROM identities", [])?;
    // All old snapshots now refer to the explicit replacement, which is inserted with the delta.
    Ok(Vec::new())
}
