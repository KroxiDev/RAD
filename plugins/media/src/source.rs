use crate::{
    bus::{Bus, Failure},
    names::first_instance,
    player,
    wake::Wake,
};
use std::{
    sync::{Mutex, MutexGuard, PoisonError},
    time::Instant,
};

struct Player {
    bus: String,
    name: String,
    owner: String,
}

impl Player {
    fn new(bus: &str, name: &str, owner: &str) -> Self {
        Self {
            bus: bus.to_owned(),
            name: name.to_owned(),
            owner: owner.to_owned(),
        }
    }
}

#[derive(Default)]
struct Selection {
    revision: u64,
    player: Option<Player>,
}

pub(crate) struct Source {
    pub(crate) wake: Wake,
    selected: Mutex<Selection>,
}

impl Source {
    pub(crate) fn new() -> Self {
        Self {
            wake: Wake::new(),
            selected: Mutex::default(),
        }
    }

    fn selected(&self) -> MutexGuard<'_, Selection> {
        self.selected.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn owner_changed(&self, bus: &str, name: &str, owner: &str) {
        let mut selected = self.selected();
        if selected
            .player
            .as_ref()
            .is_some_and(|p| p.bus == bus && name > p.name.as_str())
        {
            return;
        }
        selected.revision += 1;
        selected.player = (!owner.is_empty()).then(|| Player::new(bus, name, owner));
        self.wake.changed();
    }

    pub(crate) fn properties_changed(&self, bus: &str, owner: &str) {
        if self
            .selected()
            .player
            .as_ref()
            .is_some_and(|p| p.bus == bus && p.owner == owner)
        {
            self.wake.changed();
        }
    }

    fn record(&self, revision: u64, bus: &str, name: &str, owner: &str) -> bool {
        let mut selected = self.selected();
        if selected.revision != revision
            || selected
                .player
                .as_ref()
                .is_some_and(|p| p.bus == bus && p.name == name && p.owner == owner)
        {
            return false;
        }
        selected.player = Some(Player::new(bus, name, owner));
        true
    }

    pub(crate) fn read(
        &self,
        bus: &Bus,
        app: &str,
        deadline: Instant,
    ) -> Result<player::State, Failure> {
        let connection = bus.connection(deadline)?;
        let revision = self.selected().revision;
        let names = connection.player_names(deadline)?;
        let Some(name) = first_instance(names.iter().map(String::as_str), app) else {
            let mut selected = self.selected();
            if selected.revision == revision {
                selected.player = None;
            }
            return Err(Failure::Unavailable);
        };
        let guid = connection.guid();
        let known = self
            .selected()
            .player
            .as_ref()
            .is_some_and(|p| p.bus == guid.as_str() && p.name == name);
        if !known {
            let now = Instant::now();
            let lookup_deadline = now + deadline.saturating_duration_since(now) / 2;
            if let Ok(owner) = connection.name_owner(name, lookup_deadline) {
                self.record(revision, &guid, name, &owner);
            }
        }
        player::state(&connection, name, deadline, |sender| {
            if self.record(revision, &guid, name, sender) {
                self.wake.changed();
            }
        })
    }
}
