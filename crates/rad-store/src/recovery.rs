use crate::reconciliation::{Pending, Stamp};
use crate::{CommitOutcome, Store, StoreError, ledger};

/// Evidence handle for an explicit discard decision, not a GUI authorization.
/// Opaque and non-cloneable; changes to the observed configuration invalidate it.
pub struct RecoveryHold(pub(crate) Stamp);

impl Store {
    /// A pending hold conservatively retains all diagnostic material, not just a revision.
    pub fn recovery_hold(&self) -> Result<Option<RecoveryHold>, StoreError> {
        if self.uncertain {
            return Err(StoreError::Uncertain);
        }
        let stamp = Stamp::read(&self.connection)?;
        Ok((stamp.hold != 0).then_some(RecoveryHold(stamp)))
    }

    /// Caller must obtain an explicit decision to discard this observed material.
    /// Only authorizes future GC; does not delete or change configuration/history.
    pub fn discard_recovery(&mut self, observed: RecoveryHold) -> CommitOutcome {
        if self.uncertain {
            return CommitOutcome::NotApplied(StoreError::Uncertain);
        }
        let before = observed.0;
        let mut after = before.clone();
        after.hold = 0;
        let tx = match self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        {
            Ok(tx) => tx,
            Err(e) => return CommitOutcome::NotApplied(e.into()),
        };
        let work = (|| -> Result<(), StoreError> {
            if before.hold == 0 || Stamp::read(&tx)? != before {
                return Err(StoreError::Invalid);
            }
            if Self::read_head(&tx, &ledger::read(&tx)?)?.recovered_previous {
                return Err(StoreError::Corrupt);
            }
            tx.execute("UPDATE recovery SET hold=0 WHERE singleton=1", [])?;
            Ok(())
        })();
        let result = match work {
            Err(error) => match tx.rollback() {
                Ok(()) if !error.may_have_effect() => CommitOutcome::NotApplied(error),
                _ => CommitOutcome::Uncertain {
                    revision: before.current,
                    error,
                },
            },
            Ok(()) => match tx.commit() {
                Ok(()) => CommitOutcome::Applied {
                    revision: before.current,
                    changed: false,
                },
                Err(e) => CommitOutcome::Uncertain {
                    revision: before.current,
                    error: e.into(),
                },
            },
        };
        if let CommitOutcome::Uncertain { error, .. } = result {
            self.uncertain = true;
            self.pending = Some(Pending {
                before,
                after,
                error,
            });
        }
        result
    }
}

// One strict prerelease format. Missing metadata is not permission to discard.
pub(crate) fn validate(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    let hold: i64 =
        connection.query_row("SELECT hold FROM recovery WHERE singleton=1", [], |r| {
            r.get(0)
        })?;
    if hold < 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}
