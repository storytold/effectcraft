//! The video helper protocol end to end: a stand-in helper and the gate tests. The stand-in is
//! this binary again, started with `--effectcraft-helper` and `EC_FAKE_VIDEO=<scenario>`; it
//! "encodes" with a delay of one frame and answers each frame with a packet that holds the first
//! sample of each plane of the picture it read, so the tests can see that the planes arrived.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use effectcraft_helper_host::blocklist;
use effectcraft_helper_host::protocol::HELPER_ARG;
use effectcraft_helper_host::video::client::{EncodeParams, EncodedPacket, Options, Picture, VideoError, VideoHelper};
use effectcraft_helper_host::video::wire::{
    Codec, Colour, ColourRange, Device, EncodeCaps, ErrorCode, MAGIC, PacketRef, Region, Slot, VideoHello, VideoReply, VideoRequest,
};

use super::check;

const SCENARIO: &str = "EC_FAKE_VIDEO";
const WIDTH: u32 = 128;
const HEIGHT: u32 = 64;

pub fn is_fake() -> bool {
    std::env::var_os(SCENARIO).is_some()
}

fn say(r: &VideoReply) {
    if let Ok(json) = serde_json::to_string(r) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{MAGIC}{json}");
        let _ = out.flush();
    }
}

/// What the stand-in remembers between requests.
struct Fake {
    scenario: String,
    file: Option<File>,
    slots: Vec<Slot>,
    held: Option<(u64, Vec<u8>)>,
}

impl Fake {
    fn is(&self, name: &str) -> bool {
        self.scenario == name
    }

    fn read_byte(&mut self, at: u64) -> u8 {
        let mut b = [0u8];
        if let Some(f) = self.file.as_mut() {
            let _ = f.seek(SeekFrom::Start(at)).and_then(|_| f.read_exact(&mut b));
        }
        b[0]
    }

    fn write(&mut self, at: u64, bytes: &[u8]) {
        if let Some(f) = self.file.as_mut() {
            let _ = f.seek(SeekFrom::Start(at)).and_then(|_| f.write_all(bytes));
        }
    }

    fn hello(&mut self, frames: &str) {
        println!("driver chatter that is not a protocol line");
        eprintln!("driver chatter on stderr");
        self.file = OpenOptions::new().read(true).write(true).open(frames).ok();
        let caps = EncodeCaps {
            codec: "h264".into(),
            profiles: vec!["high".into()],
            bit_depths: vec![8],
            min_size: [64, 64],
            max_size: if self.is("huge_caps") { [u32::MAX, u32::MAX] } else { [4096, 4096] },
            rate_control: vec!["bitrate".into()],
        };
        let device = Device { id: "nvenc:0".into(), name: "Fake GPU".into(), api: "nvenc".into(), driver: "1.0".into(), encode: vec![caps], decode: vec![] };
        let (video_version, app_min) = match self.scenario.as_str() {
            "hello_video_v2" => (2, "0.0.1"),
            "hello_old_app" => (1, "99.0.0"),
            _ => (1, "0.0.1"),
        };
        let video = (!self.is("hello_no_video")).then_some(VideoHello { version: video_version, app_min: app_min.into(), devices: vec![device] });
        say(&VideoReply::Hello { version: 1, name: "Fake video".into(), helper_version: "0.1.0".into(), loads: vec![], video });
    }

