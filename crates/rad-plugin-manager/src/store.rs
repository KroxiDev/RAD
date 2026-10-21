use crate::Error;
use rad_plugin_contract::Manifest;
use std::{fs::File, path::Path, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Install,
    Update(String),
    Reinstall(String),
    Rollback(String),
    Uninstall(String),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerState {
    Ready,
    Prepared,
    RecoveryRequired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitOutcome {
    Applied,
    NotApplied(Error),
    Uncertain(Error),
}
#[derive(Clone)]
pub struct ChangeKey {
    owner: Arc<()>,
    revision: u64,
}
impl PartialEq for ChangeKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner) && self.revision == other.revision
    }
}
impl Eq for ChangeKey {}
pub struct PreparedChange {
    key: ChangeKey,
    id: String,
    manifest: Option<Manifest>,
    operation: Operation,
}
impl PreparedChange {
    pub fn key(&self) -> ChangeKey {
        self.key.clone()
    }
    pub fn plugin_id(&self) -> &str {
        &self.id
    }
    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()
    }
    pub fn operation(&self) -> &Operation {
        &self.operation
    }
}
#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    pub manifest: Manifest,
    pub previous: Option<Manifest>,
    pub source: Option<String>,
}
pub struct Manager {
    root: crate::files::Directory,
    index: crate::ledger::Index,
    host_api: u32,
    target: String,
    owner: Arc<()>,
    pending: Option<Journal>,
    serial: u64,
    recovery: bool,
    manifests: std::cell::RefCell<std::collections::BTreeMap<u64, Manifest>>,
}
impl Manager {
    pub fn create(path: &Path, host_api: u32, target: &str) -> Result<Self, Error> {
        let root = crate::files::Directory::open(path)?;
        if !root.names()?.is_empty() {
            return Err(Error::CorruptState);
        }
        let index = crate::ledger::Index::empty();
        root.write("index.json", &index.bytes()?, 0o600)?;
        root.sync()?;
        Ok(Self {
            root,
            index,
            host_api,
            target: target.into(),
            owner: Arc::new(()),
            pending: None,
            serial: 0,
            recovery: false,
            manifests: Default::default(),
        })
    }
    pub fn open(path: &Path, host_api: u32, target: &str) -> Result<Self, Error> {
        let root = crate::files::Directory::open(path)?;
        let index = crate::ledger::Index::parse(&root.read("index.json", 65536)?)?;
        let mut manager = Self {
            root,
            index,
            host_api,
            target: target.into(),
            owner: Arc::new(()),
            pending: None,
            serial: 0,
            recovery: false,
            manifests: Default::default(),
        };
        if manager.root.names()?.iter().any(|n| n == "journal.json") {
            let bytes = manager.root.read("journal.json", 131072)?;
            let journal: Journal =
                serde_json::from_slice(&bytes).map_err(|_| Error::CorruptState)?;
            if journal.bytes()? != bytes {
                return Err(Error::CorruptState);
            }
            journal.validate()?;
            manager.pending = Some(journal);
            manager.recovery = true;
            if matches!(manager.reconcile()?, CommitOutcome::Uncertain(_)) {
                return Err(Error::RecoveryRequired);
            }
        }
        manager.remove_journal_next()?;
        manager.validate_inventory()?;
        manager.installed()?;
        manager.root.sync()?;
        Ok(manager)
    }
    pub fn installed(&self) -> Result<Vec<InstalledPlugin>, Error> {
        self.index
            .plugins
            .iter()
            .map(|(id, v)| {
                Ok(InstalledPlugin {
                    manifest: self.load(v.current, id)?,
                    source: v.source.clone(),
                    previous: v.previous.map(|n| self.load(n, id)).transpose()?,
                })
            })
            .collect()
    }
    pub fn state(&self) -> ManagerState {
        if self.recovery {
            ManagerState::RecoveryRequired
        } else if self.pending.is_some() {
            ManagerState::Prepared
        } else {
            ManagerState::Ready
        }
    }
    /// A source here records the caller's explicit trust for this plugin origin only.
    pub fn prepare_from_source(
        &mut self,
        operation: Operation,
        package: Option<&[u8]>,
        source: Option<&crate::GithubSource>,
        replaces: Option<&crate::GithubSource>,
    ) -> Result<PreparedChange, Error> {
        // Only recover failures caused by this admitted operation, never older work.
        if self.state() != ManagerState::Ready {
            return Err(Error::Busy);
        }
        let result = self.prepare_inner(operation, package, source, replaces);
        if result.is_err() && self.state() == ManagerState::RecoveryRequired {
            let _ = self.reconcile();
        }
        result
    }
    /// Worker entry point: retain the original error, reconcile owned failed staging.
    pub fn prepare_recovering(
        &mut self,
        operation: Operation,
        package: Option<&[u8]>,
    ) -> Result<PreparedChange, Error> {
        self.prepare_from_source(operation, package, None, None)
    }
    pub fn prepare(
        &mut self,
        operation: Operation,
        package: Option<&[u8]>,
    ) -> Result<PreparedChange, Error> {
        self.prepare_inner(operation, package, None, None)
    }
    /// A source from another repository than the registered one replaces it only when the
    /// caller confirms that same registered origin; nothing is written before this check.
    fn confirm_source_change(
        old: Option<&crate::ledger::Versions>,
        source: Option<&crate::GithubSource>,
        replaces: Option<&crate::GithubSource>,
    ) -> Result<(), Error> {
        let (Some(registered), Some(requested)) =
            (old.and_then(|versions| versions.source.as_deref()), source)
        else {
            return Ok(());
        };
        let registered = crate::GithubSource::parse(registered).map_err(|_| Error::CorruptState)?;
        if registered.same_origin(requested) || replaces.is_some_and(|r| r.same_origin(&registered))
        {
            Ok(())
        } else {
            Err(Error::SourceChanged)
        }
    }
    fn prepare_inner(
        &mut self,
        operation: Operation,
        package: Option<&[u8]>,
        source: Option<&crate::GithubSource>,
        replaces: Option<&crate::GithubSource>,
    ) -> Result<PreparedChange, Error> {
        if self.state() != ManagerState::Ready {
            return Err(Error::Busy);
        }
        self.validate_inventory()?;
        let candidate = package
            .map(|bytes| {
                rad_plugin_package::Candidate::parse(bytes, self.host_api, &self.target)
                    .map_err(|_| Error::InvalidPackage)
            })
            .transpose()?;
        let id = match &operation {
            Operation::Install => candidate
                .as_ref()
                .ok_or(Error::InvalidPackage)?
                .manifest()
                .id
                .clone(),
            Operation::Update(id)
            | Operation::Reinstall(id)
            | Operation::Rollback(id)
            | Operation::Uninstall(id) => id.clone(),
        };
        let old = self.index.plugins.get(&id).cloned();
        if matches!(operation, Operation::Update(_) | Operation::Reinstall(_)) {
            Self::confirm_source_change(old.as_ref(), source, replaces)?;
        }
        let mut after = self.index.clone();
        after.revision = after.revision.checked_add(1).ok_or(Error::Capacity)?;
        let mut new_slot = None;
        let manifest = match &operation {
            Operation::Install | Operation::Update(_) | Operation::Reinstall(_) => {
                let c = candidate.as_ref().ok_or(Error::InvalidPackage)?;
                if c.manifest().id != id {
                    return Err(Error::PluginMismatch);
                }
                if matches!(operation, Operation::Install) {
                    if old.is_some() {
                        return Err(Error::AlreadyInstalled);
                    }
                    if after.plugins.len() >= rad_plugin_contract::MAX_PLUGINS {
                        return Err(Error::PluginLimit);
                    }
                } else if matches!(operation, Operation::Update(_)) && old.is_none() {
                    return Err(Error::InvalidOperation);
                }
                if old.is_none() && after.plugins.len() >= rad_plugin_contract::MAX_PLUGINS {
                    return Err(Error::PluginLimit);
                }
                // Replacing the selected version with itself must not evict the real previous one.
                let same_version = match &old {
                    Some(v) => self.load(v.current, &id)?.version == c.manifest().version,
                    None => false,
                };
                if same_version && matches!(operation, Operation::Update(_)) {
                    return Err(Error::SameVersion);
                }
                new_slot = Some(after.revision);
                after.plugins.insert(
                    id.clone(),
                    crate::ledger::Versions {
                        current: after.revision,
                        previous: old.as_ref().and_then(|v| {
                            if same_version {
                                v.previous
                            } else {
                                Some(v.current)
                            }
                        }),
                        source: source
                            .map(crate::GithubSource::canonical_url)
                            .or_else(|| old.as_ref().and_then(|v| v.source.clone())),
                    },
                );
                Some(c.manifest().clone())
            }
            Operation::Rollback(_) => {
                if candidate.is_some() {
                    return Err(Error::InvalidOperation);
                }
                let v = old.ok_or(Error::InvalidOperation)?;
                let previous = v.previous.ok_or(Error::InvalidOperation)?;
                let manifest = self.load(previous, &id)?;
                after.plugins.insert(
                    id.clone(),
                    crate::ledger::Versions {
                        current: previous,
                        previous: Some(v.current),
                        source: v.source,
                    },
                );
                Some(manifest)
            }
            Operation::Uninstall(_) => {
                if candidate.is_some() || old.is_none() {
                    return Err(Error::InvalidOperation);
                }
                after.plugins.remove(&id);
                None
            }
        };
        let names = self.root.names()?;
        if new_slot.is_some_and(|n| names.contains(&crate::ledger::slot(n))) {
            return Err(Error::CorruptState);
        }
        let journal = Journal {
            before: self.index.clone(),
            after,
            new_slot,
        };
        if let Err(error) = self
            .root
            .write("journal.next", &journal.bytes()?, 0o600)
            .and_then(|_| self.root.publish("journal.next", "journal.json"))
        {
            self.recovery = self.remove_journal_next().is_err();
            return Err(error);
        }
        self.pending = Some(journal);
        self.recovery = true;
        self.root.sync()?;
        if let (Some(slot), Some(candidate)) = (new_slot, candidate.as_ref()) {
            let dir = self.root.mkdir(&crate::ledger::slot(slot))?;
            dir.write("manifest.json", candidate.manifest_bytes(), 0o600)?;
            checkpoint("after_manifest")?;
            dir.write("plugin", candidate.executable(), 0o700)?;
            dir.sync()?;
            self.root.sync()?;
            self.manifests
                .borrow_mut()
                .insert(slot, candidate.manifest().clone());
        }
        self.serial = self.serial.checked_add(1).ok_or(Error::Capacity)?;
        self.recovery = false;
        Ok(PreparedChange {
            key: ChangeKey {
                owner: self.owner.clone(),
                revision: self.serial,
            },
            id,
            manifest,
            operation,
        })
    }
    pub fn commit_recovering(&mut self, change: &mut Option<PreparedChange>) -> CommitOutcome {
        // A rejected token cannot authorize reconciliation of another operation.
        if let Err(error) = self.check(change) {
            return CommitOutcome::Uncertain(error);
        }
        let original = self.commit(change).unwrap_or_else(CommitOutcome::Uncertain);
        if self.state() != ManagerState::RecoveryRequired {
            return original;
        }
        let recovered = self.reconcile().unwrap_or_else(CommitOutcome::Uncertain);
        match (original, recovered) {
            (
                CommitOutcome::NotApplied(cause) | CommitOutcome::Uncertain(cause),
                CommitOutcome::NotApplied(_),
            ) => CommitOutcome::NotApplied(cause),
            (_, result) => result,
        }
    }
    pub fn commit(&mut self, change: &mut Option<PreparedChange>) -> Result<CommitOutcome, Error> {
        self.check(change)?;
        self.recovery = true;
        let selected = crate::ledger::Index::parse(&self.root.read("index.json", 65536)?)?;
        if self.pending.as_ref().is_none_or(|j| j.before != selected) {
            return Err(Error::CorruptState);
        }
        let after = self
            .pending
            .as_ref()
            .ok_or(Error::InvalidOperation)?
            .after
            .clone();
        self.recovery = true;
        if let Err(e) = self.root.write("index.next", &after.bytes()?, 0o600) {
            return Ok(CommitOutcome::NotApplied(e));
        }
        if let Err(e) = checkpoint("before_selector")
            .and_then(|_| self.root.replace("index.next", "index.json"))
        {
            return Ok(CommitOutcome::NotApplied(e));
        }
        self.index = after;
        change.take();
        if let Err(e) = checkpoint("after_selector").and_then(|_| self.root.sync()) {
            return Ok(CommitOutcome::Uncertain(e));
        }
        if let Err(e) = self.finish_journal(self.pending.as_ref().ok_or(Error::CorruptState)?) {
            return Ok(CommitOutcome::Uncertain(e));
        }
        if let Err(e) = checkpoint("after_cleanup") {
            return Ok(CommitOutcome::Uncertain(e));
        }
        self.pending = None;
        self.recovery = false;
        Ok(CommitOutcome::Applied)
    }
    pub fn abort(&mut self, change: &mut Option<PreparedChange>) -> Result<(), Error> {
        self.check(change)?;
        self.recovery = true;
        let selected = crate::ledger::Index::parse(&self.root.read("index.json", 65536)?)?;
        let journal = self.pending.as_ref().ok_or(Error::InvalidOperation)?;
        if selected != journal.before {
            return Err(Error::RecoveryRequired);
        }
        self.finish_journal(journal)?;
        change.take();
        self.pending = None;
        self.recovery = false;
        Ok(())
    }
    pub fn reconcile(&mut self) -> Result<CommitOutcome, Error> {
        let journal = self.pending.clone().ok_or(Error::InvalidOperation)?;
        let selected = crate::ledger::Index::parse(&self.root.read("index.json", 65536)?)?;
        let applied = selected == journal.after;
        if !applied && selected != journal.before {
            return Err(Error::CorruptState);
        }
        self.index = selected;
        self.recovery = true;
        if let Err(e) = self.finish_journal(&journal) {
            return Ok(CommitOutcome::Uncertain(e));
        }
        self.pending = None;
        self.recovery = false;
        Ok(if applied {
            CommitOutcome::Applied
        } else {
            CommitOutcome::NotApplied(Error::InvalidOperation)
        })
    }
    fn finish_journal(&self, journal: &Journal) -> Result<(), Error> {
        self.installed()?;
        self.root.sync()?;
        let keep = slots(&self.index);
        self.manifests
            .borrow_mut()
            .retain(|slot, _| keep.contains(slot));
        let mut remove = slots(&journal.before);
        remove.extend(journal.new_slot);
        for slot in remove.difference(&keep) {
            let name = crate::ledger::slot(*slot);
            if self.root.names()?.contains(&name) {
                self.root.remove_slot(&name)?;
            }
        }
        if self.root.names()?.iter().any(|n| n == "index.next") {
            self.root.unlink_file("index.next")?;
        }
        if self.root.names()?.iter().any(|n| n == "journal.json") {
            self.root.unlink_file("journal.json")?;
        }
        self.root.sync()
    }
    fn remove_journal_next(&self) -> Result<(), Error> {
        if self.root.names()?.iter().any(|n| n == "journal.next") {
            self.root.unlink_file("journal.next")?;
            self.root.sync()?;
        }
        Ok(())
    }
    fn validate_inventory(&self) -> Result<(), Error> {
        let mut expected: Vec<_> = slots(&self.index)
            .into_iter()
            .map(crate::ledger::slot)
            .collect();
        expected.push("index.json".into());
        expected.sort();
        if self.root.names()? != expected {
            return Err(Error::CorruptState);
        }
        Ok(())
    }
    pub fn executable(&self, id: &str) -> Result<File, Error> {
        let v = self.index.plugins.get(id).ok_or(Error::InvalidOperation)?;
        self.load(v.current, id)?
            .validate(self.host_api, &self.target)
            .map_err(|_| Error::Incompatible)?;
        self.root
            .child(&crate::ledger::slot(v.current))?
            .file("plugin")
    }
    fn check(&self, change: &Option<PreparedChange>) -> Result<(), Error> {
        let c = change.as_ref().ok_or(Error::InvalidOperation)?;
        if !Arc::ptr_eq(&c.key.owner, &self.owner) {
            return Err(Error::WrongOwner);
        }
        if c.key.revision != self.serial || self.pending.is_none() {
            return Err(Error::InvalidOperation);
        }
        if self.recovery {
            return Err(Error::RecoveryRequired);
        }
        Ok(())
    }
    fn load(&self, slot: u64, id: &str) -> Result<Manifest, Error> {
        if let Some(manifest) = self.manifests.borrow().get(&slot) {
            return if manifest.id == id {
                Ok(manifest.clone())
            } else {
                Err(Error::CorruptState)
            };
        }
        let dir = self.root.child(&crate::ledger::slot(slot))?;
        if dir.names()? != ["manifest.json", "plugin"] {
            return Err(Error::CorruptState);
        }
        let manifest = dir.read("manifest.json", 65536)?;
        let binary = dir.read("plugin", 67108864)?;
        let metadata = rad_plugin_package::Candidate::from_parts(
            &manifest,
            &binary,
            self.host_api,
            &self.target,
        )
        .map(|candidate| candidate.manifest().clone())
        .or_else(|_| crate::prerelease::inventory(&manifest, &binary, &self.target))?;
        if metadata.id != id {
            return Err(Error::CorruptState);
        }
        self.manifests.borrow_mut().insert(slot, metadata.clone());
        Ok(metadata)
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    before: crate::ledger::Index,
    after: crate::ledger::Index,
    new_slot: Option<u64>,
}
impl Journal {
    fn bytes(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec(self).map_err(|_| Error::CorruptState)
    }
    fn validate(&self) -> Result<(), Error> {
        crate::ledger::Index::parse(&self.before.bytes()?)?;
        crate::ledger::Index::parse(&self.after.bytes()?)?;
        if self.before.revision.checked_add(1) != Some(self.after.revision) {
            return Err(Error::CorruptState);
        }
        let ids: std::collections::BTreeSet<_> = self
            .before
            .plugins
            .keys()
            .chain(self.after.plugins.keys())
            .collect();
        let changed: Vec<_> = ids
            .into_iter()
            .filter(|id| self.before.plugins.get(*id) != self.after.plugins.get(*id))
            .collect();
        if changed.len() != 1 {
            return Err(Error::CorruptState);
        }
        let id = changed[0];
        let before = self.before.plugins.get(id);
        let after = self.after.plugins.get(id);
        let valid = match (self.new_slot, before, after) {
            // A new slot demotes the replaced version, or keeps its predecessor when the
            // selected version was reinstalled over itself.
            (Some(n), b, Some(a)) => {
                n == self.after.revision
                    && a.current == n
                    && (a.previous == b.map(|v| v.current)
                        || a.previous == b.and_then(|v| v.previous))
            }
            (None, Some(_), None) => true,
            (None, Some(b), Some(a)) => {
                b.previous == Some(a.current) && a.previous == Some(b.current)
            }
            _ => false,
        };
        if !valid {
            return Err(Error::CorruptState);
        }
        Ok(())
    }
}

fn slots(index: &crate::ledger::Index) -> std::collections::BTreeSet<u64> {
    index
        .plugins
        .values()
        .flat_map(|v| std::iter::once(v.current).chain(v.previous))
        .collect()
}

#[cfg(any(test, feature = "fixture-tests"))]
thread_local! {static FAILURE:std::cell::Cell<Option<&'static str>>=const{std::cell::Cell::new(None)};}
#[cfg(feature = "fixture-tests")]
#[doc(hidden)]
pub fn fail_next_prepare_after_manifest() {
    FAILURE.with(|f| f.set(Some("after_manifest")));
}
fn checkpoint(_point: &'static str) -> Result<(), Error> {
    #[cfg(any(test, feature = "fixture-tests"))]
    if FAILURE.with(|f| {
        if f.get() == Some(_point) {
            f.set(None);
            true
        } else {
            false
        }
    }) {
        return Err(Error::Io);
    }
    Ok(())
}
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod support;
#[cfg(test)]
mod fault_tests {
    use super::*;
    #[test]
    fn rejected_requests_cannot_reconcile_preexisting_recovery() {
        let a = support::Directory::new();
        let b = support::Directory::new();
        let mut a = Manager::create(&a.0, 1, support::TARGET).unwrap();
        let mut b = Manager::create(&b.0, 1, support::TARGET).unwrap();
        let bytes = support::package(1);
        let mut foreign = Some(a.prepare(Operation::Install, Some(&bytes)).unwrap());
        FAILURE.with(|f| f.set(Some("after_manifest")));
        assert!(matches!(
            b.prepare(Operation::Install, Some(&bytes)),
            Err(Error::Io)
        ));
        assert_eq!(b.state(), ManagerState::RecoveryRequired);
        assert!(matches!(
            b.prepare_from_source(Operation::Install, Some(&bytes), None, None),
            Err(Error::Busy)
        ));
        assert_eq!(
            b.state(),
            ManagerState::RecoveryRequired,
            "rejected prepare cannot reconcile older operation"
        );
        assert_eq!(
            b.commit_recovering(&mut foreign),
            CommitOutcome::Uncertain(Error::WrongOwner)
        );
        assert_eq!(
            b.state(),
            ManagerState::RecoveryRequired,
            "foreign commit cannot reconcile older operation"
        );
        assert!(foreign.is_some());
        assert!(matches!(
            b.reconcile().unwrap(),
            CommitOutcome::NotApplied(_)
        ));
        a.abort(&mut foreign).unwrap();
    }
    #[test]
    fn recovered_commit_preserves_original_io_cause_and_releases_recovery() {
        let dir = support::Directory::new();
        let mut m = Manager::create(&dir.0, 1, support::TARGET).unwrap();
        let mut p = Some(
            m.prepare(Operation::Install, Some(&support::package(1)))
                .unwrap(),
        );
        FAILURE.with(|f| f.set(Some("before_selector")));
        assert_eq!(
            m.commit_recovering(&mut p),
            CommitOutcome::NotApplied(Error::Io)
        );
        assert_eq!(m.state(), ManagerState::Ready);
        assert!(m.installed().unwrap().is_empty());
    }
    #[test]
    fn io_failures_before_and_after_selector_leave_recoverable_state() {
        for (point, applied) in [
            ("before_selector", false),
            ("after_selector", true),
            ("after_cleanup", true),
        ] {
            let dir = support::Directory::new();
            let mut manager = Manager::create(&dir.0, 1, support::TARGET).unwrap();
            let mut change = Some(
                manager
                    .prepare(Operation::Install, Some(&support::package(1)))
                    .unwrap(),
            );
            FAILURE.with(|f| f.set(Some(point)));
            let result = manager.commit(&mut change).unwrap();
            assert_eq!(
                result,
                if applied {
                    CommitOutcome::Uncertain(Error::Io)
                } else {
                    CommitOutcome::NotApplied(Error::Io)
                }
            );
            assert_eq!(manager.state(), ManagerState::RecoveryRequired);
            drop(manager);
            let manager = Manager::open(&dir.0, 1, support::TARGET).unwrap();
            assert_eq!(manager.installed().unwrap().len(), usize::from(applied));
        }
    }
}

#[cfg(test)]
mod prepare_fault_tests {
    use super::*;
    #[test]
    fn worker_prepare_reconciles_partial_manifest_and_allows_retry() {
        let dir = support::Directory::new();
        let mut manager = Manager::create(&dir.0, 1, support::TARGET).unwrap();
        FAILURE.with(|f| f.set(Some("after_manifest")));
        assert!(matches!(
            manager.prepare_recovering(Operation::Install, Some(&support::package(1))),
            Err(Error::Io)
        ));
        assert_eq!(manager.state(), ManagerState::Ready);
        let mut p = Some(
            manager
                .prepare_recovering(Operation::Install, Some(&support::package(1)))
                .unwrap(),
        );
        assert_eq!(manager.commit(&mut p).unwrap(), CommitOutcome::Applied);
    }
    #[test]
    fn partial_staging_is_bounded_and_reconciliation_preserves_active_version() {
        let dir = support::Directory::new();
        let mut manager = Manager::create(&dir.0, 1, support::TARGET).unwrap();
        let mut install = Some(
            manager
                .prepare(Operation::Install, Some(&support::package(1)))
                .unwrap(),
        );
        manager.commit(&mut install).unwrap();
        FAILURE.with(|f| f.set(Some("after_manifest")));
        assert!(matches!(
            manager.prepare(
                Operation::Update("org.rad.fixture".into()),
                Some(&support::package(2))
            ),
            Err(Error::Io)
        ));
        assert_eq!(manager.state(), ManagerState::RecoveryRequired);
        assert!(matches!(
            manager.prepare(
                Operation::Update("org.rad.fixture".into()),
                Some(&support::package(2))
            ),
            Err(Error::Busy)
        ));
        assert!(matches!(
            manager.reconcile().unwrap(),
            CommitOutcome::NotApplied(_)
        ));
        assert_eq!(manager.installed().unwrap()[0].manifest.version.major, 1);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 2);
    }
}
