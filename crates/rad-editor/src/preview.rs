use crate::session::Error;
#[derive(Debug)]
pub struct Pixels {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}
pub fn decode(bytes: &[u8]) -> Result<Pixels, Error> {
    if bytes.len() > 512 * 1024 {
        return Err(Error::protocol());
    }
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(256);
    limits.max_image_height = Some(256);
    limits.max_alloc = Some(4 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| Error::protocol())?.to_rgba8();
    Ok(Pixels {
        size: [image.width() as usize, image.height() as usize],
        rgba: image.into_raw(),
    })
}
