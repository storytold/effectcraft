//! Probing: describe a file as an [`effectcraft_project::Footage`].

use std::path::Path;
use std::sync::Arc;

use effectcraft_project::{AlphaMode, Footage, FootageKind};
use effectcraft_time::{FrameRate, Tick};

use crate::{MediaError, Result, ext_of, rate_from_fc};

/// Frame rate given to image sequences on import (After Effects' default preference).
pub const DEFAULT_SEQUENCE_RATE: FrameRate = FrameRate::FPS_30;

/// Probe one file. Image sequences are made by the engine, which groups numbered stills
/// (`effectcraft_engine::sequence`) and probes their first frame here.
pub fn probe(path: impl AsRef<Path>) -> Result<Footage> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| MediaError::Io(format!("{}: {e}", path.display())))?;
    if crate::MODEL_EXTENSIONS.contains(&ext_of(path).as_str()) {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        return probe_model(&path.to_string_lossy(), &bytes, &|uri| std::fs::read(dir.join(uri)).ok());
    }
    probe_bytes(&path.to_string_lossy(), bytes.into())
}

/// Probe a 3D model (glTF 2.0 / OBJ): a [`FootageKind::Model`] item whose duration is its
/// longest animation. `resolve` reads sibling resources.
pub fn probe_model(path: &str, bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Footage> {
    let m = effectcraft_model::load(path, bytes, resolve).map_err(|e| MediaError::Decode(format!("{path}: {e}")))?;
    let s = m.summary();
    let dur = s.animations.iter().map(|a| a.1).fold(0.0f64, f64::max);
    let codec = match effectcraft_model::format_of(path) {
        Some("obj") => "OBJ",
        Some("glb") => "GLB",
        _ => "glTF",
    };
    Ok(Footage {
        path: path.to_string(),
        kind: FootageKind::Model,
        width: 0,
        height: 0,
        pixel_aspect: 1.0,
        frame_rate: DEFAULT_SEQUENCE_RATE,
        native_rate: None,
        duration: Tick::from_seconds_f64(dur),
        has_video: false,
        has_audio: false,
        alpha: AlphaMode::Straight,
        premul_color: [0.0; 3],
        loop_count: 1,
        codec: format!("{codec} ({} meshes, {} triangles)", s.meshes, s.triangles),
        missing: false,
        sequence: Vec::new(),
        color_profile: None,
        ..Default::default()
    })
}

/// Probe in-memory file contents; `path` names the footage (and is stored as `Footage::path`).
pub fn probe_bytes(path: &str, bytes: Arc<[u8]>) -> Result<Footage> {
    if let Some(f) = crate::layered::probe(path, &bytes)? {
        return Ok(f);
    }
    if crate::MODEL_EXTENSIONS.contains(&ext_of(Path::new(path)).as_str()) {
        return probe_model(path, &bytes, &|_| None);
    }
    if let Ok(fmt) = image::guess_format(&bytes) {
        return probe_still_bytes(path, &bytes, fmt);
    }
    let name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
    let src = filmcraft_codecs::open_bytes(&name, bytes)?;
    Ok(footage_from_info(path, src.info()))
}

pub(crate) fn footage_from_info(path: &str, info: &filmcraft_media::MediaInfo) -> Footage {
    let v = info.video.as_ref();
    let kind = match (info.kind, v) {
        (filmcraft_media::MediaKind::Still, _) => FootageKind::Still,
        (_, Some(_)) => FootageKind::Video,
        (_, None) => FootageKind::Audio,
    };
    let rate = v.map_or(FrameRate::FPS_30, |v| rate_from_fc(v.frame_rate));
    let codec = match (v, info.audio.as_ref()) {
        (Some(v), _) => v.codec.clone(),
        (None, Some(a)) => a.codec.clone(),
        _ => String::new(),
    };
    Footage {
        path: path.to_string(),
        kind,
        width: v.map_or(0, |v| v.width),
        height: v.map_or(0, |v| v.height),
        pixel_aspect: v.map_or(1.0, |v| if v.par.1 > 0 { v.par.0 as f64 / v.par.1 as f64 } else { 1.0 }),
        frame_rate: rate,
        native_rate: None,
        duration: if kind == FootageKind::Still { Tick::ZERO } else { Tick(info.duration.0) },
        has_video: v.is_some(),
        has_audio: info.audio.is_some(),
        alpha: if v.is_some_and(|v| v.has_alpha) { AlphaMode::Straight } else { AlphaMode::Ignore },
        premul_color: [0.0; 3],
        loop_count: 1,
        codec,
        missing: false,
        sequence: Vec::new(),
        color_profile: v.and_then(|v| profile_of(&v.color)),
        ..Default::default()
    }
}

/// The colour profile a video stream declares (H.273 primaries / transfer from the bitstream or
/// container). `None` (interpreted as sRGB) for Rec. 709 primaries with an sRGB curve.
fn profile_of(c: &filmcraft_color::ColorInfo) -> Option<effectcraft_project::ColorSpace> {
    use effectcraft_project::ColorSpace;
    use filmcraft_color::{Primaries, Transfer};
    match (c.primaries, c.transfer) {
        (Primaries::Bt2020, _) => Some(ColorSpace::Rec2020),
        (Primaries::P3D65, _) => Some(ColorSpace::DisplayP3),
        (_, Transfer::Bt709) => Some(ColorSpace::Rec709),
        _ => None,
    }
}

pub(crate) fn still_footage(path: &str, w: u32, h: u32, fmt: image::ImageFormat, has_alpha: bool) -> Footage {
    let codec = fmt.extensions_str().first().map_or("image", |s| s).to_ascii_uppercase();
    // OpenEXR stores premultiplied colour by convention; other formats straight alpha.
    let alpha = match (has_alpha, fmt) {
        (false, _) => AlphaMode::Ignore,
        (true, image::ImageFormat::OpenExr) => AlphaMode::Premultiplied,
        (true, _) => AlphaMode::Straight,
    };
    Footage {
        path: path.to_string(),
        kind: FootageKind::Still,
        width: w,
        height: h,
        pixel_aspect: 1.0,
        frame_rate: DEFAULT_SEQUENCE_RATE,
        native_rate: None,
        duration: Tick::ZERO,
        has_video: true,
        has_audio: false,
        alpha,
        premul_color: [0.0; 3],
        loop_count: 1,
        codec,
        missing: false,
        sequence: Vec::new(),
        color_profile: None,
        ..Default::default()
    }
}

fn probe_still_bytes(path: &str, bytes: &[u8], fmt: image::ImageFormat) -> Result<Footage> {
    use image::ImageDecoder;
    let reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), fmt);
    let (w, h, has_alpha) = match reader.into_decoder() {
        Ok(dec) => (dec.dimensions().0, dec.dimensions().1, dec.color_type().has_alpha()),
        Err(e) => {
            // A multi-layer OpenEXR file without an unnamed RGB layer.
            let img = (fmt == image::ImageFormat::OpenExr).then(|| crate::exr_channels::layered_image(bytes)).flatten();
            let img = img.ok_or_else(|| MediaError::Decode(format!("{path}: {e}")))?;
            (img.width(), img.height(), img.color().has_alpha())
        }
    };
    Ok(still_footage(path, w, h, fmt, has_alpha))
}
