//! Animated GIFs: probed as movies and decoded whole into 8-bit frames.
//!
//! A GIF gives every frame its own delay, in hundredths of a second. The footage gets the frame
//! rate that every delay is a whole number of frames at (100 / the greatest common divisor of the
//! delays), so uneven timing is kept exactly; [`Anim::frame_at`] looks the frame up by time.
//! Animated WebP ([`crate::webp_anim`]) shares [`Anim`] and this decode path.

use std::io::Cursor;

use effectcraft_project::{AlphaMode, Footage, FootageKind};
use effectcraft_time::{FrameRate, Tick};
use image::{AnimationDecoder, DynamicImage, codecs::gif::GifDecoder};

use crate::{MediaError, Result};

/// The `Footage::codec` of an animated GIF.
pub(crate) const CODEC: &str = "GIF";

/// Decoded frames of one GIF or animated WebP are kept as 8-bit RGBA, up to this many bytes (less
/// in a browser tab, whose wasm32 heap is 4 GiB at most and shared with everything else).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const MAX_BYTES: usize = 1 << 30;
#[cfg(target_arch = "wasm32")]
pub(crate) const MAX_BYTES: usize = 256 << 20;

/// Frames read from a GIF's or animated WebP's headers when probing.
pub(crate) const MAX_FRAMES: usize = 100_000;

/// Whether `f` is an animated GIF or WebP imported by [`probe`] or [`crate::webp_anim::probe`]:
/// its frames come from an [`Anim`].
pub(crate) fn is_animated(f: &Footage) -> bool {
    f.kind == FootageKind::Video && (f.codec == CODEC || f.codec == crate::webp_anim::CODEC)
}

/// The delay of a frame in hundredths of a second. Browsers show the 0 and 1 that many encoders
/// write for "as fast as possible" as 10, and so do we.
fn delay_cs(raw: u32) -> u32 {
    if raw < 2 { 10 } else { raw }
}

pub(crate) fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Describe an animated GIF as a movie. `None` for a GIF with fewer than two frames (a still) or
/// one whose headers can't be read. A damaged tail keeps the frames before it.
pub(crate) fn probe(path: &str, bytes: &[u8]) -> Option<Footage> {
    let info = craft_gif::scan(bytes, MAX_FRAMES)?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    let delays: Vec<u32> = info.frames.iter().map(|f| delay_cs(u32::from(f.delay_cs))).collect();
    if delays.len() < 2 || width == 0 || height == 0 {
        return None;
    }
    let step = delays.iter().fold(0, |g, &d| gcd(g, d)).max(1);
    let total: i64 = delays.iter().map(|&d| i64::from(d)).sum();
    Some(Footage {
        path: path.to_string(),
        kind: FootageKind::Video,
        width,
        height,
        frame_rate: FrameRate::new(100, i64::from(step)),
        duration: Tick::from_units(total, 100),
        has_video: true,
        has_audio: false,
        alpha: AlphaMode::Straight,
        codec: CODEC.to_string(),
        ..Default::default()
    })
}

/// Every frame of a GIF or animated WebP, composited onto the full canvas.
pub(crate) struct Anim {
    frames: Vec<DynamicImage>,
    /// When each frame starts (ascending, one per frame).
    starts: Vec<Tick>,
}

impl Anim {
    /// Frames and the times they start at (one each, ascending); `None` when there are none.
    pub(crate) fn new(frames: Vec<DynamicImage>, starts: Vec<Tick>) -> Option<Anim> {
        (!frames.is_empty() && frames.len() == starts.len()).then_some(Anim { frames, starts })
    }

    /// Decode all frames of a GIF or animated WebP. A damaged tail keeps the frames before it.
    pub(crate) fn load(bytes: &[u8]) -> Result<Anim> {
        if crate::webp_anim::is_webp(bytes) {
            return crate::webp_anim::load(bytes);
        }
        let mut dec = GifDecoder::new(Cursor::new(bytes)).map_err(|e| MediaError::Decode(format!("GIF: {e}")))?;
        // A frame may be far larger than the canvas, and the decoder allocates its buffer before
        // decoding it: no single buffer may pass what all the frames may hold.
        let mut limits = image::Limits::no_limits();
        limits.max_alloc = Some(MAX_BYTES as u64);
        image::ImageDecoder::set_limits(&mut dec, limits).map_err(|e| MediaError::Decode(format!("GIF: {e}")))?;
        // The decoder allocates the full logical canvas for the first frame, however small the
        // frames themselves are: check it before decoding any.
        let (w, h) = image::ImageDecoder::dimensions(&dec);
        if u64::from(w) * u64::from(h) * 4 > MAX_BYTES as u64 {
            return Err(MediaError::Unsupported(format!("GIF canvas of {w}×{h} is too large to hold in memory")));
        }
        let (mut frames, mut starts) = (Vec::new(), Vec::new());
        let (mut at, mut size) = (0i64, 0usize);
        for frame in dec.into_frames() {
            let frame = match frame {
                Ok(f) => f,
                Err(e) if frames.is_empty() => return Err(MediaError::Decode(format!("GIF: {e}"))),
                Err(e) => {
                    log::warn!("media: GIF stops after {} frames: {e}", frames.len());
                    break;
                }
            };
            let (numer, denom) = frame.delay().numer_denom_ms();
            let cs = delay_cs(numer.checked_div(denom).unwrap_or(0) / 10);
            let buf = frame.into_buffer();
            size = size.saturating_add(buf.as_raw().len());
            if size > MAX_BYTES {
                return Err(MediaError::Unsupported("GIF is too large to hold in memory".into()));
            }
            starts.push(Tick::from_units(at, 100));
            at = at.saturating_add(i64::from(cs));
            frames.push(DynamicImage::ImageRgba8(buf));
        }
        Anim::new(frames, starts).ok_or_else(|| MediaError::Decode("GIF: no frames".into()))
    }

