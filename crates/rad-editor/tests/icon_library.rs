use rad_editor::icon_library::{Choice, Renderer};

// Critical: the bytes sent to the runtime and the preview must agree, with solid colors.
#[test]
fn icon_colors_are_opaque_with_antialiased_edges_and_matching_png() {
    let choice = Choice {
        icon: 0,
        foreground: [220, 180, 90],
        background: [18, 24, 35],
    };
    let rendered = Renderer::default().render(choice).unwrap();
    assert_eq!(rendered.pixels.size, [256, 256]);
    let pixels = rendered
        .pixels
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .collect::<Vec<_>>();
    assert!(pixels.iter().all(|p| p[3] == 255));
    assert_eq!(&pixels[0][..3], &choice.background);
    assert!(pixels.iter().any(|p| p[..3] == choice.foreground));
    assert!(
        pixels
            .iter()
            .any(|p| p[..3] != choice.foreground && p[..3] != choice.background)
    );
    let decoded = rad_editor::preview::decode(&rendered.png).unwrap();
    assert_eq!(decoded.rgba, rendered.pixels.rgba);
}
