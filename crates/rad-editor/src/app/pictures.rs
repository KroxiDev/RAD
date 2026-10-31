//! Key pictures the editor holds: those on the board and, within a spare budget, those of
//! folders one press away and of past visits, so changing folder rarely shows one arriving.
//! It keeps the same order as the deck's pictures in the runtime: the board's never go,
//! nearby ones outrank past visits, and the least recently wanted goes first.
use crate::native::PictureRead;
use eframe::egui::TextureHandle;
use std::collections::{BTreeMap, BTreeSet};

/// Room for pictures off the board: about thirty 256 px icons.
pub(super) const SPARE_BYTES: usize = 8 * 1024 * 1024;
/// A picture that arrives while its key is on screen fades in for this long, in seconds.
const FADE: f64 = 0.15;
/// Pictures read ahead in one go: few, so a command waits little behind them.
const AHEAD_BATCH: usize = 8;
/// Bytes of the largest picture the editor accepts (256 × 256). Reads ahead never take more
/// than fits in the room left, counting each at this size, so they exceed it by one at most.
const LARGEST_PICTURE: usize = 256 * 256 * 4;

pub(super) struct Pictures {
    spare: usize,
    held: BTreeMap<String, Held>,
    /// Asked for since the board last changed; one that failed waits for the next change.
    asked: BTreeSet<String>,
    /// Counts the times the wanted pictures were stated; the lowest mark goes first.
    tick: u64,
}
struct Held {
    texture: TextureHandle,
    wanted: u64,
    /// When it arrived with its key on screen; it fades in from then.
    fade_from: Option<f64>,
}
impl Held {
    fn bytes(&self) -> usize {
        let [width, height] = self.texture.size();
        width * height * 4
    }
}
impl Default for Pictures {
    fn default() -> Self {
        Self::new(SPARE_BYTES)
    }
}
impl Pictures {
    pub(super) fn new(spare: usize) -> Self {
        Self {
            spare,
            held: BTreeMap::new(),
            asked: BTreeSet::new(),
            tick: 0,
        }
    }
    /// The picture and how far it has faded in, from 0 to 1, at `now` seconds.
    pub(super) fn get(&self, reference: &str, now: f64) -> Option<(&TextureHandle, f32)> {
        let held = self.held.get(reference)?;
        let shown = held
            .fade_from
            .map_or(1., |from| ((now - from) / FADE).clamp(0., 1.));
        Some((&held.texture, shown as f32))
    }
    /// Some picture is still fading in, so the screen must be drawn again.
    pub(super) fn fading(&self, now: f64) -> bool {
        self.held
            .values()
            .any(|held| held.fade_from.is_some_and(|from| now - from < FADE))
    }
    /// States what the board shows and what is one press away, and names what to read
    /// next: every missing picture of the board at once, so they appear together; else a
    /// few nearby ones while the spare room lasts. Nothing counts as asked until [`asked`].
    ///
    /// [`asked`]: Self::asked
    pub(super) fn next(
        &mut self,
        shown: &[PictureRead],
        nearby: &[PictureRead],
    ) -> Vec<PictureRead> {
        self.tick += 1;
        let on_show: BTreeSet<&str> = shown.iter().map(|p| p.reference.as_str()).collect();
        let close: BTreeSet<&str> = nearby.iter().map(|p| p.reference.as_str()).collect();
        for reference in on_show.iter().chain(&close) {
            if let Some(held) = self.held.get_mut(*reference) {
                held.wanted = self.tick;
            }
        }
        self.let_go(&on_show, &close);
        let mut chosen = BTreeSet::new();
        let mut missing = |picture: &&PictureRead| {
            !self.held.contains_key(&picture.reference)
                && !self.asked.contains(&picture.reference)
                && chosen.insert(picture.reference.clone())
        };
        let mut batch: Vec<PictureRead> = shown.iter().filter(&mut missing).cloned().collect();
        let nearby_bytes: usize = close
            .iter()
            .filter(|reference| !on_show.contains(*reference))
            .filter_map(|reference| self.held.get(*reference))
            .map(Held::bytes)
            .sum();
        if batch.is_empty() && nearby_bytes < self.spare {
            let fit = ((self.spare - nearby_bytes) / LARGEST_PICTURE).clamp(1, AHEAD_BATCH);
            batch = nearby
                .iter()
                .filter(&mut missing)
                .take(fit)
                .cloned()
                .collect();
        }
        batch
    }
    /// These pictures are being read; one that fails waits for the board to change.
    pub(super) fn asked(&mut self, batch: &[PictureRead]) {
        self.asked
            .extend(batch.iter().map(|picture| picture.reference.clone()));
    }
    /// Lets go of past visits while the pictures off the board exceed the spare room.
    fn let_go(&mut self, on_show: &BTreeSet<&str>, close: &BTreeSet<&str>) {
        let mut spare: usize = self
            .held
            .iter()
            .filter(|(reference, _)| !on_show.contains(reference.as_str()))
            .map(|(_, held)| held.bytes())
            .sum();
        while spare > self.spare {
            let Some(oldest) = self
                .held
                .iter()
                .filter(|(reference, _)| {
                    !on_show.contains(reference.as_str()) && !close.contains(reference.as_str())
                })
                .min_by_key(|(_, held)| held.wanted)
                .map(|(reference, _)| reference.clone())
            else {
                break;
            };
            spare -= self.held.remove(&oldest).map_or(0, |held| held.bytes());
        }
    }
    /// A picture was read. `fade_from` is the time it arrived if its key is on screen.
    pub(super) fn arrived(
        &mut self,
        reference: String,
        texture: TextureHandle,
        fade_from: Option<f64>,
    ) {
        self.held.insert(
            reference,
            Held {
                texture,
                wanted: self.tick,
                fade_from,
            },
        );
    }
    /// The board changed: a picture that could not be read may be asked for again.
    pub(super) fn changed(&mut self) {
        self.asked.clear();
    }
}
