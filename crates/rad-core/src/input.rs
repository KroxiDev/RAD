//! Pure 32-key physical input with explicit monotonic time.
//!
//! Outputs are candidates, not admitted executions. The authority must notify
//! EVERY published layout transition, even A → B → A between reports. Parameter
//! edits do not change the layout and are never captured here. Resolve current
//! configuration/permissions when admitting a returned Binding in the engine.
use crate::identity::{Binding, EntityKind};

/// XL input report width; fixed arrays make out-of-range positions unrepresentable.
pub const KEY_COUNT: usize = crate::configuration::Grid::XL.key_count();
/// Ordered external observations/settings; none represent a CLI or Probar invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "Fixed 32-key layout is bounded; keep this pure value interface free of heap indirection."
)]
pub enum Change {
    /// Complete active-folder assignment, validated for class and unique bindings.
    Layout {
        folder: Binding,
        buttons: [Option<Binding>; KEY_COUNT],
    },
    /// Current pressed state; false is a release, not a missing sample.
    Report([bool; KEY_COUNT]),
    /// Process deadlines using the last observed pressed state.
    Tick,
    /// Discard pending input; the next Report begins a new D9 connection.
    Disconnect,
    /// Cancel pending input and consume the next new pressure as wake only.
    Sleep,
    /// Milliseconds, captured on the next press; existing deadlines stay unchanged.
    Threshold(u64),
}
/// At most 32 candidates, or one Wake, per transition. Never external effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    Wake,
    Candidate { key: usize, button: Binding },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    InvalidLayout,
    TimeWentBackwards,
    DeadlineOverflow,
}
/// Starts awake, threshold zero, awaiting the first connection report, no layout.
/// Unassigned keys still count for wake/D9 but cannot produce candidates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Input {
    folder: Option<Binding>,
    buttons: [Option<Binding>; KEY_COUNT],
    held: [bool; KEY_COUNT],
    pending: [Option<(Binding, u64)>; KEY_COUNT],
    threshold_ms: u64,
    seen_report: bool,
    asleep: bool,
    last_ms: u64,
}
impl Input {
    /// Apply one observation atomically; errors leave all state/time unchanged.
    /// Call with nondecreasing milliseconds in one runtime-session clock domain.
    /// Schedule Tick at next_deadline_ms; deliver earlier observations first. At
    /// equal timestamps delivery order wins: a release/layout change handled before
    /// Tick prevents invocation, never fabricates a retroactive press.
    pub fn handle(&mut self, now_ms: u64, change: Change) -> Result<Vec<Output>, InputError> {
        if now_ms < self.last_ms {
            return Err(InputError::TimeWentBackwards);
        }
        let mut candidate = self.clone();
        let output = candidate.apply(now_ms, change)?;
        candidate.last_ms = now_ms;
        *self = candidate;
        Ok(output)
    }
    fn apply(&mut self, now_ms: u64, change: Change) -> Result<Vec<Output>, InputError> {
        let mut output = Vec::new();
        match change {
            Change::Layout { folder, buttons } => {
                if folder.short().kind() != EntityKind::Folder {
                    return Err(InputError::InvalidLayout);
                }
                for (key, button) in buttons.iter().enumerate() {
                    if let Some(button) = button
                        && (button.short().kind() != EntityKind::Button
                            || button.identity() == folder.identity()
                            || buttons[..key].iter().flatten().any(|other| {
                                other.short() == button.short()
                                    || other.identity() == button.identity()
                            }))
                    {
                        return Err(InputError::InvalidLayout);
                    }
                }
                for (key, button) in buttons.iter().enumerate() {
                    if self.folder != Some(folder) || *button != self.buttons[key] {
                        self.pending[key] = None;
                    }
                }
                self.folder = Some(folder);
                self.buttons = buttons;
            }
            Change::Report(pressed) => {
                let wake = self.asleep
                    && pressed
                        .iter()
                        .enumerate()
                        .any(|(key, down)| *down && !self.held[key]);
                if wake {
                    self.asleep = false;
                    output.push(Output::Wake);
                }
                let suppress =
                    wake || !self.seen_report && pressed.iter().filter(|&&down| down).count() > 1;
                self.seen_report = true;
                for (key, down) in pressed.iter().copied().enumerate() {
                    if !down {
                        self.pending[key] = None;
                    }
                    if down
                        && !self.held[key]
                        && !suppress
                        && let Some(button) = self.buttons[key]
                    {
                        self.pending[key] = Some((
                            button,
                            now_ms
                                .checked_add(self.threshold_ms)
                                .ok_or(InputError::DeadlineOverflow)?,
                        ));
                    }
                }
                self.held = pressed;
            }
            Change::Threshold(value) => self.threshold_ms = value,
            Change::Tick => {}
            Change::Sleep => {
                self.asleep = true;
                self.pending = [None; KEY_COUNT];
            }
            Change::Disconnect => {
                self.held = [false; KEY_COUNT];
                self.pending = [None; KEY_COUNT];
                self.seen_report = false;
            }
        }
        for key in 0..KEY_COUNT {
            if let Some((button, deadline)) = self.pending[key]
                && now_ms >= deadline
            {
                output.push(Output::Candidate { key, button });
                self.pending[key] = None;
            }
        }
        Ok(output)
    }
}
impl Input {
    /// Earliest pending deadline; allows a runtime to sleep instead of polling.
    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.pending
            .iter()
            .flatten()
            .map(|(_, deadline)| *deadline)
            .min()
    }
}
