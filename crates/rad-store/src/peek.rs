//! Best-effort, read-only reading of the identity counters of an unreadable database.
//!
//! `Store::open` cannot serve here: it writes (barrier, hold, migrations) and SQLite
//! replays a hot journal when a read-write connection opens. This module opens the
//! file `immutable=1`, so SQLite takes no locks, ignores `-journal`/`-wal`, and writes
//! nothing at all. Each source is read independently and an error keeps whatever was
//! read before it; the integrity digests are deliberately not checked, because a
//! reservation that is too high is harmless and one that is too low reuses IDs.
use rad_core::identity::IdentityCounters;
use rusqlite::{Connection, OpenFlags};
use std::{os::unix::ffi::OsStrExt, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeekedCounters {
    /// `meta.installation` when it could be read (16 bytes, not zero).
    pub installation: Option<u128>,
    /// Maximum of every readable source; zero where none was readable.
    pub counters: IdentityCounters,
}

/// Counters of the database at `path`, or `None` when it cannot be opened, is not an
/// SQLite database, or no source yielded anything. Never writes anything.
pub fn peek_counters(path: &Path) -> Option<PeekedCounters> {
    let connection = open_immutable(path)?;
    let installation = installation(&connection);
    let mut counters = IdentityCounters::default();
    from_counters_table(&connection, &mut counters);
    from_identities_table(&connection, installation, &mut counters);
    (installation.is_some() || counters != IdentityCounters::default()).then_some(PeekedCounters {
        installation,
        counters,
    })
}

fn open_immutable(path: &Path) -> Option<Connection> {
    let absolute = std::path::absolute(path).ok()?;
    let mut uri = String::from("file:");
    for &byte in absolute.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/._~-".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri.push_str("?immutable=1");
    Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()
}

fn installation(connection: &Connection) -> Option<u128> {
    let bytes: Vec<u8> = connection
        .query_row("SELECT installation FROM meta WHERE singleton=1", [], |r| {
            r.get(0)
        })
        .ok()?;
    let value = u128::from_be_bytes(bytes.try_into().ok()?);
    (value != 0).then_some(value)
}

fn number(bytes: Vec<u8>) -> Option<u64> {
    Some(u64::from_be_bytes(bytes.try_into().ok()?))
}

fn raise(slot: &mut u64, value: Option<u64>) {
    *slot = (*slot).max(value.unwrap_or(0));
}

fn from_counters_table(connection: &Connection, counters: &mut IdentityCounters) {
    let Ok(mut statement) =
        connection.prepare("SELECT folders,buttons,actions,serial FROM counters WHERE singleton=1")
    else {
        return;
    };
    let Ok(mut rows) = statement.query([]) else {
        return;
    };
    while let Ok(Some(row)) = rows.next() {
        let field = |index| row.get::<_, Vec<u8>>(index).ok().and_then(number);
        raise(&mut counters.folders, field(0));
        raise(&mut counters.buttons, field(1));
        raise(&mut counters.actions, field(2));
        raise(&mut counters.serial, field(3));
    }
}

fn from_identities_table(
    connection: &Connection,
    installation: Option<u128>,
    counters: &mut IdentityCounters,
) {
    let Ok(mut statement) =
        connection.prepare("SELECT kind,number,installation,serial FROM identities")
    else {
        return;
    };
    let Ok(mut rows) = statement.query([]) else {
        return;
    };
    while let Ok(Some(row)) = rows.next() {
        let bytes = |index| row.get::<_, Vec<u8>>(index).ok();
        let slot = match row.get::<_, i64>(0) {
            Ok(0) => &mut counters.folders,
            Ok(1) => &mut counters.buttons,
            Ok(2) => &mut counters.actions,
            _ => continue,
        };
        raise(slot, bytes(1).and_then(number));
        let local = bytes(2)
            .and_then(|b| Some(u128::from_be_bytes(b.try_into().ok()?)))
            .is_some_and(|owner| Some(owner) == installation);
        if local {
            raise(&mut counters.serial, bytes(3).and_then(number));
        }
    }
}
