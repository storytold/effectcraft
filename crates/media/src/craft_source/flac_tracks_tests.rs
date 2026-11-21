use std::io::Cursor;
use std::sync::Arc;

use craft_audio::flac::{Encoder, EncoderConfig, STREAMINFO_OFFSET};
use craft_media::{isobmff, matroska};
use craft_media_source::MediaKind;

use crate::stream::{self, MediaSource as OpenedSource};

const RATE: u32 = 96_000;
const BLOCK: usize = 1_000;
const FRAMES: usize = 196 * BLOCK + 221;
const COLD_SEEK: usize = 2 * RATE as usize + BLOCK + 200;

struct Fixture {
    private: Vec<u8>,
    packets: Vec<Vec<u8>>,
    samples: Vec<Vec<i32>>,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let left: Vec<i32> = (0..FRAMES).map(|i| (i as i32 * 307 % 65_536) - 30_000).collect();
        let right: Vec<i32> = (0..FRAMES).map(|i| 28_000 - (i as i32 * 701 % 59_000)).collect();
        let mut config = EncoderConfig::new(RATE, 2, 24);
        config.block_size = BLOCK as u16;
        let mut encoder = Encoder::new(config)?;
        let mut packets = encoder.encode_frames(&[&left, &right])?;
        packets.push(encoder.finish());
        let mut private = encoder.header(&[]);
        let offset = STREAMINFO_OFFSET as usize;
        private[offset..offset + 34].copy_from_slice(&encoder.streaminfo());
        Ok(Self { private, packets, samples: vec![left, right] })
    }

    fn mp4(&self) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let offset = STREAMINFO_OFFSET as usize;
        let mut entry = isobmff::SampleEntry::flac(self.private[offset..offset + 34].try_into()?);
        entry.audio.as_mut().ok_or("FLAC sample entry needs an audio header")?.sample_rate = 48_000.0;
        let mut writer = isobmff::Mp4Writer::new(Cursor::new(Vec::new()), isobmff::WriterOptions::new(isobmff::Brand::Mp4))?;
        writer.add_track(isobmff::TrackConfig::new(entry, RATE))?;
        for (i, packet) in self.packets.iter().enumerate() {
            let duration = (FRAMES - i * BLOCK).min(BLOCK) as u32;
            writer.write_sample(0, isobmff::WriteSample { data: packet, duration, composition_offset: 0, is_sync: true })?;
        }
        Ok(writer.finish()?.into_inner())
    }

    fn mkv(&self, private: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut track = matroska::TrackSpec::new(matroska::TrackKind::Audio, "A_FLAC");
        track.audio = Some((f64::from(RATE), 2, Some(24)));
        track.codec_private = private.to_vec();
        let mut writer = matroska::MkvWriter::new(Cursor::new(Vec::new()), vec![track], matroska::MuxOptions::default())?;
        for (i, packet) in self.packets.iter().enumerate() {
            // The muxer rounds starts to its millisecond clock. A 1000-frame packet
            // at 96 kHz does not start on a millisecond boundary after packet zero.
            let pts = (i * BLOCK) as i64 * 1_000_000_000 / i64::from(RATE);
            let duration = (FRAMES - i * BLOCK).min(BLOCK) as u64 * 1_000_000_000 / u64::from(RATE);
            writer.write_frame(0, pts, true, packet, Some(duration))?;
        }
        Ok(writer.finish()?.into_inner())
    }
}

#[test]
fn flac_in_mp4_and_matroska_uses_shared_samples_after_cold_and_backward_seeks() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let dir = tempfile::tempdir().unwrap();
    for (name, bytes) in [("audio.mp4", fixture.mp4()?), ("audio.mka", fixture.mkv(&fixture.private)?)] {
        let bytes: Arc<[u8]> = bytes.into();
        let path = dir.path().join(name);
        std::fs::write(&path, &bytes).unwrap();
        let disk = stream::open_source(&path).unwrap().0.unwrap();
        let memory = stream::open_bytes(name, bytes).unwrap();
        for opened in [disk, memory] {
            let OpenedSource::Craft(source) = opened else { panic!("{name}: expected the shared FLAC reader") };
            let info = source.info();
            assert_eq!(info.kind, MediaKind::AudioOnly);
            let audio = info.audio().unwrap();
            assert_eq!((audio.codec.as_str(), audio.sample_rate, audio.channels, audio.bits_per_sample), ("FLAC", RATE, 2, Some(24)));

            for (start, frames) in [(COLD_SEEK, 48), (991, 33), (FRAMES - 25, 50), (0, FRAMES)] {
                let decoded = source.audio(start as i64, frames, RATE).unwrap();
                assert_eq!((decoded.num_channels(), decoded.frames()), (2, frames));
                for (actual, expected) in decoded.channels.iter().zip(&fixture.samples) {
                    for (i, &sample) in actual.iter().enumerate() {
                        let want = expected.get(start + i).copied().unwrap_or(0) as f32 / 8_388_608.0;
                        assert_eq!(sample, want, "{name} sample {}", start + i);
                    }
                }
            }

            let whole = source.audio(451, 93, 44_100).unwrap();
            let first = source.audio(451, 31, 44_100).unwrap();
            let second = source.audio(482, 62, 44_100).unwrap();
            for ((whole, first), second) in whole.channels.iter().zip(&first.channels).zip(&second.channels) {
                assert_eq!(&whole[..31], first, "{name} first resampled segment");
                assert_eq!(&whole[31..], second, "{name} second resampled segment");
            }
        }
    }
    Ok(())
}

#[test]
fn flac_without_the_expected_private_marker_keeps_matroska_fallback() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let mut private = fixture.private.clone();
    private[..4].copy_from_slice(b"test");
    let OpenedSource::Film(source) = stream::open_bytes("legacy.mka", fixture.mkv(&private)?.into()).unwrap() else {
        panic!("expected the legacy FLAC reader");
    };
    let decoded = source.audio(0, 16, RATE).unwrap();
    assert!(decoded.channels[0].iter().any(|&sample| sample != 0.0));
    Ok(())
}