    fn frame(&mut self, job: u64, frame: u64, slot: u32, plane_offsets: [u64; 3]) {
        match (self.scenario.as_str(), frame) {
            ("crash_on_frame_2", 2) => std::process::exit(1),
            ("hang_on_frame_1", 1) => loop {
                std::thread::sleep(Duration::from_secs(60));
            },
            ("too_large", 1) => return say(&VideoReply::Error { job, frame: Some(frame), code: ErrorCode::PacketTooLarge, message: "does not fit".into() }),
            ("device_lost", 1) => return say(&VideoReply::Error { job, frame: Some(frame), code: ErrorCode::DeviceLost, message: "gone".into() }),
            ("wrong_order", 0) => return say(&VideoReply::EncodePackets { job, frame: Some(5), packets: vec![] }),
            ("never_issued", 0) => return say(&VideoReply::EncodePackets { job: 9999, frame: Some(0), packets: vec![] }),
            ("slow", _) => std::thread::sleep(Duration::from_millis(100)),
            _ => {}
        }
        let first = plane_offsets.map(|at| self.read_byte(at));
        let mine = vec![0, 0, 0, 4, frame as u8, first[0], first[1], first[2]];
        let Some(out) = self.slots.get(slot as usize).map(|s| s.out) else { return };
        let mut packets = vec![];
        if let Some((pts, bytes)) = self.held.replace((frame, mine)) {
            let offset = if self.is("oob_packet") { out.len + 1 } else { 0 };
            self.write(out.offset, &bytes);
            packets.push(PacketRef { offset, len: bytes.len() as u64, pts: pts as i64, dts: pts as i64, key: pts == 0 });
        }
        say(&VideoReply::EncodePackets { job, frame: Some(frame), packets });
    }

    fn flush(&mut self, job: u64, out: Region) {
        let slow = self.is("slow_flush");
        if slow {
            std::thread::sleep(Duration::from_millis(300));
        }
        let mut packets = vec![];
        if let Some((pts, bytes)) = self.held.take() {
            self.write(out.offset, &bytes);
            packets.push(PacketRef { offset: 0, len: bytes.len() as u64, pts: pts as i64, dts: pts as i64, key: pts == 0 });
        }
        say(&VideoReply::EncodePackets { job, frame: None, packets: packets.clone() });
        if self.is("double_batch") {
            say(&VideoReply::EncodePackets { job, frame: None, packets });
        }
        if slow {
            std::thread::sleep(Duration::from_millis(300));
        }
        say(&VideoReply::EncodeFlushed { job });
    }

    fn handle(&mut self, request: VideoRequest) -> bool {
        match request {
            VideoRequest::Hello { frames, .. } => self.hello(&frames),
            VideoRequest::EncodeOpen(open) if self.is("open_unsupported") => {
                say(&VideoReply::Error { job: open.job, frame: None, code: ErrorCode::Unsupported, message: "no".into() });
            }
            VideoRequest::EncodeOpen(open) => {
                self.slots = open.slots;
                self.held = None;
                say(&VideoReply::EncodeOpened { job: open.job, codec_config: "AQID".into(), delay: 1 });
            }
            VideoRequest::EncodeFrame { job, frame, slot, planes } => self.frame(job, frame, slot, planes.map(|p| p.offset)),
            VideoRequest::EncodeFlush { job, out } => self.flush(job, out),
            VideoRequest::Cancel { job } if self.is("cancel_spam") => {
                let started = Instant::now();
                while started.elapsed() < Duration::from_secs(8) {
                    say(&VideoReply::EncodeFlushed { job });
                }
            }
            VideoRequest::Cancel { job } if !self.is("ignore_cancel") => say(&VideoReply::Cancelled { job }),
            VideoRequest::Cancel { .. } | VideoRequest::EncodeClose { .. } => {}
            VideoRequest::Shutdown => return false,
        }
        true
    }
}

pub fn serve_fake() -> Result<(), String> {
    let mut fake = Fake { scenario: std::env::var(SCENARIO).unwrap_or_default(), file: None, slots: vec![], held: None };
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        let request: VideoRequest = serde_json::from_str(&line).map_err(|e| format!("bad request {line}: {e}"))?;
        if !fake.handle(request) {
            break;
        }
    }
    Ok(())
}

/// Makes the host slower than a helper that floods it: each dropped late reply costs 2 ms.
struct SlowOnLateReplies;

