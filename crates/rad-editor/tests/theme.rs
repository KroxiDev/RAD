use rad_editor::theme::Palette;
#[test]
fn omarchy_palette_keeps_last_complete_valid_document() {
    let mut p = Palette::default();
    assert!(p.apply("background='#010203'\nforeground='#f0f1f2'\naccent='#abcDEF'\n"));
    assert_eq!(p.background, [1, 2, 3]);
    let last = p.clone();
    assert!(!p.apply("background='#112233'\nforeground='bad'"));
    assert_eq!(p, last);
    assert!(!p.apply(&format!("#{}", "x".repeat(4096))));
    assert_eq!(p, last);
}
