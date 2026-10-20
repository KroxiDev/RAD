//! Pure identity values and a non-recycling registry.
//!
//! Restore produces a candidate, not a durable commit. The future Store must
//! persist the complete ledger, including retired bindings, atomically with the
//! configuration. Reconstructing only the active snapshot would lose reservations.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
/// Classes of configured entities; Principal is a Folder, not a special ID.
pub enum EntityKind {
    Folder,
    Button,
    Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
/// User-visible class-prefixed positive identifier, independent of names.
pub struct ShortId {
    kind: EntityKind,
    number: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
/// Opaque provenance: nonzero installation token and positive local serial.
/// The caller supplies the installation token; this module does not generate it.
pub struct EntityIdentity {
    installation: u128,
    serial: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Structural rejection or exhausted allocation space; never a storage verdict.
pub enum IdentityError {
    InvalidValue,
    ShortIdCollision,
    IdentityCollision,
    ConflictingRetirement,
    ShortIdExhausted,
    SerialExhausted,
}

impl ShortId {
    pub fn new(kind: EntityKind, number: u64) -> Result<Self, IdentityError> {
        if number == 0 {
            return Err(IdentityError::InvalidValue);
        }
        Ok(Self { kind, number })
    }
    pub fn kind(self) -> EntityKind {
        self.kind
    }
    pub fn number(self) -> u64 {
        self.number
    }
}
impl EntityIdentity {
    pub fn new(installation: u128, serial: u64) -> Result<Self, IdentityError> {
        if installation == 0 || serial == 0 {
            return Err(IdentityError::InvalidValue);
        }
        Ok(Self {
            installation,
            serial,
        })
    }
    pub fn installation(self) -> u128 {
        self.installation
    }
    pub fn serial(self) -> u64 {
        self.serial
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// An asserted association; the registry validates uniqueness in both directions.
pub struct Binding {
    short: ShortId,
    identity: EntityIdentity,
}
impl Binding {
    pub fn new(short: ShortId, identity: EntityIdentity) -> Self {
        Self { short, identity }
    }
    pub fn short(self) -> ShortId {
        self.short
    }
    pub fn identity(self) -> EntityIdentity {
        self.identity
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Store-facing value including retirement. Construction does not validate a ledger.
pub struct IdentityRecord {
    binding: Binding,
    retired: bool,
}
impl IdentityRecord {
    pub fn new(binding: Binding, retired: bool) -> Self {
        Self { binding, retired }
    }
    pub fn binding(self) -> Binding {
        self.binding
    }
    pub fn retired(self) -> bool {
        self.retired
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// Retained local ledger plus durable counters. Fields are never mutable views.
///
/// Both keys are indexed; retired assignments can be pruned without releasing their
/// numeric reservations. The live-entity limit belongs to Configuration.
pub struct IdentityRegistry {
    installation: u128,
    counters: IdentityCounters,
    records: BTreeMap<ShortId, IdentityRecord>,
    identities: BTreeMap<EntityIdentity, ShortId>,
}
impl IdentityRegistry {
    /// Start a genuinely new installation; never use as corruption recovery.
    pub fn new(installation: u128) -> Result<Self, IdentityError> {
        if installation == 0 {
            return Err(IdentityError::InvalidValue);
        }
        Ok(Self {
            installation,
            counters: IdentityCounters::default(),
            records: BTreeMap::new(),
            identities: BTreeMap::new(),
        })
    }
    pub fn installation(&self) -> u128 {
        self.installation
    }
    /// Copied records for persistence, not configuration ordering or mutable views.
    pub fn records(&self) -> impl Iterator<Item = IdentityRecord> + '_ {
        self.records.values().copied()
    }
    /// Assign a fresh binding (also the primitive for duplication).
    /// High-water marks include retired/imported entries. Checks precede mutation;
    /// local serials ignore foreign serials but short-ID counters include all origins.
    pub fn allocate(&mut self, kind: EntityKind) -> Result<Binding, IdentityError> {
        let number = self
            .counters
            .number(kind)
            .checked_add(1)
            .ok_or(IdentityError::ShortIdExhausted)?;
        let serial = self
            .counters
            .serial
            .checked_add(1)
            .ok_or(IdentityError::SerialExhausted)?;
        let binding = Binding::new(
            ShortId::new(kind, number)?,
            EntityIdentity::new(self.installation, serial)?,
        );
        self.insert(IdentityRecord::new(binding, false));
        Ok(binding)
    }

    /// Prepare replacement of the active identity set, preserving every reservation.
    /// Repeated identical bindings are idempotent; either collision rejects the whole
    /// candidate. Original state is unchanged on success as well as on failure.
    /// This does not validate a configuration tree or implement undo history.
    pub fn restore(&self, active: &[Binding]) -> Result<Self, IdentityError> {
        let mut candidate = self.clone();
        for record in candidate.records.values_mut() {
            record.retired = true;
        }
        for binding in active {
            candidate.check_binding(*binding)?;
            if let Some(record) = candidate.records.get_mut(&binding.short) {
                record.retired = false;
            } else {
                candidate.insert(IdentityRecord::new(*binding, false));
            }
        }
        Ok(candidate)
    }

    /// Reconstruct a COMPLETE ledger supplied by Store, not an old active snapshot.
    /// Exact duplicates are idempotent; contradictory retirement or either identity
    /// collision is rejected. This legacy reconstruction derives counters from a complete ledger.
    /// Missing records cannot be detected here: Store must verify integrity and must
    /// not roll this ledger back when recovering an older configuration.
    pub fn from_records(
        installation: u128,
        records: impl IntoIterator<Item = IdentityRecord>,
    ) -> Result<Self, IdentityError> {
        let mut registry = Self::new(installation)?;
        for record in records {
            registry.check_binding(record.binding)?;
            if let Some(existing) = registry.records.get(&record.binding.short) {
                if existing.retired != record.retired {
                    return Err(IdentityError::ConflictingRetirement);
                }
            } else {
                registry.insert(record);
            }
        }
        Ok(registry)
    }
}

impl IdentityRegistry {
    /// Called only after both keys have been validated or freshly allocated.
    fn insert(&mut self, record: IdentityRecord) {
        let binding = record.binding;
        let number = self.counters.number_mut(binding.short.kind);
        *number = (*number).max(binding.short.number);
        if binding.identity.installation == self.installation {
            self.counters.serial = self.counters.serial.max(binding.identity.serial);
        }
        self.identities
            .insert(record.binding.identity, record.binding.short);
        self.records.insert(record.binding.short, record);
    }

    fn check_binding(&self, binding: Binding) -> Result<(), IdentityError> {
        if self
            .records
            .get(&binding.short)
            .is_some_and(|record| record.binding.identity != binding.identity)
        {
            return Err(IdentityError::ShortIdCollision);
        }
        if self
            .identities
            .get(&binding.identity)
            .is_some_and(|short| *short != binding.short)
        {
            return Err(IdentityError::IdentityCollision);
        }
        Ok(())
    }
}
impl std::fmt::Display for ShortId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefix = match self.kind {
            EntityKind::Folder => "f",
            EntityKind::Button => "b",
            EntityKind::Action => "a",
        };
        write!(f, "{prefix}{}", self.number)
    }
}

/// Durable high-water marks; independent of the retained identity ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IdentityCounters {
    pub folders: u64,
    pub buttons: u64,
    pub actions: u64,
    pub serial: u64,
}
impl IdentityCounters {
    pub fn number(self, kind: EntityKind) -> u64 {
        match kind {
            EntityKind::Folder => self.folders,
            EntityKind::Button => self.buttons,
            EntityKind::Action => self.actions,
        }
    }
    fn number_mut(&mut self, kind: EntityKind) -> &mut u64 {
        match kind {
            EntityKind::Folder => &mut self.folders,
            EntityKind::Button => &mut self.buttons,
            EntityKind::Action => &mut self.actions,
        }
    }
    pub fn contains(self, other: Self) -> bool {
        self.folders >= other.folders
            && self.buttons >= other.buttons
            && self.actions >= other.actions
            && self.serial >= other.serial
    }
}
impl IdentityRegistry {
    pub fn counters(&self) -> IdentityCounters {
        self.counters
    }
    /// Reconstruct a pruned ledger only alongside its integrity-checked durable counters.
    pub fn from_records_with_counters(
        installation: u128,
        records: impl IntoIterator<Item = IdentityRecord>,
        counters: IdentityCounters,
    ) -> Result<Self, IdentityError> {
        let mut registry = Self::from_records(installation, records)?;
        if !counters.contains(registry.counters) {
            return Err(IdentityError::InvalidValue);
        }
        registry.counters = counters;
        Ok(registry)
    }
    pub fn reserve_counters(&mut self, counters: IdentityCounters) {
        self.counters.folders = self.counters.folders.max(counters.folders);
        self.counters.buttons = self.counters.buttons.max(counters.buttons);
        self.counters.actions = self.counters.actions.max(counters.actions);
        self.counters.serial = self.counters.serial.max(counters.serial);
    }
    /// Retired assignments outlive only their history/revision/export roots.
    pub fn retain_bindings(&mut self, roots: &std::collections::BTreeSet<EntityIdentity>) {
        self.records
            .retain(|_, r| !r.retired || roots.contains(&r.binding.identity));
        self.identities
            .retain(|_, short| self.records.contains_key(short));
    }
}
