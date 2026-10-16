//! HEVC (`effectcraft-hevcenc`) and AV1 (`effectcraft-av1enc`) video encoders for the MP4
//! movie writer (`hvc1` / `av01` sample entries) and AV1 in WebM, configured from the Output
//! Module's [`VideoCodecOptions`].

use effectcraft_project::render_queue::{CodecProfile, OutputModule, RateControlMode, VideoCodecOptions};
use effectcraft_time::FrameRate;
use filmcraft_isobmff::{Av1Config, CodecConfig, FourCc, HevcConfig, SampleEntry};
use rayon::prelude::*;

use crate::encode::{Packet, VideoEncoder};
use crate::{ExportError, Result};

fn enc_err(e: impl std::fmt::Display) -> ExportError {
    ExportError::Encode(e.to_string())
}

/// BT.709 limited-range 4:2:0 planes at `bits` (8 or 10) from RGBA8 (2×2 chroma average;
/// alpha ignored). Chroma planes are `ceil(w/2)` × `ceil(h/2)`.
pub(crate) fn rgba_to_yuv420_bits(rgba: &[u8], w: usize, h: usize, bits: u8, y: &mut Vec<u16>, u: &mut Vec<u16>, v: &mut Vec<u16>) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let scale = (1u32 << (bits - 8)) as f32;
    let (lo, hi) = (scale, 254.0 * scale + (scale - 1.0));
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
                yr[dy * w + x] = ((16.0 + 219.0 * l) * scale).round().clamp(lo, hi) as u16;
                let a = &mut acc[x / 2];
                a.0 += (b - l) / 1.8556;
                a.1 += (r - l) / 1.5748;
                a.2 += 1.0;
            }
        }
        for (cx, a) in acc.iter().enumerate() {
            ur[cx] = ((128.0 + 224.0 * a.0 / a.2) * scale).round().clamp(lo, hi) as u16;
            vr[cx] = ((128.0 + 224.0 * a.1 / a.2) * scale).round().clamp(lo, hi) as u16;
        }
    });
}

#[derive(Default)]
struct Planes {
    y: Vec<u16>,
    u: Vec<u16>,
    v: Vec<u16>,
}

// ---------------------------------------------------------------- HEVC

pub(crate) struct Hevc {
    enc: effectcraft_hevcenc::Encoder,
    w: u32,
    h: u32,
    bits: u8,
    planes: Planes,
}

/// The HEVC encoder settings for an output module.
pub(crate) fn hevc_config(w: u32, h: u32, rate: FrameRate, om: &OutputModule) -> effectcraft_hevcenc::EncoderConfig {
    use effectcraft_hevcenc as he;
    let o: &VideoCodecOptions = &om.codec;
    let mut cfg = he::EncoderConfig::new(w, h, rate.num as u32, rate.den as u32);
    cfg.profile = match o.profile {
        CodecProfile::Main => he::Profile::Main,
        CodecProfile::Main10 => he::Profile::Main10,
    };
    cfg.level_idc = o.hevc_level_idc();
    cfg.rate = match o.rate_control {
        RateControlMode::Bitrate => he::RateControl::Bitrate { kbps: om.bitrate_kbps.max(50) },
        RateControlMode::Quality => he::RateControl::ConstantQp(o.hevc_qp()),
    };
    cfg.keyint = om.keyint(rate.as_f64());
    cfg
}

impl Hevc {
    pub(crate) fn new(w: u32, h: u32, rate: FrameRate, om: &OutputModule) -> Result<Hevc> {
        let cfg = hevc_config(w, h, rate, om);
        let bits = cfg.profile.bit_depth();
        let enc = effectcraft_hevcenc::Encoder::new(cfg).map_err(enc_err)?;
        Ok(Hevc { enc, w, h, bits, planes: Planes::default() })
    }
}

impl VideoEncoder for Hevc {
    fn sample_entry(&self) -> SampleEntry {
        let cfg = HevcConfig::parse(&self.enc.hvcc()).unwrap_or_default();
        let mut entry = SampleEntry::hevc(cfg, self.w as u16, self.h as u16);
        crate::encode::tag_bt709_limited(&mut entry);
        entry
    }
    fn encode(&mut self, rgba: &[u8], _index: u64) -> Result<Vec<Packet>> {
        let (w, h) = (self.w as usize, self.h as usize);
        let p = &mut self.planes;
        rgba_to_yuv420_bits(rgba, w, h, self.bits, &mut p.y, &mut p.u, &mut p.v);
        let frame = effectcraft_hevcenc::Frame { y: &p.y, u: &p.u, v: &p.v, y_stride: w, uv_stride: w.div_ceil(2) };
        let pk = self.enc.encode(&frame);
        Ok(vec![Packet { data: pk.data, key: pk.keyframe, cto: 0 }])
    }
    fn flush(&mut self) -> Result<Vec<Packet>> {
        Ok(vec![])
    }
}

