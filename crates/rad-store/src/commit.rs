#[cfg(test)]
use crate::fault_tests;
use crate::{CommitOutcome, Store, StoreError, digest, ledger, media};
use rad_core::configuration::Candidate;
use rusqlite::{Connection, params};

pub(crate) fn apply(
    connection: &mut Connection,
    candidate: &Candidate,
    source: &mut dyn crate::MediaSource,
) -> CommitOutcome {
    let document = match rad_codec::encode(candidate.configuration()) {
        Ok(v) => v,
        Err(_) => return CommitOutcome::NotApplied(StoreError::Invalid),
    };
    if ledger::active(candidate.configuration().data()).len() > 10_000 {
        return CommitOutcome::NotApplied(StoreError::Capacity);
    }
    let tx = match connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => return CommitOutcome::NotApplied(e.into()),
    };
    let mut revision = 0;
    let prepared = (|| -> Result<bool, StoreError> {
        let old = ledger::read(&tx)?;
        let known = ledger::bindings(&old);
        let bindings = ledger::committed_bindings(&old, candidate)?;
        if old.installation() != candidate.registry().installation()
            || !crate::counters::covers(candidate.registry().counters(), old.counters())
        {
            return Err(StoreError::Invalid);
        }
        let loaded = Store::read_head(&tx, &old)?;
        let current = loaded.revision;
        let refs = media::prepare(&tx, candidate.configuration(), source)?;
        if loaded.recovered_previous {
            tx.execute(
                "UPDATE recovery SET hold=(SELECT current FROM heads) WHERE singleton=1",
                [],
            )?;
        }
        #[cfg(test)]
        fault_tests::checkpoint("media");
        if !loaded.recovered_previous
            && loaded.configuration == *candidate.configuration()
            && known == bindings
            && old.counters() == candidate.registry().counters()
        {
            revision = current;
            return Ok(false);
        }
        let comparison = crate::adoption::prepare(&tx, &old, candidate)?;
        let additions = ledger::additions(&comparison, &bindings)?;
        let max: i64 = tx.query_row("SELECT max(id) FROM revisions", [], |r| r.get(0))?;
        revision = max.checked_add(1).ok_or(StoreError::Capacity)?;
        ledger::insert(&tx, &additions)?;
        crate::counters::write(
            &tx,
            candidate.registry().installation(),
            candidate.registry().counters(),
        )?;
        #[cfg(test)]
        fault_tests::checkpoint("ledger");
        tx.execute(
            "UPDATE meta SET ledger_count=?1,ledger_digest=?2 WHERE singleton=1",
            params![
                bindings.len() as i64,
                ledger::checksum(candidate.registry().installation(), &bindings)
            ],
        )?;
        tx.execute(
            "INSERT INTO revisions VALUES(?1,?2,?3)",
            params![revision, document, digest(&document)],
        )?;
        #[cfg(test)]
        fault_tests::checkpoint("revision");
        media::link(&tx, revision, &refs)?;
        tx.execute(
            "UPDATE heads SET previous=?2,current=?1 WHERE singleton=1",
            params![revision, current],
        )?;
        #[cfg(test)]
        fault_tests::checkpoint("heads");
        #[cfg(test)]
        if fault_tests::take_rollback_fault() {
            return Err(StoreError::Io);
        }
        Ok(true)
    })();
    match prepared {
        Err(error) => match tx.rollback() {
            Ok(()) if !error.may_have_effect() => CommitOutcome::NotApplied(error),
            _ => CommitOutcome::Uncertain { revision, error },
        },
        Ok(false) => match tx.rollback() {
            Ok(()) => CommitOutcome::Applied {
                revision,
                changed: false,
            },
            Err(e) => CommitOutcome::Uncertain {
                revision,
                error: e.into(),
            },
        },
        Ok(true) => match tx.commit() {
            Ok(()) => {
                #[cfg(test)]
                fault_tests::checkpoint("committed");
                CommitOutcome::Applied {
                    revision,
                    changed: true,
                }
            }
            Err(e) => CommitOutcome::Uncertain {
                revision,
                error: e.into(),
            },
        },
    }
}
