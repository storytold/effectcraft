use std::io::Cursor;
use std::sync::Arc;

use craft_color::ColorInfo;
use craft_frame::PixelData;
use craft_media::avi;
use craft_media_source::FrameRequest;
use craft_time::Tick;

use crate::stream::{self, MediaSource};

const WIDTH: u32 = 3;
const HEIGHT: u32 = 2;
const FPS: u32 = 25;
const AUDIO_RATE: u32 = 8_000;
const AUDIO_PER_FRAME: usize = (AUDIO_RATE / FPS) as usize;

fn rgba(frame: u8, x: u8, y: u8) -> [u8; 4] {
    [16 + 30 * frame, 21 + 71 * y, 5 + 17 * x, 255]
}

fn bitmap(frame: u8, depth: u16, bottom_up: bool) -> Vec<u8> {
    let stride = (WIDTH as usize * usize::from(depth / 8)).next_multiple_of(4);
    let mut bytes = Vec::new();
    for row in 0..HEIGHT as u8 {
        let y = if bottom_up { HEIGHT as u8 - 1 - row } else { row };
        for x in 0..WIDTH as u8 {
            let [r, g, b, _] = rgba(frame, x, y);
            bytes.extend([b, g, r]);
            if depth == 32 {
                bytes.push(43);
            }
        }
        bytes.resize((usize::from(row) + 1) * stride, 111);
    }
    bytes
}

// Original in-memory fixture. The shared AVI muxer writes deterministic test pixels and PCM.
fn fixture(depth: u16, bottom_up: bool, sparse: bool) -> Result<Vec<u8>, avi::Error> {
    let mut video = avi::VideoSpec::new([0; 4], WIDTH, HEIGHT, FPS, 1);
    if !bottom_up {
        video.format.height = -(HEIGHT as i32);
    }
    video.format.bit_count = depth;
    video.format.size_image = bitmap(0, depth, bottom_up).len() as u32;
    let audio = avi::AudioSpec::pcm(AUDIO_RATE, 1, 16);
    let mut writer =
        avi::AviWriter::new(Cursor::new(Vec::new()), vec![avi::StreamSpec::Video(video), avi::StreamSpec::Audio(audio)], avi::MuxOptions::default())?;
    for i in 0..3u8 {
        let picture = if sparse && i == 1 { Vec::new() } else { bitmap(i, depth, bottom_up) };
        let sound = (1000 * (i16::from(i) + 1)).to_le_bytes().repeat(AUDIO_PER_FRAME);
        writer.write_chunk(0, &picture, true)?;
        writer.write_chunk(1, &sound, true)?;
    }
    Ok(writer.finish()?.into_inner())
}

#[test]
fn raw_rgb_avi_preserves_pixels_and_pcm_across_indexed_seeks() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    for (name, depth, bottom_up) in [("bottom-up.avi", 24, true), ("top-down.avi", 32, false)] {
        let bytes: Arc<[u8]> = fixture(depth, bottom_up, false)?.into();
        let path = dir.path().join(name);
        std::fs::write(&path, &bytes)?;
        let disk = stream::open_source(&path).ok_or("unrecognized AVI")?.0?;
        let memory = stream::open_bytes(name, bytes)?;
        for opened in [disk, memory] {
            let MediaSource::Craft(source) = opened else { panic!("{name}: expected shared raw AVI route") };
            let info = source.info();
            let video = info.video.as_ref().ok_or("missing video")?;
            assert_eq!((video.width, video.height, video.has_alpha), (WIDTH, HEIGHT, false));
            assert_eq!(video.codec, format!("Uncompressed RGB {depth}-bit"));
            assert_eq!(info.duration, Tick::from_units(3, i64::from(FPS)));
            for i in [2u8, 0, 1] {
                let time = Tick::from_units(2 * i64::from(i) + 1, 2 * i64::from(FPS));
                let frame = source.video_frame(FrameRequest { time, scale: 1.0 })?;
                assert_eq!(frame.pts, Tick::from_units(i64::from(i), i64::from(FPS)));
                assert_eq!((frame.par, frame.color), ((1, 1), ColorInfo::SRGB_FULL));
                let PixelData::Rgba8(pixels) = &frame.data else { panic!("expected 8-bit RGBA") };
                let expected: Vec<u8> = (0..HEIGHT as u8).flat_map(|y| (0..WIDTH as u8).flat_map(move |x| rgba(i, x, y))).collect();
                assert_eq!(pixels.as_slice(), expected);
            }
            let audio = source.audio(AUDIO_PER_FRAME as i64 - 2, 4, AUDIO_RATE)?;
            assert_eq!(audio.channels, [vec![1000.0 / 32768.0, 1000.0 / 32768.0, 2000.0 / 32768.0, 2000.0 / 32768.0]]);
        }
    }
    Ok(())
}

#[test]
fn raw_rgb_avi_with_dropped_slot_keeps_legacy_playback() -> Result<(), Box<dyn std::error::Error>> {
    let opened = stream::open_bytes("sparse.avi", fixture(24, true, true)?.into())?;
    let MediaSource::Film(source) = opened else { panic!("expected legacy AVI slot handling") };
    let time = filmcraft_time::Tick::from_units(1, i64::from(FPS));
    let frame = source.video_frame(filmcraft_media::FrameRequest { time, scale: 1.0 })?;
    assert_eq!((frame.width, frame.height), (WIDTH, HEIGHT));
    Ok(())
}
