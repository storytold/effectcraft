//! Movie export: FilmCraft H.264, HEVC (`effectcraft-hevcenc`) and AV1 (`effectcraft-av1enc`)
//! → MP4 (+ AAC), ProRes and uncompressed `raw ` → MOV (+ PCM), muxed by
//! FilmCraft's ISO BMFF / QuickTime writer.

use std::io::Write;

use effectcraft_project::Comp;
use effectcraft_project::render_queue::{AudioFormat, Channels, OutputFormat, ProResProfile};
use effectcraft_time::{FrameRate, Tick};
use filmcraft_isobmff::{Brand, FourCc, Mp4Writer, PcmConfig, SampleEntry, TrackConfig, WriteSample, WriterOptions};
use rayon::prelude::*;

use crate::{Cx, ExportError, Report, Result, State, batch_size, wants_audio};

fn enc(e: impl std::fmt::Display) -> ExportError {
    ExportError::Encode(e.to_string())
}
fn mux_err(e: impl std::fmt::Display) -> ExportError {
    ExportError::Io(e.to_string())
}

pub(crate) struct Packet {
    pub(crate) data: Vec<u8>,
    pub(crate) key: bool,
    /// pts − dts in the track timescale.
    pub(crate) cto: i32,
}

/// A frame for the movie encoder: 8-bit RGBA, or 8-bit 4:2:0 planes already converted on the
/// render threads (for encoders whose [`VideoEncoder::takes_yuv420`] is true).
pub(crate) enum Pixels {
    Rgba(Vec<u8>),
    Yuv420(Yuv420),
}

/// BT.709 limited-range 8-bit 4:2:0 planes ([`rgba_to_yuv420`], or Y then U and V from the
/// accelerator).
pub(crate) enum Yuv420 {
    Planes { y: Vec<u8>, u: Vec<u8>, v: Vec<u8> },
    Packed(Vec<u8>),
}

impl Yuv420 {
    fn from_rgba(rgba: &[u8], w: usize, h: usize) -> Yuv420 {
        let (mut y, mut u, mut v) = (vec![], vec![], vec![]);
        rgba_to_yuv420(rgba, w, h, &mut y, &mut u, &mut v);
        Yuv420::Planes { y, u, v }
    }
    fn slices(&self, w: usize, h: usize) -> (&[u8], &[u8], &[u8]) {
        match self {
            Yuv420::Planes { y, u, v } => (y, u, v),
            Yuv420::Packed(p) => {
                let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
                let (y, uv) = p.split_at(w * h);
                let (u, v) = uv.split_at(cw * ch);
                (y, u, v)
            }
        }
    }
}

pub(crate) trait VideoEncoder: Send {
    fn sample_entry(&self) -> SampleEntry;
    fn encode(&mut self, rgba: &[u8], index: u64) -> Result<Vec<Packet>>;
    /// Whether frames should come as [`Pixels::Yuv420`] (converted while other frames render).
    fn takes_yuv420(&self) -> bool {
        false
    }
    fn encode_yuv420(&mut self, _frame: &Yuv420, _index: u64) -> Result<Vec<Packet>> {
        Err(ExportError::Unsupported("this encoder takes RGBA frames".into()))
    }
    fn flush(&mut self) -> Result<Vec<Packet>>;
    /// Edit-list media start (B-frame delay) in the track timescale.
    fn media_start(&self) -> Option<i64> {
        None
    }
}

// ---------------------------------------------------------------- H.264

