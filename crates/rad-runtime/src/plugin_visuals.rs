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
