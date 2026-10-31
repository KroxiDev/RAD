//! Physical input, gesture admission, and ownership of held executions.
use super::{AdmissionError, Authority, Effect, ExecutionError, ExecutionId, StorageState, Target};

impl Authority {
    pub fn input_report(
        &mut self,
        now: u64,
        pressed: [bool; 32],
    ) -> Result<Vec<Effect>, AdmissionError> {
        let mut e = self.advance_clock(now)?;
        if pressed != self.held {
            self.last_activity = now;
            self.held = pressed;
            e.extend(self.reset_sleep_deadline());
        }
        let output = match self
            .input
            .handle(now, rad_core::input::Change::Report(pressed))
        {
            Ok(output) => output,
            Err(_) => {
                // Preserve Engine intents already produced; consume this report without
                // admitting a partial set of presses or retrying the impossible deadline.
                let folder = self
                    .published
                    .configuration()
                    .data()
                    .folders
                    .iter()
                    .find(|f| f.binding.identity() == self.active)
                    .unwrap()
                    .binding;
                let canceled = self
                    .input
                    .handle(
                        now,
                        rad_core::input::Change::Layout {
                            folder,
                            buttons: [None; 32],
                        },
                    )
                    .expect("valid empty layout");
                e.extend(self.input_effects(canceled));
                self.input
                    .handle(now, rad_core::input::Change::Report(pressed))
                    .expect("unassigned keys cannot overflow");
                e.extend(self.sync_input());
                e.push(Effect::InputRejected {
                    reason: ExecutionError::TimeOverflow,
                });
                return Ok(e);
            }
        };
        e.extend(self.input_effects(output));
        if self.state != StorageState::Ready {
            e.extend(self.invalidate_input());
        }
        Ok(e)
    }
    pub(super) fn invalidate_input(&mut self) -> Vec<Effect> {
        let folder = self
            .published
            .configuration()
            .data()
            .folders
            .iter()
            .find(|f| f.binding.identity() == self.active)
            .unwrap()
            .binding;
        let output = self
            .input
            .handle(
                self.now,
                rad_core::input::Change::Layout {
                    folder,
                    buttons: [None; 32],
                },
            )
            .expect("validated empty layout");
        let mut effects = self.input_effects(output);
        effects.extend(self.sync_input());
        effects
    }
    pub(super) fn input_effects(&mut self, output: Vec<rad_core::input::Output>) -> Vec<Effect> {
        let mut e = vec![];
        for output in output {
            match output {
                rad_core::input::Output::Wake => {
                    self.asleep = false;
                    e.extend(self.reset_sleep_deadline());
                    e.push(Effect::Wake);
                    e.push(self.display_effect());
                }
                rad_core::input::Output::Gesture {
                    key,
                    button: _,
                    gesture: rad_core::input::ControlGesture::HoldEnd,
                } => {
                    if let Some(execution) = self.holds[key].take() {
                        self.engine
                            .cancel_pending(execution)
                            .expect("monotonic cancellation");
                        self.releasing_holds.insert(execution);
                        e.push(Effect::ReleaseHold { execution });
                    }
                }
                rad_core::input::Output::Candidate { key, button }
                | rad_core::input::Output::Gesture { key, button, .. } => {
                    // An earlier candidate in this batch may have put the deck to sleep.
                    // HoldEnd above must still run, but invalidated starts are consumed.
                    if self.state == StorageState::Ready && !self.asleep {
                        let gesture = match output {
                            rad_core::input::Output::Gesture { gesture, .. } => gesture,
                            _ => rad_core::input::ControlGesture::Press,
                        };
                        match self.admit_control(
                            Target::Button(button.identity()),
                            rad_core::execution::Trigger::Hardware,
                            Some(rad_core::input::ControlEvent { key, gesture }),
                        ) {
                            Ok((execution, intents)) => {
                                e.push(Effect::HardwareAccepted { execution });
                                e.extend(intents);
                            }
                            Err(reason) => e.push(Effect::InputRejected { reason }),
                        }
                    }
                }
            }
        }
        e
    }
    pub(super) fn sync_input(&mut self) -> Vec<Effect> {
        let data = self.published.configuration().data();
        let folder = data
            .folders
            .iter()
            .find(|f| f.binding.identity() == self.active)
            .expect("validated selection");
        let buttons = std::array::from_fn(|i| {
            folder.slots[i].and_then(|id| {
                data.buttons
                    .iter()
                    .find(|b| b.binding.identity() == id)
                    .map(|b| b.binding)
            })
        });
        let gestures = std::array::from_fn(|i| {
            buttons[i]
                .and_then(|binding| {
                    data.buttons
                        .iter()
                        .find(|b| b.binding == binding)
                        .map(|b| b.gesture)
                })
                .unwrap_or_default()
        });
        let long_press_ms = data.settings.long_press_threshold_ms;
        let output = self
            .input
            .handle(
                self.now,
                rad_core::input::Change::Layout {
                    folder: folder.binding,
                    buttons,
                },
            )
            .expect("validated layout and monotonic clock");
        let more = self
            .input
            .handle(
                self.now,
                rad_core::input::Change::Threshold(data.settings.press_threshold_ms),
            )
            .expect("threshold update cannot overflow");
        let gestures = self
            .input
            .handle(
                self.now,
                rad_core::input::Change::Gestures {
                    selected: gestures,
                    long_press_ms,
                },
            )
            .expect("gesture update cannot overflow");
        let mut e = self.input_effects(output);
        e.extend(self.input_effects(more));
        e.extend(self.input_effects(gestures));
        self.plugin_runs.retain(|id, _| {
            self.engine.snapshot(*id).is_some()
                || self.holds.contains(&Some(*id))
                || self.releasing_holds.contains(id)
        });
        self.refresh_views();
        e
    }
    pub fn input_disconnect(&mut self, now: u64) -> Result<Vec<Effect>, AdmissionError> {
        let mut e = self.before_event(now)?;
        let output = self
            .input
            .handle(now, rad_core::input::Change::Disconnect)
            .expect("monotonic disconnect");
        self.held = [false; 32];
        e.extend(self.input_effects(output));
        Ok(e)
    }
    pub fn report_hold_end_failure(&mut self, execution: ExecutionId) {
        self.engine.report_cleanup_failure(execution);
        self.execution_revision = self.execution_revision.wrapping_add(1);
        self.refresh_views();
    }
    /// Runtime calls after all sent holds ended or their original process was lost.
    pub fn hold_released(&mut self, execution: ExecutionId) {
        self.engine.release_result(execution);
        self.execution_revision = self.execution_revision.wrapping_add(1);
        self.releasing_holds.remove(&execution);
        self.physical.remove(&execution);
        self.plugin_runs.retain(|id, _| {
            self.engine.snapshot(*id).is_some()
                || self.holds.contains(&Some(*id))
                || self.releasing_holds.contains(id)
        });
        self.runs.retain(|id, _| {
            self.engine.snapshot(*id).is_some()
                || self.holds.contains(&Some(*id))
                || self.releasing_holds.contains(id)
        });
        self.refresh_views();
    }
    pub(super) fn validate_control(
        &self,
        target: Target,
        control: rad_core::input::ControlEvent,
    ) -> Result<(), ExecutionError> {
        use rad_core::{configuration::ActionKind, input::ControlGesture};
        let button = self
            .published
            .configuration()
            .data()
            .buttons
            .iter()
            .find(|b| target == Target::Button(b.binding.identity()))
            .ok_or(ExecutionError::NotFound)?;
        let gesture = match control.gesture {
            ControlGesture::Press => rad_plugin_contract::Gesture::Press,
            ControlGesture::LongPress => rad_plugin_contract::Gesture::LongPress,
            _ => rad_plugin_contract::Gesture::Hold,
        };
        for action in &button.actions {
            if let ActionKind::External { type_key, .. } = &action.kind {
                let definition = self
                    .plugins
                    .get(type_key.plugin())
                    .and_then(|p| p.manifest.actions.iter().find(|a| a.id == type_key.name()))
                    .ok_or(ExecutionError::ExecutorUnavailable)?;
                if !definition.gestures.contains(&gesture) {
                    return Err(ExecutionError::ExecutorUnavailable);
                }
            } else if gesture == rad_plugin_contract::Gesture::Hold {
                return Err(ExecutionError::ExecutorUnavailable);
            }
        }
        Ok(())
    }
}
