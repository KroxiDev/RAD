//! Deck attachment, setting transitions, input events, and display projection.
use super::{Arc, EventCode, RuntimeError, Server, monotonic_ms};

impl Server {
    /// Adopts a deck that is already open, whatever the setting says; for tests and
    /// fixtures. The binary registers how to open one with [`Self::attach_hardware`].
    pub fn attach_deck_port(
        &mut self,
        deck: Box<dyn crate::display::DeckPort>,
    ) -> Result<(), RuntimeError> {
        if self.deck.is_some() {
            return Err(RuntimeError::Protocol);
        }
        self.deck = Some(deck);
        self.display_dirty = true;
        Ok(())
    }
    /// Production bootstrap: registers how to open the Stream Deck and opens it now if the
    /// machine setting «Usar el Stream Deck con RAD» is on. The setting decides, not the caller.
    pub fn attach_hardware(&mut self, config: rad_deck::DeviceConfig) -> Result<(), RuntimeError> {
        self.attach_deck_with(move |desired, wake| {
            let deck = rad_deck::Deck::spawn(config, desired, wake)?;
            Ok(Box::new(deck))
        })
    }
    /// `open` is the seam for the failure path; production always passes the real deck.
    pub(super) fn attach_deck_with(
        &mut self,
        open: impl FnMut(
            rad_deck::Desired,
            Arc<mio::Waker>,
        ) -> Result<Box<dyn crate::display::DeckPort>, rad_deck::DeckError>
        + 'static,
    ) -> Result<(), RuntimeError> {
        if self.deck.is_some() || self.deck_factory.is_some() {
            return Err(RuntimeError::Protocol);
        }
        self.deck_factory = Some(Box::new(open));
        self.deck_open_pending = self.machine.use_stream_deck;
        self.settle_deck()
    }
    /// One attempt to open the deck. A failure is contained: the runtime serves without a
    /// deck, says so, and does not try again until the setting is turned off and on.
    fn open_deck(&mut self) {
        self.deck_open_pending = false;
        let desired = crate::display::project(
            &self.actor,
            &self.pictures,
            self.brightness,
            self.asleep,
            self.plugin_visuals.as_ref(),
        );
        let Some(open) = &mut self.deck_factory else {
            return;
        };
        match open(desired, self._wake.clone()) {
            Ok(deck) => {
                self.deck = Some(deck);
                self.deck_inaccessible = false;
                self.display_dirty = true;
            }
            Err(_) => {
                self.deck_inaccessible = true;
                self.events.record(EventCode::DeckSpawnFailed, None);
            }
        }
    }
    /// Brings the deck to what the setting asks, one step at a time: lets go of a deck that
    /// finished stopping (Authority hears the input is gone even if the deck never said so),
    /// then opens the one that is waiting. Never while the runtime is ending.
    pub(super) fn settle_deck(&mut self) -> Result<(), RuntimeError> {
        if self.shutting_down {
            return Ok(());
        }
        if self.deck_stopping && self.deck.as_ref().is_some_and(|deck| deck.is_stopped()) {
            self.deck = None;
            self.deck_stopping = false;
            let before = (self.deck_connected, self.deck_inaccessible);
            self.deck_connected = false;
            self.deck_inaccessible = false;
            let now = monotonic_ms().map_err(|_| RuntimeError::Clock)?;
            let effects = self
                .actor
                .input_disconnect(now)
                .map_err(|_| RuntimeError::Clock)?;
            self.note_deck(before);
            self.effects(effects)?;
        }
        if self.deck.is_none() && self.deck_open_pending {
            self.open_deck();
        }
        Ok(())
    }
    /// The setting for the deck changed (already stored): turn it on at the next chance, or
    /// stop the deck in order. The real deck allows one alive at a time, so a new one waits.
    pub(super) fn apply_deck_setting(&mut self, on: bool) -> Result<(), RuntimeError> {
        self.deck_open_pending = on;
        if !on {
            match &mut self.deck {
                Some(deck) if !self.deck_stopping => {
                    deck.shutdown();
                    self.deck_stopping = true;
                }
                Some(_) => {}
                None => self.deck_inaccessible = false,
            }
        }
        self.pump_deck()?;
        self.settle_deck()
    }
    /// What clients see of the deck. A deck that could not be had is `Inaccessible`, not
    /// `Disabled`: the user asked for it.
    pub(super) fn deck_state(&self) -> rad_api::DeckState {
        use rad_api::DeckState;
        if self.deck.is_none() && !self.deck_inaccessible {
            DeckState::Disabled
        } else if self.deck_connected {
            DeckState::Connected
        } else if self.deck_inaccessible {
            DeckState::Inaccessible
        } else {
            DeckState::Disconnected
        }
    }
    pub(super) fn pump_deck(&mut self) -> Result<(), RuntimeError> {
        // Bound matches the adapter event budget; reports are never coalesced.
        for _ in 0..64 {
            let Some(event) = self.deck.as_mut().and_then(|deck| deck.next_event()) else {
                break;
            };
            let now = monotonic_ms().map_err(|_| RuntimeError::Clock)?;
            let before = (self.deck_connected, self.deck_inaccessible);
            let effects = match event {
                rad_deck::Event::Connected => {
                    self.deck_connected = true;
                    self.deck_inaccessible = false;
                    self.display_dirty = true;
                    self.actor.input_disconnect(now)
                }
                rad_deck::Event::Report(keys) => self.actor.input_report(now, keys),
                rad_deck::Event::Disconnected(error) => {
                    self.deck_connected = false;
                    self.deck_inaccessible = error != rad_deck::DeckError::Absent;
                    self.actor.input_disconnect(now)
                }
                rad_deck::Event::Stopped { .. } => {
                    self.deck_connected = false;
                    self.actor.input_disconnect(now)
                }
            }
            .map_err(|_| RuntimeError::Clock)?;
            self.note_deck(before);
            self.effects(effects)?;
        }
        Ok(())
    }
    /// Records a change of what the deck is, not each report: a flapping cable costs one
    /// event per change, and a repeated state none.
    fn note_deck(&mut self, before: (bool, bool)) {
        let after = (self.deck_connected, self.deck_inaccessible);
        if after == before {
            return;
        }
        self.events.record(
            if self.deck_connected {
                EventCode::DeckConnected
            } else if self.deck_inaccessible {
                EventCode::DeckInaccessible
            } else {
                EventCode::DeckDisconnected
            },
            None,
        );
    }
    pub(super) fn paint(&mut self) {
        if !self.display_dirty {
            return;
        }
        self.display_dirty = false;
        if let Some(deck) = &mut self.deck {
            let desired = crate::display::project(
                &self.actor,
                &self.pictures,
                self.brightness,
                self.asleep,
                self.plugin_visuals.as_ref(),
            );
            if deck.set_desired(desired).is_err() {
                let before = (self.deck_connected, self.deck_inaccessible);
                self.deck_connected = false;
                self.note_deck(before);
            }
        }
    }
}
