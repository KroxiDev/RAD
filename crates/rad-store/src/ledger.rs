use crate::{StoreError, digest};
use rad_core::identity::*;
use rusqlite::{Connection, params};

pub(crate) const CAP: usize = 10_000;
pub(crate) fn bindings(registry: &IdentityRegistry) -> Vec<Binding> {
    let mut bindings: Vec<_> = registry.records().map(|r| r.binding()).collect();
    bindings.sort_by_key(|b| b.short());
    bindings
}
pub(crate) fn checksum(installation: u128, bindings: &[Binding]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(bindings.len() * 33);
    bytes.extend(installation.to_be_bytes());
    for b in bindings {
        bytes.push(kind(b.short().kind()));
        bytes.extend(b.short().number().to_be_bytes());
        bytes.extend(b.identity().installation().to_be_bytes());
        bytes.extend(b.identity().serial().to_be_bytes());
    }
    digest(&bytes)
}
fn kind(kind: EntityKind) -> u8 {
    match kind {
        EntityKind::Folder => 0,
        EntityKind::Button => 1,
        EntityKind::Action => 2,
    }
}
pub(crate) fn insert(connection: &Connection, bindings: &[Binding]) -> Result<(), StoreError> {
    let mut statement =
        connection.prepare("INSERT OR IGNORE INTO identities VALUES (?1,?2,?3,?4)")?;
    for b in bindings {
        statement.execute(params![
            kind(b.short().kind()),
            b.short().number().to_be_bytes().as_slice(),
            b.identity().installation().to_be_bytes().as_slice(),
            b.identity().serial().to_be_bytes().as_slice()
        ])?;
    }
    Ok(())
}
pub(crate) fn read(connection: &Connection) -> Result<IdentityRegistry, StoreError> {
    let (installation, count, hash): (Vec<u8>, i64, Vec<u8>) = connection.query_row(
        "SELECT installation,ledger_count,ledger_digest FROM meta WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let count = usize::try_from(count).map_err(|_| StoreError::Corrupt)?;
    if count > CAP {
        return Err(StoreError::Capacity);
    }
    let actual: i64 = connection.query_row("SELECT count(*) FROM identities", [], |r| r.get(0))?;
    if count as i64 != actual {
        return Err(StoreError::Corrupt);
    }
    let mut statement = connection
        .prepare("SELECT kind,number,installation,serial FROM identities ORDER BY kind,number")?;
    let mut rows = statement.query([])?;
    let mut records = Vec::with_capacity(count);
    while let Some(row) = rows.next()? {
        let kind = match row.get::<_, u8>(0)? {
            0 => EntityKind::Folder,
            1 => EntityKind::Button,
            2 => EntityKind::Action,
            _ => return Err(StoreError::Corrupt),
        };
        let number = u64::from_be_bytes(array(row.get(1)?)?);
        let ns = u128::from_be_bytes(array(row.get(2)?)?);
        let serial = u64::from_be_bytes(array(row.get(3)?)?);
        let b = Binding::new(
            ShortId::new(kind, number).map_err(|_| StoreError::Corrupt)?,
            EntityIdentity::new(ns, serial).map_err(|_| StoreError::Corrupt)?,
        );
        records.push(IdentityRecord::new(b, true));
    }
    let registry =
        IdentityRegistry::from_records(u128::from_be_bytes(array(installation)?), records)
            .map_err(|_| StoreError::Corrupt)?;
    if checksum(registry.installation(), &bindings(&registry)) != hash {
        return Err(StoreError::Corrupt);
    }
    Ok(registry)
}
fn array<const N: usize>(v: Vec<u8>) -> Result<[u8; N], StoreError> {
    v.try_into().map_err(|_| StoreError::Corrupt)
}
