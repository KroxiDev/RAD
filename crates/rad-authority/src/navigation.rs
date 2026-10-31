//! Active-folder navigation and client disconnection.
use super::{Authority, Effect, EntityIdentity, Origin, Phase, Rejection, StorageState};

impl Authority {
    pub fn navigate(
        &mut self,
        origin: Origin,
        folder: EntityIdentity,
    ) -> Result<Vec<Effect>, Rejection> {
        if self.state != StorageState::Ready {
            return Err(Rejection::StorageUncertain);
        }
        if origin != Origin::Editor
            || !self
                .published
                .configuration()
                .data()
                .folders
                .iter()
                .any(|f| f.binding.identity() == folder)
        {
            return Err(Rejection::Invalid);
        }
        self.active = folder;
        let mut e = self.sync_input();
        e.push(Effect::SelectionChanged { folder });
        Ok(e)
    }
    pub fn navigate_id(
        &mut self,
        origin: Origin,
        id: rad_core::identity::ShortId,
    ) -> Result<Vec<Effect>, Rejection> {
        let folder = self
            .published
            .configuration()
            .data()
            .folders
            .iter()
            .find(|f| f.binding.short() == id)
            .ok_or(Rejection::Invalid)?
            .binding
            .identity();
        self.navigate(origin, folder)
    }
    pub fn disconnect(&mut self, connection: u64) {
        self.exports.retain(|_, pin| pin.connection != connection);
        let removed: Vec<_> = self
            .tickets
            .iter()
            .filter(|(_, s)| s.connection == connection && s.phase != Phase::Started)
            .map(|(id, _)| *id)
            .collect();
        self.queue.retain(|(id, _)| !removed.contains(id));
        for id in removed {
            self.tickets.remove(&id);
        }
    }
}
