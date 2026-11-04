//! Text resolution for one face; callers provide the images actually displayed.
use super::TextPosition;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextStyle {
    pub position: Option<TextPosition>,
    pub size: u8,
    pub color: [u8; 3],
    pub band: bool,
}
impl Default for TextStyle {
    fn default() -> Self {
        Self {
            position: None,
            size: 13,
            color: [255; 3],
            band: true,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedText<'a> {
    pub text: &'a str,
    pub position: TextPosition,
    pub size: u8,
    pub color: [u8; 3],
    pub band: bool,
}
impl TextStyle {
    pub fn resolve<'a>(
        &self,
        own_text: &'a str,
        has_image: bool,
        plugin_text: Option<&'a str>,
        fallback_name: Option<&'a str>,
    ) -> ResolvedText<'a> {
        let text = plugin_text.unwrap_or(if !own_text.is_empty() {
            own_text
        } else if has_image {
            ""
        } else {
            fallback_name.unwrap_or("")
        });
        let position = self.position.unwrap_or(if has_image {
            if text.is_empty() {
                TextPosition::Hidden
            } else {
                TextPosition::Bottom
            }
        } else {
            TextPosition::Middle
        });
        ResolvedText {
            text: if position == TextPosition::Hidden {
                ""
            } else {
                text
            },
            position,
            size: self.size,
            color: self.color,
            band: self.band,
        }
    }
}
