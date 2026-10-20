//! Prepaid, generation-scoped release ownership. A Start never exists without an End credit.
use crate::{
    Correlation, Diagnostic, Error, Host, InvokeContext, Parameters, State,
    worker::{Pending, Worker},
};
use rad_plugin_contract::{ControlGesture, Message};
use std::sync::Arc;
pub const MAX_PLUGIN_HOLDS: usize = 32;
pub const MAX_HOLDS: usize = 64;
/// Linear authority to release one dispatched Start, even after its response timed out.
/// Dropping a ticket does not release the reservation: it stays owned until process death.
pub struct HoldTicket {
    owner: Arc<()>,
    id: String,
    start: Correlation,
}
pub(super) struct Held {
    action: String,
    parameters: Parameters,
    revision: Option<u64>,
    context: InvokeContext,
}
impl Worker {
    pub fn hold_credits(&self) -> usize {
        self.holds.len() + self.pending.values().filter(|p| p.reserved_end).count()
    }
    pub fn hold_uses_revision(&self, revision: u64) -> bool {
        self.holds.values().any(|h| h.revision == Some(revision))
    }
}
impl Host {
    #[allow(clippy::too_many_arguments)]
    pub fn invoke_hold_start(
        &mut self,
        id: &str,
        generation: u64,
        action: &str,
        params: Parameters,
        timing: (u64, Option<u64>),
        context: InvokeContext,
    ) -> Result<(Correlation, HoldTicket), Error> {
        if context
            .control
            .as_ref()
            .is_none_or(|c| c.gesture != ControlGesture::HoldStart)
        {
            return Err(Error::Invalid);
        }
        if self
            .workers
            .values()
            .map(Worker::hold_credits)
            .sum::<usize>()
            >= MAX_HOLDS
            || self
                .workers
                .get(id)
                .is_some_and(|w| w.hold_credits() >= MAX_PLUGIN_HOLDS)
        {
            return Err(Error::Capacity);
        }
        let held = Held {
            action: action.into(),
            parameters: params.clone(),
            revision: timing.1,
            context: context.clone(),
        };
        // Single-threaded admission: no callback/pump can interleave the checked reservation.
        let start = self.invoke_context_inner(id, generation, action, params, timing, context)?;
        self.workers
            .get_mut(id)
            .expect("invoke admitted worker")
            .holds
            .insert(start.request, held);
        Ok((
            start,
            HoldTicket {
                owner: self.owner.clone(),
                id: id.into(),
                start,
            },
        ))
    }
    pub fn release_hold(
        &mut self,
        ticket: HoldTicket,
        deadline: u64,
    ) -> Result<Correlation, Error> {
        if !Arc::ptr_eq(&ticket.owner, &self.owner) {
            return Err(Error::Invalid);
        }
        let w = self.workers.get_mut(&ticket.id).ok_or(Error::Unavailable)?;
        if w.generation != ticket.start.generation || w.state != State::Ready {
            return Err(Error::Unavailable);
        }
        if deadline <= self.now {
            // Consumed release authority cannot be retried. Stop the owner rather than strand it.
            w.fail(Diagnostic::Deadline);
            return Err(Error::Clock);
        }
        let h = w.holds.get(&ticket.start.request).ok_or(Error::Invalid)?;
        let mut control = h.context.control.clone().expect("Start control");
        control.gesture = ControlGesture::HoldEnd;
        let revision = h.revision;
        let message = Message::Invoke {
            action: h.action.clone(),
            parameters: h.parameters.clone(),
            settings: h.context.settings.clone(),
            control: Some(control),
            credential_revision: revision,
        };
        let correlation = match w.send_reserved_end(message) {
            Ok(c) => c,
            Err(e) => {
                w.fail(Diagnostic::Backpressure);
                return Err(e);
            }
        };
        w.holds.remove(&ticket.start.request);
        let mut pending = Pending::with_credentials(deadline, revision);
        pending.reserved_end = true;
        w.pending.insert(correlation.request, pending);
        Ok(correlation)
    }
    /// Only process loss discards an unpaid release. A live hold cannot be forgotten.
    pub fn discard_hold(&mut self, ticket: HoldTicket) -> Result<(), Error> {
        if !Arc::ptr_eq(&ticket.owner, &self.owner) {
            return Err(Error::Invalid);
        }
        let Some(w) = self.workers.get_mut(&ticket.id) else {
            return Ok(());
        };
        if w.generation != ticket.start.generation {
            return Ok(());
        }
        if w.state == State::Ready || w.state == State::Starting {
            return Err(Error::Residual);
        }
        // Keep snapshot/credential ownership until reap, even if caller drops its authority.
        Ok(())
    }
}
