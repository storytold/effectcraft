//! Probing: describe a file as an [`effectcraft_project::Footage`].

use std::path::Path;
use std::sync::Arc;

use effectcraft_project::{AlphaMode, Footage, FootageKind};
use effectcraft_time::{FrameRate, Tick};
use image::metadata::Orientation;

use crate::{MediaError, Result, ext_of, rate_from_fc};

/// Frame rate given to image sequences on import (After Effects' default preference).
pub const DEFAULT_SEQUENCE_RATE: FrameRate = FrameRate::FPS_30;

/// Probe one file. Image sequences are made by the engine, which groups numbered stills
/// (`effectcraft_engine::sequence`) and probes their first frame here.
pub fn probe(path: impl AsRef<Path>) -> Result<Footage> {
    let path = path.as_ref();
    // A movie is read in place (its index), not loaded whole.
    if let Some(src) = crate::stream::probe_source(path) {
        let mut f = footage_from_info(&path.to_string_lossy(), src?.info());
        let turn = crate::stream::movie_header(path).map_or(Orientation::NoTransforms, |h| video_turn(&h));
        turn_footage(&mut f, turn);
        return Ok(f);
    }
    let bytes = std::fs::read(path).map_err(|e| MediaError::Io(format!("{}: {e}", path.display())))?;
    if crate::MODEL_EXTENSIONS.contains(&ext_of(path).as_str()) {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        return probe_model(&path.to_string_lossy(), &bytes, &|uri| std::fs::read(dir.join(uri)).ok());
    }
    probe_bytes(&path.to_string_lossy(), bytes.into())
}

