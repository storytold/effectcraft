//! The receiver table of `docs/video-helper-protocol.md` ("What EffectCraft does with each
//! reply") as plain logic, with no process and no I/O. The caller asks [`Jobs`] before it sends
//! and hands it every reply ([`Jobs::receive`]); the answer is one [`Verdict`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use super::wire::{EncodeOpen, ErrorCode, PacketRef, Region, Slot, VideoReply};

pub const MAX_FRAMES_IN_FLIGHT: usize = 8;

pub const MAX_BYTES_IN_FLIGHT: u64 = 256 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    UnknownJob,
    NotReady,
    Full,
    FramesOutstanding,
    BadRegion(String),
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendError::UnknownJob => f.write_str("no such open video job"),
            SendError::NotReady => f.write_str("the video job can't take this request now"),
            SendError::Full => f.write_str("too many frames are in flight"),
            SendError::FramesOutstanding => f.write_str("frames are still unanswered"),
            SendError::BadRegion(why) => write!(f, "bad exchange file region: {why}"),
        }
    }
}

/// `offset` is from the start of the exchange file, already checked against the reply's region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    pub offset: u64,
    pub len: u64,
    pub pts: i64,
    pub dts: i64,
    pub key: bool,
}

/// What [`Jobs::receive`] decided about one reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Opened {
        job: u64,
        codec_config: String,
        delay: u32,
    },
    /// `slot` is the one that may be reused now (`None` for the flush batch).
    Packets {
        job: u64,
        frame: Option<u64>,
        slot: Option<u32>,
        packets: Vec<Packet>,
    },
    /// `encode.flushed`: the job is complete; send `encode.close`.
    Flushed {
        job: u64,
    },
    /// The job is over: send `cancel`, then `encode.close`. The helper keeps running; not a crash.
    JobError {
        job: u64,
        frame: Option<u64>,
        code: ErrorCode,
        message: String,
    },
    Cancelled {
        job: u64,
    },
    /// A late reply for an ended job. Log it; the regions it names are not read. Not a break.
    Dropped,
    /// The helper broke the protocol: stop it. Counts towards the 3 stops.
    Break(String),
}

enum Phase {
    Opening,
    Ready,
    Flushing {
        batch: bool,
    },
    Flushed,
    /// A job error arrived; the caller is about to cancel.
    Failed,
}

struct Job {
    phase: Phase,
    slots: Vec<Slot>,
    delay: u32,
    next_frame: u64,
    /// Frames sent and not answered, oldest first, with the slot each uses.
    outstanding: VecDeque<(u64, usize)>,
    flush_out: Option<Region>,
}

pub struct Jobs {
    /// The first id this book issued; ids below it belong to an earlier helper process.
    first: u64,
    next: u64,
    open: BTreeMap<u64, Job>,
    cancelling: BTreeSet<u64>,
}

impl Jobs {
    /// Job ids are never reused, not even after a restart: start from [`Jobs::next_id`] of the earlier book.
    pub fn new(first: u64) -> Jobs {
        let first = first.max(1);
        Jobs { first, next: first, open: BTreeMap::new(), cancelling: BTreeSet::new() }
    }

    pub fn next_id(&self) -> u64 {
        self.next
    }

    pub fn cancelling(&self) -> usize {
        self.cancelling.len()
    }

    fn issued(&self, id: u64) -> bool {
        (self.first..self.next).contains(&id)
    }

    /// `make` builds the `encode.open` message from the new job id; its slots are checked before the job is registered.
    pub fn open(&mut self, make: impl FnOnce(u64) -> EncodeOpen) -> Result<EncodeOpen, SendError> {
        let id = self.next;
        let msg = make(id);
        check_slots(&msg.slots)?;
        self.next = id.checked_add(1).ok_or_else(|| SendError::BadRegion("job ids are used up".into()))?;
        self.open.insert(id, Job { phase: Phase::Opening, slots: msg.slots.clone(), delay: 0, next_frame: 0, outstanding: VecDeque::new(), flush_out: None });
        Ok(msg)
    }

