//! Pin and renew published configuration snapshots while an export is in progress.
use super::{Authority, ExportLease, ExportPin, Origin, Rejection, StorageState};

impl Authority {
    /// Pins the published configuration for `connection`'s export and returns its lease.
    /// Editor only (`Rejection::Invalid` otherwise), refused unless storage is ready
    /// and when the connection already holds a lease or two leases exist
    /// (`Rejection::Capacity`). The caller must end the lease with `release_export` or
    /// keep it alive with `touch_export`; one it forgets is reclaimed only when it
    /// expires, holding its revision and a capacity slot until then.
    pub fn pin_export(
        &mut self,
        connection: u64,
        origin: Origin,
    ) -> Result<ExportLease, Rejection> {
        if origin != Origin::Editor {
            return Err(Rejection::Invalid);
        }
        if self.state != StorageState::Ready {
            return Err(Rejection::StorageUncertain);
        }
        if self.exports.len() >= 2 || self.exports.values().any(|p| p.connection == connection) {
            return Err(Rejection::Capacity);
        }
        let total = self.now.checked_add(120000).ok_or(Rejection::Capacity)?;
        let idle = self.now + 30000;
        let id = self.command_id().ok_or(Rejection::Capacity)?;
        self.exports.insert(
            id,
            ExportPin {
                connection,
                revision: self.revision,
                idle,
                total,
            },
        );
        Ok(ExportLease {
            snapshot: self.published.clone(),
            id,
            revision: self.revision,
        })
    }
    /// Ends an export and frees its slot and revision. Returns `false` when the lease had
    /// already expired or its connection disconnected, which is not an error: the export
    /// data it covered is simply no longer guaranteed.
    pub fn release_export(&mut self, lease: ExportLease) -> bool {
        self.exports.remove(&lease.id).is_some()
    }
    /// Renews the idle window of a live lease (30 s from the last `advance` time, never
    /// past its 120 s total). Called while the export is being read, so a slow client
    /// does not lose it. `Rejection::Invalid` means the lease is gone.
    pub fn touch_export(&mut self, lease: &ExportLease) -> Result<(), Rejection> {
        let p = self.exports.get_mut(&lease.id).ok_or(Rejection::Invalid)?;
        p.idle = self
            .now
            .checked_add(30000)
            .ok_or(Rejection::Capacity)?
            .min(p.total);
        Ok(())
    }
}
