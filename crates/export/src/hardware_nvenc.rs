//! H.264 and HEVC Main 8-bit NVENC through FilmCraft's safe platform API.
//! No detached capability probe is started: the old Linux registration path raced driver teardown.

use effectcraft_project::render_queue::{CodecProfile, OutputFormat, OutputModule, RateControlMode};
use effectcraft_time::FrameRate;
use filmcraft_isobmff::{AvcConfig, SampleEntry};
use filmcraft_platform::nvenc::{Config, Nvenc, Packet as NvPacket, Profile, Signal};

use crate::encode::{Packet, VideoEncoder, rgba_to_yuv420};
use crate::{ExportError, Result};

pub(crate) fn open(format: OutputFormat, w: u32, h: u32, rate: FrameRate, output: &OutputModule) -> std::result::Result<Box<dyn VideoEncoder>, String> {
    let profile = match format {
        OutputFormat::H264 => Profile::High,
        OutputFormat::Hevc if output.codec.profile == CodecProfile::Main10 => return Err("NVENC adapter supports HEVC Main 8-bit only".into()),
        OutputFormat::Hevc if output.codec.rate_control == RateControlMode::Quality => {
            return Err("NVENC adapter does not support constant-quality control".into());
        }
        OutputFormat::Hevc => Profile::HevcMain,
        _ => return Err("NVENC supports H.264 and HEVC in this exporter".into()),
    };
    if w > u16::MAX.into() || h > u16::MAX.into() {
        return Err("video dimensions exceed MP4 sample entry limits".into());
    }
    let num = u32::try_from(rate.num).ok().filter(|n| *n > 0).ok_or("invalid frame rate numerator")?;
    let den = u32::try_from(rate.den).ok().filter(|n| *n > 0).ok_or("invalid frame rate denominator")?;
    let bitrate = output.bitrate_kbps.max(100);
    let config = Config {
        width: w,
        height: h,
        fps: (num, den),
        bitrate_kbps: bitrate,
        max_bitrate_kbps: bitrate.saturating_mul(3) / 2,
        cbr: false,
        keyint: output.keyint(rate.as_f64()),
        profile,
        level: if format == OutputFormat::Hevc { output.codec.level } else { None },
        sar: None,
        bframes: false,
    };
    let encoder = Nvenc::with_rgba_input(&config, &Signal::default()).or_else(|_| Nvenc::new(&config))?;
    let entry = match format {
        OutputFormat::H264 => {
            let (sps, pps) = encoder.parameter_sets();
            if sps.is_empty() || pps.is_empty() {
                return Err("NVENC returned incomplete H.264 parameter sets".into());
            }
            SampleEntry::avc(AvcConfig::new(vec![sps.to_vec()], vec![pps.to_vec()], 4), w as u16, h as u16)
        }
        OutputFormat::Hevc => {
            let config = encoder.hevc_config().cloned().ok_or("NVENC returned no HEVC configuration")?;
            SampleEntry::hevc(config, w as u16, h as u16)
        }
        _ => return Err("unsupported NVENC output format".into()),
    };
    Ok(Box::new(NvencAdapter { encoder, entry, w, h, emitted: 0, y: Vec::new(), u: Vec::new(), v: Vec::new() }))
}

struct NvencAdapter {
    encoder: Nvenc,
    entry: SampleEntry,
    w: u32,
    h: u32,
    emitted: i64,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl NvencAdapter {
    fn packets(&mut self, packets: Vec<NvPacket>) -> Result<Vec<Packet>> {
        let mut result = Vec::with_capacity(packets.len());
        for p in packets {
            if p.pts != self.emitted || p.dts != self.emitted {
                return Err(ExportError::Encode(format!("NVENC returned unexpected timestamps: pts {}, dts {}, expected {}", p.pts, p.dts, self.emitted)));
            }
            self.emitted = self.emitted.checked_add(1).ok_or_else(|| ExportError::Encode("NVENC frame count overflow".into()))?;
            result.push(Packet { data: p.data, key: p.key, cto: 0 });
        }
        Ok(result)
    }
}

impl VideoEncoder for NvencAdapter {
    fn sample_entry(&self) -> SampleEntry {
        self.entry.clone()
    }

    fn encode(&mut self, rgba: &[u8], index: u64) -> Result<Vec<Packet>> {
        let expected =
            (self.w as usize).checked_mul(self.h as usize).and_then(|n| n.checked_mul(4)).ok_or_else(|| ExportError::Encode("frame size overflow".into()))?;
        if rgba.len() < expected {
            return Err(ExportError::Encode("frame is smaller than encoder dimensions".into()));
        }
        let packets = if self.encoder.takes_rgba() {
            self.encoder.encode_rgba(rgba, index)
        } else {
            rgba_to_yuv420(rgba, self.w as usize, self.h as usize, &mut self.y, &mut self.u, &mut self.v);
            self.encoder.encode(&self.y, &self.u, &self.v, index)
        }
        .map_err(ExportError::Encode)?;
        self.packets(packets)
    }

    fn flush(&mut self) -> Result<Vec<Packet>> {
        let packets = self.encoder.flush().map_err(ExportError::Encode)?;
        self.packets(packets)
    }
}
