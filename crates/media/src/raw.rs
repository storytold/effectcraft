//! Sony photo RAW adapter. Sensor decoding stays in LightCraft's pinned, first-party
//! clean-room crate; no embedded preview is substituted for an unsupported sensor format.

use std::io::Read;
use std::path::Path;

use effectcraft_project::Footage;
use effectcraft_raster::Image;
use lightcraft_raw::{RawError, RawFormat, RawInfo};
use lightcraft_tiff::tags;
use rayon::prelude::*;

use crate::convert::AlphaOp;
use crate::{MediaError, Result};

// Covers the A7R IV's 61 MP sensor while bounding the float buffers before decompression.
const MAX_PIXELS: usize = 72_000_000;
const MAX_EDGE: usize = 16_384;
const MAX_FILE_BYTES: usize = 256 * 1024 * 1024;

pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).map_err(|e| MediaError::Io(format!("{}: {e}", path.display())))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| MediaError::Io(format!("{}: {e}", path.display())))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(MediaError::Unsupported(format!("{}: Sony photo RAW exceeds the 256 MiB file limit", path.display())));
    }
    Ok(bytes)
}

pub(crate) fn recognizes(path: &str, bytes: &[u8]) -> bool {
    if crate::ext_of(Path::new(path)) == "arw" {
        return true;
    }
    // Renamed ARWs are still sensor files, not the small TIFF/JPEG preview in IFD0.
    bytes.len() <= MAX_FILE_BYTES
        && lightcraft_tiff::Tiff::sniff(bytes).is_some()
        && limited_tiff(bytes).is_ok_and(|tiff| {
            tiff.find(tags::MAKE).and_then(|e| e.value.as_str()).is_some_and(|s| s.eq_ignore_ascii_case("SONY"))
                && tiff.all_ifds().iter().any(|ifd| ifd.u16(tags::PHOTOMETRIC) == Some(tags::photometric::CFA))
        })
}

fn limited_tiff(bytes: &[u8]) -> lightcraft_tiff::Result<lightcraft_tiff::Tiff> {
    let options = lightcraft_tiff::ParseOptions { max_ifds: 64, max_depth: 12, max_entries: 4096, value_budget: Some(8 * 1024 * 1024), ..Default::default() };
    lightcraft_tiff::Tiff::parse_with(bytes, &options)
}

fn error(path: &str, e: RawError) -> MediaError {
    match e {
        RawError::Unsupported(s) => MediaError::Unsupported(format!("{path}: Sony photo RAW: {s}")),
        other => MediaError::Decode(format!("{path}: Sony photo RAW: {other}")),
    }
}

fn info(path: &str, bytes: &[u8]) -> Result<RawInfo> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(MediaError::Unsupported(format!("{path}: Sony photo RAW exceeds the 256 MiB file limit")));
    }
    // Bound the complete IFD graph and metadata before the sensor API parses it again.
    let tiff = limited_tiff(bytes).map_err(|e| MediaError::Decode(format!("{path}: Sony photo RAW: {e}")))?;
    let info = lightcraft_raw::probe_info(bytes).map_err(|e| error(path, e))?;
    if info.format != RawFormat::Arw {
        return Err(MediaError::Unsupported(format!("{path}: expected Sony ARW sensor data")));
    }
    if info.cpp != 1 || !matches!(info.bits, 12 | 14 | 16) || info.cfa.as_ref().is_none_or(|cfa| !cfa.is_bayer()) {
        return Err(MediaError::Unsupported(format!("{path}: expected a 12/14/16-bit Sony Bayer sensor mosaic")));
    }
    validate_size(path, info.width, info.height)?;
    let (w, h) = info.developed_size();
    validate_size(path, w, h)?;
    validate_chunks(path, bytes, &info, &tiff)?;
    Ok(info)
}