struct H264 {
    enc: filmcraft_h264enc::Encoder,
    w: u32,
    h: u32,
    rate: FrameRate,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl H264 {
    fn new(w: u32, h: u32, rate: FrameRate, kbps: u32) -> Result<H264> {
        let mut cfg = filmcraft_h264enc::EncoderConfig::new(w, h, rate.num as u32, rate.den as u32);
        cfg.format = filmcraft_h264enc::PacketFormat::LengthPrefixed;
        cfg.aud = false;
        cfg.keyint = (rate.as_f64() * 2.0).round().max(1.0) as u32;
        let kbps = kbps.max(100);
        cfg.rate = filmcraft_h264enc::RateControl::Vbr { target_kbps: kbps, max_kbps: kbps * 3 / 2 };
        let enc = filmcraft_h264enc::Encoder::new(cfg).map_err(enc)?;
        Ok(H264 { enc, w, h, rate, y: vec![], u: vec![], v: vec![] })
    }
    fn packets(&self, ps: Vec<filmcraft_h264enc::Packet>) -> Vec<Packet> {
        ps.into_iter().map(|p| Packet { data: p.data, key: p.keyframe, cto: (p.pts - p.dts) as i32 }).collect()
    }
}

impl VideoEncoder for H264 {
    fn sample_entry(&self) -> SampleEntry {
        let cfg = filmcraft_isobmff::AvcConfig::parse(&self.enc.avcc()).unwrap_or_else(|_| {
            let (sps, pps) = self.enc.sps_pps();
            filmcraft_isobmff::AvcConfig::new(vec![sps], vec![pps], 4)
        });
        SampleEntry::avc(cfg, self.w as u16, self.h as u16)
    }
    fn encode(&mut self, rgba: &[u8], index: u64) -> Result<Vec<Packet>> {
        let (mut y, mut u, mut v) = (std::mem::take(&mut self.y), std::mem::take(&mut self.u), std::mem::take(&mut self.v));
        rgba_to_yuv420(rgba, self.w as usize, self.h as usize, &mut y, &mut u, &mut v);
        let planes = Yuv420::Planes { y, u, v };
        let r = self.encode_yuv420(&planes, index);
        if let Yuv420::Planes { y, u, v } = planes {
            (self.y, self.u, self.v) = (y, u, v);
        }
        r
    }
    fn takes_yuv420(&self) -> bool {
        true
    }
    fn encode_yuv420(&mut self, p: &Yuv420, index: u64) -> Result<Vec<Packet>> {
        let (w, cw) = (self.w as usize, (self.w as usize).div_ceil(2));
        let (y, u, v) = p.slices(w, self.h as usize);
        let frame = filmcraft_h264enc::YuvFrame { y, u, v, y_stride: w, uv_stride: cw };
        let ps = self.enc.try_encode(&frame, index as i64 * self.rate.den).map_err(enc)?;
        Ok(self.packets(ps))
    }
    fn flush(&mut self) -> Result<Vec<Packet>> {
        let ps = self.enc.flush();
        Ok(self.packets(ps))
    }
    fn media_start(&self) -> Option<i64> {
        (self.enc.delay() > 0).then_some(self.rate.den)
    }
}

pub(crate) use effectcraft_raster::yuv::rgba_to_yuv420;

// ---------------------------------------------------------------- ProRes

struct ProRes {
    enc: filmcraft_prores::Encoder,
    profile: filmcraft_prores::Profile,
    w: u32,
    h: u32,
    alpha: bool,
}

fn prores_profile(p: ProResProfile) -> filmcraft_prores::Profile {
    use filmcraft_prores::Profile;
    match p {
        ProResProfile::Proxy => Profile::Proxy,
        ProResProfile::Lt => Profile::Lt,
        ProResProfile::Standard => Profile::Standard,
        ProResProfile::Hq => Profile::Hq,
        ProResProfile::P4444 => Profile::P4444,
        ProResProfile::P4444Xq => Profile::P4444Xq,
    }
}

impl ProRes {
    fn new(w: u32, h: u32, profile: ProResProfile, channels: Channels) -> ProRes {
        // Alpha needs a 4444 flavour.
        let profile = if channels == Channels::Rgba && !profile.is_4444() { ProResProfile::P4444 } else { profile };
        let profile = prores_profile(profile);
        let alpha = channels == Channels::Rgba;
        let mut cfg = filmcraft_prores::EncoderConfig::new(profile, w, h);
        cfg.encode_alpha = alpha;
        ProRes { enc: filmcraft_prores::Encoder::with_config(cfg), profile, w, h, alpha }
    }
    fn is_444(&self) -> bool {
        matches!(self.profile, filmcraft_prores::Profile::P4444 | filmcraft_prores::Profile::P4444Xq)
    }
}

impl VideoEncoder for ProRes {
    fn sample_entry(&self) -> SampleEntry {
        SampleEntry::prores(FourCc(self.profile.fourcc()), self.w as u16, self.h as u16)
    }
    fn encode(&mut self, rgba: &[u8], _index: u64) -> Result<Vec<Packet>> {
        let chroma = if self.is_444() { filmcraft_prores::ChromaFormat::Yuv444 } else { filmcraft_prores::ChromaFormat::Yuv422 };
        let mut fr = filmcraft_prores::Frame::new(self.w, self.h, chroma, 10, self.alpha);
        rgba_to_yuv_10(rgba, self.w as usize, self.h as usize, self.is_444(), &mut fr.y, &mut fr.cb, &mut fr.cr);
        if let Some(a) = fr.alpha.as_mut() {
            for (d, s) in a.iter_mut().zip(rgba.as_chunks::<4>().0.iter()) {
                *d = ((s[3] as u32 * 1023 + 127) / 255) as u16;
            }
        }
        let data = self.enc.encode(&fr).map_err(enc)?;
        Ok(vec![Packet { data, key: true, cto: 0 }])
    }
    fn flush(&mut self) -> Result<Vec<Packet>> {
        Ok(vec![])
    }
}

/// BT.709 limited-range 10-bit 4:2:2 (horizontal chroma average) or 4:4:4 from RGBA8.
fn rgba_to_yuv_10(rgba: &[u8], w: usize, h: usize, full_chroma: bool, y: &mut [u16], cb: &mut [u16], cr: &mut [u16]) {
    let cw = if full_chroma { w } else { w.div_ceil(2) };
    debug_assert!(y.len() >= w * h && rgba.len() >= w * h * 4);
    y.par_chunks_mut(w).zip(cb.par_chunks_mut(cw).zip(cr.par_chunks_mut(cw))).enumerate().for_each(|(row, (yr, (cbr, crr)))| {
        let src = &rgba[row * w * 4..(row + 1) * w * 4];
        let mut us = vec![0f32; w];
        let mut vs = vec![0f32; w];
        for x in 0..w {
            let (r, g, b) = (src[x * 4] as f32 / 255.0, src[x * 4 + 1] as f32 / 255.0, src[x * 4 + 2] as f32 / 255.0);
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            yr[x] = (64.0 + 876.0 * l).round().clamp(4.0, 1019.0) as u16;
            us[x] = (b - l) / 1.8556;
            vs[x] = (r - l) / 1.5748;
        }
        for cx in 0..cw {
            let (u, v) = if full_chroma {
                (us[cx], vs[cx])
            } else {
                let (a, b2) = (cx * 2, (cx * 2 + 1).min(w - 1));
                ((us[a] + us[b2]) * 0.5, (vs[a] + vs[b2]) * 0.5)
            };
            cbr[cx] = (512.0 + 896.0 * u).round().clamp(4.0, 1019.0) as u16;
            crr[cx] = (512.0 + 896.0 * v).round().clamp(4.0, 1019.0) as u16;
        }
    });
}

// ---------------------------------------------------------------- audio

enum Audio {
    Aac(Box<filmcraft_aac::Encoder>),
    Pcm,
}

fn deinterleave(buf: &[f32], channels: usize) -> Vec<Vec<f32>> {
    (0..channels).map(|c| buf.chunks_exact(channels).map(|p| p[c]).collect()).collect()
}

/// The comp's audio over `n` samples from `start`, interleaved with the module's channel count
/// (mono = the average of left and right).
pub(crate) fn mix(cx: &Cx, start: effectcraft_time::Tick, n: usize, sr: u32) -> Vec<f32> {
    let st = effectcraft_render::audio::mix_comp(&cx.project, cx.footage, cx.expr, cx.comp, start, n, sr);
    if cx.output.audio_channels == 1 { st.as_chunks::<2>().0.iter().map(|p| (p[0] + p[1]) * 0.5).collect() } else { st }
}

/// PCM bytes of samples in `fmt` (little or big endian).
pub(crate) fn pcm_bytes(buf: &[f32], fmt: AudioFormat, big_endian: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len() * 4);
    for &s in buf {
        let s = s.clamp(-1.0, 1.0);
        match fmt {
            AudioFormat::S16 => {
                let v = (s * 32767.0).round() as i16;
                out.extend_from_slice(&if big_endian { v.to_be_bytes() } else { v.to_le_bytes() });
            }
            AudioFormat::S24 => {
                let v = (s * 8_388_607.0).round() as i32;
                let b = v.to_le_bytes();
                if big_endian {
                    out.extend_from_slice(&[b[2], b[1], b[0]]);
                } else {
                    out.extend_from_slice(&b[..3]);
                }
            }
            AudioFormat::F32 => out.extend_from_slice(&if big_endian { s.to_be_bytes() } else { s.to_le_bytes() }),
        }
    }
    out
}

