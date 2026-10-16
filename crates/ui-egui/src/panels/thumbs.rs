//! File thumbnails shared by the Media Browser tiles and the in-app file picker's icon views.
//!
//! One decode budget for both: at most `budget` decodes are started per frame (the caller
//! repaints while the budget lasts), and every answer — including "not decodable" — is cached
//! per path in the egui context, so a folder of audio files never probes twice.

use effectcraft_engine::project::ItemId;

use crate::EffectcraftApp;

/// A decoded thumbnail: the texture and its pixel size (`None` while not decoded, or when the
/// file has no picture to show).
pub type Thumb = Option<(egui::TextureHandle, [u32; 2])>;

/// Thumbnail of a file (cached per path; `None` while not decoded or not decodable). Still
/// images and video decode through the media layer's probe and footage cache, downscaled to at
/// most 128 px on the long side.
pub fn media(app: &EffectcraftApp, ctx: &egui::Context, path: &str, budget: &mut u32) -> Thumb {
    let id = egui::Id::new(("file-thumb", path));
    if let Some(t) = ctx.data(|d| d.get_temp::<Thumb>(id)) {
        return t;
    }
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let thumb = (|| {
        let imp = app.session.importer.clone()?;
        let f = imp.probe(path).ok()?;
        if !f.has_video {
            return None;
        }
        // A synthetic item id keyed by the path (the frame cache keys on the path).
        let hid = path.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3)) | (1 << 62);
        let img = app.session.footage.frame(ItemId(hid), &f, effectcraft_engine::time::Tick::ZERO)?;
        let step = (img.width.max(img.height) as f32 / 128.0).ceil().max(1.0) as u32;
        let (w, h) = (img.width.div_ceil(step), img.height.div_ceil(step));
        let full = img.to_rgba8();
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let i = (((y * step) * img.width + x * step) * 4) as usize;
                px.extend_from_slice(&full[i..i + 4]);
            }
        }
        let tex = ctx.load_texture(
            format!("file-thumb-{path}"),
            egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px),
            egui::TextureOptions::LINEAR,
        );
        Some((tex, [w, h]))
    })();
    ctx.data_mut(|d| d.insert_temp(id, thumb.clone()));
    thumb
}
