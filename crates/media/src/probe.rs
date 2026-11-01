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
        let header = crate::stream::movie_header(path);
        let turn = header.as_deref().map_or(Orientation::NoTransforms, video_turn);
        turn_footage(&mut f, turn);
        correct_rate(&mut f, header.as_deref().and_then(video_rate));
        trim_to_edits(&mut f, header.as_deref().and_then(video_shown));
        return Ok(f);
    }
    let bytes = crate::read_regular_file(path).map_err(|e| MediaError::Io(format!("{}: {e}", path.display())))?;
    if crate::MODEL_EXTENSIONS.contains(&ext_of(path).as_str()) {
        return probe_model(&path.to_string_lossy(), &bytes, &side_file_reader(path));
    }
    probe_bytes(&path.to_string_lossy(), bytes.into())
}

/// Reads the files a model file names (buffers, textures, MTL files). Like the pool, it follows only
/// local files and files on the model's own network share: a URI is a path stored in the model.
fn side_file_reader(model: &Path) -> impl Fn(&str) -> Option<Vec<u8>> + use<> {
    let model = std::path::absolute(model).unwrap_or_else(|_| model.to_path_buf());
    move |uri| {
        let p = craft_docpath::resolve_absolute(uri, Some(&model)).ok()?;
        crate::read_regular_file(&p).ok()
    }
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
    let rate = video_rate(&bytes);
    let shown = video_shown(&bytes);
    // A WAV's cue points and XMP markers become the footage's markers.
    let markers = crate::wav_markers::read(&bytes);
    let src = filmcraft_codecs::open_bytes(&name, bytes)?;
    let mut f = footage_from_info(path, src.info());
    f.markers = markers;
    turn_footage(&mut f, turn);
    correct_rate(&mut f, rate);
    trim_to_edits(&mut f, shown);
    Ok(f)
}

/// A movie lasts as long as its video edit list shows (#2592): the reader reports the media
/// duration (`mdhd`), which for H.264 with B-frames runs past the last shown frame by the
/// encoder delay the edit list trims (75 frames at 25 fps read as 3.04 s). Never lengthens.
fn trim_to_edits(f: &mut Footage, shown: Option<Tick>) {
    if f.kind == FootageKind::Video
        && let Some(shown) = shown.filter(|s| *s > Tick::ZERO && *s < f.duration)
    {
        f.duration = shown;
    }
}

/// How long the video track's edit list shows its media (`elst` segments, movie timescale):
/// `None` without edits, or with an empty edit (a delayed start keeps the reader's duration).
fn video_shown(bytes: &[u8]) -> Option<Tick> {
    let file = filmcraft_isobmff::open(bytes).ok()?;
    let t = file.tracks.iter().find(|t| t.kind == filmcraft_isobmff::TrackKind::Video && !t.samples.is_empty())?;
    edits_shown(&t.edits, file.timescale)
}

fn edits_shown(edits: &[filmcraft_isobmff::Edit], timescale: u32) -> Option<Tick> {
    if edits.is_empty() || timescale == 0 || edits.iter().any(|e| e.media_time < 0) {
        return None;
    }
    let units = edits.iter().try_fold(0u64, |acc, e| acc.checked_add(e.segment_duration))?;
    Some(Tick::from_units(i64::try_from(units).ok()?, i64::from(timescale)))
}

/// A movie's frame rate as its frame durations average to ([`video_rate`]), where the reader's
/// own estimate misreads it.
fn correct_rate(f: &mut Footage, rate: Option<FrameRate>) {
    if f.kind == FootageKind::Video
        && let Some(rate) = rate
    {
        f.frame_rate = rate;
    }
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

/// The widest or tallest picture decoded: the largest footage frame a project holds
/// ([`effectcraft_project::limits::MAX_SIZE`]); a wider picture could not be shown at its size.
pub(crate) const MAX_SIDE: u32 = effectcraft_project::limits::MAX_SIZE;

/// What one picture may cost to decode: no side over [`MAX_SIDE`], and no more memory than
/// `image`'s default budget (512 MiB) or what a whole animation may hold, whichever is less.
/// Decoders given these refuse a header claiming a huge picture before allocating it (#2130).
/// Stills get their own, larger decode budget ([`still_limits`]); every picture must also pass
/// [`fits_as_image`].
pub(crate) fn decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    let held = u64::try_from(crate::gif_anim::MAX_BYTES).unwrap_or(u64::MAX);
    limits.max_alloc = Some(limits.max_alloc.map_or(held, |m| m.min(held)));
    limits
}

