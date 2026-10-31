//! Monotonic actor time and deadlines, preserving input and event ordering.
use super::{AdmissionError, Authority, Effect, Phase, Rejection};

impl Authority {
    /// Moves Authority's clock to `now` and runs everything that is due at that time:
    /// Engine jobs and waits, expired tickets and export leases, the input tick and the
    /// sleep timer. `now` is monotonic and must not decrease across `advance` and
    /// `before_event` calls; an earlier value is refused with
    /// `AdmissionError::InvalidTime` and changes nothing. The runtime calls it once per
    /// turn, after serving requests. Skipping it leaves deadlines and ticks unserved.
    pub fn advance(&mut self, now: u64) -> Result<Vec<Effect>, AdmissionError> {
        let mut e = self.advance_clock(now)?;
        let output = self
            .input
            .handle(now, rad_core::input::Change::Tick)
            .map_err(|_| AdmissionError::Overflow)?;
        e.extend(self.input_effects(output));
        e.extend(self.sleep_if_due());
        Ok(e)
    }
    pub(super) fn advance_clock(&mut self, now: u64) -> Result<Vec<Effect>, AdmissionError> {
        if now < self.now {
            return Err(AdmissionError::InvalidTime);
        }
        let engine_due = self
            .engine
            .next_deadline()
            .is_some_and(|deadline| deadline <= now);
        let intents = self
            .engine
            .advance(now)
            .map_err(|_| AdmissionError::Overflow)?;
        self.now = now;
        let mut effects = if engine_due || !intents.is_empty() {
            self.dispatch_effects(intents)
        } else {
            vec![]
        };
        let expired: Vec<_> = self
            .tickets
            .iter()
            .filter(|(_, s)| {
                matches!(s.phase, Phase::Reserved | Phase::Queued) && s.deadline <= now
            })
            .map(|(id, _)| *id)
            .collect();
        self.queue.retain(|(id, _)| !expired.contains(id));
        effects.extend(
            expired
                .into_iter()
                .map(|id| self.reject(id, Rejection::Expired)),
        );
        let expired: Vec<_> = self
            .exports
            .iter()
            .filter(|(_, p)| p.idle.min(p.total) <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.exports.remove(&id);
            effects.push(Effect::ExportExpired { id });
        }
        Ok(effects)
    }
    /// Earliest time at which `advance` has something to do (ticket, sleep, Engine, input
    /// or export deadline), in the same monotonic milliseconds as `advance`. `None` means
    /// nothing is scheduled. The runtime sleeps until this time; it only reads state.
    pub fn next_deadline(&self) -> Option<u64> {
        self.tickets
            .values()
            .filter(|s| matches!(s.phase, Phase::Reserved | Phase::Queued))
            .map(|s| s.deadline)
            .chain(self.sleep_deadline)
            .chain(self.engine.next_deadline())
            .chain(self.input.next_deadline_ms())
            .chain(self.exports.values().map(|p| p.idle.min(p.total)))
            .min()
    }
    /// Advance Engine and request/resource deadlines before a non-tick event. Input ordering is preserved.
    ///
    /// Unlike `advance` it does not tick input or the sleep timer, so an event that must
    /// be ordered after earlier input (a job completion, a plugin pump) is processed at a
    /// current clock without reordering it. Same monotonic time as `advance`:
    /// `AdmissionError::InvalidTime` if `now` goes back, with no state change.
    pub fn before_event(&mut self, now: u64) -> Result<Vec<Effect>, AdmissionError> {
        self.advance_clock(now)
    }
}
