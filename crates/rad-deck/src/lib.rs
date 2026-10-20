//! XL output/input adapter only. No domain, navigation, admission or action execution.
mod protocol;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckError {
    Absent,
    Held,
    Identity,
    Io,
    Protocol,
    Overflow,
    InvalidDesired,
    Unavailable,
}
use std::sync::Arc;
/// Where a key's text is painted. At the top, in the middle or at the bottom the picture
/// fills the key behind it; above or below, the picture keeps clear of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextPlacement {
    Top,
    Middle,
    #[default]
    Bottom,
    AbovePicture,
    BelowPicture,
}
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KeyVisual {
    text: String,
    text_placement: TextPlacement,
    image: Option<Arc<rad_media::NormalizedImage>>,
    unavailable: bool,
    stale: bool,
    background: Option<[u8; 4]>,
    generic_launcher: bool,
}
impl KeyVisual {
    pub fn new(
        text: &str,
        image: Option<Arc<rad_media::NormalizedImage>>,
        unavailable: bool,
        generic_launcher: bool,
    ) -> Self {
        Self {
            text: text.chars().take(128).collect(),
            text_placement: TextPlacement::default(),
            image,
            unavailable,
            stale: false,
            background: None,
            generic_launcher,
        }
    }
    /// Last known content is not live. Presentation only; never an execution gate.
    pub fn stale(mut self, stale: bool) -> Self {
        self.stale = stale;
        self
    }
    /// Plugin color is independent of text/image and execution availability.
    pub fn background_color(mut self, color: Option<[u8; 4]>) -> Self {
        self.background = color;
        self
    }
    pub fn text_at(mut self, placement: TextPlacement) -> Self {
        self.text_placement = placement;
        self
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Desired {
    keys: [KeyVisual; 32],
    brightness: u8,
    asleep: bool,
}
impl Desired {
    pub fn new(keys: [KeyVisual; 32], brightness: u8, asleep: bool) -> Result<Self, DeckError> {
        if brightness > 100 {
            return Err(DeckError::InvalidDesired);
        }
        Ok(Self {
            keys,
            brightness,
            asleep,
        })
    }
}
mod hidraw;
mod render;
/// Construct only after an explicit physical handoff; this is not a client capability.
#[derive(Clone, Copy, Debug)]
pub struct DeviceConfig {
    _private: (),
}
impl DeviceConfig {
    pub fn xl_after_exclusive_handoff() -> Self {
        Self { _private: () }
    }
}
mod worker;
pub use worker::{Deck, Event};
