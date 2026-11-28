use std::sync::Arc;

use craft_color::ColorInfo;
use craft_frame::VideoFrame;
use craft_media::{ByteSource, avi};
use craft_media_source::{MediaError, Result, VideoStreamInfo};
use craft_time::{FrameRate, Tick};
use craft_video::raw_video::{self, RawFormat, RawPixels};

pub(super) struct RawAvi {
    format: RawFormat,
    width: u32,
    height: u32,
    bottom_up: bool,
    payload_bytes: usize,
    rate: FrameRate,
    cached: Option<(usize, Arc<VideoFrame>)>,
}

impl RawAvi {
    pub(super) fn new(stream: &avi::Stream) -> Option<Self> {
        let bitmap = stream.video.as_ref()?;
        let format = raw_video::RawFormat::from_avi(bitmap.compression, bitmap.bit_count)?;
        if !matches!(format, RawFormat::Bgr24 | RawFormat::Bgrx32) || stream.start != 0 {
            return None;
        }
        let width = u32::try_from(bitmap.width).ok()?;
        let height = bitmap.height.unsigned_abs();
        if !(1..=16_384).contains(&width) || !(1..=16_384).contains(&height) {
            return None;
        }
        let stride = (u64::from(width) * u64::from(bitmap.bit_count / 8)).next_multiple_of(4);
        let payload_bytes = stride * u64::from(height);
        let decoded_bytes = u64::from(width) * u64::from(height) * 4;
        // A request can hold its input and new picture while the old picture is cached.
        if payload_bytes + 2 * decoded_bytes > craft_video::budget::DECODE_MEMORY_BUDGET {
            return None;
        }
        // Empty AVI slots and partial chunks need the legacy reader's recovery policy.
        if stream.chunks.iter().any(|chunk| u64::from(chunk.size) < payload_bytes) {
            return None;
        }
        Some(Self {
            format,
            width,
            height,
            bottom_up: bitmap.height > 0,
            payload_bytes: usize::try_from(payload_bytes).ok()?,
            rate: FrameRate::new(i64::from(stream.rate), i64::from(stream.scale)),
            cached: None,
        })
    }

    pub(super) fn duration(&self, slots: usize) -> Tick {
        self.rate.tick_of(i64::try_from(slots).unwrap_or(i64::MAX))
    }

    pub(super) fn info(&self, stream: &avi::Stream) -> VideoStreamInfo {
        let bytes: u128 = stream.chunks.iter().map(|chunk| u128::from(chunk.size)).sum();
        let bytes_per_slot = bytes as f64 / stream.chunks.len() as f64;
        VideoStreamInfo {
            width: self.width,
            height: self.height,
            frame_rate: self.rate,
            par: (1, 1),
            codec: self.format.label().to_string(),
            pixel_format: String::new(),
            color: ColorInfo::SRGB_FULL,
            has_alpha: false,
            bitrate: Some((bytes_per_slot * 8.0 * self.rate.as_f64()) as u64),
            hdr: None,
        }
    }

    pub(super) fn frame(&mut self, file: &avi::AviFile, source: &dyn ByteSource, stream: usize, index: usize) -> Result<Arc<VideoFrame>> {
        if let Some((cached_index, frame)) = &self.cached
            && *cached_index == index
        {
            super::stats::hit();
            return Ok(frame.clone());
        }
        if filmcraft_media::cancel::cancelled() {
            return Err(MediaError::Cancelled);
        }
        super::stats::miss();
        let packet = file.read_chunk_range(source, stream, index, 0, self.payload_bytes).map_err(|e| MediaError::Io(e.to_string()))?;
        let started = super::stats::timer();
        let pixels = raw_video::decode(self.format, self.width, self.height, self.bottom_up, &packet)
            .map_err(|e| MediaError::Decode(format!("uncompressed AVI picture: {e:?}")))?;
        super::stats::add_decode_time(started);
        let RawPixels::Rgba(pixels) = pixels else {
            return Err(MediaError::Unsupported("unexpected uncompressed AVI pixel layout".into()));
        };
        let mut frame = VideoFrame::rgba8(self.width, self.height, pixels).map_err(|e| MediaError::Decode(e.to_string()))?;
        frame.pts = self.rate.tick_of(i64::try_from(index).unwrap_or(i64::MAX));
        let frame = Arc::new(frame);
        super::stats::decoded(1);
        super::stats::frames(1);
        if self.cached.replace((index, frame.clone())).is_some() {
            super::stats::evicted(1);
        }
        Ok(frame)
    }
}
