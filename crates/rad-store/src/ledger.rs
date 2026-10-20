use crate::{StoreError, digest};
use rad_core::identity::*;
use rusqlite::{Connection, params};

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
    let mut statement = connection.prepare("INSERT INTO identities VALUES (?1,?2,?3,?4)")?;
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
    let registry = read_legacy(connection)?;
    let counters = crate::counters::read(connection, registry.installation())?;
    IdentityRegistry::from_records_with_counters(
        registry.installation(),
        registry.records(),
        counters,
    )
    .map_err(|_| StoreError::Corrupt)
}
pub(crate) fn read_legacy(connection: &Connection) -> Result<IdentityRegistry, StoreError> {
    let (installation, count, hash): (Vec<u8>, i64, Vec<u8>) = connection.query_row(
        "SELECT installation,ledger_count,ledger_digest FROM meta WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let count = usize::try_from(count).map_err(|_| StoreError::Corrupt)?;
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

/// Sorted merge: validates existing bindings and returns only genuinely new rows.
pub(crate) fn additions(
    known: &[Binding],
    proposed: &[Binding],
) -> Result<Vec<Binding>, StoreError> {
    let mut old = known.iter().peekable();
    let mut added = Vec::new();
    for binding in proposed {
        match old.peek() {
            Some(previous) if previous.short() < binding.short() => {
                return Err(StoreError::Invalid);
            }
            Some(previous) if previous.short() == binding.short() => {
                if **previous != *binding {
                    return Err(StoreError::Invalid);
                }
                old.next();
            }
            _ => added.push(*binding),
        }
    }
    if old.next().is_some() {
        return Err(StoreError::Invalid);
    }
    Ok(added)
}
pub(crate) fn contains_all(known: &[Binding], subset: &[Binding]) -> bool {
    let mut known = known.iter().peekable();
    for binding in subset {
        while known.peek().is_some_and(|v| v.short() < binding.short()) {
            known.next();
        }
        if known.next() != Some(binding) {
            return false;
        }
    }
    true
}
pub(crate) fn active(data: &rad_core::configuration::ConfigurationData) -> Vec<Binding> {
    let mut bindings: Vec<_> =
        data.folders
            .iter()
            .map(|f| f.binding)
            .chain(data.buttons.iter().flat_map(|b| {
                std::iter::once(b.binding).chain(b.actions.iter().map(|a| a.binding))
            }))
            .collect();
    bindings.sort_by_key(|b| b.short());
    bindings
}

/// Retired candidate rows unknown to Store have no retained revision and must not
/// resurrect after GC. Allocation high-water marks are persisted independently.
pub(crate) fn committed_bindings(
    old: &IdentityRegistry,
    candidate: &rad_core::configuration::Candidate,
) -> Result<Vec<Binding>, StoreError> {
    let known = if candidate.root_adoption().is_some() {
        Vec::new()
    } else {
        bindings(old)
    };
    let mut incoming = active(candidate.configuration().data());
    if let Some(adoption) = candidate.root_adoption() {
        incoming.push(adoption.replacement);
        incoming.sort_by_key(|b| b.short());
    }
    let mut result = Vec::with_capacity(known.len() + incoming.len());
    let mut left = known.into_iter().peekable();
    let mut right = incoming.into_iter().peekable();
    while let (Some(a), Some(b)) = (left.peek(), right.peek()) {
        match a.short().cmp(&b.short()) {
            std::cmp::Ordering::Less => result.push(left.next().unwrap()),
            std::cmp::Ordering::Greater => result.push(right.next().unwrap()),
            std::cmp::Ordering::Equal => {
                if a != b {
                    return Err(StoreError::Invalid);
                }
                result.push(left.next().unwrap());
                right.next();
            }
        }
    }
    result.extend(left);
    result.extend(right);
    Ok(result)
}
