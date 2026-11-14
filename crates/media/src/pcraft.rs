//! PhotoCraft's saved composite and canvas-sized raster or cached layer pixels.

use craft_doc::{Layer, RenditionKind, Samples};
use effectcraft_project::{AlphaMode, Footage};
use effectcraft_raster::Image;

use crate::convert::{AlphaOp, dynamic_to_image};
use crate::{MediaError, Result};

/// Keep the decoded float frame and its conversion buffers within the still-image budget.
const MAX_PIXELS: u64 = 16 << 20;
// Conservatively count source RGBA (16 B), raw mask samples (4 B), mask image
// (16 B), two blur images (32 B) and output (16 B), even where lifetimes do not
// overlap. Twelve Mi pixels fit within a 1 GiB working-buffer budget (84 B/pixel).
const MAX_LAYER_PIXELS: u64 = 12 << 20;
pub(crate) const MAX_INPUT_BYTES: u64 = 512 << 20;

pub(crate) fn sniff(path: &str, bytes: &[u8]) -> bool {
    path.to_ascii_lowercase().ends_with(".pcraft") || craft_doc::sniff(bytes) == Some(craft_doc::Kind::PhotoCraft)
}

fn error(path: &str, e: impl std::fmt::Display) -> MediaError {
    MediaError::Decode(format!("{path}: {e}"))
}

fn check_size(path: &str, width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(error(path, format!("PhotoCraft image {width} × {height} exceeds the {MAX_PIXELS}-pixel decoding limit or has an empty side")));
    }
    Ok(())
}

fn composite(path: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    check_input(path, bytes)?;
    craft_doc::renditions(bytes, &craft_doc::Limits::default())
        .map_err(|e| error(path, e))?
        .into_iter()
        .find(|r| r.kind == RenditionKind::Composite)
        .map(|r| r.bytes)
        .ok_or_else(|| error(path, "PhotoCraft document has no saved composite"))
}

fn check_input(path: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(error(path, "PhotoCraft bundle exceeds the 512 MiB input limit"));
    }
    Ok(())
}

fn png_size(path: &str, bytes: &[u8]) -> Result<(u32, u32)> {
    let size = image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png).into_dimensions().map_err(|e| error(path, e))?;
    check_size(path, size.0, size.1)?;
    Ok(size)
}

pub(crate) fn probe(path: &str, bytes: &[u8]) -> Result<Footage> {
    let png = composite(path, bytes)?;
    let (w, h) = png_size(path, &png)?;
    let mut f = crate::probe::still_footage(path, w, h, image::ImageFormat::Png, true);
    f.codec = "PhotoCraft".into();
    f.alpha = AlphaMode::Straight;
    Ok(f)
}

/// Preorder indexes include groups and unsupported layers, so a reload addresses the same
/// outline entry used by the composition importer.
fn layer_path(layers: &[Layer], wanted: u32) -> Option<Vec<usize>> {
    fn find(layers: &[Layer], wanted: u32, next: &mut u32, path: &mut Vec<usize>, depth: usize) -> Option<Vec<usize>> {
        if depth > 100 {
            return None;
        }
        for (index, layer) in layers.iter().enumerate() {
            path.push(index);
            let current = *next;
            *next = next.checked_add(1)?;
            if current == wanted {
                return Some(path.clone());
            }
            if let Some(found) = find(&layer.children, wanted, next, path, depth + 1) {
                return Some(found);
            }
            path.pop();
        }
        None
    }
    find(layers, wanted, &mut 0, &mut Vec::new(), 0)
}

fn mask_image(path: &str, width: u32, height: u32, samples: Samples) -> Result<Image> {
    let count = usize::try_from(u64::from(width) * u64::from(height)).map_err(|e| error(path, e))?;
    if samples.len() != count {
        return Err(error(path, "PhotoCraft mask size does not match the canvas"));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(count).map_err(|e| error(path, e))?;
    let pixel = |v: f32| [if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }; 4];
    match samples {
        Samples::U8(v) => data.extend(v.into_iter().map(|v| pixel(f32::from(v) / 255.0))),
        Samples::U16(v) => data.extend(v.into_iter().map(|v| pixel(f32::from(v) / 65535.0))),
        Samples::F32(v) => data.extend(v.into_iter().map(pixel)),
        _ => return Err(error(path, "unsupported PhotoCraft mask sample type")),
    }
    Ok(Image { width, height, data })
}

