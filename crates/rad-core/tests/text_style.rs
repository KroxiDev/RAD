use rad_core::configuration::{TextPosition as P, TextStyle};
fn position(s: &str) -> Option<P> {
    Some(match s {
        "auto" => return None,
        "top" => P::Top,
        "middle" => P::Middle,
        "bottom" => P::Bottom,
        "above_image" => P::AboveImage,
        "below_image" => P::BelowImage,
        "hidden" => P::Hidden,
        _ => panic!("unknown position"),
    })
}
#[test]
fn shared_text_resolution_vectors() {
    for row in include_str!("../../../docs/fixtures/text-resolution.tsv")
        .lines()
        .filter(|l| !l.starts_with('#'))
    {
        let f: Vec<_> = row.split('\t').collect();
        let optional = |s| if s == "-" { None } else { Some(s) };
        let style = TextStyle {
            position: position(f[0]),
            size: 24,
            color: [1, 2, 3],
            band: false,
        };
        let resolved = style.resolve(f[2], f[1] == "true", optional(f[3]), optional(f[4]));
        assert_eq!(resolved.text, f[5], "{row}");
        assert_eq!(Some(resolved.position), position(f[6]), "{row}");
        assert_eq!(
            (resolved.size, resolved.color, resolved.band),
            (24, [1, 2, 3], false)
        );
    }
}

#[test]
fn configuration_rejects_invalid_text_size_in_base_and_state() {
    use rad_core::{configuration::*, identity::*};
    let initial = Configuration::prepare_new(&IdentityRegistry::new(1).unwrap()).unwrap();
    let folder = initial.configuration().data().principal;
    let c = initial
        .configuration()
        .prepare_edit(
            initial.registry(),
            Origin::Editor,
            Edit::CreateButton {
                folder,
                position: 0,
                name: "Style".into(),
            },
        )
        .unwrap();
    let button = c.configuration().data().buttons[0].binding.identity();
    for size in [0, 7, 25, 255] {
        for state in [false, true] {
            let mut appearance = Appearance::default();
            if state {
                appearance.states.insert(
                    "off".into(),
                    StateFace {
                        text_style: TextStyle {
                            size,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                );
            } else {
                appearance.text_style.size = size;
            }
            assert!(
                c.configuration()
                    .prepare_edit(
                        c.registry(),
                        Origin::Editor,
                        Edit::SetAppearance { button, appearance }
                    )
                    .is_err()
            );
        }
    }
}