// TIFF 6.0 strip/tile offsets and byte counts must describe complete ranges in the file.
// Reject truncation before the tolerant vendor decoder can pad missing samples with zero.
fn validate_chunks(path: &str, bytes: &[u8], info: &RawInfo, tiff: &lightcraft_tiff::Tiff) -> Result<()> {
    let ifds = tiff.all_ifds();
    let sensor = ifds
        .iter()
        .find(|ifd| {
            ifd.u16(tags::PHOTOMETRIC) == Some(tags::photometric::CFA)
                && ifd.u64(tags::IMAGE_WIDTH) == Some(info.width as u64)
                && ifd.u64(tags::IMAGE_LENGTH) == Some(info.height as u64)
        })
        .ok_or_else(|| MediaError::Unsupported(format!("{path}: unsupported Sony ARW sensor layout")))?;
    let offsets = sensor.u64s(tags::STRIP_OFFSETS).or_else(|| sensor.u64s(tags::TILE_OFFSETS));
    let counts = sensor.u64s(tags::STRIP_BYTE_COUNTS).or_else(|| sensor.u64s(tags::TILE_BYTE_COUNTS));
    let (Some(offsets), Some(counts)) = (offsets, counts) else {
        return Err(MediaError::Decode(format!("{path}: Sony photo RAW is missing sensor strip/tile ranges")));
    };
    if offsets.is_empty()
        || offsets.len() != counts.len()
        || offsets.iter().zip(&counts).any(|(&start, &len)| len == 0 || start.checked_add(len).is_none_or(|end| end > bytes.len() as u64))
    {
        return Err(MediaError::Decode(format!("{path}: Sony photo RAW has truncated or invalid sensor strip/tile ranges")));
    }
    let byte_count = counts
        .iter()
        .try_fold(0u64, |sum, n| sum.checked_add(*n))
        .ok_or_else(|| MediaError::Decode(format!("{path}: Sony photo RAW sensor byte count overflow")))?;
    let pixels =
        (info.width as u64).checked_mul(info.height as u64).ok_or_else(|| MediaError::Decode(format!("{path}: Sony photo RAW sensor size overflow")))?;
    match sensor.u16(tags::COMPRESSION) {
        Some(1) => {
            let minimum = pixels
                .checked_mul(info.cpp as u64)
                .and_then(|n| n.checked_mul(u64::from(info.bits)))
                .and_then(|n| n.checked_add(7))
                .map(|n| n / 8)
                .ok_or_else(|| MediaError::Decode(format!("{path}: Sony photo RAW sample size overflow")))?;
            if byte_count < minimum {
                return Err(MediaError::Decode(format!("{path}: Sony photo RAW has fewer sensor bytes than its dimensions require")));
            }
        }
        Some(32767) if sensor.u16(0x7000) == Some(2) && byte_count == pixels && info.width.is_multiple_of(32) && offsets.len() == 1 => {}
        Some(32767) => {
            return Err(MediaError::Unsupported(format!("{path}: unsupported Sony compressed RAW layout (expected one ARW2 strip, 32-pixel row blocks)")));
        }
        other => return Err(MediaError::Unsupported(format!("{path}: unsupported Sony photo RAW compression {other:?}"))),
    }
    Ok(())
}

fn validate_size(path: &str, w: usize, h: usize) -> Result<()> {
    if w == 0 || h == 0 || w > MAX_EDGE || h > MAX_EDGE || w.checked_mul(h).is_none_or(|n| n > MAX_PIXELS) {
        return Err(MediaError::Unsupported(format!("{path}: Sony photo RAW exceeds the 72 MP / 16384 pixel limits or has empty dimensions")));
    }
    Ok(())
}

pub(crate) fn probe(path: &str, bytes: &[u8]) -> Result<Footage> {
    let info = info(path, bytes)?;
    let (w, h) = info.developed_size();
    let (w, h) = if info.orientation.swaps_axes() { (h, w) } else { (w, h) };
    // Dimensions have been bounded to MAX_EDGE above; the image is converted to sRGB primaries.
    let mut footage = crate::probe::still_footage(path, w as u32, h as u32, image::ImageFormat::Tiff, false);
    footage.codec = format!("Sony ARW sensor ({}-bit, {})", info.bits, info.metadata.model.as_deref().unwrap_or("unknown camera"));
    Ok(footage)
}

