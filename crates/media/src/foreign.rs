//! Stills in file types EffectCraft does not know, read by a plug-in (`format` contribution).
//!
//! The plug-in host registers a [`Reader`] once. [`crate::probe_bytes`] and the pool's still decoder
//! ask the readers about a file that no built-in still reader recognises, so footage in a plug-in's
//! format is probed and decoded like a PNG. A file the built-in readers recognise (PNG, JPEG,
//! TIFF, EXR, ...) never reaches a plug-in, whatever extension the plug-in declares. A reader
//! answers `None` for a file that is not its own.

use std::sync::{Arc, RwLock};

use effectcraft_project::{AlphaMode, Footage};
use effectcraft_raster::Image;

use crate::convert::{AlphaOp, dynamic_to_image};
use crate::{MediaError, Result};

/// A decoded picture: straight alpha RGBA8, top row first.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
    pub has_alpha: bool,
}

/// `(path, file bytes)` to a picture, `None` when the file is not this reader's, `Err(reason)` when it is but cannot be read.
pub type Reader = Arc<dyn Fn(&str, &[u8]) -> Option<std::result::Result<Raster, String>> + Send + Sync>;

static READERS: RwLock<Vec<Reader>> = RwLock::new(Vec::new());

/// Add a reader. Readers run in the order they were added.
pub fn register(reader: Reader) {
    READERS.write().unwrap_or_else(|e| e.into_inner()).push(reader);
}

fn read(path: &str, bytes: &[u8]) -> Result<Option<Raster>> {
    // A format EffectCraft reads itself stays with the built-in reader: a plug-in is not run for
    // every frame of it, and its error cannot fail a file the built-in reader opens.
    if crate::still_format(path, bytes).is_some() {
        return Ok(None);
    }
    let readers = READERS.read().unwrap_or_else(|e| e.into_inner()).clone();
    match readers.iter().find_map(|r| r(path, bytes)) {
        Some(Ok(r)) => Ok(Some(r)),
        Some(Err(why)) => Err(MediaError::Decode(format!("{path}: {why}"))),
        None => Ok(None),
    }
}

/// Footage for a file a reader owns (`None` for other files).
pub(crate) fn probe(path: &str, bytes: &[u8]) -> Result<Option<Footage>> {
    Ok(read(path, bytes)?.map(|r| {
        let mut f = crate::probe::still_footage(path, r.width, r.height, image::ImageFormat::Png, r.has_alpha);
        f.codec = path.rsplit('.').next().unwrap_or("image").to_ascii_uppercase();
        f.alpha = if r.has_alpha { AlphaMode::Straight } else { AlphaMode::Ignore };
        f
    }))
}

/// The image of a file a reader owns (`None` for other files).
pub(crate) fn decode(path: &str, bytes: &[u8], op: AlphaOp) -> Result<Option<Image>> {
    let Some(r) = read(path, bytes)? else { return Ok(None) };
    let img = image::RgbaImage::from_raw(r.width, r.height, r.rgba8)
        .ok_or_else(|| MediaError::Decode(format!("{path}: the pixel count does not match {}x{}", r.width, r.height)))?;
    Ok(Some(dynamic_to_image(&image::DynamicImage::ImageRgba8(img), op)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A reader that claims `*.ecforeign` files and also `ecforeign-claims-png.png`, a PNG
    /// it must not get.
    fn register_test_reader() {
        register(Arc::new(|path, _| {
            (path.ends_with(".ecforeign") || path.ends_with("ecforeign-claims-png.png"))
                .then(|| Ok(Raster { width: 1, height: 1, rgba8: vec![255, 0, 0, 255], has_alpha: false }))
        }));
    }

    fn png(width: u32, height: u32) -> Arc<[u8]> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height)).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner().into()
    }

    #[test]
    fn a_reader_gets_the_files_the_built_in_readers_do_not_know() {
        register_test_reader();
        let f = crate::probe_bytes("shot.ecforeign", Arc::from(&b"opaque"[..])).unwrap();
        assert_eq!((f.width, f.height), (1, 1));
    }

    #[test]
    fn a_reader_does_not_take_over_a_format_the_built_in_readers_know() {
        register_test_reader();
        let f = crate::probe_bytes("a/ecforeign-claims-png.png", png(3, 2)).unwrap();
        assert_eq!((f.width, f.height), (3, 2), "the PNG is probed by the built-in reader");
        assert!(read("a/ecforeign-claims-png.png", &png(3, 2)).unwrap().is_none());
    }
}
