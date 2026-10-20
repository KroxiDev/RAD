//! Visible-folder subscriptions and one bounded PNG/JPEG decoder, independent of execution.
use rad_authority::{Authority, Visible};
use rad_core::{configuration::Origin, identity::ShortId};
use rad_plugin_contract::{Correlation, VisualContent, VisualState};
use rad_plugin_host::Host;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
    thread,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Availability {
    Current,
    Stale,
    #[default]
    Unavailable,
}
impl Availability {
    fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
        }
    }
}
#[derive(Default)]
pub(crate) struct VisualView {
    pub availability: Availability,
    pub revision: u64,
    pub text: Option<String>,
    pub color: Option<[u8; 4]>,
    pub image: bool,
}
pub(crate) struct ResolvedVisual {
    pub view: VisualView,
    pub text: String,
    pub position: rad_core::configuration::TextPosition,
}
pub(crate) struct Visuals {
    entries: BTreeMap<ShortId, Entry>,
    next: u64,
    tx: mpsc::SyncSender<Decode>,
    rx: mpsc::Receiver<Decoded>,
    decoding: bool,
    _worker: Option<thread::JoinHandle<()>>,
}
struct Entry {
    plugin: String,
    source: String,
    bytes: Vec<u8>,
    settings: Option<Vec<u8>>,
    generation: Option<u64>,
    credential_revision: Option<u64>,
    key: Option<Correlation>,
    raw: VisualState,
    revision: u64,
    image_revision: u64,
    image_waiting: bool,
    pending: Option<String>,
    image: Option<Arc<rad_media::NormalizedImage>>,
    invalid_image: bool,
}
struct Decode {
    id: ShortId,
    revision: u64,
    hex: String,
}
struct Decoded {
    id: ShortId,
    revision: u64,
    image: Option<Arc<rad_media::NormalizedImage>>,
}
impl Visuals {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, jobs) = mpsc::sync_channel::<Decode>(1);
        let (results, rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("rad-plugin-images".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let image = decode(&job.hex).map(Arc::new);
                    if results
                        .send(Decoded {
                            id: job.id,
                            revision: job.revision,
                            image,
                        })
                        .is_err()
                    {
                        break;
                    }
                    wake();
                }
            })
            .ok();
        Self {
            entries: BTreeMap::new(),
            next: 0,
            tx,
            rx,
            decoding: false,
            _worker: worker,
        }
    }
    pub fn poll(&mut self, actor: &Authority, host: &mut Host) -> bool {
        let mut desired = BTreeMap::new();
        if let Some(folder) = actor
            .status(Origin::Editor)
            .active
            .and_then(|b| actor.folder(Origin::Editor, b.short()))
        {
            for item in folder.slots.into_iter().flatten() {
                if let Visible::Visible(b) = item
                    && let Some((key, bytes, true)) =
                        actor.plugin_configuration(Origin::Editor, b.short())
                {
                    let generation = host
                        .bindings()
                        .find(|(m, _, s)| {
                            m.id == key.plugin() && *s == rad_plugin_host::State::Ready
                        })
                        .map(|(_, g, _)| g);
                    let credential_revision = actor
                        .plugin_manifests()
                        .find(|(m, _, _)| m.id == key.plugin())
                        .and_then(|(_, s, _)| actor.current_plugin_credentials(key.plugin(), s))
                        .map(|s| s.revision());
                    desired.insert(
                        b.short(),
                        (
                            key.plugin().to_owned(),
                            key.name().to_owned(),
                            bytes.to_vec(),
                            generation,
                            credential_revision,
                            actor
                                .plugin_settings(Origin::Editor, key.plugin())
                                .map(<[u8]>::to_vec),
                        ),
                    );
                }
            }
        }
        let mut changed = false;
        self.entries.retain(|id, e| {
            let keep = desired.get(id).is_some_and(|(p, s, b, g, r, settings)| {
                p == &e.plugin
                    && s == &e.source
                    && b == &e.bytes
                    && g == &e.generation
                    && r == &e.credential_revision
                    && settings == &e.settings
            });
            if !keep {
                if let Some(key) = e.key {
                    let _ = host.unsubscribe(&e.plugin, key);
                }
                changed = true;
            }
            keep
        });
        for (id, (plugin, source, bytes, generation, credential_revision, settings)) in desired {
            if self.entries.contains_key(&id) {
                continue;
            }
            self.entries.insert(
                id,
                Entry {
                    plugin,
                    source,
                    bytes,
                    settings,
                    generation,
                    credential_revision,
                    key: None,
                    raw: VisualState::Unavailable,
                    revision: 0,
                    image_revision: 0,
                    image_waiting: false,
                    pending: None,
                    image: None,
                    invalid_image: false,
                },
            );
            changed = true;
        }
        for e in self.entries.values_mut() {
            if e.key.is_none() && e.generation.is_some() {
                e.key = subscribe(
                    host,
                    &e.plugin,
                    &e.source,
                    &e.bytes,
                    e.credential_revision,
                    e.settings.as_deref(),
                );
            }
            let raw = e
                .key
                .and_then(|key| host.visual(&e.plugin, key))
                .cloned()
                .unwrap_or(VisualState::Unavailable);
            if raw != e.raw {
                let Some(revision) = self.next.checked_add(1) else {
                    continue;
                };
                self.next = revision;
                e.revision = revision;
                let hex = content(&raw).and_then(image_hex).cloned();
                let old = content(&e.raw).and_then(image_hex);
                if e.image_revision == 0 || hex.as_ref() != old {
                    e.image_revision = revision;
                }
                if hex.as_ref() != old {
                    e.image_waiting = hex.is_some();
                    e.pending = hex;
                    e.invalid_image = false;
                    if e.pending.is_none() {
                        e.image = None;
                    }
                }
                e.raw = raw;
                changed = true;
            }
        }
        if let Ok(done) = self.rx.try_recv() {
            self.decoding = false;
            changed |= self.complete_decode(done);
        }
        if !self.decoding
            && let Some((id, e)) = self.entries.iter_mut().find(|(_, e)| e.pending.is_some())
        {
            let job = Decode {
                id: *id,
                revision: e.image_revision,
                hex: e.pending.take().unwrap(),
            };
            match self.tx.try_send(job) {
                Ok(()) => self.decoding = true,
                Err(mpsc::TrySendError::Full(job)) => e.pending = Some(job.hex),
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    e.invalid_image = true;
                    changed = true;
                }
            }
        }
        changed
    }
    fn complete_decode(&mut self, done: Decoded) -> bool {
        if let Some(e) = self
            .entries
            .get_mut(&done.id)
            .filter(|e| e.image_revision == done.revision)
            && let Some(revision) = self.next.checked_add(1)
        {
            self.next = revision;
            e.revision = revision;
            e.image_waiting = false;
            e.invalid_image = done.image.is_none();
            e.image = done.image;
            return true;
        }
        false
    }
    pub fn resolved(&self, actor: &Authority, id: ShortId) -> ResolvedVisual {
        let view = self.view(id);
        let appearance = actor.appearance(Origin::Editor, id);
        let entity = actor.get(Origin::Editor, id);
        let fallback = match (&appearance, &entity) {
            (Some(a), Some(rad_authority::EntityView::Button(b))) => a.appearance.label(&b.name),
            _ => "",
        };
        let text = view.text.as_deref().unwrap_or(fallback).to_owned();
        let position = appearance
            .map(|a| a.appearance.effective_text_position())
            .unwrap_or(rad_core::configuration::TextPosition::Middle);
        ResolvedVisual {
            view,
            text,
            position,
        }
    }
    /// JSON exists only at the IPC boundary; deck consumes the same typed resolution.
    pub fn ipc_view(&self, actor: &Authority, id: ShortId) -> Value {
        let resolved = self.resolved(actor, id);
        let v = resolved.view;
        json!({"availability":v.availability.as_str(),"revision":v.revision.to_string(),"text":resolved.text,"text_position":position_name(resolved.position),"color":v.color,"image":v.image})
    }
    pub fn view(&self, id: ShortId) -> VisualView {
        let Some(e) = self.entries.get(&id) else {
            return VisualView::default();
        };
        let c = content(&e.raw);
        let availability = if e.invalid_image {
            Availability::Unavailable
        } else {
            match e.raw {
                VisualState::Unavailable => Availability::Unavailable,
                VisualState::Stale { .. } => Availability::Stale,
                VisualState::Current { .. }
                    if c.and_then(image_hex).is_some()
                        && (e.image_waiting || e.image.is_none()) =>
                {
                    Availability::Stale
                }
                VisualState::Current { .. } => Availability::Current,
            }
        };
        VisualView {
            availability,
            revision: e.revision,
            text: c.and_then(|c| c.text.clone()),
            color: c.and_then(|c| c.color),
            image: e.image.is_some(),
        }
    }
    pub fn image(&self, id: ShortId) -> Option<Arc<rad_media::NormalizedImage>> {
        self.entries.get(&id).and_then(|e| e.image.clone())
    }
    pub fn read(&self, id: ShortId, revision: u64, offset: usize) -> Result<Value, &'static str> {
        let e = self
            .entries
            .get(&id)
            .filter(|e| e.revision == revision)
            .ok_or("cursor_expired")?;
        let bytes = e.image.as_ref().ok_or("executor_unavailable")?.png_bytes();
        if offset > bytes.len() {
            return Err("invalid_request");
        }
        let end = (offset + 4096).min(bytes.len());
        Ok(
            json!({"hex":bytes[offset..end].iter().map(|b|format!("{b:02x}")).collect::<String>(),"next_offset":(end<bytes.len()).then(||end.to_string())}),
        )
    }
}
fn position_name(position: rad_core::configuration::TextPosition) -> &'static str {
    use rad_core::configuration::TextPosition;
    match position {
        TextPosition::Top => "top",
        TextPosition::Middle => "middle",
        TextPosition::Bottom => "bottom",
        TextPosition::AboveImage => "above_image",
        TextPosition::BelowImage => "below_image",
    }
}
fn subscribe(
    host: &mut Host,
    plugin: &str,
    source: &str,
    bytes: &[u8],
    revision: Option<u64>,
    settings: Option<&[u8]>,
) -> Option<Correlation> {
    if let Some(revision) = revision {
        let generation = host.bindings().find(|(m, _, _)| m.id == plugin)?.1;
        if host.credentials_state(plugin, generation, revision)
            != Some(rad_plugin_host::CredentialState::Applied)
        {
            return None;
        }
    }
    let definition = host
        .bindings()
        .find(|(m, _, s)| m.id == plugin && *s == rad_plugin_host::State::Ready)?
        .0
        .visuals
        .iter()
        .find(|d| d.id == source)?;
    rad_plugin_contract::Document::new(bytes)
        .ok()?
        .parameters(definition)
        .ok()?;
    let params = serde_json::from_slice(bytes).ok()?;
    let settings = settings.map(serde_json::from_slice).transpose().ok()?;
    host.subscribe_with_settings(plugin, source, params, 1000, revision, settings)
        .ok()
}
fn content(state: &VisualState) -> Option<&VisualContent> {
    match state {
        VisualState::Current { content } | VisualState::Stale { content } => Some(content),
        VisualState::Unavailable => None,
    }
}
/// The plugin's image, PNG or JPEG: the contract allows at most one of them.
fn image_hex(content: &VisualContent) -> Option<&String> {
    content.png_hex.as_ref().or(content.jpeg_hex.as_ref())
}
fn decode(hex: &str) -> Option<rad_media::NormalizedImage> {
    if hex.len() > 16384 || !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<_>>>()?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") && !bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return None;
    }
    // rad-media caps input dimensions 2048² and decoder allocation 64MiB before decode.
    let image = rad_media::normalize(&bytes).ok()?;
    (image.png_bytes().len() <= 256 * 1024).then_some(image)
}
#[cfg(all(test, feature = "fixture-tests"))]
mod tests {
    use super::*;
    use rad_core::{configuration::*, identity::IdentityRegistry};
    use rad_plugin_host::State;
    use std::{
        process::Command,
        sync::Arc,
        time::{Duration, Instant},
    };
    #[test]
    fn missing_subscriptions_retry_after_capacity_and_twenty_sources_survive_folder_switches() {
        let mut c = Configuration::prepare_new(&IdentityRegistry::new(812).unwrap()).unwrap();
        let root = c.configuration().data().principal;
        for position in 0..2 {
            c = c
                .configuration()
                .prepare_edit(
                    c.registry(),
                    Origin::Editor,
                    Edit::CreateFolder {
                        parent: root,
                        position,
                        name: format!("folder{position}"),
                    },
                )
                .unwrap();
        }
        let folders: Vec<_> = c
            .configuration()
            .data()
            .folders
            .iter()
            .filter(|f| f.binding.identity() != root)
            .map(|f| f.binding.identity())
            .collect();
        for folder in &folders {
            for position in 1..=20 {
                c = c
                    .configuration()
                    .prepare_edit(
                        c.registry(),
                        Origin::Editor,
                        Edit::CreateButton {
                            folder: *folder,
                            position,
                            name: format!("source{position}"),
                        },
                    )
                    .unwrap();
                let id = c
                    .configuration()
                    .data()
                    .buttons
                    .last()
                    .unwrap()
                    .binding
                    .identity();
                c = c
                    .configuration()
                    .prepare_edit(
                        c.registry(),
                        Origin::Editor,
                        Edit::SetVisual {
                            button: id,
                            visual: VisualSource::External {
                                type_key: ExternalType::new(
                                    "org.rad.fixture".into(),
                                    "status".into(),
                                )
                                .unwrap(),
                                configuration: br#"{"schema_version":1,"values":{"mode":"echo"}}"#
                                    .to_vec(),
                            },
                        },
                    )
                    .unwrap();
            }
        }
        let mut actor = Authority::new(c, 1, std::num::NonZeroU128::new(10).unwrap());
        let manifest = serde_json::from_slice(include_bytes!(
            "../../rad-plugin-host/fixture/manifest-v1.json"
        ))
        .unwrap();
        let mut host = Host::default();
        let bin = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("rad-plugin-fixture-v1");
        host.activate(Command::new(bin), manifest, 0).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while host.state("org.rad.fixture") != Some(State::Ready) {
            assert!(Instant::now() < end);
            host.pump(0).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        let params: rad_plugin_contract::Parameters =
            serde_json::from_slice(br#"{"schema_version":1,"values":{"mode":"echo"}}"#).unwrap();
        let mut occupied = vec![];
        while let Ok(key) = host.subscribe("org.rad.fixture", "status", params.clone(), 1000) {
            occupied.push(key);
        }
        assert!(!occupied.is_empty());
        let mut visuals = Visuals::new(Arc::new(|| {}));
        actor.navigate(Origin::Editor, folders[0]).unwrap();
        visuals.poll(&actor, &mut host);
        assert!(visuals.entries.values().any(|e| e.key.is_none()));
        for key in occupied {
            let _ = host.unsubscribe("org.rad.fixture", key);
        }
        for folder in [folders[0], folders[1], folders[0]] {
            actor.navigate(Origin::Editor, folder).unwrap();
            loop {
                visuals.poll(&actor, &mut host);
                host.pump(0).unwrap();
                if visuals.entries.len() == 20
                    && visuals
                        .entries
                        .values()
                        .all(|e| matches!(e.raw, VisualState::Current { .. }))
                {
                    break;
                }
                assert!(
                    Instant::now() < end,
                    "all twenty visible subscriptions must recover"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        host.shutdown();
        while host.resources().0 > 0 {
            assert!(Instant::now() < end);
            host.pump(0).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn visible_source_is_current_then_stale_and_folder_removal_unsubscribes_without_execution_reservation()
     {
        let mut c = Configuration::prepare_new(&IdentityRegistry::new(811).unwrap()).unwrap();
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::CreateButton {
                    folder: c.configuration().data().principal,
                    position: 0,
                    name: "Visual".into(),
                },
            )
            .unwrap();
        let binding = c.configuration().data().buttons[0].binding;
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::SetVisual {
                    button: binding.identity(),
                    visual: VisualSource::External {
                        type_key: ExternalType::new("org.rad.fixture".into(), "status".into())
                            .unwrap(),
                        configuration: br#"{"schema_version":1,"values":{"mode":"echo"}}"#.to_vec(),
                    },
                },
            )
            .unwrap();
        c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::SetAppearance {
                    button: binding.identity(),
                    appearance: Appearance {
                        text: String::new(),
                        image: None,
                        text_position: Some(TextPosition::Top),
                    },
                },
            )
            .unwrap();
        let mut actor = Authority::new(c, 1, std::num::NonZeroU128::new(9).unwrap());
        let manifest = serde_json::from_slice(include_bytes!(
            "../../rad-plugin-host/fixture/manifest-v1.json"
        ))
        .unwrap();
        let mut host = Host::default();
        let bin = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("rad-plugin-fixture-v1");
        host.activate(Command::new(bin), manifest, 0).unwrap();
        let end = Instant::now() + Duration::from_secs(3);
        while host.state("org.rad.fixture") != Some(State::Ready) {
            assert!(Instant::now() < end);
            host.pump(0).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut visuals = Visuals::new(Arc::new(|| {}));
        assert_eq!(visuals.ipc_view(&actor, binding.short())["text"], "Visual");
        assert_eq!(
            visuals.ipc_view(&actor, binding.short())["text_position"],
            "top"
        );
        while visuals.view(binding.short()).availability != Availability::Current {
            assert!(Instant::now() < end, "visual never became current");
            visuals.poll(&actor, &mut host);
            host.pump(0).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(visuals.view(binding.short()).text.as_deref(), Some("v1"));
        let mut expected = std::array::from_fn(|_| rad_deck::KeyVisual::default());
        expected[0] = rad_deck::KeyVisual::new("v1", None, false, false)
            .text_at(rad_deck::TextPlacement::Top);
        assert_eq!(
            crate::display::project_with_plugins(
                &actor,
                &BTreeMap::new(),
                100,
                false,
                Some(&visuals)
            ),
            rad_deck::Desired::new(expected, 100, false).unwrap()
        );
        assert_eq!(
            visuals.ipc_view(&actor, binding.short())["text_position"],
            "top"
        );
        assert!(actor.plugin_change_allowed("org.rad.fixture").is_ok());
        host.pump(1000).unwrap();
        visuals.poll(&actor, &mut host);
        assert_eq!(
            visuals.view(binding.short()).availability,
            Availability::Stale
        );
        // Control only the decoder completion transport. The content and TTL
        // revisions deliberately differ; an old completion cannot replace a new image.
        let e = visuals.entries.get_mut(&binding.short()).unwrap();
        let stamp = e.image_revision;
        e.image_waiting = true;
        let before = e.revision;
        assert!(!visuals.complete_decode(Decoded {
            id: binding.short(),
            revision: stamp + 100,
            image: None
        }));
        assert!(visuals.complete_decode(Decoded {
            id: binding.short(),
            revision: stamp,
            image: None
        }));
        assert_eq!(
            visuals.view(binding.short()).availability,
            Availability::Unavailable
        );
        assert!(visuals.entries[&binding.short()].revision > before);
        // Actual bounded decoder worker, then revision-bound IPC reads.
        let before = visuals.entries[&binding.short()].revision;
        visuals.tx.send(Decode{id:binding.short(),revision:stamp,hex:"89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c63f8cfc0f01f00050001ff89993d1d0000000049454e44ae426082".into()}).unwrap();
        let decoded = visuals.rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(decoded.image.is_some());
        assert!(visuals.complete_decode(decoded));
        let revision = visuals.entries[&binding.short()].revision;
        assert!(revision > before);
        assert_eq!(
            visuals.read(binding.short(), before, 0),
            Err("cursor_expired")
        );
        assert!(
            visuals.read(binding.short(), revision, 0).unwrap()["hex"]
                .as_str()
                .unwrap()
                .starts_with("89504e47")
        );
        assert_eq!(
            visuals.read(binding.short(), revision, usize::MAX),
            Err("invalid_request")
        );
        assert!(decode(&"0".repeat(16386)).is_none());
        assert!(decode("f").is_none());
        let t = actor.reserve(1, Origin::Editor).unwrap();
        let e = actor.submit(
            t,
            vec![Edit::SetVisual {
                button: binding.identity(),
                visual: VisualSource::Manual,
            }],
        );
        let id = e
            .iter()
            .find_map(|e| {
                if let rad_authority::Effect::Persist { id, .. } = e {
                    Some(*id)
                } else {
                    None
                }
            })
            .unwrap();
        actor.complete(
            id,
            rad_authority::Outcome::Applied {
                revision: 2,
                changed: true,
            },
        );
        visuals.poll(&actor, &mut host);
        assert_eq!(
            visuals.view(binding.short()).availability,
            Availability::Unavailable
        );
        assert_eq!(host.resources().1, 0);
        host.shutdown();
        while host.resources().0 > 0 {
            assert!(Instant::now() < end);
            host.pump(1000).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// What a plugin may put in `jpeg_hex`: only a JPEG that decodes within `rad_media`'s limits
/// becomes an image; anything else leaves the source without one and never panics.
#[cfg(test)]
mod image_tests {
    use super::*;
    use rad_core::identity::EntityKind;
    use std::time::Duration;

    /// A valid 16x16 baseline JPEG (the options test plugin's probe image).
    const JPEG: &str = "ffd8ffe000104a46494600010100000100010000ffdb0043000d090a0b0a080d0b0a0b0e0e0d0f13201513121213271c1e17202e2931302e292d2c333a4a3e333646372c2d405741464c4e525352323e5a615a50604a51524fffdb0043010e0e0e131113261515264f352d354f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4f4fffc00011080010001003012200021101031101ffc400160001010100000000000000000000000000000405ffc400161000030000000000000000000000000000001461ffc4001501010100000000000000000000000000000002ffc40017110003010000000000000000000000000000146115ffda000c03010002110311003f0089581586cab02b09d6a1187fffd9";

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn a_jpeg_that_does_not_decode_or_is_too_large_leaves_the_source_without_an_image() {
        let valid = bytes(JPEG);
        // Real decoder control: the unmodified image becomes a PNG.
        assert!(decode(JPEG).is_some_and(|image| image.png_bytes().starts_with(b"\x89PNG")));

        let mut oversized = valid.clone();
        let sof = oversized
            .windows(2)
            .position(|w| w == [0xff, 0xc0])
            .unwrap();
        // Height and width live five bytes after the marker; declare 4096 x 4096.
        oversized[sof + 5..sof + 9].copy_from_slice(&[0x10, 0x00, 0x10, 0x00]);
        assert_eq!(
            rad_media::normalize(&oversized).err(),
            Some(rad_media::MediaError::DimensionsExceeded),
            "the case really exercises the pixel limit"
        );
        let scan = valid.windows(2).position(|w| w == [0xff, 0xda]).unwrap();
        let cases = [
            ("signature only", vec![0xff, 0xd8, 0xff]),
            (
                "signature and noise",
                [vec![0xff, 0xd8, 0xff], vec![0x5a; 200]].concat(),
            ),
            ("cut before the scan", valid[..scan].to_vec()),
            ("cut inside the scan", valid[..valid.len() - 12].to_vec()),
            ("beyond the pixel limit", oversized),
        ];

        let mut visuals = Visuals::new(Arc::new(|| {}));
        let id = ShortId::new(EntityKind::Button, 1).unwrap();
        for (name, case) in cases {
            let case = hex(&case);
            assert!(decode(&case).is_none(), "{name}");
            // Through the bounded decoder worker, as the runtime does it.
            visuals.entries.insert(
                id,
                Entry {
                    plugin: "p".into(),
                    source: "s".into(),
                    bytes: vec![],
                    settings: None,
                    generation: Some(1),
                    credential_revision: None,
                    key: None,
                    raw: VisualState::Current {
                        content: VisualContent {
                            text: Some("t".into()),
                            png_hex: None,
                            jpeg_hex: Some(case.clone()),
                            color: None,
                        },
                    },
                    revision: 1,
                    image_revision: 1,
                    image_waiting: true,
                    pending: None,
                    image: None,
                    invalid_image: false,
                },
            );
            visuals
                .tx
                .send(Decode {
                    id,
                    revision: 1,
                    hex: case,
                })
                .unwrap();
            let decoded = visuals.rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(visuals.complete_decode(decoded), "{name}");
            let view = visuals.view(id);
            assert_eq!(view.availability, Availability::Unavailable, "{name}");
            assert!(!view.image, "{name}");
        }
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    use rad_core::{configuration::*, identity::IdentityRegistry};

    #[test]
    fn changed_settings_invalidate_cached_visual_even_before_process_ready() {
        let c = Configuration::prepare_new(&IdentityRegistry::new(951).unwrap()).unwrap();
        let c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::CreateButton {
                    folder: c.configuration().data().principal,
                    position: 0,
                    name: "Visual".into(),
                },
            )
            .unwrap();
        let button = c.configuration().data().buttons[0].binding;
        let c = c
            .configuration()
            .prepare_edit(
                c.registry(),
                Origin::Editor,
                Edit::SetVisual {
                    button: button.identity(),
                    visual: VisualSource::External {
                        type_key: ExternalType::new("org.rad.obs".into(), "record".into()).unwrap(),
                        configuration: br#"{"schema_version":1,"values":{}}"#.to_vec(),
                    },
                },
            )
            .unwrap();
        let mut actor = Authority::new(c, 1, std::num::NonZeroU128::new(952).unwrap());
        actor
            .register_plugin(
                serde_json::from_slice(include_bytes!("../../../plugins/obs/manifest.json"))
                    .unwrap(),
                rad_plugin_host::TARGET,
            )
            .unwrap();
        let mut visuals = Visuals::new(Arc::new(|| {}));
        let mut host = Host::default();
        assert!(visuals.poll(&actor, &mut host));
        assert!(!visuals.poll(&actor, &mut host));
        let ticket = actor.reserve(1, Origin::Editor).unwrap();
        let effects = actor.submit(
            ticket,
            vec![Edit::SetPluginSettings {
                plugin: "org.rad.obs".into(),
                settings: Some(
                    PluginSettings::new(br#"{"schema_version":1,"values":{"port":5555}}"#.to_vec())
                        .unwrap(),
                ),
            }],
        );
        let id = effects
            .iter()
            .find_map(|e| match e {
                rad_authority::Effect::Persist { id, .. } => Some(*id),
                _ => None,
            })
            .unwrap();
        actor.complete(
            id,
            rad_authority::Outcome::Applied {
                revision: 2,
                changed: true,
            },
        );
        assert!(
            visuals.poll(&actor, &mut host),
            "settings change must replace binding, not retain cached state"
        );
        assert_eq!(
            visuals.view(button.short()).availability,
            Availability::Unavailable
        );
        assert!(!visuals.poll(&actor, &mut host));
    }
}