pub(crate) fn decode(path: &str, bytes: &[u8], op: AlphaOp) -> Result<Image> {
    let header = info(path, bytes)?;
    let mut raw = lightcraft_raw::decode(bytes).map_err(|e| error(path, e))?;
    validate_size(path, raw.width, raw.height)?;
    // Use LightCraft's fitted spectral profiles where present. Uncalibrated cameras keep the
    // documented camera-RGB ~= linear-sRGB fallback, never an Adobe camera matrix.
    if let Some(model) = raw.metadata.model.as_deref()
        && let Some(camera) = lightcraft_raw::spectral::find(raw.metadata.make.as_deref(), model)
    {
        camera.fill(&mut raw.color);
    }
    let white = lightcraft_raw::color::as_shot_white_xy(&raw);
    let transform = lightcraft_raw::color::camera_transform(&raw, white);
    if transform.matrix_is_fallback {
        log::warn!("media: {path}: uncalibrated Sony camera colour; using the neutral camera-RGB fallback");
    }
    if transform.wb.iter().any(|v| !v.is_finite() || *v <= 0.0) || !transform.baseline_exposure.is_finite() {
        return Err(MediaError::Decode(format!("{path}: invalid Sony photo RAW white balance or exposure")));
    }
    let mut rgb = raw.develop(lightcraft_raw::Method::Ppg).map_err(|e| error(path, e))?;
    drop(raw);
    validate_size(path, rgb.width, rgb.height)?;
    if (rgb.width, rgb.height) != header.developed_size() {
        return Err(MediaError::Decode(format!("{path}: Sony photo RAW developed dimensions disagree with its header")));
    }
    // Sensor-clipped channels cannot retain their original chromaticity. Reconstruct them
    // before white balance, rather than allowing blown reflections/sky to turn magenta.
    lightcraft_raw::highlight::reconstruct(&mut rgb, transform.wb, 0.99);
    let matrix = lightcraft_color::REC2020.to_space(&lightcraft_color::SRGB).mul(&transform.matrix);
    let gain = (transform.baseline_exposure as f32).exp2();
    if !gain.is_finite() || matrix.0.iter().flatten().any(|v| !v.is_finite()) {
        return Err(MediaError::Decode(format!("{path}: invalid Sony photo RAW colour transform")));
    }
    rgb.map_in_place(|c| {
        let balanced = std::array::from_fn(|k| c[k] * transform.wb[k] * gain);
        matrix.apply_f32(balanced)
    });
    let rgb = rgb.into_oriented(header.orientation);
    if rgb.data.iter().flatten().any(|v| !v.is_finite()) {
        return Err(MediaError::Decode(format!("{path}: Sony photo RAW developed non-finite pixels")));
    }
    let data = rgb.data.par_iter().copied().map(|c| op.linear_opaque(c)).collect();
    Ok(Image { width: rgb.width as u32, height: rgb.height as u32, data })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use effectcraft_project::FootageKind;
    use effectcraft_time::Tick;

    use super::*;

    // Original synthetic TIFF/EP mosaic; no camera photograph, JPEG or private user path.
    fn arw(w: u32, h: u32, orientation: u16, compression: u16) -> Vec<u8> {
        let short = |v: u16| v.to_le_bytes().to_vec();
        let long = |v: u32| v.to_le_bytes().to_vec();
        let mut entries = vec![
            (256u16, 4u16, 1u32, long(w)),
            (257, 4, 1, long(h)),
            (258, 3, 1, short(16)),
            (259, 3, 1, short(compression)),
            (262, 3, 1, short(32803)),
            (271, 2, 5, b"SONY\0".to_vec()),
            (272, 2, 12, b"Test camera\0".to_vec()),
            (273, 4, 1, long(0)),
            (274, 3, 1, short(orientation)),
            (277, 3, 1, short(1)),
            (278, 4, 1, long(h)),
            (279, 4, 1, long(w.saturating_mul(h).saturating_mul(2))),
            (33421, 3, 2, vec![2, 0, 2, 0]),
            (33422, 1, 4, vec![0, 1, 1, 2]),
            (0x7000, 3, 1, short(1)),
            (0x7310, 3, 4, [512u16; 4].into_iter().flat_map(u16::to_le_bytes).collect()),
            (0x7313, 3, 4, [512u16, 256, 256, 384].into_iter().flat_map(u16::to_le_bytes).collect()),
        ];
        entries.sort_by_key(|e| e.0);
        let mut b = b"II*\0\x08\0\0\0".to_vec();
        b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        let data_start = 8 + 2 + entries.len() * 12 + 4;
        b.resize(data_start, 0);
        for (k, (tag, typ, count, data)) in entries.iter().enumerate() {
            let q = 10 + k * 12;
            b[q..q + 2].copy_from_slice(&tag.to_le_bytes());
            b[q + 2..q + 4].copy_from_slice(&typ.to_le_bytes());
            b[q + 4..q + 8].copy_from_slice(&count.to_le_bytes());
            if data.len() <= 4 {
                b[q + 8..q + 8 + data.len()].copy_from_slice(data);
            } else {
                let off = b.len() as u32;
                b[q + 8..q + 12].copy_from_slice(&off.to_le_bytes());
                b.extend_from_slice(data);
            }
        }
        let strip = b.len() as u32;
        let q = 10 + entries.iter().position(|e| e.0 == 273).unwrap() * 12;
        b[q + 8..q + 12].copy_from_slice(&strip.to_le_bytes());
        for y in 0..h.min(64) {
            for x in 0..w.min(64) {
                b.extend_from_slice(&(2048 + x as u16 * 40 + y as u16 * 100).to_le_bytes());
            }
        }
        b
    }

    #[test]
    fn working_gamma_and_preserve_rgb_use_distinct_cached_frames() {
        use effectcraft_render::{FloatCurve, FootageSource};

        let bytes: Arc<[u8]> = arw(16, 12, 1, 1).into();
        let f = crate::probe_bytes("working-gamma.ARW", bytes.clone()).unwrap();
        let pool = crate::MediaPool::new();
        pool.add_bytes(&f.path, bytes);
        let item = effectcraft_project::ItemId(0);
        let linear = pool.frame_with(item, &f, Tick::ZERO, None, FloatCurve::Linear).unwrap();
        let gamma = pool.frame_with(item, &f, Tick::ZERO, None, FloatCurve::Gamma24).unwrap();
        let srgb = pool.frame_with(item, &f, Tick::ZERO, None, FloatCurve::Srgb).unwrap();
        assert!(!Arc::ptr_eq(&linear, &gamma));
        assert!(!Arc::ptr_eq(&srgb, &gamma));
        for channel in 0..3 {
            let v = linear.get(4, 4)[channel];
            assert!((gamma.get(4, 4)[channel] - v.abs().powf(1.0 / 2.4).copysign(v)).abs() < 1e-6);
        }
        let again = pool.frame_with(item, &f, Tick::ZERO, None, FloatCurve::Gamma24).unwrap();
        assert!(Arc::ptr_eq(&gamma, &again));
        let preserve = pool.frame_with(item, &Footage { preserve_rgb: true, ..f }, Tick::ZERO, None, FloatCurve::Gamma24).unwrap();
        assert_eq!(preserve.get(4, 4), linear.get(4, 4));
    }

    #[test]
    fn sensor_import_uses_full_dimensions_orientation_and_frame_cache() {
        let bytes: Arc<[u8]> = arw(16, 12, 6, 1).into();
        let f = crate::probe_bytes("test.ARW", bytes.clone()).unwrap();
        assert_eq!((f.width, f.height, f.kind), (12, 16, FootageKind::Still));
        assert!(f.codec.starts_with("Sony ARW sensor"));
        let pool = crate::MediaPool::new();
        pool.add_bytes(&f.path, bytes);
        let frame = pool.frame_at(&f, Tick::ZERO).unwrap();
        assert_eq!((frame.width, frame.height), (12, 16));
        assert!(frame.data.iter().all(|c| c.iter().all(|v| v.is_finite()) && c[3] == 1.0));
        assert!(frame.data.iter().any(|c| c[0] > 0.1));
        assert!(frame.get(4, 4)[0] > frame.get(4, 4)[1] && frame.get(4, 4)[2] > frame.get(4, 4)[1], "as-shot WB must affect the sensor channels");
        assert!(Arc::ptr_eq(&frame, &pool.frame_at(&f, Tick::ZERO).unwrap()));
        let linear = pool.frame_at(&Footage { preserve_rgb: true, ..f.clone() }, Tick::ZERO).unwrap();
        assert!(frame.get(4, 4)[0] > linear.get(4, 4)[0]);

        let mut project = effectcraft_project::Project::default();
        project.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
        let comp = effectcraft_project::Comp::new(f.width, f.height, effectcraft_time::FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
        let item = project.add_item("RAW", Default::default(), None, effectcraft_project::ItemKind::Footage(f.clone()));
        let layer =
            effectcraft_project::build::layer(&mut project, &comp, "RAW", effectcraft_project::LayerSource::Footage { item }, (f.width, f.height), None);
        let mut comp = comp;
        comp.layers.push(layer);
        let comp_id = project.add_item("Comp", Default::default(), None, effectcraft_project::ItemKind::Comp(comp.into()));
        let rendered = effectcraft_render::Renderer::new(&project, &pool, Default::default()).comp_frame(comp_id, Tick::ZERO);
        assert_eq!((rendered.width, rendered.height), (12, 16));
        assert!((rendered.get(4, 4)[0] - frame.get(4, 4)[0]).abs() < 1e-5);
        assert_eq!(rendered.get(4, 4)[3], 1.0);
    }

    #[test]
    fn compressed_arw2_sensor_blocks_do_not_need_a_jpeg_preview() {
        let (w, h) = (32u32, 12u32);
        let mut bytes = arw(w, h, 1, 32767);
        let tiff = lightcraft_tiff::Tiff::parse(&bytes).unwrap();
        let sensor = tiff.ifds.first().unwrap();
        let bits = sensor.get(tags::BITS_PER_SAMPLE).unwrap().offset as usize;
        let kind = sensor.get(0x7000).unwrap().offset as usize;
        let count = sensor.get(tags::STRIP_BYTE_COUNTS).unwrap().offset as usize;
        let start = sensor.u64(tags::STRIP_OFFSETS).unwrap() as usize;
        bytes[bits..bits + 2].copy_from_slice(&14u16.to_le_bytes());
        bytes[kind..kind + 2].copy_from_slice(&2u16.to_le_bytes());
        bytes[count..count + 4].copy_from_slice(&(w * h).to_le_bytes());
        bytes.truncate(start);
        // Dietz (2016): each 128-bit ARW2 block holds equal 11-bit min/max, their two
        // 4-bit positions, and fourteen zero 7-bit deltas. A constant sensor block.
        for _ in 0..w * h / 16 {
            let packed = 512u32 | (512u32 << 11) | (1u32 << 26);
            bytes.extend_from_slice(&packed.to_le_bytes());
            bytes.extend_from_slice(&[0u8; 12]);
        }
        let footage = crate::probe_bytes("synthetic.arw", bytes.clone().into()).unwrap();
        let pool = crate::MediaPool::new();
        pool.add_bytes(&footage.path, bytes.into());
        let frame = pool.frame_at(&footage, Tick::ZERO).unwrap();
        assert_eq!((frame.width, frame.height), (w, h));
        assert!(frame.data.iter().all(|c| c.iter().all(|v| v.is_finite()) && c[3] == 1.0));
        assert!(frame.get(8, 4)[0] > 0.1, "must develop the sensor without any JPEG");
    }

    #[test]
    fn every_exif_orientation_reorders_the_developed_pixels() {
        let op = AlphaOp::new(effectcraft_project::AlphaMode::Ignore, [0.0; 3]);
        let base = decode("test.arw", &arw(16, 12, 1, 1), op).unwrap();
        for tag in 1..=8 {
            let image = decode("test.arw", &arw(16, 12, tag, 1), op).unwrap();
            let orientation = lightcraft_raw::Orientation::from_exif(tag);
            let size = if orientation.swaps_axes() { (12, 16) } else { (16, 12) };
            assert_eq!((image.width, image.height), size);
            for (x, y) in [(3, 4), (12, 8)] {
                let (ox, oy) = orientation.map(x as f64 + 0.5, y as f64 + 0.5, 16.0, 12.0);
                assert_eq!(image.get(ox.floor() as i64, oy.floor() as i64), base.get(x, y), "orientation {tag}");
            }
        }
    }

    #[test]
    fn malformed_or_oversized_inputs_fail_without_a_preview_fallback() {
        for bytes in [vec![], b"II*\0\xff\xff\xff\xff".to_vec(), arw(u32::MAX, 12, 1, 1), arw(16, 12, 1, 99)] {
            let pool = crate::MediaPool::new();
            pool.add_bytes("broken.arw", bytes.clone().into());
            if let Ok(f) = crate::probe_bytes("broken.arw", bytes.into()) {
                assert!(pool.frame_at(&f, Tick::ZERO).is_err());
            }
        }
        let bytes = arw(16, 12, 1, 1);
        for length in 0..bytes.len() {
            let truncated: Arc<[u8]> = bytes[..length].to_vec().into();
            if let Ok(f) = crate::probe_bytes("truncated.arw", truncated.clone()) {
                let pool = crate::MediaPool::new();
                pool.add_bytes(&f.path, truncated);
                assert!(pool.frame_at(&f, Tick::ZERO).is_err(), "accepted truncation at {length}");
            }
        }
        let mut short_sensor = arw(16, 12, 1, 1);
        // Keep a complete in-file strip, but lie about its dimensions (16 -> 32).
        short_sensor[18..22].copy_from_slice(&32u32.to_le_bytes());
        assert!(crate::probe_bytes("short-sensor.arw", short_sensor.into()).is_err());
    }
}
