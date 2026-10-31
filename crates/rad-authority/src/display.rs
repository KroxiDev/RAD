//! Deck display, sleep transitions, and persistent native brightness operations.
use super::{
    Authority, Completion, Edit, Effect, NativeBrightness, Origin, Pending, PublicationTarget,
};

impl Authority {
    pub(super) fn display_effect(&self) -> Effect {
        Effect::Display {
            brightness: self
                .published
                .configuration()
                .settings()
                .data()
                .brightness_percent,
            asleep: self.asleep,
        }
    }
    pub(super) fn reset_sleep_deadline(&mut self) -> Vec<Effect> {
        let duration = self.published.configuration().settings().data().sleep_ms;
        self.sleep_deadline = if duration == 0 || self.asleep {
            None
        } else {
            self.last_activity.checked_add(duration)
        };
        if duration > 0 && !self.asleep && self.sleep_deadline.is_none() {
            vec![Effect::DisplayClockOverflow]
        } else {
            vec![]
        }
    }
    pub(super) fn sleep_if_due(&mut self) -> Vec<Effect> {
        if self
            .sleep_deadline
            .is_some_and(|deadline| deadline <= self.now)
        {
            return self.enter_sleep();
        }
        vec![]
    }
    /// Manual actions and the idle deadline share the exact same input/display transition.
    pub(super) fn enter_sleep(&mut self) -> Vec<Effect> {
        self.sleep_deadline = None;
        self.asleep = true;
        let output = self
            .input
            .handle(self.now, rad_core::input::Change::Sleep)
            .expect("monotonic sleep");
        let mut effects = self.input_effects(output);
        effects.push(self.display_effect());
        effects
    }
    pub(super) fn project_display(&mut self) -> Vec<Effect> {
        let mut e = self.reset_sleep_deadline();
        e.extend(self.sleep_if_due());
        if !e.iter().any(|e| matches!(e, Effect::Display { .. })) {
            e.push(self.display_effect());
        }
        e
    }
    pub(super) fn fail_queued_brightness(&mut self) -> Vec<Effect> {
        let mut effects = vec![];
        while let Some(job) = self.brightness_queue.pop_front() {
            effects.extend(
                self.job_complete(
                    job.id,
                    Completion::Failed(rad_core::execution::Failure::ActionFailed),
                )
                .expect("queued native dispatch"),
            );
        }
        effects
    }
    pub(super) fn persist_brightness(&mut self, job: NativeBrightness) -> Vec<Effect> {
        let fail = |this: &mut Self| {
            this.job_complete(
                job.id,
                Completion::Failed(rad_core::execution::Failure::ActionFailed),
            )
            .expect("queued brightness dispatch")
        };
        // A job that expired while waiting for another publication must never write.
        if job.deadline <= self.now {
            return fail(self);
        }
        let mut settings = self.published.configuration().settings().data();
        let next = job.direction.apply(settings.brightness_percent);
        if next == settings.brightness_percent {
            return self
                .job_complete(job.id, Completion::Succeeded)
                .expect("saturated native dispatch");
        }
        if self.plugin_barrier.is_some() {
            return fail(self);
        }
        settings.brightness_percent = next;
        // The admitted native operation owns precisely this fixed brightness edit;
        // it does not grant any client a general settings-modification capability.
        let Ok(candidate) = self.published.configuration().prepare_edit(
            self.published.registry(),
            Origin::Editor,
            Edit::SetSettings(settings),
        ) else {
            return fail(self);
        };
        let Ok(document) = rad_codec::encode(candidate.configuration()) else {
            return fail(self);
        };
        let Some(id) = self.command_id() else {
            return fail(self);
        };
        self.pending = Some(Pending {
            id,
            candidate: Some(candidate.clone()),
            cursor: None,
            target: PublicationTarget::Brightness(job.id),
        });
        self.awaiting = true;
        vec![Effect::Persist {
            id,
            candidate,
            document,
        }]
    }
}
