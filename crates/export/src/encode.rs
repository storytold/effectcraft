//! Movie export: FilmCraft H.264, HEVC (`effectcraft-hevcenc`) and AV1 (`effectcraft-av1enc`)
//! → MP4 (+ AAC) and ProRes → MOV (+ PCM), muxed by
//! FilmCraft's ISO BMFF / QuickTime writer.

use std::io::Write;

use effectcraft_project::render_queue::{AudioFormat, Channels, OutputFormat, ProResProfile};
use effectcraft_project::{ColorSpace, Comp};
use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
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

pub(crate) trait VideoEncoder {
    fn sample_entry(&self) -> SampleEntry;
    fn encode(&mut self, rgba: &[u8], index: u64) -> Result<Vec<Packet>>;
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
        rgba_to_yuv420(rgba, self.w as usize, self.h as usize, &mut self.y, &mut self.u, &mut self.v);
        let cw = (self.w as usize).div_ceil(2);
        let frame = filmcraft_h264enc::YuvFrame { y: &self.y, u: &self.u, v: &self.v, y_stride: self.w as usize, uv_stride: cw };
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

/// BT.709 limited-range 8-bit 4:2:0 from RGBA8 (2×2 chroma average; alpha ignored).
pub(crate) fn rgba_to_yuv420(rgba: &[u8], w: usize, h: usize, y: &mut Vec<u8>, u: &mut Vec<u8>, v: &mut Vec<u8>) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    y.resize(w * h, 0);
    u.resize(cw * ch, 0);
    v.resize(cw * ch, 0);
    y.par_chunks_mut(w * 2).zip(u.par_chunks_mut(cw).zip(v.par_chunks_mut(cw))).enumerate().for_each(|(cy, (yr, (ur, vr)))| {
        let rows = yr.len() / w;
        let mut acc = vec![(0f32, 0f32, 0f32); cw];
        for dy in 0..rows {
            let src = &rgba[(cy * 2 + dy) * w * 4..][..w * 4];
            for x in 0..w {
                let (r, g, b) = (src[x * 4] as f32 / 255.0, src[x * 4 + 1] as f32 / 255.0, src[x * 4 + 2] as f32 / 255.0);
                let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                yr[dy * w + x] = (16.0 + 219.0 * l).round().clamp(1.0, 254.0) as u8;
                let a = &mut acc[x / 2];
                a.0 += (b - l) / 1.8556;
                a.1 += (r - l) / 1.5748;
                a.2 += 1.0;
            }
        }
        for (cx, a) in acc.iter().enumerate() {
            ur[cx] = (128.0 + 224.0 * a.0 / a.2).round().clamp(1.0, 254.0) as u8;
            vr[cx] = (128.0 + 224.0 * a.1 / a.2).round().clamp(1.0, 254.0) as u8;
        }
    });
}

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
    fn new(w: u32, h: u32, profile: ProResProfile, channels: Channels, srgb_transfer: bool) -> ProRes {
        // Alpha needs a 4444 flavour.
        let profile = if channels == Channels::Rgba && !profile.is_4444() { ProResProfile::P4444 } else { profile };
        let profile = prores_profile(profile);
        let alpha = channels == Channels::Rgba;
        let mut cfg = filmcraft_prores::EncoderConfig::new(profile, w, h);
        cfg.encode_alpha = alpha;
        // The renderer returns encoded output pixels; describe sRGB without changing their
        // values or the BT.709 YCbCr matrix (H.273 transfer code 13).
        if srgb_transfer {
            cfg.color.transfer = 13;
        }
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
        Channels::Rgba if fmt != OutputFormat::ProRes => Channels::Rgb,
        c => c,
    };
    let color = &job.project.settings;
    let srgb_transfer = channels != Channels::Alpha && (color.working_space.is_none() || color.output_space.unwrap_or(ColorSpace::Srgb) == ColorSpace::Srgb);
    let mut venc: Box<dyn VideoEncoder> = match fmt {
        OutputFormat::H264 => Box::new(H264::new(w, h, rate, job.output.bitrate_kbps)?),
        OutputFormat::Hevc => Box::new(crate::hevc_av1::Hevc::new(w, h, rate, job.output)?),
        OutputFormat::Av1 => Box::new(crate::hevc_av1::Av1::new(w, h, rate, job.output)?),
        _ => Box::new(ProRes::new(w, h, job.output.prores_profile, if channels == Channels::Alpha { Channels::Rgb } else { channels }, srgb_transfer)),
    };
    let brand = if fmt == OutputFormat::ProRes { Brand::Mov } else { Brand::Mp4 };
    let batch = batch_size();
    let render = |_: u64, img: effectcraft_raster::Image| -> Vec<u8> { job.pixels(&img, comp, channels, w, h) };

    // Encode the first batch before creating tracks: encoders finalise their config on frame 1.
    let first_end = batch.min(st.total);
    let first: Vec<Vec<u8>> = job.frames(comp, (0..first_end).collect(), render).await;
    let mut pending = Vec::new();
    for (k, px) in first.iter().enumerate() {
        pending.extend(venc.encode(px, k as u64)?);
    }
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
        let start = Tick(((*cursor as i128 * TICKS_PER_SECOND as i128) / sr as i128) as i64);
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
    let total = st.total;
    let frame_end_time = |k: u64| -> Tick { if k >= total { span_end } else { job.settings.frame_time(comp, k) } };
    write_video(&mut mux, pending)?;
    write_audio_until(&mut mux, frame_end_time(first_end), &mut cursor)?;
    let mut i = first_end;
    while i < total {
        let end = (i + batch).min(total);
        let frames: Vec<Vec<u8>> = job.frames(comp, (i..end).collect(), render).await;
        for (k, px) in frames.iter().enumerate() {
            let ps = venc.encode(px, i + k as u64)?;
            write_video(&mut mux, ps)?;
        }
        write_audio_until(&mut mux, frame_end_time(end), &mut cursor)?;
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
    use super::*;

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
