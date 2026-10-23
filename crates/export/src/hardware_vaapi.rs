//! Linux VA-API H.264 / HEVC Main encoding through FilmCraft's safe export interface.
//! FilmCraft owns libva FFI and codec-specific driver parameter handling.

use effectcraft_project::render_queue::{CodecProfile, OutputFormat, OutputModule, RateControlMode};
use effectcraft_time::FrameRate;
use filmcraft_export::{BitrateMode, EncodedPacket, EncoderFrame, ExportSettings, Format, H264Profile, HardwareEncoding, VideoEncoder as FilmEncoder};
use filmcraft_isobmff::SampleEntry;
use filmcraft_time::FrameRate as FilmFrameRate;

use crate::encode::{Packet, VideoEncoder};
use crate::{ExportError, Result};

pub(crate) fn open(format: OutputFormat, w: u32, h: u32, rate: FrameRate, output: &OutputModule) -> std::result::Result<Box<dyn VideoEncoder>, String> {
    let film_format = match format {
        OutputFormat::H264 => Format::H264,
        OutputFormat::Hevc if output.codec.profile == CodecProfile::Main10 => return Err("VA-API adapter supports HEVC Main 8-bit only".into()),
        OutputFormat::Hevc if output.codec.rate_control == RateControlMode::Quality => {
            return Err("VA-API adapter does not support constant-quality control".into());
        }
        OutputFormat::Hevc if output.codec.level.is_some() => return Err("VA-API adapter cannot enforce an explicit HEVC level".into()),
        OutputFormat::Hevc => Format::Hevc,
        _ => return Err("VA-API supports H.264 and HEVC in this exporter".into()),
    };
    if w == 0 || h == 0 || w > u16::MAX.into() || h > u16::MAX.into() {
        return Err("video dimensions are invalid or exceed MP4 sample entry limits".into());
    }
    let num = u32::try_from(rate.num).ok().filter(|n| *n > 0).ok_or("invalid frame rate numerator")?;
    let den = u32::try_from(rate.den).ok().filter(|n| *n > 0).ok_or("invalid frame rate denominator")?;
    let bitrate = output.bitrate_kbps.max(100);
    let settings = ExportSettings {
        format: film_format,
        hardware_encoding: HardwareEncoding::Auto,
        h264_profile: H264Profile::High,
        bitrate_mode: BitrateMode::Vbr1Pass,
        bitrate_kbps: bitrate,
        max_bitrate_kbps: Some(bitrate.saturating_mul(3) / 2),
        keyframe_distance: Some(output.keyint(rate.as_f64())),
        include_audio: false,
        ..ExportSettings::default()
    };
    let film_rate = FilmFrameRate::new(i64::from(num), i64::from(den));
    let encoder = match film_format {
        Format::H264 => filmcraft_platform::vaapi::h264enc::factory(film_format, w, h, film_rate, &settings)
            .ok_or("VA-API H.264 encoder unavailable or declined these settings")?,
        Format::Hevc => filmcraft_platform::vaapi::hevcenc::factory(film_format, w, h, film_rate, &settings).ok_or("VA-API HEVC encoder unavailable")?,
        _ => return Err("unsupported VA-API output format".into()),
    }
    .map_err(|e| e.to_string())?;
    let entry = encoder.sample_entry();
    Ok(Box::new(VaapiAdapter { encoder, entry, w, h, duration: den }))
}

struct VaapiAdapter {
    encoder: Box<dyn FilmEncoder>,
    entry: SampleEntry,
    w: u32,
    h: u32,
    duration: u32,
}

impl VaapiAdapter {
    fn packets(&self, packets: Vec<EncodedPacket>) -> Result<Vec<Packet>> {
        packets
            .into_iter()
            .map(|p| {
                if p.duration != self.duration {
                    return Err(ExportError::Encode(format!("VA-API packet duration {} differs from frame duration {}", p.duration, self.duration)));
                }
                Ok(Packet { data: p.data, key: p.key, cto: p.composition_offset })
            })
            .collect()
    }
}

impl VideoEncoder for VaapiAdapter {
    fn sample_entry(&self) -> SampleEntry {
        self.entry.clone()
    }

    fn encode(&mut self, rgba: &[u8], index: u64) -> Result<Vec<Packet>> {
        let expected =
            (self.w as usize).checked_mul(self.h as usize).and_then(|n| n.checked_mul(4)).ok_or_else(|| ExportError::Encode("frame size overflow".into()))?;
        if rgba.len() < expected {
            return Err(ExportError::Encode("frame is smaller than encoder dimensions".into()));
        }
        let packets = self
            .encoder
            .encode(&EncoderFrame { width: self.w, height: self.h, rgba: &rgba[..expected], hdr: None, index })
            .map_err(|e| ExportError::Encode(format!("VA-API: {e}")))?;
        self.packets(packets)
    }

    fn flush(&mut self) -> Result<Vec<Packet>> {
        let packets = self.encoder.flush().map_err(|e| ExportError::Encode(format!("VA-API: {e}")))?;
        self.packets(packets)
    }
}