    /// The frame shown at source time `t` (the first one before the start, the last one past the
    /// end).
    pub(crate) fn frame_at(&self, t: Tick) -> Option<&DynamicImage> {
        let i = self.starts.partition_point(|&s| s <= t).saturating_sub(1);
        self.frames.get(i).or(self.frames.last())
    }
}

#[cfg(test)]
mod tests {
    use effectcraft_project::{Footage, FootageKind};
    use effectcraft_time::{FrameRate, Tick};

    use crate::{MediaPool, probe_bytes};

    /// A 2×2 GIF with one solid-colour frame per `(rgb, delay)`.
    fn gif(frames: &[([u8; 3], u16)]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = gif::Encoder::new(&mut out, 2, 2, &[]).unwrap();
            for (rgb, delay) in frames {
                let mut f = gif::Frame::from_rgb(2, 2, &rgb.repeat(4));
                f.delay = *delay;
                enc.write_frame(&f).unwrap();
            }
        }
        out
    }

    const RED: [u8; 3] = [255, 0, 0];
    const GREEN: [u8; 3] = [0, 255, 0];
    const BLUE: [u8; 3] = [0, 0, 255];

    fn colour_at(pool: &MediaPool, f: &Footage, seconds: f64) -> [f32; 3] {
        let img = pool.frame_at(f, Tick::from_seconds_f64(seconds)).unwrap();
        let p = img.data[0];
        [p[0], p[1], p[2]]
    }

    #[test]
    fn an_animated_gif_is_a_movie_with_the_files_timing() {
        let bytes = gif(&[(RED, 5), (GREEN, 10), (BLUE, 5)]);
        let f = probe_bytes("a.gif", bytes.clone().into()).unwrap();
        assert_eq!(f.kind, FootageKind::Video);
        assert!(f.has_video && !f.has_audio);
        assert_eq!((f.width, f.height), (2, 2));
        // Delays of 5, 10 and 5 hundredths: 20 frames a second, 0.2 s in all.
        assert_eq!(f.frame_rate, FrameRate::new(20, 1));
        assert_eq!(f.duration, Tick::from_units(20, 100));

        let pool = MediaPool::new();
        pool.add_bytes("a.gif", bytes.into());
        assert_eq!(colour_at(&pool, &f, 0.01), [1.0, 0.0, 0.0]);
        assert_eq!(colour_at(&pool, &f, 0.06), [0.0, 1.0, 0.0]);
        assert_eq!(colour_at(&pool, &f, 0.14), [0.0, 1.0, 0.0]);
        assert_eq!(colour_at(&pool, &f, 0.17), [0.0, 0.0, 1.0]);
        // Past the end it holds the last frame.
        assert_eq!(colour_at(&pool, &f, 5.0), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_frame_far_larger_than_the_canvas_is_refused_before_it_is_allocated() {
        // A 1×1 canvas and a 20000×20000 frame (1.6 GB of RGBA); the data itself is not there.
        let mut bytes = b"GIF89a".to_vec();
        // (a two-entry global colour table, so the frame gets as far as its buffer)
        bytes.extend([1, 0, 1, 0, 0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
        bytes.extend([0x2c, 0, 0, 0, 0, 0x20, 0x4e, 0x20, 0x4e, 0, 2, 0]);
        let e = crate::gif_anim::Anim::load(&bytes).err().expect("refused").to_string();
        assert!(e.to_lowercase().contains("memory"), "{e}");
    }

    #[test]
    fn a_zero_delay_is_ten_hundredths_like_in_browsers() {
        let f = probe_bytes("a.gif", gif(&[(RED, 0), (GREEN, 1)]).into()).unwrap();
        assert_eq!(f.frame_rate, FrameRate::new(10, 1));
        assert_eq!(f.duration, Tick::from_units(20, 100));
    }

    #[test]
    fn a_single_frame_gif_stays_a_still() {
        let f = probe_bytes("a.gif", gif(&[(RED, 10)]).into()).unwrap();
        assert_eq!(f.kind, FootageKind::Still);
    }

    #[test]
    fn a_damaged_gif_is_an_error_not_a_panic() {
        let mut bytes = gif(&[(RED, 5), (GREEN, 5)]);
        bytes.truncate(20);
        let pool = MediaPool::new();
        pool.add_bytes("a.gif", bytes.clone().into());
        let f = Footage {
            path: "a.gif".into(),
            kind: FootageKind::Video,
            codec: "GIF".into(),
            has_video: true,
            width: 2,
            height: 2,
            duration: Tick::from_units(10, 100),
            frame_rate: FrameRate::new(20, 1),
            ..Default::default()
        };
        assert!(pool.frame_at(&f, Tick::ZERO).is_err());
        let _ = probe_bytes("a.gif", bytes.into());
    }

    #[test]
    fn a_huge_canvas_is_refused_before_decoding() {
        // A 65535×65535 logical screen (16 GiB of RGBA) holding one 1×1 frame.
        let mut out = Vec::new();
        {
            let mut enc = gif::Encoder::new(&mut out, u16::MAX, u16::MAX, &[]).unwrap();
            for _ in 0..2 {
                enc.write_frame(&gif::Frame::from_rgb(1, 1, &RED)).unwrap();
            }
        }
        let e = super::Anim::load(&out).err().unwrap();
        assert!(e.to_string().contains("too large"), "{e}");
    }
}