impl log::Log for SlowOnLateReplies {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if record.args().to_string().contains("dropped a late reply") {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn flush(&self) {}
}

// --- the checks -----------------------------------------------------------------------------

fn options(frame_timeout_ms: u64) -> Options {
    Options { frame_timeout: Duration::from_millis(frame_timeout_ms), load_timeout: Duration::from_secs(20), stop_wait: Duration::from_secs(5) }
}

fn command(exe: &Path, scenario: &str) -> Command {
    let mut cmd = Command::new(exe);
    cmd.arg(HELPER_ARG).env(SCENARIO, scenario);
    cmd
}

/// A helper in `scenario`, with the stop count reset first so that one check's crashes don't
/// count against the next.
fn start(exe: &Path, scenario: &str, frame_timeout_ms: u64) -> Result<VideoHelper, String> {
    blocklist::clear();
    VideoHelper::start_with(command(exe, scenario), options(frame_timeout_ms))
}

fn params() -> EncodeParams {
    EncodeParams {
        device: "nvenc:0".into(),
        codec: Codec::H264,
        width: WIDTH,
        height: HEIGHT,
        rate: [30, 1],
        bitrate_kbps: 5000,
        profile: "high".into(),
        keyint: 30,
        colour: Colour { primaries: 1, transfer: 1, matrix: 1, range: ColourRange::Limited },
    }
}

fn picture(frame: u8) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let luma = (WIDTH * HEIGHT) as usize;
    (vec![10 + frame; luma], vec![20 + frame; luma / 4], vec![30 + frame; luma / 4])
}

fn encode(h: &mut VideoHelper, frames: u8) -> Result<Vec<EncodedPacket>, VideoError> {
    let job = h.open_encoder(&params())?.job;
    let mut packets = vec![];
    for f in 0..frames {
        let (y, u, v) = picture(f);
        h.submit(job, &Picture { y: &y, u: &u, v: &v })?;
        packets.extend(h.take_packets(job)?);
    }
    packets.extend(h.finish(job)?);
    Ok(packets)
}

fn expected(frame: u8) -> Vec<u8> {
    vec![0, 0, 0, 4, frame, 10 + frame, 20 + frame, 30 + frame]
}

fn failure(exe: &Path, scenario: &str, frame_timeout_ms: u64) -> Option<(VideoError, bool)> {
    let mut h = start(exe, scenario, frame_timeout_ms).ok()?;
    let e = encode(&mut h, 6).err()?;
    Some((e, h.is_running()))
}