    pub fn start_frame(&mut self, job: u64) -> Result<(u64, u32), SendError> {
        let j = self.open.get_mut(&job).ok_or(SendError::UnknownJob)?;
        if !matches!(j.phase, Phase::Ready) {
            return Err(SendError::NotReady);
        }
        let queued: u64 = j.outstanding.iter().filter_map(|(_, s)| j.slots.get(*s)).map(|s| s.input.len).sum();
        let free = (0..j.slots.len()).find(|i| !j.outstanding.iter().any(|(_, s)| s == i));
        let Some(slot) = free else { return Err(SendError::Full) };
        let bytes = j.slots.get(slot).map_or(0, |s| s.input.len);
        if j.outstanding.len() >= MAX_FRAMES_IN_FLIGHT || queued.saturating_add(bytes) > MAX_BYTES_IN_FLIGHT {
            return Err(SendError::Full);
        }
        let slot_number = u32::try_from(slot).map_err(|_| SendError::BadRegion("too many slots".into()))?;
        let frame = j.next_frame;
        j.next_frame += 1;
        j.outstanding.push_back((frame, slot));
        Ok((frame, slot_number))
    }

    /// Give back the frame [`Jobs::start_frame`] reserved last, when nothing was sent for it.
    pub fn abandon_frame(&mut self, job: u64) {
        if let Some(j) = self.open.get_mut(&job)
            && j.outstanding.pop_back().is_some()
        {
            j.next_frame -= 1;
        }
    }

    /// Allowed once every frame is answered; `out` must hold `delay + 1` slots' worth of output.
    pub fn start_flush(&mut self, job: u64, out: Region) -> Result<(), SendError> {
        let j = self.open.get_mut(&job).ok_or(SendError::UnknownJob)?;
        if !matches!(j.phase, Phase::Ready) {
            return Err(SendError::NotReady);
        }
        if !j.outstanding.is_empty() {
            return Err(SendError::FramesOutstanding);
        }
        let slot_out = j.slots.iter().map(|s| s.out.len).max().unwrap_or(0);
        let need = slot_out.checked_mul(u64::from(j.delay) + 1);
        if out.end().is_none() || need.is_none_or(|n| out.len < n) {
            return Err(SendError::BadRegion("the flush region is too small for the frames the encoder holds".into()));
        }
        j.phase = Phase::Flushing { batch: false };
        j.flush_out = Some(out);
        Ok(())
    }

    pub fn cancel(&mut self, job: u64) -> Result<(), SendError> {
        if self.open.remove(&job).is_some() {
            self.cancelling.insert(job);
            Ok(())
        } else if self.cancelling.contains(&job) {
            Ok(())
        } else {
            Err(SendError::UnknownJob)
        }
    }

    pub fn close(&mut self, job: u64) -> Result<(), SendError> {
        if self.open.remove(&job).is_some() || self.issued(job) { Ok(()) } else { Err(SendError::UnknownJob) }
    }

    /// Checked in this order: a never-issued id is a break; `cancelled` is accepted only for a
    /// job we cancelled; anything else for an ended job is dropped; the rest depends on the state.
    pub fn receive(&mut self, reply: &VideoReply) -> Verdict {
        let job = match reply {
            VideoReply::Hello { .. } => return Verdict::Break("a second hello".into()),
            VideoReply::EncodeOpened { job, .. }
            | VideoReply::EncodePackets { job, .. }
            | VideoReply::EncodeFlushed { job }
            | VideoReply::Cancelled { job }
            | VideoReply::Error { job, .. } => *job,
        };
        if !self.issued(job) {
            return Verdict::Break(format!("a reply for job {job}, which was never issued"));
        }
        if matches!(reply, VideoReply::Cancelled { .. }) {
            if self.cancelling.remove(&job) {
                return Verdict::Cancelled { job };
            }
            return if self.open.contains_key(&job) {
                Verdict::Break(format!("job {job} was cancelled, but EffectCraft did not cancel it"))
            } else {
                Verdict::Dropped
            };
        }
        match self.open.get_mut(&job) {
            Some(j) => j.accept(job, reply),
            None => Verdict::Dropped,
        }
    }
}

