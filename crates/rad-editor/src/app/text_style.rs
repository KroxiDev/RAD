//! Converts effective API views back into editable per-face styles.
use rad_api::{StateFace, TextPosition, TextStyle};
use serde_json::Value;

pub(super) fn color(value: &str) -> [u8; 3] {
    let Some(hex) = value
        .strip_prefix('#')
        .filter(|hex| hex.len() == 6 && hex.is_ascii())
    else {
        return [255; 3];
    };
    let channels: Option<Vec<_>> = (0..3)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
        .collect();
    channels.and_then(|v| v.try_into().ok()).unwrap_or([255; 3])
}
pub(super) fn color_string([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}
pub(super) fn position(value: &str) -> Option<TextPosition> {
    Some(match value {
        "top" => TextPosition::Top,
        "middle" => TextPosition::Middle,
        "bottom" => TextPosition::Bottom,
        "above_image" => TextPosition::AboveImage,
        "below_image" => TextPosition::BelowImage,
        "hidden" => TextPosition::Hidden,
        _ => return None,
    })
}
pub(super) fn position_name(value: Option<TextPosition>) -> &'static str {
    match value {
        Some(TextPosition::Top) => "top",
        Some(TextPosition::Middle) => "middle",
        Some(TextPosition::Bottom) => "bottom",
        Some(TextPosition::AboveImage) => "above_image",
        Some(TextPosition::BelowImage) => "below_image",
        Some(TextPosition::Hidden) => "hidden",
        None => "",
    }
}
pub(super) fn from_view(value: &Value) -> TextStyle {
    TextStyle {
        position: if value["text_position_automatic"] == true {
            None
        } else {
            value["text_position"].as_str().and_then(position)
        },
        size: value["text_size"]
            .as_u64()
            .and_then(|s| u8::try_from(s).ok())
            .unwrap_or(13),
        color: color(value["text_color"].as_str().unwrap_or("#FFFFFF")),
        band: value["text_band"].as_bool().unwrap_or(true),
    }
}
pub(super) fn from_face(face: &StateFace) -> TextStyle {
    TextStyle {
        position: face.text_position,
        size: face.text_size.unwrap_or(13),
        color: color(face.text_color.as_deref().unwrap_or("#FFFFFF")),
        band: face.text_band.unwrap_or(true),
    }
}
pub(super) fn face(text: String, image: Option<String>, style: TextStyle) -> StateFace {
    StateFace {
        text,
        image,
        text_position: style.position,
        text_size: (style.size != 13).then_some(style.size),
        text_color: (style.color != [255; 3]).then(|| color_string(style.color)),
        text_band: (!style.band).then_some(false),
    }
}
pub(super) fn face_from_view(value: &Value) -> StateFace {
    face(
        super::string(&value["text"]),
        value["image"].as_str().map(str::to_owned),
        from_view(value),
    )
}
pub(super) fn states(value: &Value) -> std::collections::BTreeMap<String, StateFace> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, value)| (id.clone(), face_from_view(value)))
        .collect()
}
pub(super) fn retain(fresh: &mut TextStyle, before: TextStyle, edited: TextStyle) {
    if before.position != edited.position {
        fresh.position = edited.position;
    }
    if before.size != edited.size {
        fresh.size = edited.size;
    }
    if before.color != edited.color {
        fresh.color = edited.color;
    }
    if before.band != edited.band {
        fresh.band = edited.band;
    }
}

pub(super) struct Controls {
    pub text: bool,
    pub placement: bool,
    pub size_color: bool,
    pub band: bool,
}
pub(super) fn controls(style: TextStyle, effective: TextPosition, has_image: bool) -> Controls {
    Controls {
        text: style.position != Some(TextPosition::Hidden),
        placement: !matches!(effective, TextPosition::Hidden | TextPosition::Middle),
        size_color: effective != TextPosition::Hidden,
        band: has_image
            && matches!(
                effective,
                TextPosition::Top | TextPosition::Middle | TextPosition::Bottom
            ),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_hidden_remains_editable_and_style_controls_follow_effective_position() {
        let style = TextStyle::default();
        let hidden = style.resolve("", true, None, Some("Name"));
        assert_eq!(hidden.position, TextPosition::Hidden);
        let c = controls(style, hidden.position, true);
        assert!(c.text);
        assert!(!c.placement && !c.size_color && !c.band);
        let chosen = TextStyle {
            position: Some(TextPosition::Hidden),
            ..style
        };
        assert!(!controls(chosen, TextPosition::Hidden, true).text);
        for position in [
            TextPosition::Top,
            TextPosition::Middle,
            TextPosition::Bottom,
            TextPosition::AboveImage,
            TextPosition::BelowImage,
        ] {
            for has_image in [true, false] {
                let c = controls(style, position, has_image);
                assert!(c.text && c.size_color);
                assert_eq!(c.placement, position != TextPosition::Middle);
                assert_eq!(
                    c.band,
                    has_image
                        && matches!(
                            position,
                            TextPosition::Top | TextPosition::Middle | TextPosition::Bottom
                        )
                );
            }
        }
        assert_eq!(
            style.resolve("Typed", true, None, Some("Name")).position,
            TextPosition::Bottom
        );
    }
}
