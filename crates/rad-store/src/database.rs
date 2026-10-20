use rusqlite::Connection;

pub(crate) fn configure(connection: &Connection) -> rusqlite::Result<()> {
    connection.busy_timeout(std::time::Duration::ZERO)?;
    connection.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        2 * 1024 * 1024,
    )?;
    connection.execute_batch("PRAGMA page_size=4096; PRAGMA max_page_count=262144;")?;
    let page_size: i64 = connection.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    let pages: i64 = connection.query_row("PRAGMA max_page_count", [], |r| r.get(0))?;
    if page_size != 4096 || pages != 262144 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;")?;
    let journal: String = connection.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    if journal != "delete" {
        return Err(rusqlite::Error::InvalidQuery);
    }
    for (pragma, expected) in [
        ("PRAGMA synchronous", 3),
        ("PRAGMA foreign_keys", 1),
        ("PRAGMA trusted_schema", 0),
    ] {
        let value: i64 = connection.query_row(pragma, [], |r| r.get(0))?;
        if value != expected {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(())
}

pub(crate) fn initialize_schema(connection: &Connection) -> rusqlite::Result<()> {
    let tx = connection.unchecked_transaction()?;
    tx.execute_batch(include_str!("schema.sql"))?;
    tx.execute_batch(include_str!("recovery.sql"))?;
    tx.execute_batch(include_str!("counters.sql"))?;
    tx.commit()
}

#[cfg(test)]
mod tests;