// ---------------------------------------------------------------- the movie

type Writer<'f, 's> = Mp4Writer<&'f mut crate::out::Out<'s>>;

pub(crate) async fn movie(job: &Cx<'_>, comp: &Comp, w: u32, h: u32, st: &mut State<'_>) -> Result<Report> {
    let rate = job.settings.rate(comp);
    let fmt = job.output.format;
    let channels = match job.output.channels {
        Channels::Rgba if !matches!(fmt, OutputFormat::ProRes | OutputFormat::Uncompressed) => Channels::Rgb,
        c => c,
    };
    let mut venc: Box<dyn VideoEncoder> = match fmt {
        OutputFormat::H264 => Box::new(H264::new(w, h, rate, job.output.bitrate_kbps)?),
        OutputFormat::Hevc => Box::new(crate::hevc_av1::Hevc::new(w, h, rate, job.output)?),
        OutputFormat::Av1 => Box::new(crate::hevc_av1::Av1::new(w, h, rate, job.output)?),
        OutputFormat::Uncompressed => Box::new(crate::uncompressed::Raw::new(w, h, channels == Channels::Rgba)?),
        _ => Box::new(ProRes::new(w, h, job.output.prores_profile, if channels == Channels::Alpha { Channels::Rgb } else { channels })),
    };
    let brand = if matches!(fmt, OutputFormat::ProRes | OutputFormat::Uncompressed) { Brand::Mov } else { Brand::Mp4 };
    let batch = batch_size();
    let yuv = venc.takes_yuv420();
    let render = |_: u64, img: effectcraft_raster::Image| -> Pixels {
        let px = job.pixels(&img, comp, channels, w, h);
        if yuv { Pixels::Yuv420(Yuv420::from_rgba(&px, w as usize, h as usize)) } else { Pixels::Rgba(px) }
    };
    // H.264 frames the accelerator can convert itself come back as planes (12 MB instead of
    // 133 MB of RGBA f32 read back at 4K); everything else renders and converts as before.
    // Deferred readbacks (a browser worker) keep their pass-based path: no blocking there.
    let gpu_yuv = yuv && channels == Channels::Rgb && !job.deferred();
    let batch_frames = async |ks: Vec<u64>| -> Vec<Pixels> {
        if gpu_yuv { job.h264_frames(comp, ks, w, h, |p| Pixels::Yuv420(Yuv420::Packed(p)), render) } else { job.frames(comp, ks, render).await }
    };
    let encode = |venc: &mut Box<dyn VideoEncoder>, px: &Pixels, index: u64| match px {
        Pixels::Rgba(rgba) => venc.encode(rgba, index),
        Pixels::Yuv420(planes) => venc.encode_yuv420(planes, index),
    };

    let beside = crate::pipeline::Beside::shared();
    let total = st.total;

    // Encode the first batch before creating tracks: encoders finalise their config on frame 1.
    // Each batch encodes while the next one renders.
    let first_end = batch.min(total);
    let first: Vec<Pixels> = batch_frames((0..first_end).collect()).await;
    let next_end = (first_end + batch).min(total);
    let (pending, mut ready) = job
        .beside(beside, batch_frames((first_end..next_end).collect()), || -> Result<Vec<Packet>> {
            let mut pending = Vec::new();
            for (k, px) in first.iter().enumerate() {
                pending.extend(encode(&mut venc, px, k as u64)?);
            }
            Ok(pending)
        })
        .await?;
    let pending = pending?;
    drop(first);
    st.advance(first_end)?;

    let path = job.place(job.path, 1);
    let mut file = job.create(&path)?;
    let opts = WriterOptions::new(brand);
    let movie_ts = opts.movie_timescale.max(1) as i128;
    let mut mux: Writer = Mp4Writer::new(&mut file, opts).map_err(mux_err)?;
    let mut vcfg = TrackConfig::new(venc.sample_entry(), rate.num as u32);
    if let Some(start) = venc.media_start() {
        // An explicit edit: the convenience `media_start` edit would subtract the B-frame delay
        // from the media duration and drop the last frame.
        let frames_ts = st.total as i128 * rate.den as i128;
        let seg = (frames_ts * movie_ts + rate.num as i128 / 2) / rate.num as i128;
        vcfg.edits = vec![filmcraft_isobmff::Edit { segment_duration: seg as u64, media_time: start, media_rate: 0x10000 }];
    }
    let vt = mux.add_track(vcfg).map_err(mux_err)?;
    let sr = job.output.audio_sample_rate.clamp(8_000, 192_000);
    let with_audio = wants_audio(job);
    let chans = if job.output.audio_channels == 1 { 1u32 } else { 2 };
    let afmt = job.output.audio_format;
    let mut audio = None;
    if with_audio {
        let (a, entry, start) = if brand == Brand::Mp4 {
            let e = filmcraft_aac::Encoder::new(filmcraft_aac::EncoderConfig::cbr(sr, chans as usize, 160_000 * chans)).map_err(enc)?;
            let entry = SampleEntry::aac(e.audio_specific_config(), chans, sr);
            let priming = e.priming_samples() as i64;
            (Audio::Aac(Box::new(e)), entry, Some(priming))
        } else {
            let (bits, float) = match afmt {
                AudioFormat::S16 => (16, false),
                AudioFormat::S24 => (24, false),
                AudioFormat::F32 => (32, true),
            };
            let pcm = PcmConfig { bits, float, big_endian: false, signed: true, channels: chans, sample_rate: sr as f64 };
            (Audio::Pcm, SampleEntry::pcm(pcm), None)
        };
        let mut c = TrackConfig::new(entry, sr);
        c.media_start = start;
        let at = mux.add_track(c).map_err(mux_err)?;
        audio = Some((a, at));
    }
    let (span_start, span_end) = job.settings.span(comp);
    let mut cursor = span_start.to_units_floor(sr as i64);

    let write_video = |mux: &mut Writer, ps: Vec<Packet>| -> Result<()> {
        for p in ps {
            mux.write_sample(vt, WriteSample { data: &p.data, duration: rate.den as u32, composition_offset: p.cto, is_sync: p.key }).map_err(mux_err)?;
        }
        Ok(())
    };
    let mut write_audio_until = |mux: &mut Writer, until: Tick, cursor: &mut i64| -> Result<()> {
        let Some((a, at)) = audio.as_mut() else { return Ok(()) };
        let end = until.to_units_floor(sr as i64);
        if end <= *cursor {
            return Ok(());
        }
        let n = (end - *cursor) as usize;
        let start = Tick::from_units(*cursor, i64::from(sr));
        let start = if start.to_units_floor(i64::from(sr)) < *cursor { Tick(start.0.saturating_add(1)) } else { start };
        let buf = mix(job, start, n, sr);
        *cursor = end;
        match a {
            Audio::Aac(e) => {
                let planar = deinterleave(&buf, chans as usize);
                let refs: Vec<&[f32]> = planar.iter().map(Vec::as_slice).collect();
                for au in e.encode(&refs) {
                    mux.write_sample(*at, WriteSample { data: &au, duration: 1024, composition_offset: 0, is_sync: true }).map_err(mux_err)?;
                }
            }
            Audio::Pcm => {
                let pcm = pcm_bytes(&buf, afmt, false);
                mux.write_sample(*at, WriteSample { data: &pcm, duration: n as u32, composition_offset: 0, is_sync: true }).map_err(mux_err)?;
            }
        }
        Ok(())
    };
    let frame_end_time = |k: u64| -> Tick { if k >= total { span_end } else { job.settings.frame_time(comp, k) } };
    write_video(&mut mux, pending)?;
    write_audio_until(&mut mux, frame_end_time(first_end), &mut cursor)?;
    let mut i = first_end;
    while i < total {
        let end = (i + batch).min(total);
        let next_end = (end + batch).min(total);
        let frames = std::mem::take(&mut ready);
        let (done, next) = job
            .beside(beside, batch_frames((end..next_end).collect()), || -> Result<()> {
                for (k, px) in frames.iter().enumerate() {
                    let ps = encode(&mut venc, px, i + k as u64)?;
                    write_video(&mut mux, ps)?;
                }
                write_audio_until(&mut mux, frame_end_time(end), &mut cursor)
            })
            .await?;
        done?;
        ready = next;
        st.advance(end - i)?;
        i = end;
    }
    write_video(&mut mux, venc.flush()?)?;
    write_audio_until(&mut mux, span_end, &mut cursor)?;
    if let Some((Audio::Aac(e), at)) = audio.as_mut() {
        for au in e.flush() {
            mux.write_sample(*at, WriteSample { data: &au, duration: 1024, composition_offset: 0, is_sync: true }).map_err(mux_err)?;
        }
    }
    let out = mux.finish().map_err(mux_err)?;
    out.flush().map_err(mux_err)?;
    let bytes = file.finish()?;
    Ok(Report { path, frames: 0, width: w, height: h, seconds: 0.0, bytes, audio: with_audio, log: None, overflow: vec![] })
}