pub fn run_checks(exe: &Path) -> bool {
    let mut all = true;

    // A job: more frames than fit in flight, so the helper's pace sets ours.
    let mut h = start(exe, "ok", 20_000).ok();
    let got = h.as_mut().map(|h| (h.devices().first().map(|d| d.id.clone()), encode(h, 20)));
    let ok = matches!(&got, Some((Some(id), Ok(p))) if id == "nvenc:0" && p.len() == 20
        && p.iter().enumerate().all(|(i, p)| p.pts == i as i64 && p.data == expected(i as u8) && p.key == (i == 0)));
    all &= check("video: frames come back in order with the planes the helper read", ok, got.map(|g| g.1.map(|p| p.len())));

    let opened = h.as_mut().map(|h| h.open_encoder(&params()).map(|o| (o.codec_config, o.delay)));
    all &= check("video: opened gives the codec config and the delay", matches!(opened, Some(Ok((c, 1))) if c == [1, 2, 3]), "");
    drop(h);

    // Settings the device can't do are refused before anything is opened.
    let mut h = start(exe, "ok", 20_000).ok();
    let refused: Vec<_> = [(WIDTH + 1, HEIGHT), (32, 32), (8192, 8192)]
        .iter()
        .map(|&(width, height)| h.as_mut().map(|h| h.open_encoder(&EncodeParams { width, height, ..params() })))
        .collect();
    let all_refused = refused.iter().all(|r| matches!(r, Some(Err(VideoError::Job { code: ErrorCode::Unsupported, .. }))));
    let mut other = params();
    other.device = "vaapi:9".into();
    let no_device = h.as_mut().map(|h| h.open_encoder(&other));
    all &= check(
        "video: settings the device can't do are refused and the helper is fine",
        all_refused && matches!(no_device, Some(Err(VideoError::Job { code: ErrorCode::Unsupported, .. }))) && h.as_ref().is_some_and(VideoHelper::is_running),
        refused.len(),
    );
    drop(h);

    // A helper that advertises absurd limits can't make us allocate or overflow.
    let mut h = start(exe, "huge_caps", 20_000).ok();
    let huge: Vec<_> = [(16384, 16384), (u32::MAX - 1, u32::MAX - 1)]
        .iter()
        .map(|&(width, height)| h.as_mut().map(|h| h.open_encoder(&EncodeParams { width, height, ..params() }).err()))
        .collect();
    let too_big = huge.iter().all(|r| matches!(r, Some(Some(VideoError::Job { code: ErrorCode::Unsupported, .. }))));
    all &= check("video: a picture over 256 MiB is refused before anything is allocated", too_big && h.as_ref().is_some_and(VideoHelper::is_running), &huge);
    drop(h);

    // A request that can't be written leaves no job behind.
    let mut h = start(exe, "ok", 20_000).ok();
    let long = EncodeParams { profile: "x".repeat(2 << 20), ..params() };
    let refused = h.as_mut().map(|h| h.open_encoder(&long).err());
    let after = h.as_mut().map(|h| encode(h, 3).map(|p| p.len()));
    all &= check(
        "video: an oversized encode.open is refused and the helper still works",
        matches!(refused, Some(Some(VideoError::Request(_)))) && matches!(after, Some(Ok(3))),
        &after,
    );
    drop(h);

    // A helper that doesn't fit this EffectCraft is not used, and the message names its version.
    for scenario in ["hello_video_v2", "hello_old_app", "hello_no_video"] {
        let r = start(exe, scenario, 20_000);
        all &= check(
            &format!("video: {scenario} is refused"),
            r.as_ref().err().is_some_and(|e| e.contains("0.1.0") || e.contains("not a video helper")),
            r.as_ref().err(),
        );
    }
    let missing = VideoHelper::start(Path::new("/no/such/effectcraft-helper-video"));
    all &= check("video: a missing helper is an error", missing.is_err(), missing.as_ref().err());

    // Crashes, hangs and broken replies stop the helper and fail the export.
    let t0 = Instant::now();
    let crash = failure(exe, "crash_on_frame_2", 20_000);
    all &= check(
        "video: a crash fails the export and stops the helper",
        matches!(&crash, Some((VideoError::Helper(m), false)) if m.contains("stopped") || m.contains("can't write")),
        &crash,
    );
    let hang = failure(exe, "hang_on_frame_1", 500);
    all &= check(
        "video: a hang is cut off by the frame timeout",
        matches!(&hang, Some((VideoError::Helper(m), false)) if m.contains("did not answer")) && t0.elapsed() < Duration::from_secs(30),
        &hang,
    );
    let breaks = [("wrong_order", "oldest unanswered"), ("oob_packet", "outside its"), ("never_issued", "never issued"), ("double_batch", "nobody asked for")];
    for (scenario, why) in breaks {
        let r = failure(exe, scenario, 5_000);
        all &= check(&format!("video: {scenario} is a protocol break"), matches!(&r, Some((VideoError::Helper(m), false)) if m.contains(why)), &r);
    }

    // A refused job ends that job; the helper carries on and is not charged.
    blocklist::clear();
    for (scenario, code) in [("too_large", ErrorCode::PacketTooLarge), ("device_lost", ErrorCode::DeviceLost)] {
        let mut runs = vec![];
        for _ in 0..4 {
            let mut h = VideoHelper::start_with(command(exe, scenario), options(5_000));
            let r = h.as_mut().map_err(|e| e.clone()).map(|h| (encode(h, 6).err(), h.is_running(), h.open_encoder(&params()).is_ok()));
            runs.push(r);
        }
        let fine = runs.iter().all(|r| matches!(r, Ok((Some(VideoError::Job { code: c, .. }), true, true)) if *c == code));
        all &= check(&format!("video: {scenario} ends the job only and is never counted as a stop"), fine, &runs);
    }
    let open = start(exe, "open_unsupported", 5_000).map(|mut h| (h.open_encoder(&params()).err(), h.is_running()));
    all &= check(
        "video: an error answering encode.open is a job error",
        matches!(&open, Ok((Some(VideoError::Job { code: ErrorCode::Unsupported, .. }), true))),
        &open,
    );

    // One job at a time: a second open is refused while the first is active, and a new job is
    // accepted after the first is finished or cancelled.
    let mut h = start(exe, "ok", 5_000).ok();
    let sequence = h.as_mut().map(|h| {
        let first = h.open_encoder(&params())?.job;
        let second = h.open_encoder(&params()).err();
        let (y, u, v) = picture(0);
        h.submit(first, &Picture { y: &y, u: &u, v: &v })?;
        let finished = h.finish(first)?.len();
        let after_finish = h.open_encoder(&params())?.job;
        h.cancel(after_finish)?;
        let after_cancel = h.open_encoder(&params()).map(|o| o.job);
        Ok::<_, VideoError>((second, finished, after_cancel.is_ok()))
    });
    all &= check(
        "video: a second job is refused while one is open and accepted after finish or cancel",
        matches!(&sequence, Some(Ok((Some(VideoError::Request(_)), 1, true)))) && h.as_ref().is_some_and(VideoHelper::is_running),
        &sequence,
    );
    drop(h);

    // Cancel: replies to frames still in flight arrive first and are dropped, then the helper
    // carries on with a new job.
    let mut h = start(exe, "slow", 5_000);
    let cancelled = h.as_mut().map(|h| {
        let job = h.open_encoder(&params())?.job;
        for f in 0..3 {
            let (y, u, v) = picture(f);
            h.submit(job, &Picture { y: &y, u: &u, v: &v })?;
        }
        h.cancel(job)?;
        encode(h, 3).map(|p| p.len())
    });
    all &= check("video: cancel drops late replies and the helper carries on", matches!(cancelled, Ok(Ok(3))), &cancelled);
    let _ = log::set_logger(&SlowOnLateReplies);
    log::set_max_level(log::LevelFilter::Info);
    let started = Instant::now();
    let spam = failure_on_cancel(exe, "cancel_spam");
    all &= check(
        "video: a flood of late replies does not keep a cancel waiting past its timeout",
        matches!(&spam, Some((VideoError::Helper(m), false)) if m.contains("cancel")) && started.elapsed() < Duration::from_secs(5),
        (&spam, started.elapsed()),
    );
    let slow = Options { load_timeout: Duration::from_millis(500), ..options(5_000) };
    let flushed = VideoHelper::start_with(command(exe, "slow_flush"), slow).map(|mut h| encode(&mut h, 3).err());
    all &= check(
        "video: the flush batch and encode.flushed share one timeout",
        matches!(&flushed, Ok(Some(VideoError::Helper(m))) if m.contains("flush")),
        &flushed,
    );
    let ignored = failure_on_cancel(exe, "ignore_cancel");
    all &= check("video: a helper that ignores cancel is stopped", matches!(&ignored, Some((VideoError::Helper(m), false)) if m.contains("cancel")), &ignored);

    // 3 stops turn the helper off until the user retries.
    blocklist::clear();
    let mut stops = HashMap::new();
    for i in 0..3 {
        let mut h = VideoHelper::start_with(command(exe, "crash_on_frame_2"), options(5_000));
        stops.insert(i, h.as_mut().map(|h| encode(h, 6).is_err()).unwrap_or(false));
    }
    let fourth = VideoHelper::start_with(command(exe, "ok"), options(5_000));
    all &= check(
        "video: after 3 stops the helper is turned off",
        stops.values().all(|s| *s) && fourth.as_ref().err().is_some_and(|e| e.contains("turned off")),
        (&stops, fourth.as_ref().err()),
    );
    blocklist::clear();
    let retried = VideoHelper::start_with(command(exe, "ok"), options(5_000));
    all &= check("video: retrying turns it back on", retried.is_ok(), retried.as_ref().err());
    blocklist::clear();
    all
}

fn failure_on_cancel(exe: &Path, scenario: &str) -> Option<(VideoError, bool)> {
    let mut h = start(exe, scenario, 500).ok()?;
    let job = h.open_encoder(&params()).ok()?.job;
    let e = h.cancel(job).err()?;
    Some((e, h.is_running()))
}
