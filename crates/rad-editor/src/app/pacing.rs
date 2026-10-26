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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_frame_does_not_wait() {
        let mut pacer = FramePacer::default();

        assert_eq!(pacer.wait(Instant::now()), Duration::ZERO);
    }

    #[test]
    fn immediate_frame_waits_only_remaining_budget() {
        let mut pacer = FramePacer::default();
        let start = Instant::now();
        assert_eq!(pacer.wait(start), Duration::ZERO);

        assert_eq!(
            pacer.wait(start + Duration::from_millis(4)),
            Duration::from_nanos(12_666_667)
        );
    }

    #[test]
    fn frame_after_inactivity_does_not_wait() {
        let mut pacer = FramePacer::default();
        let start = Instant::now();
        assert_eq!(pacer.wait(start), Duration::ZERO);

        assert_eq!(
            pacer.wait(start + Duration::from_nanos(16_666_667)),
            Duration::ZERO
        );
        assert_eq!(pacer.wait(start + Duration::from_secs(5)), Duration::ZERO);
    }

    #[test]
    fn continuous_frames_stay_at_or_below_sixty_fps_without_drift() {
        let mut pacer = FramePacer::default();
        let origin = Instant::now();
        assert_eq!(pacer.wait(origin), Duration::ZERO);
        let mut previous_start = origin;

        for frame in 1..=600 {
            // Simulate varying frame work and apply the returned wait, without sleeping.
            let now = previous_start + Duration::from_millis(u64::from(frame % 16));
            let start = now + pacer.wait(now);
            assert!(start.duration_since(previous_start) >= Duration::from_nanos(16_666_667));
            assert_eq!(
                start.duration_since(origin),
                Duration::from_nanos(16_666_667) * frame,
                "frame {frame} drifted"
            );
            previous_start = start;
        }
        assert!(previous_start.duration_since(origin) >= Duration::from_secs(10));
    }
}