#[cfg(test)]
mod tests {
    use effectcraft_raster::yuv::{UNIT8, round_pos};

    use super::*;

    /// The conversion as first written (per-pixel division and `f32::round`).
    fn yuv420_reference(rgba: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let (mut y, mut u, mut v) = (vec![0u8; w * h], vec![0u8; cw * ch], vec![0u8; cw * ch]);
        for cy in 0..ch {
            let mut acc = vec![(0f32, 0f32, 0f32); cw];
            for row in cy * 2..(cy * 2 + 2).min(h) {
                for x in 0..w {
                    let s = &rgba[(row * w + x) * 4..];
                    let (r, g, b) = (s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0);
                    let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                    y[row * w + x] = (16.0 + 219.0 * l).round().clamp(1.0, 254.0) as u8;
                    let a = &mut acc[x / 2];
                    a.0 += (b - l) / 1.8556;
                    a.1 += (r - l) / 1.5748;
                    a.2 += 1.0;
                }
            }
            for (cx, a) in acc.iter().enumerate() {
                u[cy * cw + cx] = (128.0 + 224.0 * a.0 / a.2).round().clamp(1.0, 254.0) as u8;
                v[cy * cw + cx] = (128.0 + 224.0 * a.1 / a.2).round().clamp(1.0, 254.0) as u8;
            }
        }
        (y, u, v)
    }

