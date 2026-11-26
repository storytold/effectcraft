//! The video helper messages of `docs/video-helper-protocol.md`, one JSON object per line.
//! Pictures and packets never go through the pipe: they live in [`Region`]s of the exchange file.

use serde::{Deserialize, Serialize};

pub use crate::protocol::{HELPER_ARG, MAGIC};

pub const VIDEO_VERSION: u32 = 1;

pub const MAX_LINE: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub offset: u64,
    pub len: u64,
}

impl Region {
    pub fn end(&self) -> Option<u64> {
        self.offset.checked_add(self.len)
    }
}

/// The memory for one frame in flight: the picture goes in `in`, the helper writes packets to `out`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    #[serde(rename = "in")]
    pub input: Region,
    pub out: Region,
}

/// `offset` is from the start of the file, `size` is in samples, `len` is `stride × size[1]` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plane {
    pub offset: u64,
    pub stride: u32,
    pub size: [u32; 2],
    pub len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColourRange {
    Limited,
    Full,
}

/// [H.273](https://www.itu.int/rec/T-REC-H.273) code points, given once per job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Colour {
    pub primaries: u32,
    pub transfer: u32,
    pub matrix: u32,
    pub range: ColourRange,
}

/// Strings in [`EncodeCaps`], so that one codec we don't know doesn't make a `hello` unreadable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    H264,
    Hevc,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EncodeOpen {
    pub job: u64,
    pub device: String,
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub rate: [u32; 2],
    pub bitrate_kbps: u32,
    pub profile: String,
    pub keyint: u32,
    pub colour: Colour,
    pub slots: Vec<Slot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum VideoRequest {
    #[serde(rename = "hello")]
    Hello { version: u32, frames: String, video: u32 },
    #[serde(rename = "encode.open")]
    EncodeOpen(EncodeOpen),
    #[serde(rename = "encode.frame")]
    EncodeFrame { job: u64, frame: u64, slot: u32, planes: [Plane; 3] },
    #[serde(rename = "encode.flush")]
    EncodeFlush { job: u64, out: Region },
    #[serde(rename = "encode.close")]
    EncodeClose { job: u64 },
    #[serde(rename = "cancel")]
    Cancel { job: u64 },
    #[serde(rename = "shutdown")]
    Shutdown,
}

/// `offset` is counted from the start of the region the reply uses; `pts` and `dts` are in units
/// of 1 / `rate[0]` seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketRef {
    pub offset: u64,
    pub len: u64,
    pub pts: i64,
    pub dts: i64,
    pub key: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unsupported,
    DeviceLost,
    OutOfMemory,
    /// The encoder's output for a frame does not fit the `out` region it was given.
    PacketTooLarge,
    Encoder,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EncodeCaps {
    pub codec: String,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub bit_depths: Vec<u32>,
    pub min_size: [u32; 2],
    pub max_size: [u32; 2],
    #[serde(default)]
    pub rate_control: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub api: String,
    pub driver: String,
    #[serde(default)]
    pub encode: Vec<EncodeCaps>,
    #[serde(default)]
    pub decode: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoHello {
    pub version: u32,
    pub app_min: String,
    pub devices: Vec<Device>,
}

/// An `op` or error code this build does not know does not parse, which is a protocol break.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum VideoReply {
    #[serde(rename = "hello")]
    Hello {
        version: u32,
        name: String,
        #[serde(default)]
        helper_version: String,
        #[serde(default)]
        loads: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        video: Option<VideoHello>,
    },
    /// `codec_config` is the base64 `avcC` (H.264) or `hvcC` (HEVC) record.
    #[serde(rename = "encode.opened")]
    EncodeOpened { job: u64, codec_config: String, delay: u32 },
    /// `frame` is `null` for the batch that answers `encode.flush`.
    #[serde(rename = "encode.packets")]
    EncodePackets { job: u64, frame: Option<u64>, packets: Vec<PacketRef> },
    #[serde(rename = "encode.flushed")]
    EncodeFlushed { job: u64 },
    #[serde(rename = "cancelled")]
    Cancelled { job: u64 },
    #[serde(rename = "error")]
    Error {
        job: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<u64>,
        code: ErrorCode,
        #[serde(default)]
        message: String,
    },
}

pub fn request_line(r: &VideoRequest) -> Result<String, String> {
    let line = serde_json::to_string(r).map(|s| format!("{s}\n")).map_err(|e| e.to_string())?;
    if line.len() > MAX_LINE {
        return Err(format!("a request of {} bytes does not fit a control line", line.len()));
    }
    Ok(line)
}

/// `None` for a line without [`MAGIC`]: the helper's own output, ignored.
pub fn parse_reply(line: &str) -> Option<Result<VideoReply, String>> {
    let at = line.find(MAGIC)?;
    if line.len() > MAX_LINE {
        return Some(Err(format!("a reply line of {} bytes is longer than the {MAX_LINE} allowed", line.len())));
    }
    let json = line.get(at + MAGIC.len()..)?.trim_end();
    Some(serde_json::from_str(json).map_err(|e| format!("bad reply from the video helper: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(json: &str) -> VideoReply {
        parse_reply(&format!("{MAGIC}{json}")).unwrap().unwrap()
    }

    /// The examples on `docs/video-helper-protocol.md` must parse: the page is the contract.
    #[test]
    fn the_documented_messages_parse() {
        let VideoReply::Hello { video: Some(v), .. } = reply(
            r#"{"op":"hello","version":1,"name":"Hardware video","helper_version":"0.1.0","loads":[],
            "video":{"version":1,"app_min":"0.25.0","devices":[
              {"id":"nvenc:0","name":"NVIDIA GeForce RTX 3090","api":"nvenc","driver":"NVIDIA 570.86",
               "encode":[{"codec":"h264","profiles":["high"],"bit_depths":[8],"min_size":[64,64],
                          "max_size":[4096,4096],"rate_control":["bitrate"]}],"decode":[]}]}}"#,
        ) else {
            panic!("not a video hello");
        };
        assert_eq!(v.devices.first().map(|d| d.id.as_str()), Some("nvenc:0"));

        assert!(matches!(reply(r#"{"op":"encode.opened","job":1,"codec_config":"AQID","delay":2}"#), VideoReply::EncodeOpened { job: 1, delay: 2, .. }));
        let VideoReply::EncodePackets { frame: None, packets, .. } =
            reply(r#"{"op":"encode.packets","job":1,"frame":null,"packets":[{"offset":0,"len":9,"pts":0,"dts":-2,"key":true}]}"#)
        else {
            panic!("not a flush batch");
        };
        assert_eq!(packets.first().map(|p| p.dts), Some(-2));
        assert!(matches!(
            reply(r#"{"op":"error","job":1,"frame":3,"code":"packet_too_large","message":"x"}"#),
            VideoReply::Error { frame: Some(3), code: ErrorCode::PacketTooLarge, .. }
        ));
    }

    /// Requests carry the spec's field names, in particular `in` for a slot's picture region and
    /// the three plane descriptors on every frame.
    #[test]
    fn requests_use_the_documented_field_names() {
        let slot = Slot { input: Region { offset: 0, len: 3_110_400 }, out: Region { offset: 3_110_400, len: 1 << 20 } };
        let open = VideoRequest::EncodeOpen(EncodeOpen {
            job: 1,
            device: "nvenc:0".into(),
            codec: Codec::H264,
            width: 1920,
            height: 1080,
            rate: [30, 1],
            bitrate_kbps: 15_000,
            profile: "high".into(),
            keyint: 60,
            colour: Colour { primaries: 1, transfer: 1, matrix: 1, range: ColourRange::Limited },
            slots: vec![slot],
        });
        let line = request_line(&open).unwrap();
        for part in [r#""op":"encode.open""#, r#""codec":"h264""#, r#""range":"limited""#, r#""in":{"offset":0,"len":3110400}"#] {
            assert!(line.contains(part), "{part} missing from {line}");
        }
        let plane = Plane { offset: 0, stride: 1920, size: [1920, 1080], len: 2_073_600 };
        let frame = request_line(&VideoRequest::EncodeFrame { job: 1, frame: 0, slot: 0, planes: [plane; 3] }).unwrap();
        assert_eq!(frame.matches(r#""stride":1920"#).count(), 3);
        // Requests round-trip.
        assert_eq!(serde_json::from_str::<VideoRequest>(line.trim_end()).unwrap(), open);
    }

    #[test]
    fn other_output_is_ignored_and_oversized_or_unknown_lines_are_errors() {
        assert!(parse_reply("driver chatter").is_none());
        assert!(parse_reply(&format!("loading...{MAGIC}{{\"op\":\"encode.flushed\",\"job\":4}}")).unwrap().is_ok());
        // An op or error code from another version does not parse (EffectCraft then stops the helper).
        assert!(parse_reply(&format!("{MAGIC}{{\"op\":\"encode.rewind\",\"job\":1}}")).unwrap().is_err());
        assert!(parse_reply(&format!("{MAGIC}{{\"op\":\"error\",\"job\":1,\"code\":\"sulking\"}}")).unwrap().is_err());
        let long = format!("{MAGIC}{{\"op\":\"cancelled\",\"job\":1,\"pad\":\"{}\"}}", "x".repeat(MAX_LINE));
        assert!(parse_reply(&long).unwrap().is_err());
    }
}
