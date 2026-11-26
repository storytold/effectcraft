//! Standalone FLAC through the pinned shared decoder. The source retains bounded decoded PCM.

use std::sync::Arc;

use craft_audio::{AudioBuffer, flac};
use craft_frame::VideoFrame;
use craft_media_source::{AudioStreamInfo, DEFAULT_MAX_MEDIA_BYTES, FrameRequest, MediaError, MediaInfo, MediaKind, MediaSource, Result, SharedSource};
#[cfg(any(unix, windows))]
use craft_media_source::{SharedBytes, read_range};
use craft_time::Tick;

use super::{resample_audio_with_phase, resampling_origin};

/// Preserve the previous reader's ceiling for an individual PCM request.
const MAX_AUDIO_REQUEST_BYTES: usize = 512 << 20;

fn looks_like_flac(name: &str, head: &[u8]) -> bool {
    head.starts_with(b"fLaC")
        || head.starts_with(b"ID3") && std::path::Path::new(name).extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("flac"))
}

#[cfg(any(unix, windows))]
pub(super) fn open_reader(name: &str, head: &[u8], source: &SharedBytes) -> Option<SharedSource> {
    if !looks_like_flac(name, head) {
        return None;
    }
    let bytes = read_range(&**source, 0, usize::try_from(source.len()).ok()?).ok()?;
    open_bytes(name, &bytes)
}

pub(super) fn open_bytes(name: &str, bytes: &[u8]) -> Option<SharedSource> {
    if !looks_like_flac(name, bytes) || bytes.len() > DEFAULT_MAX_MEDIA_BYTES {
        return None;
    }
    // Keep both the compressed and decoded inputs within the shared media buffer budget.
    // Declining the file leaves the prior reader available, including its broader recovery.
    let max_values = (DEFAULT_MAX_MEDIA_BYTES / std::mem::size_of::<i32>()) as u64;
    let decoded = flac::decode_with_limit(bytes, max_values).ok()?;
    let rate = decoded.info.sample_rate;
    let frames = decoded.channels.first()?.len();
    if frames == 0 || rate == 0 {
        return None;
    }

    let scale = 2.0_f32.powi(1 - i32::from(decoded.info.bits_per_sample));
    let samples = decoded.channels.into_iter().map(|plane| plane.into_iter().map(|sample| sample as f32 * scale).collect()).collect();
    let info = MediaInfo {
        name: name.into(),
        kind: MediaKind::AudioOnly,
        duration: Tick::from_units(i64::try_from(frames).ok()?, i64::from(rate)),
        video: None,
        audio_streams: vec![AudioStreamInfo {
            sample_rate: rate,
            channels: u32::from(decoded.info.channels),
            codec: "FLAC".into(),
            bits_per_sample: Some(u32::from(decoded.info.bits_per_sample)),
        }],
        container: "FLAC".into(),
        start_timecode: None,
        file_size: Some(bytes.len() as u64),
    };
    Some(Arc::new(FlacSource { info, rate, samples }))
}

struct FlacSource {
    info: MediaInfo,
    rate: u32,
    samples: Vec<Vec<f32>>,
}

