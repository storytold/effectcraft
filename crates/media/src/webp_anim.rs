//! Animated WebP: probed as movies and decoded whole into 8-bit frames, like animated GIF
//! ([`crate::gif_anim`], whose [`Anim`] holds the frames).
//!
//! An animated WebP is a RIFF file with a `VP8X` header whose animation flag is set and one `ANMF`
//! chunk per frame, each with its own duration in milliseconds. Evenly spaced frames get the rate
//! they are spaced at. Encoders round the frame times of a rate like 24 or 29.97 frames a second to
//! whole milliseconds (41, 42, 42, 41… ms), so delays that differ by at most one millisecond are
//! read as the standard rate whose frame times they round to; any other timing gets the rate that
//! every delay is a whole number of frames at (1000 / the greatest common divisor of the delays),
//! so it is kept exactly. A still WebP (no animation flag, or a single frame) stays a still.

use std::io::Cursor;

use effectcraft_project::{AlphaMode, Footage, FootageKind};
use effectcraft_time::{FrameRate, Tick};
use image::{AnimationDecoder, DynamicImage, codecs::webp::WebPDecoder};

use crate::gif_anim::{Anim, MAX_BYTES, MAX_FRAMES, gcd};
use crate::{MediaError, Result};

/// The `Footage::codec` of an animated WebP.
pub(crate) const CODEC: &str = "WebP";

/// Whether `bytes` start like a WebP file.
pub(crate) fn is_webp(bytes: &[u8]) -> bool {
    bytes.get(0..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP")
}

/// What the chunk headers of an animated WebP say.
struct Header {
    width: u32,
    height: u32,
    alpha: bool,
    /// Each frame's delay in milliseconds, as stored.
    delays: Vec<u32>,
}

fn u24(b: &[u8]) -> Option<u32> {
    match b {
        [a, b, c, ..] => Some(u32::from(*a) | u32::from(*b) << 8 | u32::from(*c) << 16),
        _ => None,
    }
}

/// Read the `VP8X` header and the `ANMF` frame headers. `None` when the file isn't an animated
/// WebP. A truncated tail keeps the frames before it.
fn header(bytes: &[u8]) -> Option<Header> {
    if !is_webp(bytes) {
        return None;
    }
    let riff = bytes.get(4..8).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes)?;
    let end = usize::try_from(riff).ok()?.saturating_add(8).min(bytes.len());
    let (mut at, mut canvas, mut delays) = (12usize, None, Vec::new());
    while delays.len() < MAX_FRAMES {
        let Some(head) = bytes.get(at..at.saturating_add(8)).filter(|_| at.saturating_add(8) <= end) else { break };
        let size = head.get(4..8).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes)?;
        let size = usize::try_from(size).ok()?;
        let body_end = at.saturating_add(8).saturating_add(size);
        let Some(body) = bytes.get(at.saturating_add(8)..body_end).filter(|_| body_end <= end) else { break };
        match head.get(0..4) {
            Some(b"VP8X") => {
                let flags = *body.first()?;
                if flags & 0x02 == 0 {
                    // No animation flag: a still.
                    return None;
                }
                let w = u24(body.get(4..)?)?.saturating_add(1);
                let h = u24(body.get(7..)?)?.saturating_add(1);
                canvas = Some((w, h, flags & 0x10 != 0));
            }
            Some(b"ANMF") if body.len() >= 16 => delays.push(u24(body.get(12..)?)?),
            _ => {}
        }
        at = body_end.saturating_add(size & 1);
    }
    let (width, height, alpha) = canvas?;
    Some(Header { width, height, alpha, delays })
}

/// The delay of a frame in milliseconds. Browsers show delays of 10 ms or less (that encoders
/// write for "as fast as possible") as 100 ms, and so do we, as for GIF.
fn delay_ms(raw: u32) -> u32 {
    if raw <= 10 { 100 } else { raw }
}

/// The footage rate, when each frame starts, and the duration, for frames of these delays (ms).
struct Timing {
    rate: FrameRate,
    starts: Vec<Tick>,
    duration: Tick,
}

