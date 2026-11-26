//! A running video helper and the encode jobs on it. Everything the helper does wrong ends the
//! helper, never EffectCraft: a crash, a hang or a broken reply stops it and fails the export
//! with a message; a refused job ends that job only.

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel, sync_channel};
use std::time::{Duration, Instant};

use super::check_hello;
use super::jobs::{Jobs, MAX_BYTES_IN_FLIGHT, MAX_FRAMES_IN_FLIGHT, SendError, Verdict};
use super::wire::{
    Codec, Colour, Device, EncodeOpen, ErrorCode, HELPER_ARG, MAX_LINE, Plane, Region, Slot, VIDEO_VERSION, VideoReply, VideoRequest, parse_reply, request_line,
};
use crate::blocklist;
use crate::frames::Exchange;
use crate::protocol::PROTOCOL_VERSION;

const BLOCK_ID: &str = "effectcraft-helper-video";

/// Parsed replies waiting for us. When it is full the reader waits, then the helper's pipe fills
/// and the helper waits: a helper can't make us hold unlimited memory.
const REPLY_QUEUE: usize = 64;

/// One poll looks at this many late replies at most, so a helper that floods them can't stall it.
const MAX_DROPPED_PER_POLL: usize = 64;

/// `timeout` from now; one too long to represent is a year.
fn deadline(timeout: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(timeout).unwrap_or_else(|| now + Duration::from_secs(365 * 24 * 3600))
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// How long the oldest unanswered frame may wait (`EFFECTCRAFT_HELPER_VIDEO_TIMEOUT`), and
    /// how long `cancelled` may take.
    pub frame_timeout: Duration,
    pub load_timeout: Duration,
    pub stop_wait: Duration,
}