impl Job {
    fn accept(&mut self, job: u64, reply: &VideoReply) -> Verdict {
        match (&self.phase, reply) {
            (Phase::Failed, _) => Verdict::Dropped,
            (Phase::Opening, VideoReply::EncodeOpened { codec_config, delay, .. }) => {
                self.phase = Phase::Ready;
                self.delay = *delay;
                Verdict::Opened { job, codec_config: codec_config.clone(), delay: *delay }
            }
            (Phase::Ready, VideoReply::EncodePackets { frame: Some(f), packets, .. }) => self.frame_reply(job, *f, packets),
            (Phase::Flushing { batch: false }, VideoReply::EncodePackets { frame: None, packets, .. }) => {
                let Some(out) = self.flush_out else { return Verdict::Break("a flush reply with no flush region".into()) };
                match place(&out, packets) {
                    Ok(packets) => {
                        self.phase = Phase::Flushing { batch: true };
                        Verdict::Packets { job, frame: None, slot: None, packets }
                    }
                    Err(why) => Verdict::Break(why),
                }
            }
            (Phase::Flushing { batch: true }, VideoReply::EncodeFlushed { .. }) => {
                self.phase = Phase::Flushed;
                Verdict::Flushed { job }
            }
            (Phase::Ready, VideoReply::Error { frame: Some(f), code, message, .. }) => match self.outstanding.front() {
                Some((oldest, _)) if oldest == f => {
                    self.outstanding.pop_front();
                    self.phase = Phase::Failed;
                    Verdict::JobError { job, frame: Some(*f), code: *code, message: message.clone() }
                }
                _ => Verdict::Break(self.out_of_order(*f)),
            },
            (Phase::Opening | Phase::Flushing { .. }, VideoReply::Error { frame: None, code, message, .. }) => {
                self.phase = Phase::Failed;
                Verdict::JobError { job, frame: None, code: *code, message: message.clone() }
            }
            (_, other) => Verdict::Break(format!("a reply nobody asked for: {}", describe(other))),
        }
    }

    fn frame_reply(&mut self, job: u64, frame: u64, packets: &[PacketRef]) -> Verdict {
        let slot = match self.outstanding.front() {
            Some((oldest, slot)) if *oldest == frame => *slot,
            _ => return Verdict::Break(self.out_of_order(frame)),
        };
        let Some(out) = self.slots.get(slot).map(|s| s.out) else {
            return Verdict::Break("a frame in a slot that does not exist".into());
        };
        match place(&out, packets) {
            Ok(packets) => {
                self.outstanding.pop_front();
                Verdict::Packets { job, frame: Some(frame), slot: u32::try_from(slot).ok(), packets }
            }
            Err(why) => Verdict::Break(why),
        }
    }

    fn out_of_order(&self, frame: u64) -> String {
        match self.outstanding.front() {
            Some((oldest, _)) => format!("a reply for frame {frame}; the oldest unanswered frame is {oldest}"),
            None => format!("a reply for frame {frame}, but no frame is unanswered"),
        }
    }
}

fn describe(r: &VideoReply) -> &'static str {
    match r {
        VideoReply::Hello { .. } => "hello",
        VideoReply::EncodeOpened { .. } => "encode.opened",
        VideoReply::EncodePackets { frame: None, .. } => "encode.packets (flush batch)",
        VideoReply::EncodePackets { .. } => "encode.packets",
        VideoReply::EncodeFlushed { .. } => "encode.flushed",
        VideoReply::Cancelled { .. } => "cancelled",
        VideoReply::Error { .. } => "error",
    }
}

