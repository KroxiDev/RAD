use crate::{StoreError, digest};
use rad_core::identity::IdentityCounters;
use rusqlite::{Connection, params};

pub(crate) fn checksum(installation: u128, counters: IdentityCounters) -> Vec<u8> {
    let mut bytes = installation.to_be_bytes().to_vec();
    for value in [
        counters.folders,
        counters.buttons,
        counters.actions,
        counters.serial,
    ] {
        bytes.extend(value.to_be_bytes());
    }
    digest(&bytes)
}
pub(crate) fn write(
    connection: &Connection,
    installation: u128,
    c: IdentityCounters,
) -> Result<(), StoreError> {
    connection.execute("INSERT INTO counters VALUES(1,?1,?2,?3,?4,?5) ON CONFLICT(singleton) DO UPDATE SET folders=excluded.folders,buttons=excluded.buttons,actions=excluded.actions,serial=excluded.serial,digest=excluded.digest",
        params![c.folders.to_be_bytes().as_slice(), c.buttons.to_be_bytes().as_slice(), c.actions.to_be_bytes().as_slice(), c.serial.to_be_bytes().as_slice(), checksum(installation,c)])?;
    Ok(())
}
pub(crate) fn read(
    connection: &Connection,
    installation: u128,
) -> Result<IdentityCounters, StoreError> {
    let ([folders, buttons, actions, serial], hash): ([Vec<u8>; 4], Vec<u8>) = connection
        .query_row(
            "SELECT folders,buttons,actions,serial,digest FROM counters WHERE singleton=1",
            [],
            |r| Ok(([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?], r.get(4)?)),
        )?;
    let number = |bytes: Vec<u8>| -> Result<u64, StoreError> {
        Ok(u64::from_be_bytes(
            bytes.try_into().map_err(|_| StoreError::Corrupt)?,
        ))
    };
    let c = IdentityCounters {
        folders: number(folders)?,
        buttons: number(buttons)?,
        actions: number(actions)?,
        serial: number(serial)?,
    };
    if checksum(installation, c) != hash {
        return Err(StoreError::Corrupt);
    }
    Ok(c)
}
pub(crate) fn covers(new: IdentityCounters, old: IdentityCounters) -> bool {
    new.folders >= old.folders
        && new.buttons >= old.buttons
        && new.actions >= old.actions
        && new.serial >= old.serial
}
