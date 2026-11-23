//! Our own ProRes HQ grayscale frames exercise the real native decoder under concurrent
//! seeks. The subprocess bounds a deadlock regression without leaving the test runner stuck.

use std::io::Cursor;
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use craft_media::isobmff::{Brand, FourCc, Mp4Writer, SampleEntry, TrackConfig, WriteSample, WriterOptions};
use craft_video::prores::{ChromaFormat, Encoder, EncoderConfig, Frame, Profile};
use effectcraft_media::{MediaPool, probe};
use effectcraft_time::FrameRate;

const CHILD: &str = "EFFECTCRAFT_TEST_MOVIE_SCRUBBING_CHILD";
const TEST: &str = "movie_scrubbing::concurrent_out_of_order_movie_frames_remain_repeatable";
const FRAMES: usize = 40;

fn movie() -> Vec<u8> {
    let (width, height) = (256, 192);
    let mut encoder = Encoder::with_config(EncoderConfig::new(Profile::Hq, width, height));
    let mut writer = Mp4Writer::new(Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).unwrap();
    let entry = SampleEntry::prores(FourCc(Profile::Hq.fourcc()), width as u16, height as u16);
    let track = writer.add_track(TrackConfig::new(entry, 24)).unwrap();
    for index in 0..FRAMES {
        let mut frame = Frame::new(width, height, ChromaFormat::Yuv422, 10, false).unwrap();
        frame.y.fill(96 + index as u16 * 20);
        let data = encoder.encode(&frame).unwrap();
        writer.write_sample(track, WriteSample { data: &data, duration: 1, composition_offset: 0, is_sync: true }).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn concurrent_seeks() {
    let directory = tempfile::Builder::new().prefix("effectcraft-movie-scrubbing-").tempdir().unwrap();
    let path = directory.path().join("generated-prores-hq.mov");
    std::fs::write(&path, movie()).unwrap();
    let footage = probe(&path).unwrap();
    let rate = FrameRate::FPS_24;
    let reference = MediaPool::new();
    reference.set_read_ahead(false);
    let expected: Vec<_> = (0..FRAMES).map(|index| reference.frame_at(&footage, rate.tick_of(index as i64)).unwrap()).collect();
    assert!(expected[0].data != expected[FRAMES - 1].data, "the movie has distinguishable frames");

    let pool = MediaPool::with_budget(0);
    pool.set_read_ahead(false);
    for pass in 0..3 {
        // Reopen cold, including after many backward seeks, rather than only hitting cached
        // decoded frames from the preceding pass.
        pool.forget(&footage.path);
        let start = Arc::new(Barrier::new(FRAMES + 1));
        std::thread::scope(|scope| {
            let mut requests = Vec::new();
            for request in 0..FRAMES {
                let index = (request * 17 + pass * 7) % FRAMES;
                let (pool, footage, start) = (pool.clone(), footage.clone(), start.clone());
                requests.push((
                    index,
                    scope.spawn(move || {
                        start.wait();
                        pool.frame_at(&footage, rate.tick_of(index as i64)).unwrap()
                    }),
                ));
            }
            start.wait();
            for (index, request) in requests {
                let actual = request.join().unwrap();
                assert!(actual.data == expected[index].data, "seek to frame {index} on pass {pass} differs from a serial read");
            }
        });
    }
}

#[test]
fn concurrent_out_of_order_movie_frames_remain_repeatable() {
    if std::env::var_os(CHILD).is_some() {
        concurrent_seeks();
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait().unwrap().is_some() {
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "concurrent movie seeks failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("concurrent movie seeks stalled: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
