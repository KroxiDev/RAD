//! Pure frame pacing; the UI owns the clock and the wait.
use std::time::{Duration, Instant};

// Round up to a whole nanosecond so the cap never exceeds 60 fps.
const FRAME_BUDGET: Duration = Duration::from_nanos(1_000_000_000u64.div_ceil(60));

#[derive(Default)]
pub(super) struct FramePacer {
    previous_start: Option<Instant>,
}

impl FramePacer {
    /// Return the wait before this frame; the caller must apply it before drawing.
    pub(super) fn wait(&mut self, now: Instant) -> Duration {
        let wait = self.previous_start.map_or(Duration::ZERO, |previous| {
            FRAME_BUDGET.saturating_sub(now.duration_since(previous))
        });
        // Include this wait in the frame start used by the next call.
        self.previous_start = Some(now + wait);
        wait
    }
}
