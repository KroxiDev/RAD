use std::time::{Duration, Instant};

const FRAME_BUDGET: Duration = Duration::from_nanos(1_000_000_000u64.div_ceil(60));

#[derive(Default)]
pub(super) struct FramePacer {
    previous_start: Option<Instant>,
}

impl FramePacer {
    pub(super) fn wait(&mut self, now: Instant) -> Duration {
        let wait = self.previous_start.map_or(Duration::ZERO, |previous| {
            FRAME_BUDGET.saturating_sub(now.duration_since(previous))
        });
        self.previous_start = Some(now + wait);
        wait
    }
}
