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
/// `page` of a PDF framed as `framing` says, restricted to the footage's layer when it names one.
pub fn vector_doc(
    path: &str,
    bytes: &[u8],
    layer: Option<&effectcraft_project::SourceLayer>,
    page: u32,
    framing: effectcraft_pdf::Framing,
) -> Option<std::result::Result<effectcraft_svg::Doc, String>> {
    if is_svg(path, bytes) {
        return Some(effectcraft_svg::parse(bytes).map_err(|e| format!("{path}: {e}")));
    }
    effectcraft_pdf::sniff(bytes)?;
    Some(
        effectcraft_pdf::parse_page_framed(bytes, page as usize, framing)
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
        let doc = footage_doc(path, bytes, footage).unwrap_or(Err(String::new())).map_err(MediaError::Decode)?;
        let ((w, h), to_image) = fit(&doc, footage, 1.0);
        // A declared size up to 30000² would be a 14 GB allocation that aborts the app (#2136):
        // past what Continuously Rasterize can draw, the footage reports an error instead.
        if u64::from(w) * u64::from(h) > u64::from(MAX_VECTOR_SIDE) * u64::from(MAX_VECTOR_SIDE) {
            return Err(MediaError::Decode(format!("{path}: {w}×{h} pixels is larger than vector footage can be drawn ({MAX_VECTOR_SIDE}×{MAX_VECTOR_SIDE})")));
        }
        let img = effectcraft_svg::try_rasterize_with(&doc, w, h, to_image).map_err(|e| MediaError::Decode(format!("{path}: {e}")))?;
        // The rasteriser produces premultiplied pixels already.
        let _ = op;
        return Ok(Some(img));
    }
    Ok(None)
}

/// The largest side vector footage is drawn at, decoded or continuously rasterised.
const MAX_VECTOR_SIDE: u32 = 16384;

/// The vector footage `footage` (of the file `path` with these `bytes`) rasterised at `scale` ×
/// its size as [`effectcraft_render::vector_fit`] draws it (Continuously Rasterize).
///
/// `None` when the file cannot be read as a vector document. `Some(Err(_))` when a render budget
/// refused part of the drawing (for example a layer scaled past the surface limit), so the caller
/// can report it instead of showing a partial or empty image as if it were complete.
pub(crate) fn rasterize_vector(path: &str, bytes: &[u8], footage: &Footage, scale: f64) -> Option<Result<Image>> {
    let doc = footage_doc(path, bytes, footage)?.ok()?;
    let ((sw, sh), to_image) = fit(&doc, footage, scale);
    let drawn = effectcraft_svg::try_rasterize_with(&doc, sw.clamp(1, MAX_VECTOR_SIDE), sh.clamp(1, MAX_VECTOR_SIDE), to_image);
    Some(drawn.map_err(|e| MediaError::Decode(format!("{path}: {e}"))))
}

/// The page framing of vector footage: as After Effects places it for footage from an After
/// Effects project ([`Footage::placed_page`]), as a PDF viewer shows it otherwise.
fn framing(footage: &Footage) -> effectcraft_pdf::Framing {
    if footage.placed_page { effectcraft_pdf::Framing::Placed } else { effectcraft_pdf::Framing::Viewer }
}

/// [`vector_doc`] of `footage`'s page, layer and framing.
fn footage_doc(path: &str, bytes: &[u8], footage: &Footage) -> Option<std::result::Result<effectcraft_svg::Doc, String>> {
    vector_doc(path, bytes, footage.layer.as_ref(), footage.page, framing(footage))
}