/// Probe a 3D model (glTF 2.0 / OBJ / STL / FBX): a [`FootageKind::Model`] item whose duration is its
/// longest animation. `resolve` reads sibling resources.
pub fn probe_model(path: &str, bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Footage> {
    let m = effectcraft_model::load(path, bytes, resolve).map_err(|e| MediaError::Decode(format!("{path}: {e}")))?;
    let s = m.summary();
    let dur = s.animations.iter().map(|a| a.1).fold(0.0f64, f64::max);
    let codec = match effectcraft_model::format_of(path) {
        Some("obj") => "OBJ",
        Some("stl") => "STL",
        Some("fbx") => "FBX",
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
    // DPX / Cineon film scans (our own reader; `image` has none).
    if let Some(f) = crate::dpx::probe(path, &bytes)? {
        return Ok(f);
    }
    if let Some(f) = crate::layered::probe(path, &bytes)? {
        return Ok(f);
    }
    if crate::MODEL_EXTENSIONS.contains(&ext_of(Path::new(path)).as_str()) {
        return probe_model(path, &bytes, &|_| None);
    }
    if let Some(fmt) = crate::still_format(path, &bytes) {
        // An animated GIF is a movie; one with a single frame is a still.
        if fmt == image::ImageFormat::Gif
            && let Some(f) = crate::gif_anim::probe(path, &bytes)
        {
            return Ok(f);
        }
        // So is an animated WebP; a still WebP stays a still.
        if fmt == image::ImageFormat::WebP
            && let Some(f) = crate::webp_anim::probe(path, &bytes)
        {
            return Ok(f);
        }
        return probe_still_bytes(path, &bytes, fmt);
    }
    let name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
    let turn = video_turn(&bytes);
    let src = filmcraft_codecs::open_bytes(&name, bytes)?;
    let mut f = footage_from_info(path, src.info());
    turn_footage(&mut f, turn);
    Ok(f)
}

/// The size as displayed: a movie shot with the camera on its side trades width and height.
fn turn_footage(f: &mut Footage, turn: Orientation) {
    if swaps_sides(turn) {
        std::mem::swap(&mut f.width, &mut f.height);
        if f.pixel_aspect > 0.0 {
            f.pixel_aspect = 1.0 / f.pixel_aspect;
        }
    }
}

pub(crate) fn footage_from_info(path: &str, info: &filmcraft_media::MediaInfo) -> Footage {
    let v = info.video.as_ref();
    let kind = match (info.kind, v) {
        (filmcraft_media::MediaKind::Still, _) => FootageKind::Still,
        (_, Some(_)) => FootageKind::Video,
        (_, None) => FootageKind::Audio,
    };
    let rate = v.map_or(FrameRate::FPS_30, |v| rate_from_fc(v.frame_rate));
    let codec = match (v, info.audio()) {
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
        has_audio: info.has_audio(),
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
    if fmt == image::ImageFormat::OpenExr
        && let Some(why) = crate::exr_channels::unsupported_compression(bytes)
    {
        return Err(MediaError::Decode(format!("{path}: {why}")));
    }
    let reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), fmt);
    let (w, h, has_alpha) = match reader.into_decoder() {
        Ok(mut dec) => {
            // The size as displayed: an Exif orientation of a quarter turn swaps the sides.
            let (w, h) = dec.dimensions();
            let (w, h) = if swaps_sides(dec.orientation().unwrap_or(Orientation::NoTransforms)) { (h, w) } else { (w, h) };
            (w, h, dec.color_type().has_alpha())
        }
        Err(e) => {
            // A multi-layer OpenEXR file without an unnamed RGB layer.
            let img = (fmt == image::ImageFormat::OpenExr).then(|| crate::exr_channels::layered_image(bytes)).flatten();
            let img = img.ok_or_else(|| MediaError::Decode(format!("{path}: {e}")))?;
            (img.width(), img.height(), img.color().has_alpha())
        }
    };
    Ok(still_footage(path, w, h, fmt, has_alpha))
}

/// Whether an Exif orientation turns the picture a quarter turn (its width and height trade places).
pub(crate) fn swaps_sides(o: Orientation) -> bool {
    matches!(o, Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Rotate90FlipH | Orientation::Rotate270FlipH)
}

/// Decode a still the way it is meant to be seen: its Exif orientation (a camera held sideways, a
/// mirrored scan) is applied to the pixels, as image viewers do. A file without
/// the tag, or with a broken one, decodes as stored. `fmt`: the format when the file's extension
/// decides it (Targa has no signature to guess from), else guessed from the bytes.
pub(crate) fn load_oriented(bytes: &[u8], fmt: Option<image::ImageFormat>) -> image::ImageResult<image::DynamicImage> {
    use image::ImageDecoder;
    let reader = match fmt {
        Some(fmt) => image::ImageReader::with_format(std::io::Cursor::new(bytes), fmt),
        None => image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?,
    };
    let mut dec = reader.into_decoder()?;
    let orientation = dec.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(dec)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// The rotation an MP4/MOV movie is shown with (a phone held upright records sideways frames and
/// says so in its video track's header): the quarter turn nearest the track's rotation. Only the
/// rotation counts; any mirroring, scaling or offset in the header is ignored, and other
/// containers have none. FilmCraft turns the frames of a pure 0/90/180/270° header itself, so this
/// is only the quarter turn nearest to a header it does not turn.
pub(crate) fn video_turn(bytes: &[u8]) -> Orientation {
    let Ok(file) = filmcraft_isobmff::open(bytes) else { return Orientation::NoTransforms };
    let Some(track) = file.tracks.iter().find(|t| t.kind == filmcraft_isobmff::TrackKind::Video) else {
        return Orientation::NoTransforms;
    };
    // A pure rotation is already applied by FilmCraft's decoder (frames and size come out
    // upright); only the headers it leaves as stored (mirrored, scaled, sheared) are turned here.
    if track.display_rotation().is_some() {
        return Orientation::NoTransforms;
    }
    let m = track.matrix;
    // [a b u; c d v; x y w]: (a, b) is where the picture's x axis points.
    let (a, b) = (m[0] as i64, m[1] as i64);
    match (a.abs() >= b.abs(), a >= 0, b >= 0) {
        (true, true, _) => Orientation::NoTransforms,
        (true, false, _) => Orientation::Rotate180,
        (false, _, true) => Orientation::Rotate90,
        (false, _, false) => Orientation::Rotate270,
    }
}