impl Default for Options {
    fn default() -> Options {
        let secs = |key: &str, default: u64| {
            let from_env = std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).filter(|s| *s > 0);
            Duration::from_secs(from_env.unwrap_or(default))
        };
        Options {
            frame_timeout: secs("EFFECTCRAFT_HELPER_VIDEO_TIMEOUT", 30),
            load_timeout: secs("EFFECTCRAFT_HELPER_LOAD_TIMEOUT", 600),
            stop_wait: Duration::from_secs(5),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoError {
    /// The helper refused this job, or the settings were declined before it was opened. The job is
    /// over and the helper is fine.
    Job {
        code: ErrorCode,
        message: String,
    },
    /// The helper crashed, hung or broke the protocol and has been stopped.
    Helper(String),
    Request(String),
}

impl std::fmt::Display for VideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VideoError::Job { code, message } => write!(f, "the video encoder refused the job ({code:?}): {message}"),
            VideoError::Helper(why) | VideoError::Request(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for VideoError {}

pub struct EncodeParams {
    pub device: String,
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub rate: [u32; 2],
    pub bitrate_kbps: u32,
    pub profile: String,
    pub keyint: u32,
    pub colour: Colour,
}

/// An 8-bit 4:2:0 picture, planes tightly packed (`width × height`, then two `width/2 × height/2`).
pub struct Picture<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
}

pub struct Opened {
    pub job: u64,
    /// The `avcC` (H.264) or `hvcC` (HEVC) record for the MP4 sample entry.
    pub codec_config: Vec<u8>,
    pub delay: u32,
}

/// One MP4 sample: length-prefixed NAL units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub pts: i64,
    pub dts: i64,
    pub key: bool,
}

enum Line {
    Reply(Result<VideoReply, String>),
    Closed,
}

struct Conn {
    child: Child,
    /// Request lines for the writer thread, which owns the helper's stdin: a helper that stops
    /// reading can block that thread, never the export waiting for it.
    input: Sender<Vec<u8>>,
    written: Receiver<Result<(), String>>,
    rx: Receiver<Line>,
}

struct Shared {
    exchange: Exchange,
    file: File,
}

struct JobState {
    width: u32,
    height: u32,
    slots: Vec<Slot>,
    delay: u32,
    sent: VecDeque<Instant>,
    packets: VecDeque<EncodedPacket>,
}

pub struct VideoHelper {
    conn: Option<Conn>,
    shared: Option<Shared>,
    jobs: Jobs,
    state: HashMap<u64, JobState>,
    /// The first free byte of the exchange file; reset when no job is open.
    top: u64,
    devices: Vec<Device>,
    opts: Options,
}

/// Job ids are never reused, even across helper processes.
static NEXT_JOB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl VideoHelper {
    pub fn start(program: &Path) -> Result<VideoHelper, String> {
        let mut cmd = Command::new(program);
        cmd.arg(HELPER_ARG);
        VideoHelper::start_with(cmd, Options::default())
    }

    /// For a prepared command (tests give the stand-in helper its settings in the environment).
    pub fn start_with(mut cmd: Command, opts: Options) -> Result<VideoHelper, String> {
        if blocklist::is_blocked(BLOCK_ID) {
            return Err("the video helper was turned off after it stopped 3 times; retry it in Settings".into());
        }
        let exchange = Exchange::create().map_err(|e| format!("can't create the frame exchange file: {e}"))?;
        let file = OpenOptions::new().read(true).write(true).open(exchange.path()).map_err(|e| format!("can't open the frame exchange file: {e}"))?;
        let conn = spawn(&mut cmd)?;
        let first = NEXT_JOB.load(std::sync::atomic::Ordering::Relaxed);
        let mut helper = VideoHelper {
            conn: Some(conn),
            shared: Some(Shared { exchange, file }),
            jobs: Jobs::new(first),
            state: HashMap::new(),
            top: 0,
            devices: vec![],
            opts,
        };
        helper.hello().map_err(|e| e.to_string())?;
        Ok(helper)
    }

    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    pub fn is_running(&self) -> bool {
        self.conn.is_some()
    }

    fn hello(&mut self) -> Result<(), VideoError> {
        let frames = self.shared.as_ref().map(|s| s.exchange.path().to_string_lossy().into_owned()).unwrap_or_default();
        let hello = VideoRequest::Hello { version: PROTOCOL_VERSION, frames, video: VIDEO_VERSION };
        self.send(&hello, self.opts.load_timeout)?;
        let reply = self.next(self.opts.load_timeout)?.ok_or_else(|| self.broke("the video helper did not say hello in time".into()))?;
        match check_hello(reply, env!("CARGO_PKG_VERSION")) {
            Ok(video) => self.devices = video.devices,
            Err(why) => {
                self.stop(false);
                return Err(VideoError::Helper(why));
            }
        }
        // Both sides have it open: nothing is left behind if either dies.
        if let Some(s) = self.shared.as_mut() {
            s.exchange.unlink();
        }
        Ok(())
    }

    /// Start a job. One job at a time: a second open is refused until the first is finished or
    /// cancelled, so every job's error reaches its own caller. Settings the device can't do are
    /// refused here too, before anything is opened.
    pub fn open_encoder(&mut self, p: &EncodeParams) -> Result<Opened, VideoError> {
        self.alive()?;
        if !self.state.is_empty() {
            return Err(VideoError::Request("a video job is already open on this helper: finish or cancel it first".into()));
        }
        self.check_supported(p)?;
        let in_len = u64::from(p.width).checked_mul(u64::from(p.height)).and_then(|n| n.checked_mul(3)).map(|n| n / 2).filter(|n| *n <= MAX_BYTES_IN_FLIGHT);
        let Some(in_len) = in_len else {
            let message = format!("a {}×{} picture is larger than {} MiB", p.width, p.height, MAX_BYTES_IN_FLIGHT >> 20);
            return Err(VideoError::Job { code: ErrorCode::Unsupported, message });
        };
        let out_len = (in_len / 4).max(1 << 20);
        let count = (MAX_BYTES_IN_FLIGHT / in_len).clamp(1, MAX_FRAMES_IN_FLIGHT as u64);
        let (slots, end) = layout(self.top, in_len, out_len, count);
        let build = |job| EncodeOpen {
            job,
            device: p.device.clone(),
            codec: p.codec,
            width: p.width,
            height: p.height,
            rate: p.rate,
            bitrate_kbps: p.bitrate_kbps,
            profile: p.profile.clone(),
            keyint: p.keyint,
            colour: p.colour,
            slots: slots.clone(),
        };
        // A request that can't be written is found before the job, the ids and the regions exist.
        request_line(&VideoRequest::EncodeOpen(build(self.jobs.next_id()))).map_err(VideoError::Request)?;
        self.grow(end)?;
        let msg = self.jobs.open(build).map_err(|e| VideoError::Request(e.to_string()))?;
        let job = msg.job;
        NEXT_JOB.fetch_max(self.jobs.next_id(), std::sync::atomic::Ordering::Relaxed);
        self.top = end;
        self.state.insert(job, JobState { width: p.width, height: p.height, slots, delay: 0, sent: VecDeque::new(), packets: VecDeque::new() });
        self.send(&VideoRequest::EncodeOpen(msg), self.opts.load_timeout)?;
        let (config, delay) = loop {
            match self.receive(self.opts.load_timeout)? {
                Some(Verdict::Opened { job: j, codec_config, delay }) if j == job => break (codec_config, delay),
                Some(other) => self.unexpected(job, other)?,
                None => return Err(self.broke("the video helper did not answer encode.open in time".into())),
            }
        };
        if let Some(st) = self.state.get_mut(&job) {
            st.delay = delay;
        }
        let Some(codec_config) = base64_decode(&config) else {
            return Err(self.broke("the video helper sent a codec_config that is not base64".into()));
        };
        Ok(Opened { job, codec_config, delay })
    }

    /// Send one picture. Waits for a reply when 8 frames or 256 MiB are already out.
    pub fn submit(&mut self, job: u64, picture: &Picture) -> Result<(), VideoError> {
        self.alive()?;
        self.drain(job)?;
        let (width, height) = self.state.get(&job).map(|st| (st.width, st.height)).ok_or_else(|| VideoError::Request("no such video job".into()))?;
        let (w, h) = (u64::from(width), u64::from(height));
        let sizes = [(picture.y.len(), w * h), (picture.u.len(), w * h / 4), (picture.v.len(), w * h / 4)];
        if sizes.iter().any(|(got, want)| *got as u64 != *want) {
            return Err(VideoError::Request("the picture does not match the job's size".into()));
        }
        let (frame, slot) = loop {
            match self.jobs.start_frame(job) {
                Ok(x) => break x,
                Err(SendError::Full) => self.wait_for_reply(job)?,
                Err(e) => return Err(VideoError::Request(e.to_string())),
            }
        };
        let base = self.state.get(&job).and_then(|st| st.slots.get(slot as usize)).map(|s| s.input.offset);
        let base = base.ok_or_else(|| VideoError::Request("no such slot".into()))?;
        let planes = planes(base, width, height);
        for (plane, data) in planes.iter().zip([picture.y, picture.u, picture.v]) {
            if let Err(e) = self.write_at(plane.offset, data) {
                self.jobs.abandon_frame(job);
                return Err(e);
            }
        }
        self.send(&VideoRequest::EncodeFrame { job, frame, slot, planes }, self.opts.frame_timeout)?;
        if let Some(st) = self.state.get_mut(&job) {
            st.sent.push_back(Instant::now());
        }
        Ok(())
    }

    pub fn take_packets(&mut self, job: u64) -> Result<Vec<EncodedPacket>, VideoError> {
        self.alive()?;
        self.drain(job)?;
        let st = self.state.get_mut(&job).ok_or_else(|| VideoError::Request("no such video job".into()))?;
        Ok(st.packets.drain(..).collect())
    }

    /// End the stream: waits for every frame, then returns all packets not taken yet, and closes
    /// the job.
    pub fn finish(&mut self, job: u64) -> Result<Vec<EncodedPacket>, VideoError> {
        self.alive()?;
        while self.state.get(&job).is_some_and(|st| !st.sent.is_empty()) {
            self.wait_for_reply(job)?;
        }
        let (out_len, delay) = match self.state.get(&job) {
            Some(st) => (st.slots.iter().map(|s| s.out.len).max().unwrap_or(0), st.delay),
            None => return Err(VideoError::Request("no such video job".into())),
        };
        let out = Region { offset: align(self.top), len: out_len * (u64::from(delay) + 1) };
        self.grow(out.offset + out.len)?;
        self.top = out.offset + out.len;
        self.jobs.start_flush(job, out).map_err(|e| VideoError::Request(e.to_string()))?;
        self.send(&VideoRequest::EncodeFlush { job, out }, self.opts.load_timeout)?;
        // The batch and `encode.flushed` share one timeout.
        let end = deadline(self.opts.load_timeout);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(self.broke("the video helper did not finish the flush in time".into()));
            }
            match self.receive(left)? {
                Some(Verdict::Flushed { job: j }) if j == job => break,
                Some(Verdict::Packets { job: j, frame: None, .. }) if j == job => {}
                Some(other) => self.unexpected(job, other)?,
                None => return Err(self.broke("the video helper did not finish the flush in time".into())),
            }
        }
        let packets = self.state.get_mut(&job).map(|st| st.packets.drain(..).collect()).unwrap_or_default();
        self.close(job)?;
        Ok(packets)
    }

    /// Stop the job now. The helper has `frame_timeout` to answer `cancelled`, or it is stopped.
    pub fn cancel(&mut self, job: u64) -> Result<(), VideoError> {
        self.alive()?;
        self.jobs.cancel(job).map_err(|e| VideoError::Request(e.to_string()))?;
        self.cancel_sent(job)
    }

    /// Only after `encode.flushed` or `cancelled`: until then the helper may still use the regions.
    fn close(&mut self, job: u64) -> Result<(), VideoError> {
        self.alive()?;
        self.jobs.close(job).map_err(|e| VideoError::Request(e.to_string()))?;
        self.send(&VideoRequest::EncodeClose { job }, self.opts.frame_timeout)?;
        self.state.remove(&job);
        if self.state.is_empty() {
            self.top = 0;
        }
        Ok(())
    }

    fn alive(&self) -> Result<(), VideoError> {
        if self.conn.is_some() { Ok(()) } else { Err(VideoError::Helper("the video helper has stopped".into())) }
    }

    fn check_supported(&self, p: &EncodeParams) -> Result<(), VideoError> {
        let refuse = |why: String| Err(VideoError::Job { code: ErrorCode::Unsupported, message: why });
        let codec = match p.codec {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
        };
        if p.width == 0 || p.height == 0 || !p.width.is_multiple_of(2) || !p.height.is_multiple_of(2) {
            return refuse(format!("{}×{} is not an even size", p.width, p.height));
        }
        let Some(device) = self.devices.iter().find(|d| d.id == p.device) else {
            return refuse(format!("the video helper has no device {}", p.device));
        };
        let Some(caps) = device.encode.iter().find(|c| c.codec == codec) else {
            return refuse(format!("{} can't encode {codec}", device.name));
        };
        let fits = p.width >= caps.min_size[0] && p.height >= caps.min_size[1] && p.width <= caps.max_size[0] && p.height <= caps.max_size[1];
        if !fits {
            return refuse(format!(
                "{} encodes {codec} from {}×{} to {}×{}",
                device.name, caps.min_size[0], caps.min_size[1], caps.max_size[0], caps.max_size[1]
            ));
        }
        Ok(())
    }

    fn wait_for_reply(&mut self, job: u64) -> Result<(), VideoError> {
        let left = self.oldest_wait();
        loop {
            match self.receive(left)? {
                Some(Verdict::Packets { frame: Some(_), .. }) => return Ok(()),
                Some(other) => self.unexpected(job, other)?,
                None => return Err(self.broke(format!("the video helper did not answer a frame within {} s", self.opts.frame_timeout.as_secs_f64()))),
            }
        }
    }

    fn drain(&mut self, job: u64) -> Result<(), VideoError> {
        while let Some(v) = self.receive(Duration::ZERO)? {
            self.unexpected(job, v)?;
        }
        Ok(())
    }

    /// A refused job is cancelled and closed; the other verdicts need nothing here, their effect
    /// was applied when they arrived. The error is returned only for the job the caller is using.
    fn unexpected(&mut self, job: u64, v: Verdict) -> Result<(), VideoError> {
        let Verdict::JobError { job: failed, code, message, .. } = v else { return Ok(()) };
        if self.jobs.cancel(failed).is_ok() {
            self.cancel_sent(failed)?;
        }
        if failed == job { Err(VideoError::Job { code, message }) } else { Ok(()) }
    }

    fn cancel_sent(&mut self, job: u64) -> Result<(), VideoError> {
        self.send(&VideoRequest::Cancel { job }, self.opts.frame_timeout)?;
        loop {
            match self.receive(self.opts.frame_timeout)? {
                Some(Verdict::Cancelled { job: j }) if j == job => break,
                Some(_) => {}
                None => return Err(self.broke("the video helper did not answer cancel in time".into())),
            }
        }
        self.close(job)
    }

    fn oldest_wait(&self) -> Duration {
        let oldest = self.state.values().filter_map(|st| st.sent.front()).min();
        match oldest {
            Some(sent) => self.opts.frame_timeout.saturating_sub(sent.elapsed()),
            None => self.opts.frame_timeout,
        }
    }

    /// A zero `timeout` only looks at what is already queued. A longer one ends at its deadline
    /// even if replies keep arriving.
    fn receive(&mut self, timeout: Duration) -> Result<Option<Verdict>, VideoError> {
        let end = deadline(timeout);
        let mut dropped = 0;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let spent = if timeout.is_zero() { dropped >= MAX_DROPPED_PER_POLL } else { left.is_zero() };
            if spent {
                return Ok(None);
            }
            let Some(reply) = self.next(left)? else { return Ok(None) };
            match self.jobs.receive(&reply) {
                Verdict::Dropped => {
                    dropped += 1;
                    log::info!("video helper: dropped a late reply: {reply:?}");
                }
                Verdict::Break(why) => return Err(self.broke(why)),
                verdict => {
                    self.apply(&verdict)?;
                    return Ok(Some(verdict));
                }
            }
        }
    }

    /// Packets are copied out of the exchange file as soon as they arrive: the slot they sit in
    /// is free for the next frame from this moment.
    fn apply(&mut self, v: &Verdict) -> Result<(), VideoError> {
        let Verdict::Packets { job, frame, packets, .. } = v else { return Ok(()) };
        let mut taken = Vec::with_capacity(packets.len());
        for p in packets {
            let data = match self.read_at(p.offset, p.len) {
                Ok(data) => data,
                Err(e) => {
                    // The frame is answered but its packets are lost: the job can't go on, and the
                    // fault is ours, not the helper's.
                    self.stop(false);
                    return Err(VideoError::Helper(e.to_string()));
                }
            };
            taken.push(EncodedPacket { data, pts: p.pts, dts: p.dts, key: p.key });
        }
        if let Some(st) = self.state.get_mut(job) {
            st.packets.extend(taken);
            if frame.is_some() {
                st.sent.pop_front();
            }
        }
        Ok(())
    }

    fn next(&mut self, timeout: Duration) -> Result<Option<VideoReply>, VideoError> {
        let Some(conn) = self.conn.as_mut() else { return Err(VideoError::Helper("the video helper has stopped".into())) };
        match conn.rx.recv_timeout(timeout) {
            Ok(Line::Reply(Ok(reply))) => Ok(Some(reply)),
            Ok(Line::Reply(Err(why))) => Err(self.broke(why)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Ok(Line::Closed) | Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(50));
                let status = match conn.child.try_wait() {
                    Ok(Some(s)) => s.to_string(),
                    _ => "closed its output".into(),
                };
                Err(self.broke(format!("the video helper stopped ({status})")))
            }
        }
    }

    fn send(&mut self, r: &VideoRequest, timeout: Duration) -> Result<(), VideoError> {
        let line = request_line(r).map_err(VideoError::Request)?;
        let Some(conn) = self.conn.as_ref() else { return Err(VideoError::Helper("the video helper has stopped".into())) };
        let written =
            conn.input.send(line.into_bytes()).map_err(|_| "its input is closed".to_string()).and_then(|()| match conn.written.recv_timeout(timeout) {
                Ok(r) => r,
                Err(RecvTimeoutError::Timeout) => Err(format!("it did not take a request within {} s", timeout.as_secs())),
                Err(RecvTimeoutError::Disconnected) => Err("its input is closed".into()),
            });
        written.map_err(|why| self.broke(format!("can't write to the video helper: {why}")))
    }

    fn broke(&mut self, why: String) -> VideoError {
        self.stop(true);
        VideoError::Helper(why)
    }

    /// Kill the helper, wait for it at most `stop_wait`, and drop the exchange file: whatever
    /// else still has it mapped can only write into a file nobody reads, and the next helper
    /// gets a new one.
    fn stop(&mut self, count: bool) {
        let Some(mut conn) = self.conn.take() else { return };
        self.shared = None;
        let _ = conn.child.kill();
        let started = Instant::now();
        loop {
            match conn.child.try_wait() {
                Ok(None) if started.elapsed() < self.opts.stop_wait => std::thread::sleep(Duration::from_millis(10)),
                Ok(None) => {
                    log::warn!("video helper: still running {} s after it was killed; giving up on it", self.opts.stop_wait.as_secs());
                    break;
                }
                Ok(Some(_)) | Err(_) => break,
            }
        }
        if count && blocklist::note_crash(BLOCK_ID) {
            log::warn!("video helper: stopped {} times, turned off", blocklist::CRASH_LIMIT);
        }
    }

    fn grow(&mut self, len: u64) -> Result<(), VideoError> {
        let Some(s) = self.shared.as_mut() else { return Err(VideoError::Helper("the video helper has stopped".into())) };
        if !s.file.metadata().is_ok_and(|m| m.len() >= len) {
            s.file.set_len(len).map_err(|e| VideoError::Request(format!("frame exchange: {e}")))?;
        }
        Ok(())
    }

    fn write_at(&mut self, at: u64, bytes: &[u8]) -> Result<(), VideoError> {
        let Some(s) = self.shared.as_mut() else { return Err(VideoError::Helper("the video helper has stopped".into())) };
        s.file.seek(SeekFrom::Start(at)).and_then(|_| s.file.write_all(bytes)).map_err(|e| VideoError::Request(format!("frame exchange: {e}")))
    }

    fn read_at(&mut self, at: u64, len: u64) -> Result<Vec<u8>, VideoError> {
        let Some(s) = self.shared.as_mut() else { return Err(VideoError::Helper("the video helper has stopped".into())) };
        let io = |e: std::io::Error| VideoError::Request(format!("frame exchange: {e}"));
        let mut data = vec![0; usize::try_from(len).map_err(|_| VideoError::Request("a packet that large".into()))?];
        s.file.seek(SeekFrom::Start(at)).and_then(|_| s.file.read_exact(&mut data)).map_err(io)?;
        Ok(data)
    }
}