/// The image size and document → image mapping of `doc` drawn for `footage` at `scale`.
fn fit(doc: &effectcraft_svg::Doc, footage: &Footage, scale: f64) -> ((u32, u32), effectcraft_svg::Affine) {
    let (size, [sx, sy]) = effectcraft_render::vector_fit(footage, [doc.width, doc.height], doc.pixel_size(), scale);
    (size, effectcraft_svg::Affine::scale_non_uniform(sx, sy))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2136: an SVG that declares 30000×30000 asked for a 14 GB image and aborted the app; it
    /// is now an error, and a drawable size still decodes.
    #[test]
    fn vector_footage_past_the_drawable_size_is_an_error_not_an_abort() {
        let huge = br#"<svg xmlns="http://www.w3.org/2000/svg" width="30000" height="30000"/>"#;
        let footage = probe("huge.svg", huge).unwrap().unwrap();
        let e = decode("huge.svg", huge, &footage, AlphaOp::new(footage.alpha, footage.premul_color)).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(e.contains("30000×30000"), "{e}");
        let small = br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20"/></svg>"#;
        let footage = probe("small.svg", small).unwrap().unwrap();
        let img = decode("small.svg", small, &footage, AlphaOp::new(footage.alpha, footage.premul_color)).unwrap().unwrap();
        assert_eq!((img.width, img.height), (40, 20));
    }

    /// Continuously Rasterize reports a layer scaled past the render budget as an error instead of
    /// returning a partial or empty image, and still draws the same layer at an ordinary scale.
    #[test]
    fn rasterizing_vector_footage_past_the_budget_is_reported() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100"/></svg>"#;
        let footage = probe("scaled.svg", svg).unwrap().unwrap();
        let ok = rasterize_vector("scaled.svg", svg, &footage, 1.0).unwrap().unwrap();
        assert_eq!((ok.width, ok.height), (100, 100));
        let e = rasterize_vector("scaled.svg", svg, &footage, 1000.0).unwrap().err().map(|e| e.to_string()).unwrap_or_default();
        assert!(e.contains("scaled.svg"), "{e}");
    }

    /// A 200 × 100 pt page: red left half, blue right half.
    fn halves_pdf() -> Vec<u8> {
        effectcraft_pdf::write::pdf(
            &[
                (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
                (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), None),
                (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".into(), None),
                (4, "<< >>".into(), Some(b"1 0 0 rg 0 0 100 100 re f 0 0 1 rg 100 0 100 100 re f".to_vec())),
            ],
            1,
        )
    }

    /// An opaque red pixel.
    fn is_red(p: [f32; 4]) -> bool {
        p[3] > 0.99 && p[0] > 0.9 && p[2] < 0.1
    }

    /// An opaque blue pixel.
    fn is_blue(p: [f32; 4]) -> bool {
        p[3] > 0.99 && p[2] > 0.9 && p[0] < 0.1
    }

    /// #3099: an After Effects project keeps the size After Effects measured for a placed .ai or
    /// PDF, which can differ from this reader's page size. A placed page fills that size (the
    /// layer's anchor and masks are measured against it), decoded and continuously rasterised;
    /// other footage is drawn at the page's own size as before, whatever size it says.
    #[test]
    fn placed_vector_footage_fills_the_footage_size() {
        let pdf = halves_pdf();
        let mut footage = probe("art.ai", &pdf).unwrap().unwrap();
        assert_eq!((footage.width, footage.height), (200, 100));
        assert!(!footage.placed_page);
        let op = AlphaOp::new(footage.alpha, footage.premul_color);
        let own = decode("art.ai", &pdf, &footage, op).unwrap().unwrap();
        assert_eq!((own.width, own.height), (200, 100));
        assert!(is_red(own.get(50, 50)) && is_blue(own.get(150, 50)));

        (footage.width, footage.height) = (100, 80);
        let unplaced = decode("art.ai", &pdf, &footage, op).unwrap().unwrap();
        assert!(unplaced == own, "not placed: drawn as before");
        assert_eq!(rasterize_vector("art.ai", &pdf, &footage, 0.5).and_then(|r| r.ok()).map(|i| (i.width, i.height)), Some((100, 50)));

        footage.placed_page = true;
        let img = decode("art.ai", &pdf, &footage, op).unwrap().unwrap();
        assert_eq!((img.width, img.height), (100, 80));
        for (x, y) in [(25, 3), (25, 76)] {
            assert!(is_red(img.get(x, y)), "red at ({x}, {y}): {:?}", img.get(x, y));
        }
        for (x, y) in [(75, 3), (75, 76)] {
            assert!(is_blue(img.get(x, y)), "blue at ({x}, {y}): {:?}", img.get(x, y));
        }

        let sharp = rasterize_vector("art.ai", &pdf, &footage, 2.0).unwrap().unwrap();
        assert_eq!((sharp.width, sharp.height), (200, 160));
        assert!(is_red(sharp.get(50, 155)) && is_blue(sharp.get(150, 155)));
        // Placed at the page's own size: 1:1.
        (footage.width, footage.height) = (200, 100);
        assert!(decode("art.ai", &pdf, &footage, op).unwrap().unwrap() == own, "placed at its own size: 1:1");
    }
}
