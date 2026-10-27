//! Layered and vector stills: Photoshop documents (`effectcraft-psd`: the merged image, or one
//! layer for footage imported as a composition) and vector files rasterised at any scale: SVG
//! (`effectcraft-svg`), PDF / Illustrator / EPS (`effectcraft-pdf`; one layer of the file for
//! footage imported as a composition).

use effectcraft_project::{AlphaMode, Footage};
use effectcraft_raster::Image;

use crate::convert::{AlphaOp, straight_to_image};
use crate::{MediaError, Result};

fn is_svg(path: &str, bytes: &[u8]) -> bool {
    path.to_ascii_lowercase().ends_with(".svg") || effectcraft_svg::looks_like_svg(bytes)
}

/// The vector document of an SVG / PDF / AI / EPS file (`None` for other formats): page
/// `page` of a PDF, restricted to the footage's layer when it names one.
pub fn vector_doc(
    path: &str,
    bytes: &[u8],
    layer: Option<&effectcraft_project::SourceLayer>,
    page: u32,
) -> Option<std::result::Result<effectcraft_svg::Doc, String>> {
    if is_svg(path, bytes) {
        return Some(effectcraft_svg::parse(bytes).map_err(|e| format!("{path}: {e}")));
    }
    effectcraft_pdf::sniff(bytes)?;
    Some(
        effectcraft_pdf::parse_page(bytes, page as usize)
            .map(|d| match layer {
                Some(l) => effectcraft_pdf::layer_doc_with_sizes(&d, l.index as usize, l.layer_size).0,
                None => d,
            })
            .map_err(|e| format!("{path}: {e}")),
    )
}

fn is_pdf_like(path: &str, bytes: &[u8]) -> bool {
    let ext = std::path::Path::new(path).extension().and_then(std::ffi::OsStr::to_str).unwrap_or("").to_ascii_lowercase();
    effectcraft_pdf::sniff(bytes).is_some() && (effectcraft_pdf::EXTENSIONS.contains(&ext.as_str()) || bytes.starts_with(b"%PDF"))
}

/// Whether `bytes` (the first 64 KiB suffice) are a Photoshop or vector file [`probe`] handles.
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
pub(crate) fn sniff(path: &str, bytes: &[u8]) -> bool {
    effectcraft_psd::is_psd(bytes) || is_svg(path, bytes) || is_pdf_like(path, bytes)
}

/// Footage for a Photoshop or vector file (`None` for other formats).
pub(crate) fn probe(path: &str, bytes: &[u8]) -> Result<Option<Footage>> {
    if effectcraft_psd::is_psd(bytes) {
        let psd = effectcraft_psd::Psd::parse(bytes.to_vec()).map_err(|e| MediaError::Decode(format!("{path}: {e}")))?;
        let mut f = crate::probe::still_footage(path, psd.width, psd.height, image::ImageFormat::Png, true);
        f.codec = if psd.psb { "PSB".into() } else { "PSD".into() };
        f.alpha = if psd.merged_alpha || !psd.layers.is_empty() { AlphaMode::Straight } else { AlphaMode::Ignore };
        return Ok(Some(f));
    }
    if is_svg(path, bytes) {
        let doc = effectcraft_svg::parse(bytes).map_err(|e| MediaError::Decode(format!("{path}: {e}")))?;
        let (w, h) = doc.pixel_size();
        let mut f = crate::probe::still_footage(path, w, h, image::ImageFormat::Png, true);
        f.codec = "SVG".into();
        f.alpha = AlphaMode::Straight;
        return Ok(Some(f));
    }
    if is_pdf_like(path, bytes) {
        let doc = effectcraft_pdf::parse(bytes).map_err(|e| MediaError::Decode(format!("{path}: {e}")))?;
        let (w, h) = doc.pixel_size();
        let mut f = crate::probe::still_footage(path, w, h, image::ImageFormat::Png, true);
        f.codec = effectcraft_pdf::codec(path, bytes).unwrap_or("PDF").into();
        f.alpha = AlphaMode::Straight;
        return Ok(Some(f));
    }
    Ok(None)
}

/// Decode a Photoshop or SVG still (`None` for other formats).
pub(crate) fn decode(path: &str, bytes: &[u8], footage: &Footage, op: AlphaOp) -> Result<Option<Image>> {
    if effectcraft_psd::is_psd(bytes) {
        let psd = effectcraft_psd::Psd::parse(bytes.to_vec()).map_err(|e| MediaError::Decode(format!("{path}: {e}")))?;
        // A smart object's embedded file: an embedded document's merged image, or an image.
        if let Some(l) = footage.layer.as_ref()
            && let Some(uuid) = l.embedded.as_deref()
        {
            let data = psd.linked_data(uuid).ok_or_else(|| MediaError::Decode(format!("{path}: no embedded file {uuid}")))?;
            let px = if effectcraft_psd::is_psd(data) {
                let inner = effectcraft_psd::Psd::parse(data.to_vec()).map_err(|e| MediaError::Decode(format!("{path} (smart object): {e}")))?;
                inner.composite().map_err(|e| MediaError::Decode(format!("{path} (smart object): {e}")))?
            } else {
                let img = crate::probe::load_oriented(data, None).map_err(|e| MediaError::Decode(format!("{path} (smart object): {e}")))?;
                let rgba = img.to_rgba32f();
                effectcraft_psd::Pixels { width: rgba.width(), height: rgba.height(), data: rgba.pixels().map(|p| p.0).collect() }
            };
            // A perspective quad or a warp: baked as placed.
            if l.placed
                && let Some(so) = psd.layers.get(l.index as usize).and_then(|pl| pl.smart_object.as_ref())
            {
                let bb = so.placed_bounds(px.width as f64, px.height as f64);
                let k = footage.width as f64 / (bb[2] - bb[0]).max(1e-9);
                if let Some((baked, _)) = so.render_placed(&px, k, Some((footage.width, footage.height))) {
                    return Ok(Some(straight_to_image(baked.width, baked.height, &baked.data, op)));
                }
            }
            return Ok(Some(straight_to_image(px.width, px.height, &px.data, op)));
        }
        let px = match &footage.layer {
            Some(l) => psd.layer_pixels(l.index as usize, !l.layer_size),
            None => psd.composite(),
        }
        .map_err(|e| MediaError::Decode(format!("{path}: {e}")))?;
        return Ok(Some(straight_to_image(px.width, px.height, &px.data, op)));
    }
    if is_svg(path, bytes) || is_pdf_like(path, bytes) {
        let doc = vector_doc(path, bytes, footage.layer.as_ref(), footage.page).unwrap_or(Err(String::new())).map_err(MediaError::Decode)?;
        let (w, h) = doc.pixel_size();
        let img = effectcraft_svg::rasterize(&doc, w, h, 1.0);
        // The rasteriser produces premultiplied pixels already.
        let _ = op;
        return Ok(Some(img));
    }
    Ok(None)
}

/// A vector file rasterised at `scale` × its pixel size (Continuously Rasterize).
pub(crate) fn rasterize_vector(path: &str, bytes: &[u8], layer: Option<&effectcraft_project::SourceLayer>, page: u32, scale: f64) -> Option<Image> {
    let doc = vector_doc(path, bytes, layer, page)?.ok()?;
    let (w, h) = doc.pixel_size();
    let (sw, sh) = (((w as f64 * scale).ceil() as u32).clamp(1, 16384), ((h as f64 * scale).ceil() as u32).clamp(1, 16384));
    Some(effectcraft_svg::rasterize(&doc, sw, sh, scale))
}