fn timing(delays: &[u32]) -> Timing {
    let delays: Vec<u32> = delays.iter().map(|&d| delay_ms(d)).collect();
    let mut starts_ms = Vec::with_capacity(delays.len());
    let mut total = 0i64;
    for &d in &delays {
        starts_ms.push(total);
        total = total.saturating_add(i64::from(d));
    }
    let n = i64::try_from(delays.len()).unwrap_or(i64::MAX);
    let (min, max) = (delays.iter().copied().min().unwrap_or(0), delays.iter().copied().max().unwrap_or(0));
    // Frame times of a standard rate rounded to milliseconds: that rate if every frame starts (and
    // the last one ends) within a millisecond of it.
    let fps = if total > 0 { n.saturating_mul(1000).saturating_add(total / 2) / total } else { 0 };
    if max == min.saturating_add(1) && fps > 0 {
        for rate in [FrameRate::new(fps, 1), FrameRate::new(fps.saturating_mul(1000), 1001)] {
            // |start_ms - k * 1000 * den / num| <= 1, in integers.
            let (num, den) = (i128::from(rate.num), i128::from(rate.den));
            let fits = starts_ms.iter().copied().chain([total]).enumerate().all(|(k, ms)| {
                let k = i128::try_from(k).unwrap_or(i128::MAX);
                (i128::from(ms) * num - k * 1000 * den).abs() <= num
            });
            if fits {
                let starts = (0..n).map(|k| rate.tick_of(k)).collect();
                return Timing { rate, starts, duration: rate.tick_of(n) };
            }
        }
    }
    let step = delays.iter().fold(0, |g, &d| gcd(g, d)).max(1);
    Timing {
        rate: FrameRate::new(1000, i64::from(step)),
        starts: starts_ms.iter().map(|&ms| Tick::from_units(ms, 1000)).collect(),
        duration: Tick::from_units(total, 1000),
    }
}

/// Describe an animated WebP as a movie. `None` for a still WebP (no animation, or one frame) or
/// one whose headers can't be read.
pub(crate) fn probe(path: &str, bytes: &[u8]) -> Option<Footage> {
    let h = header(bytes)?;
    if h.delays.len() < 2 || h.width == 0 || h.height == 0 {
        return None;
    }
    let t = timing(&h.delays);
    Some(Footage {
        path: path.to_string(),
        kind: FootageKind::Video,
        width: h.width,
        height: h.height,
        frame_rate: t.rate,
        duration: t.duration,
        has_video: true,
        has_audio: false,
        alpha: if h.alpha { AlphaMode::Straight } else { AlphaMode::Ignore },
        codec: CODEC.to_string(),
        ..Default::default()
    })
}

/// Decode every frame of an animated WebP, composited onto the full canvas. A damaged tail keeps
/// the frames before it; an animation too large to hold in memory is refused before decoding.
pub(crate) fn load(bytes: &[u8]) -> Result<Anim> {
    let h = header(bytes).ok_or_else(|| MediaError::Decode("WebP: not an animated WebP".into()))?;
    let frame_bytes = usize::try_from(u64::from(h.width) * u64::from(h.height) * 4).unwrap_or(usize::MAX);
    if frame_bytes.saturating_mul(h.delays.len()) > MAX_BYTES {
        return Err(MediaError::Unsupported(format!("animated WebP of {} frames of {}×{} is too large to hold in memory", h.delays.len(), h.width, h.height)));
    }
    let dec = WebPDecoder::new(Cursor::new(bytes)).map_err(|e| MediaError::Decode(format!("WebP: {e}")))?;
    let (mut frames, mut delays, mut size) = (Vec::new(), Vec::new(), 0usize);
    for frame in dec.into_frames() {
        let frame = match frame {
            Ok(f) => f,
            Err(e) if frames.is_empty() => return Err(MediaError::Decode(format!("WebP: {e}"))),
            Err(e) => {
                log::warn!("media: WebP stops after {} frames: {e}", frames.len());
                break;
            }
        };
        let (numer, denom) = frame.delay().numer_denom_ms();
        delays.push(numer.checked_div(denom).unwrap_or(0));
        let buf = frame.into_buffer();
        size = size.saturating_add(buf.as_raw().len());
        if size > MAX_BYTES {
            return Err(MediaError::Unsupported("animated WebP is too large to hold in memory".into()));
        }
        frames.push(DynamicImage::ImageRgba8(buf));
    }
    let starts = timing(&delays).starts;
    Anim::new(frames, starts).ok_or_else(|| MediaError::Decode("WebP: no frames".into()))
}

