use crate::{Store, StoreError, ledger, reconciliation::Stamp};
use rusqlite::{Connection, params};
use std::collections::BTreeSet;

pub(crate) fn collect(
    connection: &mut Connection,
    roots: &[i64],
    after: &mut Stamp,
) -> Result<(), StoreError> {
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let work = (|| -> Result<(), StoreError> {
        tx.execute_batch(
            "CREATE TEMP TABLE retained(id INTEGER PRIMARY KEY);
            INSERT INTO retained SELECT current FROM heads;
            INSERT OR IGNORE INTO retained SELECT previous FROM heads WHERE previous IS NOT NULL;",
        )?;
        for root in roots {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM revisions WHERE id=?1)",
                [root],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(StoreError::Invalid);
            }
            tx.execute("INSERT OR IGNORE INTO retained VALUES(?1)", [root])?;
        }
        let held: i64 = tx.query_row("SELECT hold FROM recovery WHERE singleton=1", [], |r| {
            r.get(0)
        })?;
        if held != 0 {
            tx.execute_batch("DROP TABLE retained")?;
            return Ok(());
        }
        let registry = ledger::read(&tx)?;
        let ids: Vec<i64> = tx
            .prepare("SELECT id FROM retained")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let mut retained = BTreeSet::new();
        for revision in ids {
            let loaded = Store::read_revision(&tx, revision, &registry)?;
            retained.extend(
                ledger::active(loaded.configuration.data())
                    .iter()
                    .map(|b| b.identity()),
            );
        }
        // read() marks all rows retired; retained roots alone decide what can disappear.
        let mut pruned = registry.clone();
        pruned.retain_bindings(&retained);
        let bindings = ledger::bindings(&pruned);
        after.ledger = ledger::checksum(registry.installation(), &bindings);
        let mut remove = tx.prepare("DELETE FROM identities WHERE kind=?1 AND number=?2")?;
        for record in registry.records() {
            let b = record.binding();
            if !retained.contains(&b.identity()) {
                let kind = match b.short().kind() {
                    rad_core::identity::EntityKind::Folder => 0,
                    rad_core::identity::EntityKind::Button => 1,
                    rad_core::identity::EntityKind::Action => 2,
                };
                remove.execute(params![kind, b.short().number().to_be_bytes().as_slice()])?;
            }
        }
        drop(remove);
        tx.execute(
            "UPDATE meta SET ledger_count=?1,ledger_digest=?2 WHERE singleton=1",
            params![bindings.len() as i64, after.ledger],
        )?;
        tx.execute_batch("DELETE FROM revision_media WHERE revision NOT IN (SELECT id FROM retained);
            DELETE FROM revisions WHERE id NOT IN (SELECT id FROM retained);
            DELETE FROM media WHERE NOT EXISTS(SELECT 1 FROM revision_media WHERE revision_media.media=media.digest);
            DROP TABLE retained;")?;
        Ok(())
    })();
    match work {
        Err(error) => match tx.rollback() {
            Ok(()) if !error.may_have_effect() => Err(error),
            _ => Err(StoreError::Uncertain),
        },
        Ok(()) => tx.commit().map_err(|_| StoreError::Uncertain),
    }
}
