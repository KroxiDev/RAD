use crate::StoreError;
use rusqlite::Connection;
pub(crate) fn collect(connection: &mut Connection, roots: &[i64]) -> Result<(), StoreError> {
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
