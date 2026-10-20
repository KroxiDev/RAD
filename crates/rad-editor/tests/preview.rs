#[test]
fn normalized_png_becomes_bounded_gui_pixels_without_file_io() {
    use image::ImageEncoder;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&[1, 2, 3, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    let pixels = rad_editor::preview::decode(&png).unwrap();
    assert_eq!(pixels.size, [1, 1]);
    assert_eq!(pixels.rgba, vec![1, 2, 3, 255]);
}