    #[test]
    fn round_pos_is_round() {
        for i in 0..2_000_000u32 {
            let v = i as f32 * 0.000_25;
            assert_eq!(round_pos(v).to_bits(), v.round().to_bits(), "{v}");
            let n = f32::from_bits(v.to_bits().wrapping_sub(1)).max(0.0);
            assert_eq!(round_pos(n).to_bits(), n.round().to_bits(), "{n}");
        }
        for k in 0..1024u32 {
            for v in [k as f32 + 0.5, f32::from_bits((k as f32 + 0.5).to_bits() - 1), f32::from_bits((k as f32 + 0.5).to_bits() + 1)] {
                assert_eq!(round_pos(v), v.round(), "{v}");
            }
        }
        for (i, u) in UNIT8.iter().enumerate() {
            assert_eq!(u.to_bits(), (i as f32 / 255.0).to_bits());
        }
    }

    #[test]
    fn yuv420_matches_the_reference() {
        // Every 8-bit level in every channel, odd sizes, and pseudo-random pixels.
        for (w, h) in [(1, 1), (2, 2), (3, 5), (17, 9), (64, 33)] {
            let mut rgba = vec![0u8; w * h * 4];
            let mut seed = (w * 31 + h) as u32;
            for (i, b) in rgba.iter_mut().enumerate() {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *b = if i % 7 == 0 { (i / 4 % 256) as u8 } else { (seed >> 24) as u8 };
            }
            let (mut y, mut u, mut v) = (vec![], vec![], vec![]);
            rgba_to_yuv420(&rgba, w, h, &mut y, &mut u, &mut v);
            assert_eq!((y, u, v), yuv420_reference(&rgba, w, h), "{w}x{h}");
        }
        let grey: Vec<u8> = (0..=255u8).flat_map(|g| [g, g, g, 255, g, 255 - g, g / 2, 255]).collect();
        let (mut y, mut u, mut v) = (vec![], vec![], vec![]);
        rgba_to_yuv420(&grey, 512, 1, &mut y, &mut u, &mut v);
        assert_eq!((y, u, v), yuv420_reference(&grey, 512, 1));
    }

    #[test]
    fn yuv_of_primaries() {
        let rgba = [255u8, 255, 255, 255, 0, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255];
        let (mut y, mut u, mut v) = (vec![], vec![], vec![]);
        rgba_to_yuv420(&rgba, 2, 2, &mut y, &mut u, &mut v);
        assert_eq!(y[0], 235);
        assert_eq!(y[1], 16);
        assert_eq!((u.len(), v.len()), (1, 1));
        let (mut y10, mut cb, mut cr) = (vec![0u16; 4], vec![0u16; 4], vec![0u16; 4]);
        rgba_to_yuv_10(&rgba, 2, 2, true, &mut y10, &mut cb, &mut cr);
        assert_eq!(y10[0], 940);
        assert_eq!(y10[1], 64);
        assert_eq!(cb[0], 512);
        assert!(cr[2] > 900, "red has high Cr: {}", cr[2]);
    }
}