pub(crate) fn decode(path: &str, bytes: &[u8], footage: &Footage, op: AlphaOp) -> Result<Image> {
    check_input(path, bytes)?;
    let Some(layer) = &footage.layer else {
        let png = composite(path, bytes)?;
        png_size(path, &png)?;
        let image = crate::probe::load_oriented(&png, Some(image::ImageFormat::Png)).map_err(|e| error(path, e))?;
        return Ok(dynamic_to_image(&image, op));
    };
    if layer.layer_size || layer.embedded.is_some() || layer.placed {
        return Err(error(path, "PhotoCraft layer footage supports canvas-sized saved pixels, not cropped or embedded-file sources"));
    }
    let outline = craft_doc::outline(bytes, &craft_doc::Limits::default()).map_err(|e| error(path, e))?;
    let canvas = outline.canvas.ok_or_else(|| error(path, "PhotoCraft document has no canvas"))?;
    check_size(path, canvas.width, canvas.height)?;
    if u64::from(canvas.width) * u64::from(canvas.height) > MAX_LAYER_PIXELS {
        return Err(error(path, "PhotoCraft layer exceeds the 1 GiB working-buffer budget; import as Footage for the saved composite"));
    }
    let indexes = layer_path(&outline.layers, layer.index).ok_or_else(|| error(path, format!("no PhotoCraft layer {}", layer.index)))?;
    let pixels = craft_doc::pcraft_layer_rgba(bytes, &indexes, &craft_doc::Limits::default())
        .map_err(|e| error(path, e))?
        .ok_or_else(|| error(path, format!("PhotoCraft layer {} has no saved pixels", layer.name)))?;
    let (w, h) = (pixels.width, pixels.height);
    let count = usize::try_from(u64::from(w) * u64::from(h)).map_err(|e| error(path, e))?;
    if count.checked_mul(4) != Some(pixels.rgba.len()) {
        return Err(error(path, "PhotoCraft layer sample count does not match the canvas"));
    }
    let mask = if let Some(mask) = pixels.mask.filter(|m| m.enabled) {
        let mut alpha = mask_image(path, w, h, mask.alpha)?;
        let sigma = if mask.feather.is_finite() { f64::from(mask.feather.clamp(0.0, 1000.0)) } else { 0.0 };
        if sigma > 0.0 {
            alpha = if effectcraft_raster::gaussian_is_exact(sigma, sigma) {
                effectcraft_raster::gaussian_blur_exact(&alpha, sigma, sigma, true)
            } else {
                effectcraft_raster::gaussian_blur(&alpha, sigma, sigma, true)
            };
        }
        let density = if mask.density.is_finite() { mask.density.clamp(0.0, 1.0) } else { 1.0 };
        Some((alpha, density))
    } else {
        None
    };
    let mut data = Vec::new();
    data.try_reserve_exact(count).map_err(|e| error(path, e))?;
    let mut push = |index: usize, rgba: [f32; 4], floating: bool| -> Result<()> {
        let rgba = rgba.map(|v| if v.is_finite() { v } else { 0.0 });
        let coverage = match &mask {
            Some((alpha, density)) => 1.0 - density * (1.0 - alpha.data.get(index).ok_or_else(|| error(path, "PhotoCraft mask pixel is missing"))?[3]),
            None => 1.0,
        };
        data.push(op.rgba([rgba[0], rgba[1], rgba[2]], rgba[3].clamp(0.0, 1.0), floating, coverage));
        Ok(())
    };
    match pixels.rgba {
        Samples::U8(v) => {
            for (index, [r, g, b, a]) in v.as_chunks::<4>().0.iter().enumerate() {
                push(index, [f32::from(*r), f32::from(*g), f32::from(*b), f32::from(*a)].map(|v| v / 255.0), false)?;
            }
        }
        Samples::U16(v) => {
            for (index, [r, g, b, a]) in v.as_chunks::<4>().0.iter().enumerate() {
                push(index, [f32::from(*r), f32::from(*g), f32::from(*b), f32::from(*a)].map(|v| v / 65535.0), false)?;
            }
        }
        Samples::F32(v) => {
            for (index, [r, g, b, a]) in v.as_chunks::<4>().0.iter().enumerate() {
                push(index, [*r, *g, *b, *a], true)?;
            }
        }
        _ => return Err(error(path, "unsupported PhotoCraft layer sample type")),
    }
    Ok(Image { width: w, height: h, data })
}
