//! Per-process credential state. No retries; ACKs are one-shot and deadline bounded.
use crate::{Diagnostic, Error, Host, State};
use rad_plugin_contract::{Correlation, CredentialValues, MAX_CREDENTIAL_REVISIONS, Message};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialState {
    Installing,
    Applied,
    Releasing,
    Released,
}
pub(super) struct Installed {
    pub state: CredentialState,
    pub correlation: Correlation,
    pub deadline: u64,
    // Retain the private copy until the peer releases it or is actually reaped.
    pub values: Option<CredentialValues>,
}
impl Host {
    /// Install one complete snapshot. Only an Applied state permits dispatch.
    pub fn install_credentials(
        &mut self,
        id: &str,
        generation: u64,
        revision: u64,
        values: CredentialValues,
    ) -> Result<Correlation, Error> {
        let deadline = self.now.checked_add(2000).ok_or(Error::Clock)?;
        let w = self.workers.get_mut(id).ok_or(Error::Unavailable)?;
        if w.generation != generation
            || w.state != State::Ready
            || !rad_plugin_contract::supports(&w.capabilities, rad_plugin_contract::CREDENTIALS)
        {
            return Err(Error::Unavailable);
        }
        let declared = w.manifest.credential_slots();
        if declared.is_empty()
            || revision == 0
            || revision <= w.last_revision
            || values
                .as_slice()
                .iter()
                .any(|v| !declared.contains(v.slot.as_str()))
        {
            return Err(Error::Invalid);
        }
        if w.credentials.len() >= MAX_CREDENTIAL_REVISIONS {
            return Err(Error::Capacity);
        }
        let correlation = w.send(Message::Credentials {
            revision,
            values: values.as_slice().to_vec(),
        })?;
        w.credentials.insert(
            revision,
            Installed {
                state: CredentialState::Installing,
                correlation,
                deadline,
                values: Some(values),
            },
        );
        w.last_revision = revision;
        Ok(correlation)
    }
    /// None means unknown, never proof of a release ACK.
    pub fn credentials_state(
        &self,
        id: &str,
        generation: u64,
        revision: u64,
    ) -> Option<CredentialState> {
        let w = self.workers.get(id)?;
        (w.generation == generation && w.state == State::Ready)
            .then(|| w.credentials.get(&revision).map(|r| r.state))
            .flatten()
    }
    /// Caller must also have relinquished snapshots for operations not yet dispatched.
    /// Active jobs, residuals and options cannot be released by a logical timeout.
    pub fn release_credentials(
        &mut self,
        id: &str,
        generation: u64,
        revision: u64,
    ) -> Result<Correlation, Error> {
        let deadline = self.now.checked_add(2000).ok_or(Error::Clock)?;
        let w = self.workers.get_mut(id).ok_or(Error::Unavailable)?;
        if w.generation != generation || w.state != State::Ready {
            return Err(Error::Unavailable);
        }
        if w.credentials
            .get(&revision)
            .is_none_or(|r| r.state != CredentialState::Applied)
        {
            return Err(Error::Invalid);
        }
        if w.hold_uses_revision(revision)
            || w.pending
                .values()
                .any(|p| p.credential_revision == Some(revision))
            || w.queries
                .values()
                .any(|p| p.credential_revision == Some(revision))
            || w.visuals
                .values()
                .any(|p| p.credential_revision == Some(revision))
            || !w.unsubscribes.is_empty()
        {
            return Err(Error::Residual);
        }
        let correlation = w.send(Message::CredentialsRelease { revision })?;
        let r = w.credentials.get_mut(&revision).expect("checked revision");
        r.state = CredentialState::Releasing;
        r.correlation = correlation;
        r.deadline = deadline;
        Ok(correlation)
    }
    /// One-shot confirmed release. Undrained ACKs occupy their original revision slot.
    pub fn take_credentials_released(&mut self) -> Option<(String, u64, u64)> {
        for (id, w) in &mut self.workers {
            if let Some(revision) = w
                .credentials
                .iter()
                .find_map(|(r, v)| (v.state == CredentialState::Released).then_some(*r))
            {
                w.credentials.remove(&revision);
                return Some((id.clone(), w.generation, revision));
            }
        }
        None
    }
}
impl crate::worker::Worker {
    pub fn check_credentials(&self, revision: Option<u64>) -> Result<(), Error> {
        if self.manifest.credential_slots().is_empty() {
            return if revision.is_none() {
                Ok(())
            } else {
                Err(Error::Invalid)
            };
        }
        if rad_plugin_contract::supports(&self.capabilities, rad_plugin_contract::CREDENTIALS)
            && revision
                .and_then(|r| self.credentials.get(&r))
                .is_some_and(|r| r.state == CredentialState::Applied)
        {
            Ok(())
        } else {
            Err(Error::Unavailable)
        }
    }
    pub fn credential_ack(
        &mut self,
        correlation: Correlation,
        revision: u64,
        release: bool,
        now: u64,
    ) {
        let expected = if release {
            CredentialState::Releasing
        } else {
            CredentialState::Installing
        };
        let Some(r) = self.credentials.get_mut(&revision) else {
            self.fail(Diagnostic::Protocol);
            return;
        };
        if self.state != State::Ready
            || !rad_plugin_contract::supports(&self.capabilities, rad_plugin_contract::CREDENTIALS)
            || r.state != expected
            || r.correlation != correlation
            || now >= r.deadline
        {
            self.fail(Diagnostic::Protocol);
            return;
        }
        r.state = if release {
            CredentialState::Released
        } else {
            CredentialState::Applied
        };
        if release {
            r.values.take();
        }
    }
    pub fn credential_deadline(&self) -> Option<u64> {
        self.credentials
            .values()
            .filter(|r| {
                matches!(
                    r.state,
                    CredentialState::Installing | CredentialState::Releasing
                )
            })
            .map(|r| r.deadline)
            .min()
    }
}
