//! VideoToolbox through FilmCraft's separately maintained platform crate.
//!
//! FilmCraft owns the OS FFI; this adapter only uses its safe Rust API.

use effectcraft_project::render_queue::{CodecProfile, OutputFormat, OutputModule, RateControlMode};
use effectcraft_time::FrameRate;
use filmcraft_isobmff::{AvcConfig, HevcConfig, SampleEntry};
use filmcraft_platform::videotoolbox_encode::{VtConfig, VtEncoder, VtPacket, VtProfile, VtRate};

use crate::encode::{Packet, VideoEncoder, rgba_to_yuv420};
use crate::{ExportError, Result};

pub(crate) fn open(format: OutputFormat, w: u32, h: u32, rate: FrameRate, output: &OutputModule) -> std::result::Result<Box<dyn VideoEncoder>, String> {
    let profile = match format {
        OutputFormat::H264 => VtProfile::High,
        OutputFormat::Hevc if output.codec.profile == CodecProfile::Main10 => return Err("VideoToolbox adapter supports HEVC Main 8-bit only".into()),
        OutputFormat::Hevc if output.codec.rate_control == RateControlMode::Quality => {
            return Err("VideoToolbox adapter does not support constant-quality control".into());
        }
        OutputFormat::Hevc if output.codec.level.is_some() => return Err("VideoToolbox adapter cannot enforce an explicit HEVC level".into()),
        OutputFormat::Hevc => VtProfile::HevcMain,
        _ => return Err("VideoToolbox supports H.264 and HEVC in this exporter".into()),
    };
    let timescale = u32::try_from(rate.num).ok().filter(|n| *n > 0).ok_or("invalid frame rate numerator")?;
    let frame_duration = u32::try_from(rate.den).ok().filter(|n| *n > 0).ok_or("invalid frame rate denominator")?;
    let bitrate = output.bitrate_kbps.max(100);
    let cfg = VtConfig {
        width: w,
        height: h,
        timescale,
        frame_duration,
        keyframe_interval: output.keyint(rate.as_f64()),
        profile,
        rate: VtRate::Vbr { target_kbps: bitrate, max_kbps: bitrate.saturating_mul(3) / 2 },
    };
    let encoder = VtEncoder::new(cfg)?;
    if !encoder.uses_hardware() {
        return Err("VideoToolbox did not create a hardware session".into());
    }
    Ok(Box::new(VtAdapter { encoder, format, w, h, duration: frame_duration, emitted: 0, y: Vec::new(), u: Vec::new(), v: Vec::new() }))
}

struct VtAdapter {
    encoder: VtEncoder,
    format: OutputFormat,
    w: u32,
    h: u32,
    duration: u32,
    emitted: i64,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl VtAdapter {
    fn packets(&mut self, packets: Vec<VtPacket>) -> Result<Vec<Packet>> {
        let mut result = Vec::with_capacity(packets.len());
        for p in packets {
            let expected = self.emitted.checked_mul(i64::from(self.duration)).ok_or_else(|| ExportError::Encode("VideoToolbox timestamp overflow".into()))?;
            if p.pts != expected || p.dts != expected {
                return Err(ExportError::Encode(format!("VideoToolbox returned unexpected timestamps: pts {}, dts {}, expected {expected}", p.pts, p.dts)));
            }
            self.emitted = self.emitted.checked_add(1).ok_or_else(|| ExportError::Encode("VideoToolbox frame count overflow".into()))?;
            result.push(Packet { data: p.data, key: p.key, cto: 0 });
        }
        Ok(result)
    }
}

impl VideoEncoder for VtAdapter {
    fn sample_entry(&self) -> SampleEntry {
        let params = self.encoder.parameter_sets();
        if self.format == OutputFormat::H264 {
            let (sps, pps) = params.map(|p| (p.sps, p.pps)).unwrap_or_default();
            SampleEntry::avc(AvcConfig::new(sps, pps, 4), self.w as u16, self.h as u16)
        } else {
            let cfg = params.and_then(|p| p.hvcc).and_then(|v| HevcConfig::parse(&v).ok()).unwrap_or_default();
            SampleEntry::hevc(cfg, self.w as u16, self.h as u16)
        }
    }

    fn encode(&mut self, rgba: &[u8], index: u64) -> Result<Vec<Packet>> {
        let expected =
            (self.w as usize).checked_mul(self.h as usize).and_then(|n| n.checked_mul(4)).ok_or_else(|| ExportError::Encode("frame size overflow".into()))?;
        if rgba.len() < expected {
            return Err(ExportError::Encode("frame is smaller than encoder dimensions".into()));
        }
        rgba_to_yuv420(rgba, self.w as usize, self.h as usize, &mut self.y, &mut self.u, &mut self.v);
        let packets = self.encoder.encode(index, &self.y, &self.u, &self.v).map_err(ExportError::Encode)?;
        if index == 0 {
            let params = self.encoder.parameter_sets().ok_or_else(|| ExportError::Encode("VideoToolbox returned no parameter sets".into()))?;
            if params.sps.is_empty() || params.pps.is_empty() {
                return Err(ExportError::Encode("VideoToolbox returned incomplete parameter sets".into()));
            }
            if self.format == OutputFormat::Hevc {
                let data = params.hvcc.ok_or_else(|| ExportError::Encode("VideoToolbox returned no HEVC configuration".into()))?;
                HevcConfig::parse(&data).map_err(|e| ExportError::Encode(format!("VideoToolbox HEVC configuration: {e}")))?;
            }
        }
        self.packets(packets)
    }

    fn flush(&mut self) -> Result<Vec<Packet>> {
        let packets = self.encoder.flush().map_err(ExportError::Encode)?;
        self.packets(packets)
    }
}
