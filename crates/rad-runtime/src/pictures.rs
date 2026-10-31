//! Key pictures held for the deck: those of the folder on show and, within a byte budget,
//! those of folders it may show next, so a change of folder rarely waits for a load.
use rad_media::NormalizedImage;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Room for pictures beyond the folder on show: about forty 256 px icons.
pub(crate) const SPARE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct Pictures {
    spare: usize,
    held: BTreeMap<String, Held>,
    /// Pictures that could not be loaded, and whether they may be tried again.
    failed: BTreeMap<String, bool>,
    /// Counts the times the wanted pictures were stated; the lowest mark goes first.
    tick: u64,
}
struct Held {
    image: Arc<NormalizedImage>,
    wanted: u64,
}
impl Held {
    fn bytes(&self) -> usize {
        self.image.png_bytes().len()
    }
}
impl Pictures {
    pub(crate) fn new(spare: usize) -> Self {
        Self {
            spare,
            held: BTreeMap::new(),
            failed: BTreeMap::new(),
            tick: 0,
        }
    }
    pub(crate) fn get(&self, key: &str) -> Option<Arc<NormalizedImage>> {
        self.held.get(key).map(|held| held.image.clone())
    }
    /// The picture could not be loaded: the deck marks its key unavailable.
    pub(crate) fn failed(&self, key: &str) -> bool {
        self.failed.contains_key(key)
    }
    /// Some picture of the folder on show has not arrived yet.
    pub(crate) fn loading(&self, shown: &BTreeSet<String>) -> bool {
        shown
            .iter()
            .any(|key| !self.held.contains_key(key) && !self.failed.contains_key(key))
    }
    /// States what the deck shows and may show next, and names the next picture to load:
    /// one on show first, then one nearby while the spare room lasts. Pictures on show are
    /// never let go; past visits go before nearby ones, the least recently wanted first.
    pub(crate) fn next(
        &mut self,
        shown: &BTreeSet<String>,
        nearby: &BTreeSet<String>,
    ) -> Option<String> {
        self.tick += 1;
        for key in shown.iter().chain(nearby) {
            if let Some(held) = self.held.get_mut(key) {
                held.wanted = self.tick;
            }
        }
        self.failed
            .retain(|key, _| shown.contains(key) || nearby.contains(key));
        self.let_go(shown, nearby);
        let wanted = |key: &&String| {
            !self.held.contains_key(*key) && self.failed.get(*key).is_none_or(|retry| *retry)
        };
        let nearby_bytes: usize = nearby
            .iter()
            .filter(|key| !shown.contains(*key))
            .filter_map(|key| self.held.get(key))
            .map(Held::bytes)
            .sum();
        let key = shown
            .iter()
            .find(wanted)
            .or_else(|| (nearby_bytes < self.spare).then(|| nearby.iter().find(wanted))?)?
            .clone();
        if let Some(retry) = self.failed.get_mut(&key) {
            *retry = false;
        }
        Some(key)
    }
    /// Lets go of past visits while the pictures off show exceed the spare room.
    fn let_go(&mut self, shown: &BTreeSet<String>, nearby: &BTreeSet<String>) {
        let mut spare: usize = self
            .held
            .iter()
            .filter(|(key, _)| !shown.contains(*key))
            .map(|(_, held)| held.bytes())
            .sum();
        while spare > self.spare {
            let Some(oldest) = self
                .held
                .iter()
                .filter(|(key, _)| !shown.contains(*key) && !nearby.contains(*key))
                .min_by_key(|(_, held)| held.wanted)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            spare -= self.held.remove(&oldest).map_or(0, |held| held.bytes());
        }
    }
    pub(crate) fn loaded(&mut self, key: String, image: Arc<NormalizedImage>) {
        self.failed.remove(&key);
        self.held.insert(
            key,
            Held {
                image,
                wanted: self.tick,
            },
        );
    }
    pub(crate) fn fail(&mut self, key: String) {
        self.failed.insert(key, false);
    }
    /// After a change a failed picture may load again; it keeps its mark until it does.
    pub(crate) fn retry_failed(&mut self) {
        self.failed.values_mut().for_each(|retry| *retry = true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240,
        31, 0, 5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    fn image() -> Arc<NormalizedImage> {
        Arc::new(rad_media::normalize(PNG).unwrap())
    }
    fn set(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|k| (*k).to_owned()).collect()
    }
    /// Room for `count` pictures beyond those on show.
    fn room(count: usize) -> Pictures {
        Pictures::new(count * image().png_bytes().len())
    }
    fn show(pictures: &mut Pictures, shown: &[&str], nearby: &[&str]) -> Vec<String> {
        let (shown, nearby) = (set(shown), set(nearby));
        let mut loads = Vec::new();
        while let Some(key) = pictures.next(&shown, &nearby) {
            assert!(loads.len() < 10, "loads never repeat: {loads:?}");
            loads.push(key.clone());
            pictures.loaded(key, image());
        }
        loads
    }

    #[test]
    fn nearby_folders_load_after_the_shown_one_until_the_room_is_full() {
        let mut pictures = room(2);
        assert_eq!(show(&mut pictures, &["x"], &[]), ["x"]);
        assert_eq!(
            show(&mut pictures, &["a", "b"], &["c", "d", "e"]),
            ["a", "b", "c", "d"]
        );
        assert!(pictures.get("x").is_none(), "nearby outranks a past visit");
        assert!(pictures.get("a").is_some() && pictures.get("b").is_some());
        assert_eq!(show(&mut pictures, &["c"], &["a"]), Vec::<String>::new());
    }

    #[test]
    fn a_failed_picture_keeps_its_mark_until_it_loads() {
        let mut pictures = room(0);
        let shown = set(&["a"]);
        assert_eq!(pictures.next(&shown, &set(&[])).as_deref(), Some("a"));
        assert!(pictures.loading(&shown));
        pictures.fail("a".into());
        assert!(pictures.failed("a") && !pictures.loading(&shown));
        assert_eq!(pictures.next(&shown, &set(&[])), None, "no retry loop");
        pictures.retry_failed();
        assert!(
            pictures.failed("a"),
            "the mark stays while it is tried again"
        );
        assert_eq!(pictures.next(&shown, &set(&[])).as_deref(), Some("a"));
        pictures.loaded("a".into(), image());
        assert!(!pictures.failed("a") && !pictures.loading(&shown));
    }
}
