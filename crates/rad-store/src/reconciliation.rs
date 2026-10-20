use crate::{CommitOutcome, Loaded, Store, StoreError, ledger};
use rusqlite::Connection;
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub hold: i64,
    pub current: i64,
    pub previous: Option<i64>,
    pub document: Vec<u8>,
    pub ledger: Vec<u8>,
    pub counters: Vec<u8>,
}
impl Stamp {
    pub fn read(connection: &Connection) -> Result<Self, StoreError> {
        Ok(connection.query_row("SELECT current,previous,revisions.digest,meta.ledger_digest,(SELECT hold FROM recovery WHERE singleton=1),(SELECT digest FROM counters WHERE singleton=1) FROM heads JOIN revisions ON revisions.id=heads.current CROSS JOIN meta WHERE heads.singleton=1 AND meta.singleton=1",[],|r|Ok(Self {current:r.get(0)?,previous:r.get(1)?,document:r.get(2)?,ledger:r.get(3)?,hold:r.get(4)?,counters:r.get(5)?}))?)
    }
}
pub(crate) struct Pending {
    pub before: Stamp,
    pub after: Stamp,
    pub error: StoreError,
}
pub(crate) fn reconcile(connection: &mut Connection, pending: &Pending) -> CommitOutcome {
    let uncertain = |error| CommitOutcome::Uncertain {
        revision: pending.after.current,
        error,
    };
    if !connection.is_autocommit() && connection.execute_batch("ROLLBACK").is_err() {
        return uncertain(StoreError::Sql);
    }
    let tx = match connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => return uncertain(e.into()),
    };
    let result = (|| -> Result<bool, StoreError> {
        let actual = Stamp::read(&tx)?;
        let registry = ledger::read(&tx)?;
        let Loaded {
            recovered_previous, ..
        } = Store::read_head(&tx, &registry)?;
        if recovered_previous {
            return Err(StoreError::Corrupt);
        }
        let applied = if actual == pending.after {
            true
        } else if actual == pending.before {
            false
        } else {
            return Err(StoreError::Corrupt);
        };
        let barrier: i64 = tx.query_row("SELECT barrier FROM meta WHERE singleton=1", [], |r| {
            r.get(0)
        })?;
        // A different stored value is mandatory: an identical UPDATE may perform no sync.
        let next = barrier.checked_add(1).ok_or(StoreError::Capacity)?;
        tx.execute("UPDATE meta SET barrier=?1 WHERE singleton=1", [next])?;
        Ok(applied)
    })();
    match result {
        Err(error) => {
            let _ = tx.rollback();
            uncertain(error)
        }
        Ok(applied) => match tx.commit() {
            Err(e) => uncertain(e.into()),
            Ok(()) if applied => CommitOutcome::Applied {
                revision: pending.after.current,
                changed: pending.after.current != pending.before.current,
            },
            Ok(()) => CommitOutcome::NotApplied(pending.error),
        },
    }
}