/// Packets are distinct pieces of the region, so together they can't be longer than it: without
/// this, thousands of references to the same bytes would make the host copy them thousands of times.
fn place(region: &Region, packets: &[PacketRef]) -> Result<Vec<Packet>, String> {
    let placed: Vec<Packet> = packets
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let inside = p.offset.checked_add(p.len).is_some_and(|end| end <= region.len);
            let at = region.offset.checked_add(p.offset).filter(|_| inside);
            at.map(|offset| Packet { offset, len: p.len, pts: p.pts, dts: p.dts, key: p.key })
                .ok_or_else(|| format!("packet {i} (offset {}, length {}) is outside its {}-byte region", p.offset, p.len, region.len))
        })
        .collect::<Result<_, _>>()?;
    let total = placed.iter().try_fold(0u64, |sum, p| sum.checked_add(p.len));
    if total.is_none_or(|t| t > region.len) {
        return Err(format!("{} packets add up to more than their {}-byte region", placed.len(), region.len));
    }
    Ok(placed)
}

fn check_slots(slots: &[Slot]) -> Result<(), SendError> {
    let bad = |why: &str| Err(SendError::BadRegion(why.into()));
    if slots.is_empty() || slots.len() > MAX_FRAMES_IN_FLIGHT {
        return bad("a job needs 1 to 8 slots");
    }
    let mut regions = Vec::with_capacity(slots.len() * 2);
    for s in slots {
        if s.input.len == 0 || s.out.len == 0 || s.input.len > MAX_BYTES_IN_FLIGHT {
            return bad("a slot's regions must be non-empty and a picture at most 256 MiB");
        }
        regions.push(s.input);
        regions.push(s.out);
    }
    regions.sort_by_key(|r| r.offset);
    for pair in regions.windows(2) {
        let [a, b] = pair else { continue };
        match a.end() {
            Some(end) if end <= b.offset => {}
            _ => return bad("regions overlap or run past the end of the file"),
        }
    }
    match regions.last().and_then(Region::end) {
        Some(_) => Ok(()),
        None => bad("regions run past the end of the file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::wire::{Codec, Colour, ColourRange};

    const PICTURE: u64 = 3_110_400;
    const OUT: u64 = 1 << 20;

    fn slots(n: u64) -> Vec<Slot> {
        (0..n)
            .map(|i| {
                let at = i * (PICTURE + OUT);
                Slot { input: Region { offset: at, len: PICTURE }, out: Region { offset: at + PICTURE, len: OUT } }
            })
            .collect()
    }

    fn open_msg(job: u64, slots: Vec<Slot>) -> EncodeOpen {
        EncodeOpen {
            job,
            device: "nvenc:0".into(),
            codec: Codec::H264,
            width: 1920,
            height: 1080,
            rate: [30, 1],
            bitrate_kbps: 15_000,
            profile: "high".into(),
            keyint: 60,
            colour: Colour { primaries: 1, transfer: 1, matrix: 1, range: ColourRange::Limited },
            slots,
        }
    }

    fn pkt(offset: u64, len: u64, pts: i64) -> PacketRef {
        PacketRef { offset, len, pts, dts: pts, key: pts == 0 }
    }

    fn opened(job: u64, delay: u32) -> VideoReply {
        VideoReply::EncodeOpened { job, codec_config: "AQID".into(), delay }
    }

    fn packets(job: u64, frame: Option<u64>, packets: Vec<PacketRef>) -> VideoReply {
        VideoReply::EncodePackets { job, frame, packets }
    }

    fn ready(slot_count: u64, delay: u32) -> (Jobs, u64) {
        let mut jobs = Jobs::new(1);
        let id = jobs.open(|id| open_msg(id, slots(slot_count))).unwrap().job;
        assert!(matches!(jobs.receive(&opened(id, delay)), Verdict::Opened { .. }));
        (jobs, id)
    }

    fn is_break(v: &Verdict) -> bool {
        matches!(v, Verdict::Break(_))
    }

    #[test]
    fn a_normal_job_runs_from_open_to_flushed() {
        let (mut jobs, id) = ready(2, 1);
        let (f0, s0) = jobs.start_frame(id).unwrap();
        let (f1, s1) = jobs.start_frame(id).unwrap();
        assert_eq!((f0, f1), (0, 1));
        assert_ne!(s0, s1, "frames in flight use different slots");
        // The encoder holds frame 0 back: an empty reply, and the slot is free again.
        let Verdict::Packets { slot, packets: p, .. } = jobs.receive(&packets(id, Some(0), vec![])) else { panic!() };
        assert_eq!((slot, p.len()), (Some(s0), 0));
        // Offsets in a reply are counted from the slot's `out` region; the verdict is in file terms.
        let Verdict::Packets { packets: p, .. } = jobs.receive(&packets(id, Some(1), vec![pkt(16, 100, 0)])) else { panic!() };
        let slot1_out = slots(2).get(s1 as usize).unwrap().out.offset;
        assert_eq!(p.first().map(|p| p.offset), Some(slot1_out + 16));
        let out = Region { offset: 10 * PICTURE, len: 2 * OUT };
        jobs.start_flush(id, out).unwrap();
        let Verdict::Packets { frame: None, slot: None, packets: p, .. } = jobs.receive(&packets(id, None, vec![pkt(0, 50, 1)])) else { panic!() };
        assert_eq!(p.first().map(|p| p.offset), Some(out.offset));
        assert!(matches!(jobs.receive(&VideoReply::EncodeFlushed { job: id }), Verdict::Flushed { .. }));
        jobs.close(id).unwrap();
        assert_eq!(jobs.receive(&VideoReply::EncodeFlushed { job: id }), Verdict::Dropped);
    }

    #[test]
    fn the_budget_is_8_frames_and_256_mib_and_a_reply_frees_a_slot() {
        let (mut jobs, id) = ready(8, 0);
        for _ in 0..8 {
            jobs.start_frame(id).unwrap();
        }
        assert_eq!(jobs.start_frame(id), Err(SendError::Full));
        assert!(matches!(jobs.receive(&packets(id, Some(0), vec![])), Verdict::Packets { .. }));
        assert_eq!(jobs.start_frame(id).map(|(frame, _)| frame), Ok(8));

        // Few huge pictures hit the byte budget before the frame count.
        let big = |i: u64| Slot { input: Region { offset: i * (200 << 20), len: 200 << 20 }, out: Region { offset: 8 * (200 << 20) + i * OUT, len: OUT } };
        let mut jobs = Jobs::new(1);
        let id = jobs.open(|id| open_msg(id, vec![big(0), big(1)])).unwrap().job;
        jobs.receive(&opened(id, 0));
        jobs.start_frame(id).unwrap();
        assert_eq!(jobs.start_frame(id), Err(SendError::Full), "2 x 200 MiB is over 256 MiB");
    }

    #[test]
    fn packets_that_together_exceed_their_region_are_a_break_even_if_each_fits() {
        let (mut jobs, id) = ready(1, 0);
        jobs.start_frame(id).unwrap();
        let each = OUT / 2 + 1;
        let overlapping = vec![pkt(0, each, 0), pkt(0, each, 1)];
        assert!(is_break(&jobs.receive(&packets(id, Some(0), overlapping))));
        let (mut jobs, id) = ready(1, 0);
        jobs.start_frame(id).unwrap();
        let fitting = vec![pkt(0, OUT / 2, 0), pkt(OUT / 2, OUT / 2, 1)];
        assert!(matches!(jobs.receive(&packets(id, Some(0), fitting)), Verdict::Packets { .. }));
    }

    #[test]
    fn an_unsent_frame_can_be_given_back_and_the_next_one_takes_its_place() {
        let (mut jobs, id) = ready(1, 0);
        let first = jobs.start_frame(id).unwrap();
        jobs.abandon_frame(id);
        assert_eq!(jobs.start_frame(id), Ok(first), "same number and slot");
        assert!(matches!(jobs.receive(&packets(id, Some(first.0), vec![])), Verdict::Packets { .. }));
        jobs.abandon_frame(id);
        assert_eq!(jobs.start_frame(id).map(|(frame, _)| frame), Ok(1), "an answered frame is not given back");
    }

    #[test]
    fn replies_must_come_in_frame_order_and_match_a_frame_in_flight() {
        let (mut jobs, id) = ready(2, 0);
        jobs.start_frame(id).unwrap();
        jobs.start_frame(id).unwrap();
        assert!(is_break(&jobs.receive(&packets(id, Some(1), vec![]))), "frame 1 before frame 0");
        assert!(is_break(&jobs.receive(&packets(id, Some(7), vec![]))), "a frame never sent");
        jobs.receive(&packets(id, Some(0), vec![]));
        assert!(is_break(&jobs.receive(&packets(id, Some(0), vec![]))), "frame 0 answered twice");
        let (mut jobs, id) = ready(2, 0);
        jobs.start_frame(id).unwrap();
        jobs.start_frame(id).unwrap();
        let err = |frame| VideoReply::Error { job: id, frame: Some(frame), code: ErrorCode::Encoder, message: String::new() };
        assert!(is_break(&jobs.receive(&err(1))));
        assert!(matches!(jobs.receive(&err(0)), Verdict::JobError { frame: Some(0), .. }));
    }

    #[test]
    fn a_packet_outside_its_region_is_a_break_but_packet_too_large_is_a_job_error() {
        for bad in [pkt(OUT, 1, 0), pkt(0, OUT + 1, 0), pkt(u64::MAX, 2, 0), pkt(1, u64::MAX, 0)] {
            let (mut jobs, id) = ready(1, 0);
            jobs.start_frame(id).unwrap();
            assert!(is_break(&jobs.receive(&packets(id, Some(0), vec![pkt(0, 8, 0), bad]))), "{bad:?}");
        }
        // The helper says the output does not fit and did not write past the region.
        let (mut jobs, id) = ready(1, 0);
        jobs.start_frame(id).unwrap();
        let too_large = VideoReply::Error { job: id, frame: Some(0), code: ErrorCode::PacketTooLarge, message: "x".into() };
        assert!(matches!(jobs.receive(&too_large), Verdict::JobError { code: ErrorCode::PacketTooLarge, .. }));
        assert_eq!(jobs.receive(&packets(id, Some(1), vec![])), Verdict::Dropped);
        assert_eq!(jobs.start_frame(id), Err(SendError::NotReady));
    }

    #[test]
    fn cancel_drops_late_replies_and_accepts_one_cancelled() {
        let (mut jobs, id) = ready(2, 0);
        jobs.start_frame(id).unwrap();
        jobs.cancel(id).unwrap();
        assert_eq!(jobs.cancelling(), 1);
        assert_eq!(jobs.receive(&packets(id, Some(0), vec![pkt(0, 4, 0)])), Verdict::Dropped, "in flight when we cancelled");
        assert_eq!(jobs.receive(&VideoReply::Cancelled { job: id }), Verdict::Cancelled { job: id });
        assert_eq!(jobs.cancelling(), 0);
        jobs.close(id).unwrap();
        assert_eq!(jobs.receive(&VideoReply::Cancelled { job: id }), Verdict::Dropped, "a second cancelled");

        let (mut jobs, id) = ready(1, 0);
        assert!(is_break(&jobs.receive(&VideoReply::Cancelled { job: id })));
    }

    #[test]
    fn ended_jobs_are_dropped_and_never_issued_jobs_are_breaks() {
        let (mut jobs, id) = ready(1, 0);
        jobs.close(id).unwrap();
        assert_eq!(jobs.receive(&VideoReply::EncodeFlushed { job: id }), Verdict::Dropped);
        for never in [0, id + 1, 99] {
            assert!(is_break(&jobs.receive(&VideoReply::EncodeFlushed { job: never })), "job {never}");
        }
        // Ids go on from where the earlier helper process stopped: its ids are not ours.
        let mut next = Jobs::new(jobs.next_id());
        assert!(is_break(&next.receive(&VideoReply::EncodeFlushed { job: id })));
        let again = next.open(|j| open_msg(j, slots(1))).unwrap().job;
        assert!(again > id);
    }

    #[test]
    fn the_flush_has_one_batch_then_one_flushed_and_waits_for_every_frame() {
        let flush_out = Region { offset: 20 * PICTURE, len: OUT };
        let (mut jobs, id) = ready(2, 0);
        jobs.start_frame(id).unwrap();
        assert_eq!(jobs.start_flush(id, flush_out), Err(SendError::FramesOutstanding));
        jobs.receive(&packets(id, Some(0), vec![]));
        assert!(matches!(jobs.start_flush(id, Region { len: OUT - 1, ..flush_out }), Err(SendError::BadRegion(_))));
        jobs.start_flush(id, flush_out).unwrap();
        assert_eq!(jobs.start_frame(id), Err(SendError::NotReady), "no frames after a flush");

        let flushed = VideoReply::EncodeFlushed { job: id };
        assert!(is_break(&jobs.receive(&flushed)), "flushed before the batch");
        let (mut jobs, id) = ready(2, 0);
        jobs.start_flush(id, flush_out).unwrap();
        assert!(matches!(jobs.receive(&packets(id, None, vec![])), Verdict::Packets { .. }));
        assert!(is_break(&jobs.receive(&packets(id, None, vec![]))), "a second batch");
        assert!(matches!(jobs.receive(&VideoReply::EncodeFlushed { job: id }), Verdict::Flushed { .. }));
        assert!(is_break(&jobs.receive(&VideoReply::EncodeFlushed { job: id })), "a second flushed before the close");
        let (mut jobs, id) = ready(2, 0);
        jobs.start_flush(id, flush_out).unwrap();
        assert!(is_break(&jobs.receive(&packets(id, None, vec![pkt(0, OUT + 1, 0)]))));
    }

    #[test]
    fn replies_nobody_asked_for_are_breaks() {
        let (mut jobs, id) = ready(1, 0);
        assert!(is_break(&jobs.receive(&packets(id, None, vec![]))));
        assert!(is_break(&jobs.receive(&VideoReply::EncodeFlushed { job: id })));
        assert!(is_break(&jobs.receive(&opened(id, 0))));
        let mut jobs = Jobs::new(1);
        let id = jobs.open(|id| open_msg(id, slots(1))).unwrap().job;
        assert!(is_break(&jobs.receive(&packets(id, Some(0), vec![]))));
        let err = VideoReply::Error { job: id, frame: None, code: ErrorCode::Unsupported, message: String::new() };
        assert!(matches!(jobs.receive(&err), Verdict::JobError { code: ErrorCode::Unsupported, .. }));
        let (mut jobs, id) = ready(1, 0);
        let err = VideoReply::Error { job: id, frame: None, code: ErrorCode::Encoder, message: String::new() };
        assert!(is_break(&jobs.receive(&err)));
    }

    #[test]
    fn unusable_slots_are_refused_before_the_job_starts() {
        let mut jobs = Jobs::new(1);
        let mut overlapping = slots(2);
        if let Some(s) = overlapping.get_mut(1) {
            s.input.offset = 0;
        }
        for bad in [vec![], slots(9), overlapping, vec![Slot { input: Region { offset: u64::MAX, len: 2 }, out: Region { offset: 0, len: 1 } }]] {
            assert!(matches!(jobs.open(|id| open_msg(id, bad)), Err(SendError::BadRegion(_))));
        }
        assert_eq!(jobs.next_id(), 1, "a refused job uses no id");
    }
}