#[cfg(test)]
mod tests {
    use effectcraft_project::{AlphaMode, Footage, FootageKind};
    use effectcraft_time::{FrameRate, Tick};
    use image::{ExtendedColorType, ImageEncoder, codecs::webp::WebPEncoder};

    use super::timing;
    use crate::{MediaPool, probe_bytes};

    fn chunk(fourcc: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = fourcc.to_vec();
        out.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
        out.extend_from_slice(body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn u24(v: u32) -> [u8; 3] {
        let b = v.to_le_bytes();
        [b[0], b[1], b[2]]
    }

    /// The `VP8L` chunk of a lossless still WebP of a solid RGBA colour.
    fn vp8l(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut still = Vec::new();
        let px = rgba.repeat((w * h) as usize);
        WebPEncoder::new_lossless(&mut still).write_image(&px, w, h, ExtendedColorType::Rgba8).unwrap();
        let at = still.windows(4).position(|c| c == b"VP8L").unwrap();
        still[at..].to_vec()
    }

    /// A `w`×`h` animated WebP with alpha, one solid full-canvas frame per `(rgba, delay ms)`.
    fn webp(w: u32, h: u32, frames: &[([u8; 4], u32)]) -> Vec<u8> {
        let mut vp8x = vec![0x12, 0, 0, 0];
        vp8x.extend_from_slice(&u24(w - 1));
        vp8x.extend_from_slice(&u24(h - 1));
        let mut body = b"WEBP".to_vec();
        body.extend(chunk(b"VP8X", &vp8x));
        body.extend(chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]));
        for (rgba, delay) in frames {
            let mut anmf = Vec::new();
            anmf.extend_from_slice(&[0; 6]);
            anmf.extend_from_slice(&u24(w - 1));
            anmf.extend_from_slice(&u24(h - 1));
            anmf.extend_from_slice(&u24(*delay));
            anmf.push(0b10); // no blending, no disposal
            anmf.extend(vp8l(w, h, *rgba));
            body.extend(chunk(b"ANMF", &anmf));
        }
        chunk(b"RIFF", &body)
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 128];

    fn pixel_at(pool: &MediaPool, f: &Footage, seconds: f64) -> [f32; 4] {
        pool.frame_at(f, Tick::from_seconds_f64(seconds)).unwrap().data[0]
    }

    #[test]
    fn an_animated_webp_is_a_movie_with_its_frames() {
        let bytes = webp(4, 2, &[(RED, 50), (GREEN, 100), (BLUE, 50)]);
        let f = probe_bytes("a.webp", bytes.clone().into()).unwrap();
        assert_eq!(f.kind, FootageKind::Video);
        assert!(f.has_video && !f.has_audio);
        assert_eq!((f.width, f.height), (4, 2));
        assert_eq!(f.alpha, AlphaMode::Straight);
        // Delays of 50, 100 and 50 ms: 20 frames a second, 0.2 s in all.
        assert_eq!(f.frame_rate, FrameRate::new(20, 1));
        assert_eq!(f.duration, Tick::from_units(200, 1000));

        let pool = MediaPool::new();
        pool.add_bytes("a.webp", bytes.into());
        let first = pixel_at(&pool, &f, 0.01);
        assert_eq!(&first[..3], &[1.0, 0.0, 0.0]);
        let second = pixel_at(&pool, &f, 0.06);
        assert_ne!(first, second, "a later frame differs from the first");
        assert_eq!(&second[..3], &[0.0, 1.0, 0.0]);
        assert_eq!(&pixel_at(&pool, &f, 0.14)[..3], &[0.0, 1.0, 0.0]);
        // The last frame keeps its alpha, and holds past the end.
        let last = pixel_at(&pool, &f, 0.17);
        assert!((last[3] - 128.0 / 255.0).abs() < 1e-3, "{last:?}");
        assert_eq!(pixel_at(&pool, &f, 5.0), last);
    }

