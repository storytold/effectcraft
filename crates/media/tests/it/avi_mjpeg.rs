use std::io::Cursor;
use std::sync::Arc;

use craft_media::avi::{AudioSpec, AviWriter, MuxOptions, StreamSpec, VideoSpec};
use effectcraft_media::{MediaPool, probe, probe_bytes};
use effectcraft_project::FootageKind;
use effectcraft_time::{FrameRate, Tick};
use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};

const SIDE: u32 = 16;
const FPS: u32 = 25;
const SAMPLE_RATE: u32 = 48_000;

fn mjpeg_with_pcm() -> Arc<[u8]> {
    let video = StreamSpec::Video(VideoSpec::new(*b"MJPG", SIDE, SIDE, FPS, 1));
    let audio = StreamSpec::Audio(AudioSpec::pcm(SAMPLE_RATE, 1, 16));
    let mut writer = AviWriter::new(Cursor::new(Vec::new()), vec![video, audio], MuxOptions::default()).expect("AVI writer");
    let samples_per_frame = (SAMPLE_RATE / FPS) as usize;
    for (color, sample) in [([220_u8, 40, 30], 8_192_i16), ([30, 40, 220], -8_192)] {
        let pixels = color.repeat((SIDE * SIDE) as usize);
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 90).encode(&pixels, SIDE, SIDE, ExtendedColorType::Rgb8).expect("JPEG picture");
        writer.write_chunk(0, &jpeg, true).expect("video chunk");

        let pcm = sample.to_le_bytes().repeat(samples_per_frame);
        writer.write_chunk(1, &pcm, true).expect("matching audio chunk");
    }
    writer.finish().expect("finish AVI").into_inner().into()
}

#[test]
fn generated_mjpeg_avi_preserves_video_seek_and_pcm_alignment() {
    let bytes = mjpeg_with_pcm();
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("native-mjpeg.avi");
    std::fs::write(&path, &bytes).expect("write AVI");
    let native = probe(&path).expect("native AVI probe");
    let name = "in-memory-mjpeg.avi";
    let memory = probe_bytes(name, bytes.clone()).expect("in-memory AVI probe");

    for footage in [&native, &memory] {
        assert_eq!(footage.kind, FootageKind::Video);
        assert_eq!((footage.width, footage.height), (SIDE, SIDE));
        assert_eq!(footage.frame_rate, FrameRate::FPS_25);
        assert_eq!(footage.duration, Tick::from_units(2, i64::from(FPS)));
        assert_eq!(footage.codec, "Motion JPEG");
        assert!(footage.has_video && footage.has_audio);
    }

    let native_pool = MediaPool::new();
    let memory_pool = MediaPool::new();
    memory_pool.add_bytes(name, bytes);
    for (frame, dominant, other, sign) in [(0, 0, 2, 1.0), (1, 2, 0, -1.0), (0, 0, 2, 1.0)] {
        let at = Tick::from_units(frame, i64::from(FPS));
        let native_frame = native_pool.frame_at(&native, at).expect("native picture");
        let memory_frame = memory_pool.frame_at(&memory, at).expect("in-memory picture");
        let pixel = native_frame.get(i64::from(SIDE / 2), i64::from(SIDE / 2));
        assert_eq!(pixel, memory_frame.get(i64::from(SIDE / 2), i64::from(SIDE / 2)));
        assert!(pixel[dominant] > pixel[other] + 0.4, "picture at frame {frame}: {pixel:?}");

        let native_pcm = native_pool.audio_samples(&native, at, 16, SAMPLE_RATE);
        let memory_pcm = memory_pool.audio_samples(&memory, at, 16, SAMPLE_RATE);
        assert_eq!(native_pcm, memory_pcm);
        assert_eq!(native_pcm.len(), 32);
        assert!(native_pcm.iter().all(|sample| sign * sample > 0.05), "PCM at frame {frame}: {native_pcm:?}");
    }
}