/// [`decode_limits`] for a still: one picture may use what a whole animation may hold
/// ([`crate::gif_anim::MAX_BYTES`]) to decode.
fn still_limits() -> image::Limits {
    let mut limits = decode_limits();
    limits.max_alloc = Some(u64::try_from(crate::gif_anim::MAX_BYTES).unwrap_or(u64::MAX));
    limits
}

/// Whether a `w`×`h` picture fits the budget once it becomes what the compositor holds: an f32
/// RGBA [`effectcraft_raster::Image`] of 16 bytes a pixel, up to [`crate::gif_anim::MAX_BYTES`]
/// (about 67 MP on desktop, as many as a DPX may have). The decoded bytes alone are no measure:
/// an 8-bit grey picture grows sixteenfold.
pub(crate) fn fits_as_image(w: u32, h: u32) -> bool {
    let px = std::mem::size_of::<effectcraft_raster::Px>() as u64;
    u64::from(w) * u64::from(h) * px <= u64::try_from(crate::gif_anim::MAX_BYTES).unwrap_or(u64::MAX)
}

/// Decode a still the way it is meant to be seen: its Exif orientation (a camera held sideways, a
/// mirrored scan) is applied to the pixels, as image viewers do. A file without
/// the tag, or with a broken one, decodes as stored. `fmt`: the format when the file's extension
/// decides it (Targa has no signature to guess from), else guessed from the bytes.
pub(crate) fn load_oriented(bytes: &[u8], fmt: Option<image::ImageFormat>) -> image::ImageResult<image::DynamicImage> {
    use image::ImageDecoder;
    let mut reader = match fmt {
        Some(fmt) => image::ImageReader::with_format(std::io::Cursor::new(bytes), fmt),
        None => image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?,
    };
    let mut limits = still_limits();
    reader.limits(limits.clone());
    let mut dec = reader.into_decoder()?;
    let (w, h) = dec.dimensions();
    if !fits_as_image(w, h) {
        return Err(image::ImageError::Limits(image::error::LimitError::from_kind(image::error::LimitErrorKind::InsufficientMemory)));
    }
    // `from_decoder` allocates the whole picture without asking the limits (unlike
    // `ImageReader::decode`), so reserve it here and leave the decoder the rest.
    limits.reserve(dec.total_bytes())?;
    dec.set_limits(limits)?;
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

/// The frame rate of an MP4/MOV movie's video track when the reader's own estimate, the median
/// frame duration, misreads it. Phones and messaging apps store frame times rounded to whole
/// milliseconds: a 30 fps clip's frames last 33 or 34 ms and a 60 fps clip's 16 or 17 ms, whose
/// median reads as 30.303 or 58.824 fps. `None` for other containers and for movies the median
/// reads right.
fn video_rate(bytes: &[u8]) -> Option<FrameRate> {
    let file = filmcraft_isobmff::open(bytes).ok()?;
    let t = file.tracks.iter().find(|t| t.kind == filmcraft_isobmff::TrackKind::Video && !t.samples.is_empty())?;
    averaged_rate(t.timescale, t.samples.iter().take(240).map(|s| s.duration))
}

/// The standard rate ([`FrameRate::COMMON`]) within 0.5 % of what the frame durations near their
/// median average to (`durations` in `1 / timescale` s); `None` when the median duration is a
/// standard rate already, or when the average is no standard rate (variable or unusual rates keep
/// the reader's estimate).
fn averaged_rate(timescale: u32, durations: impl Iterator<Item = u32>) -> Option<FrameRate> {
    let ts = i128::from(timescale);
    let mut durs: Vec<u64> = durations.filter(|&d| d > 0).map(u64::from).collect();
    durs.sort_unstable();
    let median = *durs.get(durs.len() / 2)?;
    if FrameRate::COMMON.iter().any(|r| ts * i128::from(r.den) == i128::from(median) * i128::from(r.num)) {
        return None;
    }
    // Durations within a quarter of the median (widened, so extreme durations can't overflow).
    let (n, sum) = durs.iter().filter(|&&d| d.abs_diff(median).saturating_mul(4) <= median).fold((0u64, 0u64), |(n, s), &d| (n + 1, s.saturating_add(d)));
    if sum == 0 {
        return None;
    }
    let fps = timescale as f64 * n as f64 / sum as f64;
    FrameRate::COMMON
        .into_iter()
        .map(|r| (r, (fps / r.as_f64() - 1.0).abs()))
        .filter(|&(_, off)| off <= 0.005)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(r, _)| r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2592: an encoder-delayed H.264 track's edit list shows 75 frames at 25 fps (3 s at a
    /// 1000 timescale), though its media runs a frame longer; empty edits and no edits leave it.
    #[test]
    fn the_edit_list_trims_a_movie_to_the_frames_it_shows() {
        let edit = |segment_duration, media_time| filmcraft_isobmff::Edit { segment_duration, media_time, media_rate: 0x0001_0000 };
        let shown = edits_shown(&[edit(3000, 1024)], 1000);
        assert_eq!(shown, Some(Tick::from_units(3, 1)));
        let mut f = Footage { kind: FootageKind::Video, duration: Tick::from_units(76, 25), ..Footage::default() };
        trim_to_edits(&mut f, shown);
        assert_eq!(f.duration, Tick::from_units(75, 25));
        // Never longer than the reader's duration, and only for video.
        trim_to_edits(&mut f, Some(Tick::from_units(4, 1)));
        assert_eq!(f.duration, Tick::from_units(75, 25));
        assert_eq!(edits_shown(&[edit(1000, -1), edit(3000, 0)], 1000), None);
        assert_eq!(edits_shown(&[], 1000), None);
        assert_eq!(edits_shown(&[edit(3000, 0)], 0), None);
    }

    /// Durations (in 1/`timescale` s) of `n` frames at `rate` whose times are rounded to the timescale.
    fn rounded(rate: FrameRate, timescale: i64, n: i64) -> Vec<u32> {
        let at = |k: i64| (2 * k * timescale * rate.den + rate.num) / (2 * rate.num);
        (0..n).map(|k| (at(k + 1) - at(k)) as u32).collect()
    }

    /// A model's side files are read from beside it; a URI to another computer is not followed.
    #[test]
    fn model_side_files_are_not_read_from_other_computers() {
        let dir = std::env::temp_dir().join(format!("ec-probe-side-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("buf.bin"), b"abc").unwrap();
        let read = side_file_reader(&dir.join("model.gltf"));
        assert_eq!(read("buf.bin"), Some(b"abc".to_vec()));
        assert_eq!(read(r"\\192.0.2.1\share\buf.bin"), None);
        assert_eq!(read("file://192.0.2.1/share/buf.bin"), None);
        assert_eq!(read(r"C:\buf.bin"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn millisecond_rounded_frame_times_average_to_their_rate() {
        assert_eq!(rounded(FrameRate::FPS_30, 1000, 6), [33, 34, 33, 33, 34, 33]);
        assert_eq!(averaged_rate(1000, rounded(FrameRate::FPS_30, 1000, 120).into_iter()), Some(FrameRate::FPS_30));
        assert_eq!(averaged_rate(1000, rounded(FrameRate::FPS_60, 1000, 240).into_iter()), Some(FrameRate::FPS_60));
        // NTSC rates stay NTSC, not the round rate next to them.
        for r in [FrameRate::FPS_23_976, FrameRate::FPS_29_97, FrameRate::FPS_59_94] {
            assert_eq!(averaged_rate(1000, rounded(r, 1000, 240).into_iter()), Some(r), "{r:?}");
        }
        // A short clip: its few frames average to the rate as well.
        assert_eq!(averaged_rate(1000, [33, 34, 33].into_iter()), Some(FrameRate::FPS_30));
    }

    #[test]
    fn exact_and_variable_rates_keep_the_readers_estimate() {
        // A median that is a standard rate already reads right.
        assert_eq!(averaged_rate(30000, rounded(FrameRate::FPS_29_97, 30000, 240).into_iter()), None);
        assert_eq!(averaged_rate(90000, [3000; 10].into_iter()), None);
        assert_eq!(averaged_rate(600, [20, 20, 19, 21, 20].into_iter()), None);
        // Durations averaging to no standard rate (22.2 fps).
        assert_eq!(averaged_rate(1000, [40, 45, 50].repeat(20).into_iter()), None);
        // A truly 30.3 fps clip is not pulled to 30 (1 % off).
        assert_eq!(averaged_rate(1000, [33; 30].into_iter()), None);
    }

    #[test]
    fn degenerate_durations_have_no_rate() {
        assert_eq!(averaged_rate(1000, std::iter::empty()), None);
        assert_eq!(averaged_rate(1000, [0, 0, 0].into_iter()), None);
        assert_eq!(averaged_rate(0, [33, 34, 33].into_iter()), None);
        assert_eq!(averaged_rate(u32::MAX, [u32::MAX, u32::MAX - 1, 1, 0].into_iter()), None);
        assert_eq!(averaged_rate(u32::MAX, [u32::MAX; 300].into_iter()), None);
        assert_eq!(video_rate(b"not a movie"), None);
    }

    /// A PNG claiming `w`×`h` 8-bit pixels of PNG colour type `colour` (0 grey, 6 RGBA) with no
    /// pixel data: a few dozen bytes.
    fn png_header_only(w: u32, h: u32, colour: u8) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut c = !0u32;
            for &b in data {
                c ^= u32::from(b);
                for _ in 0..8 {
                    c = if c & 1 == 1 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                }
            }
            !c
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let body = [kind.as_slice(), data].concat();
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
        }
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let ihdr = [w.to_be_bytes().as_slice(), h.to_be_bytes().as_slice(), &[8, colour, 0, 0, 0]].concat();
        chunk(&mut out, b"IHDR", &ihdr);
        // A zlib stream of nothing: the pixel data runs out at once.
        chunk(&mut out, b"IDAT", &[0x78, 0x9C, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn a_still_too_big_for_memory_is_refused_before_allocating() {
        // 20000×20000 RGBA (1.6 GB) declared in a 65-byte file (#2130): the decode budget refuses it
        // up front instead of allocating the picture and failing on the missing pixel data.
        let bytes = png_header_only(20_000, 20_000, 6);
        let err = load_oriented(&bytes, Some(image::ImageFormat::Png)).err();
        assert!(matches!(err, Some(image::ImageError::Limits(_))), "{err:?}");
    }

    #[test]
    fn a_still_too_big_once_widened_to_float_is_refused() {
        // 30000×30000 8-bit grey decodes to 900 MB, inside the budget, but every still becomes an
        // f32 RGBA `Image` of 16 bytes a pixel: 14.4 GB asked of the allocator by a 1 MB file.
        let bytes = png_header_only(MAX_SIDE, MAX_SIDE, 0);
        let err = load_oriented(&bytes, Some(image::ImageFormat::Png)).err();
        assert!(matches!(err, Some(image::ImageError::Limits(_))), "{err:?}");
    }

    #[test]
    fn the_still_budget_is_the_float_image_it_becomes() {
        // The largest grey still whose f32 RGBA `Image` fits the budget gets past the limits (and
        // then fails on the missing pixel data); one more row is refused.
        let w = 8192;
        let h = u32::try_from(crate::gif_anim::MAX_BYTES / 16 / 8192).unwrap();
        let err = load_oriented(&png_header_only(w, h, 0), Some(image::ImageFormat::Png)).err();
        assert!(matches!(err, Some(image::ImageError::Decoding(_))), "{err:?}");
        let err = load_oriented(&png_header_only(w, h + 1, 0), Some(image::ImageFormat::Png)).err();
        assert!(matches!(err, Some(image::ImageError::Limits(_))), "{err:?}");
    }

    #[test]
    fn a_still_wider_than_any_comp_is_refused() {
        let bytes = png_header_only(MAX_SIDE + 1, 1, 6);
        let err = load_oriented(&bytes, None).err();
        assert!(matches!(err, Some(image::ImageError::Limits(_))), "{err:?}");
    }

    #[test]
    fn a_small_still_still_decodes() {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 4])).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        let img = load_oriented(&bytes, None).unwrap();
        assert_eq!((img.width(), img.height()), (3, 2));
    }
}