// ---------------------------------------------------------------- AV1

pub(crate) struct Av1 {
    enc: effectcraft_av1enc::Encoder,
    w: u32,
    h: u32,
    bits: u8,
    planes: Planes,
}

/// The AV1 encoder settings for an output module.
pub(crate) fn av1_config(w: u32, h: u32, rate: FrameRate, om: &OutputModule) -> effectcraft_av1enc::EncoderConfig {
    use effectcraft_av1enc as ae;
    let o: &VideoCodecOptions = &om.codec;
    let mut cfg = ae::EncoderConfig::new(w, h, rate.num as u32, rate.den as u32);
    cfg.bit_depth = o.profile.bit_depth();
    cfg.level_idx = o.av1_level_idx();
    cfg.rate = match o.rate_control {
        RateControlMode::Bitrate => ae::RateControl::Bitrate { kbps: om.bitrate_kbps.max(50) },
        RateControlMode::Quality => ae::RateControl::ConstantQ(o.av1_qindex()),
    };
    cfg.keyint = om.keyint(rate.as_f64());
    cfg
}

impl Av1 {
    pub(crate) fn new(w: u32, h: u32, rate: FrameRate, om: &OutputModule) -> Result<Av1> {
        let cfg = av1_config(w, h, rate, om);
        let bits = cfg.bit_depth;
        let enc = effectcraft_av1enc::Encoder::new(cfg).map_err(enc_err)?;
        Ok(Av1 { enc, w, h, bits, planes: Planes::default() })
    }
    /// The `av1C` record (also the WebM CodecPrivate).
    pub(crate) fn av1c(&self) -> Vec<u8> {
        self.enc.av1c()
    }
}

impl VideoEncoder for Av1 {
    fn sample_entry(&self) -> SampleEntry {
        let cfg = Av1Config::parse(&self.enc.av1c()).unwrap_or_default();
        let mut entry = SampleEntry::video(FourCc(*b"av01"), CodecConfig::Av1(cfg), self.w as u16, self.h as u16);
        crate::encode::tag_bt709_limited(&mut entry);
        entry
    }
    fn encode(&mut self, rgba: &[u8], _index: u64) -> Result<Vec<Packet>> {
        let (w, h) = (self.w as usize, self.h as usize);
        let p = &mut self.planes;
        rgba_to_yuv420_bits(rgba, w, h, self.bits, &mut p.y, &mut p.u, &mut p.v);
        let frame = effectcraft_av1enc::Frame { y: &p.y, u: &p.u, v: &p.v, y_stride: w, uv_stride: w.div_ceil(2) };
        let pk = self.enc.encode(&frame);
        Ok(vec![Packet { data: pk.data, key: pk.keyframe, cto: 0 }])
    }
    fn flush(&mut self) -> Result<Vec<Packet>> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_bit_planes_scale_eight_bit_ones() {
        let rgba = [255u8, 255, 255, 255, 0, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255];
        let (mut y, mut u, mut v) = (vec![], vec![], vec![]);
        rgba_to_yuv420_bits(&rgba, 2, 2, 8, &mut y, &mut u, &mut v);
        assert_eq!((y[0], y[1]), (235, 16));
        rgba_to_yuv420_bits(&rgba, 2, 2, 10, &mut y, &mut u, &mut v);
        assert_eq!((y[0], y[1]), (940, 64));
        assert_eq!(u.len(), 1);
    }

    #[test]
    fn codec_options_map_to_encoder_settings() {
        let mut om = OutputModule::for_format(effectcraft_project::render_queue::OutputFormat::Hevc);
        om.codec.profile = CodecProfile::Main10;
        om.codec.level = Some(41);
        om.codec.rate_control = RateControlMode::Quality;
        om.codec.quality = 100;
        om.keyframe_interval = 12;
        let rate = FrameRate::new(30, 1);
        let h = hevc_config(64, 32, rate, &om);
        assert_eq!(h.profile, effectcraft_hevcenc::Profile::Main10);
        assert_eq!(h.level_idc, Some(123));
        assert_eq!(h.rate, effectcraft_hevcenc::RateControl::ConstantQp(4));
        assert_eq!(h.keyint, 12);
        let a = av1_config(64, 32, rate, &om);
        assert_eq!((a.bit_depth, a.level_idx, a.keyint), (10, Some(9), 12));
        om.codec.rate_control = RateControlMode::Bitrate;
        om.keyframe_interval = 0;
        om.bitrate_kbps = 3000;
        let a = av1_config(64, 32, rate, &om);
        assert_eq!((a.rate, a.keyint), (effectcraft_av1enc::RateControl::Bitrate { kbps: 3000 }, 60));
    }
}