impl MediaSource for FlacSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }

    fn video_frame(&self, _req: FrameRequest) -> Result<Arc<VideoFrame>> {
        Err(MediaError::NoStream("video"))
    }

    fn audio(&self, start: i64, frames: usize, sample_rate: u32) -> Result<AudioBuffer> {
        if sample_rate == 0 {
            return Err(MediaError::Decode("audio sample rate must be nonzero".into()));
        }
        if frames.checked_mul(self.samples.len()).and_then(|n| n.checked_mul(std::mem::size_of::<f32>())).is_none_or(|n| n > MAX_AUDIO_REQUEST_BYTES) {
            return Err(MediaError::Decode("FLAC audio request exceeds the PCM buffer limit".into()));
        }
        let prefix = if start < 0 { usize::try_from(start.unsigned_abs()).unwrap_or(frames).min(frames) } else { 0 };
        let Some((origin, phase)) = resampling_origin(start.max(0), self.rate, sample_rate) else {
            return Ok(AudioBuffer::new(sample_rate, self.samples.len() as u16, frames));
        };
        let origin = usize::try_from(origin).unwrap_or(usize::MAX);
        let tail: Vec<&[f32]> = self.samples.iter().map(|plane| plane.get(origin..).unwrap_or_default()).collect();
        // Keep the true end of the PCM tail so interpolation holds the final sample until
        // the clip ends. Padding this input tail would fade the final sample into silence.
        let mut channels = resample_audio_with_phase(&tail, self.rate, sample_rate, frames - prefix, phase, 0);
        if prefix != 0 {
            for channel in &mut channels {
                channel.resize(frames, 0.0);
                channel.rotate_right(prefix);
            }
        }
        Ok(AudioBuffer { sample_rate, channels })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use craft_audio::{AudioBuffer, BitDepth, EncodeFormat, EncodeOptions};
    use craft_media_source::{MediaError, MediaKind, SharedSource};
    use craft_time::Tick;

    use crate::stream::{self, MediaSource as OpenedSource};

    const RATE: u32 = 48_000;
    const FRAMES: usize = 4_501;

    fn fixture() -> (Vec<u8>, AudioBuffer) {
        let left = (0..FRAMES).map(|i| ((i * 23 % 350) as f32 - 170.0) / 512.0).collect();
        let right = (0..FRAMES).map(|i| (220.0 - (i * 37 % 470) as f32) / 512.0).collect();
        let pcm = AudioBuffer { sample_rate: RATE, channels: vec![left, right] };
        let opts = EncodeOptions::new(EncodeFormat::Flac).with_bit_depth(BitDepth::Int24);
        (craft_audio::encode(&pcm, &opts).unwrap(), pcm)
    }

    fn require_shared(opened: OpenedSource) -> SharedSource {
        let OpenedSource::Craft(source) = opened else { panic!("FLAC should use the shared reader") };
        source
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn standalone_flac_uses_shared_source_for_disk_and_memory() {
        let (bytes, pcm) = fixture();
        let bytes: Arc<[u8]> = bytes.into();
        let memory = require_shared(stream::open_bytes("music.flac", bytes.clone()).unwrap());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("music.flac");
        std::fs::write(&path, &bytes).unwrap();
        let disk = require_shared(stream::open_source(&path).unwrap().0.unwrap());
        let legacy = filmcraft_codecs::open_bytes("music.flac", bytes.clone()).unwrap();

        for source in [memory, disk] {
            let info = source.info();
            let stream = info.audio().unwrap();
            assert_eq!(info.kind, MediaKind::AudioOnly);
            assert_eq!(info.duration, Tick::from_units(FRAMES as i64, i64::from(RATE)));
            assert_eq!(info.file_size, Some(bytes.len() as u64));
            assert_eq!(info.container, legacy.info().container);
            assert_eq!(stream.codec, legacy.info().audio().unwrap().codec);
            assert_eq!((stream.sample_rate, stream.channels, stream.bits_per_sample), (RATE, 2, Some(24)));
            let decoded = source.audio(0, FRAMES, RATE).unwrap();
            assert_eq!((decoded.sample_rate, decoded.num_channels(), decoded.frames()), (RATE, 2, FRAMES));
            for (got, want) in decoded.channels.iter().flatten().zip(pcm.channels.iter().flatten()) {
                assert_eq!(got, want);
            }

            // Seek backwards, across the encoder's block boundary, and around both clip edges.
            for (rate, start, frames) in [(RATE, 4091, 20), (RATE, -3, 8), (96_000, 8998, 10), (44_100, 251, 39), (44_100, 61, 39)] {
                let got = source.audio(start, frames, rate).unwrap();
                let want = legacy.audio(start, frames, rate).unwrap();
                assert_eq!(got.channels[0].len(), frames);
                for (got, want) in got.channels.iter().zip(&want.channels) {
                    for (got, want) in got.iter().zip(want) {
                        assert!((got - want).abs() <= 1e-6, "output rate {rate}, start {start}: {got} vs {want}");
                    }
                }
            }

            let full = source.audio(61, 63, 44_100).unwrap();
            let left = source.audio(61, 23, 44_100).unwrap();
            let right = source.audio(84, 40, 44_100).unwrap();
            for ((whole, left), right) in full.channels.iter().zip(&left.channels).zip(&right.channels) {
                assert_eq!(&whole[..23], left);
                assert_eq!(&whole[23..], right);
            }
            assert!(matches!(source.audio(0, usize::MAX, RATE), Err(MediaError::Decode(_))));
            assert!(matches!(source.audio(0, 1, 0), Err(MediaError::Decode(_))));
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn standalone_flac_keeps_legacy_trailer_compatibility() {
        let (mut bytes, pcm) = fixture();
        bytes.extend_from_slice(b"trailing bytes");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trailer.flac");
        std::fs::write(&path, &bytes).unwrap();
        let disk = stream::open_source(&path).unwrap().0.unwrap();
        let memory = stream::open_bytes("trailer.flac", bytes.into()).unwrap();
        for opened in [disk, memory] {
            let OpenedSource::Film(source) = opened else { panic!("the existing reader should accept the trailer") };
            let prefix = source.audio(0, 32, RATE).unwrap();
            assert_eq!((prefix.channels[0].as_slice(), prefix.channels[1].as_slice()), (&pcm.channels[0][..32], &pcm.channels[1][..32]));
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn native_flac_opener_declines_an_oversized_file_without_reading_it() {
        use std::sync::atomic::{AtomicBool, Ordering};

        use craft_media::ByteSource;
        use craft_media_source::{DEFAULT_MAX_MEDIA_BYTES, SharedBytes};

        struct OversizedFile(AtomicBool);
        impl ByteSource for OversizedFile {
            fn len(&self) -> u64 {
                DEFAULT_MAX_MEDIA_BYTES as u64 + 1
            }

            fn read_at(&self, _: u64, _: &mut [u8]) -> std::io::Result<()> {
                self.0.store(true, Ordering::Relaxed);
                Err(std::io::Error::other("oversized input was read"))
            }
        }

        let file = Arc::new(OversizedFile(AtomicBool::new(false)));
        let source: SharedBytes = file.clone();
        assert!(super::open_reader("oversized.flac", b"fLaC", &source).is_none());
        assert!(!file.0.load(Ordering::Relaxed));
    }
}