impl Drop for VideoHelper {
    fn drop(&mut self) {
        self.stop(false);
    }
}

fn align(n: u64) -> u64 {
    n.div_ceil(64) * 64
}

/// `count` slots from `base`, 64-byte aligned for helpers that map the file, and the end.
fn layout(base: u64, in_len: u64, out_len: u64, count: u64) -> (Vec<Slot>, u64) {
    let mut at = align(base);
    let slots = (0..count)
        .map(|_| {
            let slot = Slot { input: Region { offset: at, len: in_len }, out: Region { offset: align(at + in_len), len: out_len } };
            at = align(slot.out.offset + out_len);
            slot
        })
        .collect();
    (slots, at)
}

fn planes(base: u64, width: u32, height: u32) -> [Plane; 3] {
    let (w, h) = (u64::from(width), u64::from(height));
    let y = Plane { offset: base, stride: width, size: [width, height], len: w * h };
    let u = Plane { offset: y.offset + y.len, stride: width / 2, size: [width / 2, height / 2], len: w * h / 4 };
    let v = Plane { offset: u.offset + u.len, ..u };
    [y, u, v]
}

fn spawn(cmd: &mut Command) -> Result<Conn, String> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console window for the helper.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("can't start the video helper {:?}: {e}", cmd.get_program()))?;
    let (Some(stdin), Some(stdout), stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("the video helper's pipes are unavailable".into());
    };
    let (tx, rx) = sync_channel(REPLY_QUEUE);
    let reader = std::thread::Builder::new().name("video-helper-out".into()).spawn(move || {
        let mut out = BufReader::new(stdout);
        loop {
            let mut buf = vec![];
            match (&mut out).take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Line::Closed);
                    return;
                }
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf);
                    match parse_reply(&line) {
                        Some(reply) => {
                            if tx.send(Line::Reply(reply)).is_err() {
                                return;
                            }
                        }
                        None => log::info!("video helper: {}", line.trim_end()),
                    }
                }
            }
        }
    });
    let (input, lines) = channel::<Vec<u8>>();
    let (done, written) = channel();
    let writer = std::thread::Builder::new().name("video-helper-in".into()).spawn(move || {
        let mut stdin = stdin;
        for line in lines {
            let r = stdin.write_all(&line).and_then(|()| stdin.flush()).map_err(|e| e.to_string());
            let failed = r.is_err();
            if done.send(r).is_err() || failed {
                return;
            }
        }
    });
    if let Err(e) = reader.and(writer) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("can't start a thread for the video helper: {e}"));
    }
    if let Some(err) = stderr {
        // Drained whatever it holds: a helper blocked on a full stderr pipe would hang.
        let _ = std::thread::Builder::new().name("video-helper-err".into()).spawn(move || {
            let mut r = BufReader::new(err);
            let mut buf = vec![];
            loop {
                buf.clear();
                match (&mut r).take(64 << 10).read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => log::info!("video helper: {}", String::from_utf8_lossy(&buf).trim_end()),
                }
            }
        });
    }
    Ok(Conn { child, input, written, rx })
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let body = s.trim_end_matches('=');
    let padding = s.len() - body.len();
    if padding > 2 || (padding > 0 && !s.len().is_multiple_of(4)) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in body.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // A last group of one character holds 6 bits, not a whole byte.
    (bits < 6).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_with_and_without_padding_and_refuses_other_characters() {
        assert_eq!(base64_decode("AQID"), Some(vec![1, 2, 3]));
        assert_eq!(base64_decode("AQI="), Some(vec![1, 2]));
        assert_eq!(base64_decode("AQ"), Some(vec![1]));
        assert_eq!(base64_decode("+/8="), Some(vec![0xfb, 0xff]));
        assert_eq!(base64_decode("AQ!D"), None);
        assert_eq!(base64_decode("A"), None);
        assert_eq!(base64_decode("AQIDB"), None);
        assert_eq!(base64_decode("AQ=="), Some(vec![1]));
        for padding_only in ["=", "====", "A===", "AQ===", "AQ=D"] {
            assert_eq!(base64_decode(padding_only), None, "{padding_only}");
        }
    }

    /// The planes tile the slot's `in` region exactly, in Y, U, V order, so a helper that reads
    /// them reads the whole picture and nothing else.
    #[test]
    fn a_timeout_too_long_to_represent_does_not_panic() {
        assert!(deadline(Duration::MAX) > Instant::now());
        assert!(deadline(Duration::ZERO) <= Instant::now());
    }

    #[test]
    fn planes_fill_the_picture_region_without_gaps() {
        let [y, u, v] = planes(640, 1920, 1080);
        assert_eq!(y.offset, 640);
        assert_eq!(u.offset, y.offset + y.len);
        assert_eq!(v.offset, u.offset + u.len);
        assert_eq!(y.len + u.len + v.len, 1920 * 1080 * 3 / 2);
        assert_eq!(u.size, [960, 540]);
    }

    #[test]
    fn slots_are_aligned_and_do_not_overlap() {
        let (slots, end) = layout(10, 1000, 300, 3);
        let mut regions: Vec<Region> = slots.iter().flat_map(|s| [s.input, s.out]).collect();
        regions.sort_by_key(|r| r.offset);
        assert!(regions.iter().all(|r| r.offset % 64 == 0));
        assert!(regions.windows(2).all(|w| w[0].end().is_some_and(|e| e <= w[1].offset)));
        assert!(regions.last().and_then(Region::end).is_some_and(|e| e <= end));
    }
}