    #[test]
    fn delays_rounded_to_milliseconds_read_as_the_standard_rate() {
        // 24 frames a second rounded to whole milliseconds, as ffmpeg writes them.
        let delays: Vec<u32> = (0..24).map(|k| (((k + 1) * 1000 + 12) / 24 - (k * 1000 + 12) / 24) as u32).collect();
        assert!(delays.iter().all(|&d| d == 41 || d == 42));
        let t = timing(&delays);
        assert_eq!(t.rate, FrameRate::new(24, 1));
        assert_eq!(t.duration, Tick::from_seconds_f64(1.0));
        assert_eq!(t.starts[5], FrameRate::new(24, 1).tick_of(5));
        // 29.97: 33 and 34 ms over a long run.
        let ntsc = FrameRate::new(30_000, 1001);
        let delays: Vec<u32> = (0..300).map(|k| (((k + 1) * 1_001_000 / 30_000) - (k * 1_001_000 / 30_000)) as u32).collect();
        assert_eq!(timing(&delays).rate, ntsc);
        // Uneven timing is kept exactly.
        assert_eq!(timing(&[40, 80, 40]).rate, FrameRate::new(25, 1));
        assert_eq!(timing(&[33, 34, 50]).rate, FrameRate::new(1000, 1));
        // As fast as possible is 100 ms, as in browsers.
        assert_eq!(timing(&[0, 10]).duration, Tick::from_units(200, 1000));
    }

    #[test]
    fn a_still_webp_stays_a_still() {
        let mut still = Vec::new();
        WebPEncoder::new_lossless(&mut still).write_image(&RED.repeat(4), 2, 2, ExtendedColorType::Rgba8).unwrap();
        let f = probe_bytes("s.webp", still.into()).unwrap();
        assert_eq!(f.kind, FootageKind::Still);
        // So does an animation of one frame.
        let f = probe_bytes("one.webp", webp(2, 2, &[(RED, 100)]).into()).unwrap();
        assert_eq!(f.kind, FootageKind::Still);
    }

    #[test]
    fn a_truncated_animated_webp_is_an_error_not_a_panic() {
        let full = webp(4, 2, &[(RED, 50), (GREEN, 50), (BLUE, 50)]);
        let f = probe_bytes("a.webp", full.clone().into()).unwrap();
        for len in [0, 11, 20, 40, full.len() / 2, full.len() - 3] {
            let bytes = &full[..len];
            let _ = probe_bytes("t.webp", bytes.to_vec().into());
            let pool = MediaPool::new();
            pool.add_bytes("a.webp", bytes.to_vec().into());
            // Cut before the second frame's data: no frames at all, or the ones before the cut.
            let r = pool.frame_at(&f, Tick::ZERO);
            if len < full.len() / 2 {
                assert!(r.is_err(), "{len} bytes");
            }
        }
    }

    #[test]
    fn a_huge_animation_is_refused_before_decoding() {
        // 16384×16384 canvas, 10 frames: 10 GiB of RGBA. The frame data is a tiny 1×1 picture;
        // the size check comes first.
        let mut bytes = webp(1, 1, &[(RED, 50); 10]);
        bytes[24..27].copy_from_slice(&u24(16383));
        bytes[27..30].copy_from_slice(&u24(16383));
        let e = super::load(&bytes).err().unwrap();
        assert!(e.to_string().contains("too large"), "{e}");
    }
}
