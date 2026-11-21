//! The helper protocol, version [`PROTOCOL_VERSION`] (`docs/external-helpers.md`).
//!
//! Control messages are JSON, one per line. EffectCraft writes requests ([`Request`]) to the
//! helper's stdin; the helper writes replies ([`Reply`]) to its stdout, each line starting with
//! [`MAGIC`] so that anything else a helper (or a library it loads) prints is ignored. Pixels
//! never travel over the pipe: they live in the **frame exchange file** named in
//! [`Request::Hello`] and are addressed by [`ImageRef`]s (byte offset + size).

use effectcraft_effects::plugin::PluginManifest;
use serde::{Deserialize, Serialize};

/// The protocol version this build speaks. A helper that answers `hello` with another version
/// is not used.
pub const PROTOCOL_VERSION: u32 = 1;

/// Every reply line from a helper starts with this (anything before it on the line, and lines
/// without it, are the helper's own output and only logged).
pub const MAGIC: &str = "@ec-helper ";

/// The argument EffectCraft starts helpers with.
pub const HELPER_ARG: &str = "--effectcraft-helper";

/// The longest control line either side accepts (bytes). Hidden parameters can carry a few MB.
pub const MAX_LINE: u64 = 256 << 20;

/// The most pixels one image may have (16384 × 16384).
pub const MAX_PIXELS: u64 = 1 << 28;

/// Bytes per pixel in the exchange file: premultiplied RGBA `f32`, native byte order.
pub const PIXEL_BYTES: u64 = 16;

/// An image in the frame exchange file: `width * height` premultiplied RGBA `f32` pixels, row 0
/// on top, starting `offset` bytes into the file. `origin` and `scale` are as in
/// [`effectcraft_effects::plugin::PluginImage`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageRef {
    pub offset: u64,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub origin: [f64; 2],
    #[serde(default = "one")]
    pub scale: f64,
}

fn one() -> f64 {
    1.0
}

impl ImageRef {
    /// Pixels in the image, `None` when it is larger than [`MAX_PIXELS`].
    pub fn pixels(&self) -> Option<usize> {
        let n = u64::from(self.width).checked_mul(u64::from(self.height))?;
        (n <= MAX_PIXELS).then_some(n).and_then(|n| usize::try_from(n).ok())
    }
    /// Bytes the image takes in the exchange file.
    pub fn bytes(&self) -> Option<u64> {
        u64::try_from(self.pixels()?).ok()?.checked_mul(PIXEL_BYTES)
    }
}

/// One render of one effect instance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderRequest {
    /// The effect id (as described by the helper).
    pub effect: String,
    /// A stable id of the applied effect instance ([`effectcraft_effects::plugin::PluginHost::instance_key`]).
    pub instance: u64,
    /// Layer time, seconds.
    pub time: f64,
    /// The buffer to process in place (already padded); the result is written back to it.
    pub frame: ImageRef,
    /// The layer's size in layer pixels.
    pub layer_size: [f64; 2],
    /// Flattened parameter values (plug-in API 2 rules).
    pub flat: Vec<f64>,
    /// Text / hidden parameter strings.
    #[serde(default)]
    pub texts: Vec<String>,
    pub frame_rate: f64,
    /// The layer's source range in layer-time seconds, when it has one.
    #[serde(default)]
    pub frame_range: Option<(f64, f64)>,
}

/// EffectCraft → helper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// First message: the protocol version and the frame exchange file to open (read + write).
    /// `features` lists optional callbacks this host answers (`"layer_transform"`:
    /// [`Reply::LayerTransform`]); hosts without the field answer none of them.
    Hello {
        version: u32,
        frames: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        features: Vec<String>,
    },
    /// List the effects the helper offers → [`Reply::Effects`].
    Describe,
    /// Load more effects from a file or folder the user picked → [`Reply::Effects`] (only the
    /// new ones).
    Load { path: String },
    /// Transparent pixels to add around the buffer before [`Request::Render`] → [`Reply::Padding`].
    Padding { effect: String, instance: u64, time: f64, scale: f64, layer_size: [f64; 2], flat: Vec<f64>, texts: Vec<String> },
    /// Render → callbacks ([`Reply::InputAt`], ...) answered with [`Request::Image`] /
    /// [`Request::Params`], then [`Reply::Done`] or [`Reply::Error`].
    Render(RenderRequest),
    /// The answer to [`Reply::InputAt`] / [`Reply::LayerInput`]: the image, or `None`.
    Image { image: Option<ImageRef> },
    /// The answer to [`Reply::ParamsAt`]: flattened values and texts, or `None`.
    Params { values: Option<(Vec<f64>, Vec<String>)> },
    /// The answer to [`Reply::LayerTransform`], or `None`.
    Transform { transform: Option<effectcraft_effects::plugin::LayerTransform> },
    /// The answer to [`Reply::CompCamera`], or `None`.
    Camera { camera: Option<effectcraft_effects::plugin::CompCamera> },
    /// The answer to [`Reply::CompLights`], or `None`.
    Lights { lights: Option<Vec<effectcraft_effects::plugin::CompLightInfo>> },
    /// The effect instance is gone; the helper may free what it keeps for it. No reply.
    Destroy { instance: u64 },
    /// Heartbeat → [`Reply::Pong`].
    Ping,
    /// Exit. No reply. (Closing stdin means the same.)
    Shutdown,
}

/// One effect a helper offers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteEffect {
    /// The plug-in API 2 manifest (id, name, category, parameters).
    pub manifest: PluginManifest,
    /// Whether to ask [`Request::Padding`] before every render (otherwise no padding).
    #[serde(default)]
    pub padding: bool,
}

/// Helper → EffectCraft.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Reply {
    /// Answer to [`Request::Hello`]. `loads` lists the file extensions [`Request::Load`] accepts
    /// (`["ofx"]`), empty when the helper loads nothing on request.
    Hello {
        version: u32,
        name: String,
        #[serde(default)]
        helper_version: String,
        #[serde(default)]
        loads: Vec<String>,
    },
    Effects {
        effects: Vec<RemoteEffect>,
        #[serde(default)]
        errors: Vec<String>,
    },
    Padding {
        pixels: u32,
    },
    /// During a render: the effect's input at another layer time.
    InputAt {
        time: f64,
    },
    /// During a render: the layer chosen in a layer parameter, at a layer time.
    LayerInput {
        param: String,
        time: f64,
    },
    /// During a render: the parameter values at another layer time.
    ParamsAt {
        time: f64,
    },
    /// During a render: where the layer chosen in a layer parameter sits at a layer time,
    /// relative to the effect's layer (only when the host listed `"layer_transform"` in
    /// [`Request::Hello`]) → [`Request::Transform`].
    LayerTransform {
        param: String,
        time: f64,
    },
    /// During a render: the composition's camera as the effect's layer sees it (only when the
    /// host listed `"comp_camera"` in [`Request::Hello`]) → [`Request::Camera`]; at another
    /// layer time `time` when the host also listed `"comp_camera_time"` (camera motion blur).
    CompCamera {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time: Option<f64>,
    },
    /// During a render: the composition's lights at a layer time as the effect's layer sees them
    /// (only when the host listed `"comp_lights"` in [`Request::Hello`]) → [`Request::Lights`].
    CompLights {
        time: f64,
    },
    /// The render finished; the result is in the request's frame.
    Done,
    /// The request failed (the helper keeps running).
    Error {
        message: String,
    },
    Pong,
}

/// A reply as one protocol line (with [`MAGIC`] and the newline).
pub fn reply_line(r: &Reply) -> Result<String, String> {
    serde_json::to_string(r).map(|s| format!("{MAGIC}{s}\n")).map_err(|e| e.to_string())
}

/// A request as one protocol line (with the newline).
pub fn request_line(r: &Request) -> Result<String, String> {
    serde_json::to_string(r).map(|s| format!("{s}\n")).map_err(|e| e.to_string())
}

/// Parse a helper output line: `Some` for a protocol line (after [`MAGIC`]), `None` for other
/// output.
pub fn parse_reply(line: &str) -> Option<Result<Reply, String>> {
    let at = line.find(MAGIC)?;
    let json = line.get(at + MAGIC.len()..)?.trim_end();
    Some(serde_json::from_str(json).map_err(|e| format!("bad reply from helper: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_round_trip_and_other_output_is_ignored() {
        let r = Reply::InputAt { time: 1.5 };
        let line = reply_line(&r).unwrap();
        assert_eq!(parse_reply(&line).unwrap().unwrap(), r);
        // A plug-in that printed without a newline doesn't break the protocol line.
        assert_eq!(parse_reply(&format!("loading...{line}")).unwrap().unwrap(), r);
        assert!(parse_reply("plug-in chatter").is_none());
        assert!(parse_reply("@ec-helper {nope").unwrap().is_err());
    }

    /// `features` and the layer transform callback are optional: an older host's hello still
    /// parses, and a helper that never asks is unaffected.
    #[test]
    fn hello_features_and_layer_transform() {
        let old: Request = serde_json::from_str(r#"{"op":"hello","version":1,"frames":"f"}"#).unwrap();
        assert_eq!(old, Request::Hello { version: 1, frames: "f".into(), features: vec![] });
        let new = Request::Hello { version: 1, frames: "f".into(), features: vec!["layer_transform".into()] };
        assert_eq!(serde_json::from_str::<Request>(&request_line(&new).unwrap()).unwrap(), new);
        let r = Reply::LayerTransform { param: "emitter_layer".into(), time: 0.5 };
        assert_eq!(parse_reply(&reply_line(&r).unwrap()).unwrap().unwrap(), r);
        let c = Request::Camera {
            camera: Some(effectcraft_effects::plugin::CompCamera {
                matrix: [[1.0, 0.0, 960.0, 0.0], [0.0, 1.0, 540.0, 0.0], [0.0, 0.0, 1.0, 2666.0]],
                zoom: 2666.0,
                camera_layer: true,
                dof: Some([2666.0, 200.0, 1.0]),
            }),
        };
        assert_eq!(serde_json::from_str::<Request>(&request_line(&c).unwrap()).unwrap(), c);
        // a camera without depth of field leaves `dof` out, and older hosts' replies still parse
        let off = r#"{"op":"camera","camera":{"matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,1]],"zoom":1.0,"camera_layer":false}}"#;
        let Request::Camera { camera: Some(cam) } = serde_json::from_str::<Request>(off).unwrap() else { panic!("camera") };
        assert_eq!(cam.dof, None);
        assert!(!request_line(&Request::Camera { camera: Some(cam) }).unwrap().contains("dof"));
        assert_eq!(parse_reply(&reply_line(&Reply::CompCamera { time: None }).unwrap()).unwrap().unwrap(), Reply::CompCamera { time: None });
        // an older helper's plain request still parses
        assert_eq!(serde_json::from_str::<Reply>(r#"{"op":"comp_camera"}"#).unwrap(), Reply::CompCamera { time: None });
        let at = Reply::CompCamera { time: Some(0.25) };
        assert_eq!(parse_reply(&reply_line(&at).unwrap()).unwrap().unwrap(), at);
        let l = Reply::CompLights { time: 0.5 };
        assert_eq!(parse_reply(&reply_line(&l).unwrap()).unwrap().unwrap(), l);
        let lights = Request::Lights {
            lights: Some(vec![effectcraft_effects::plugin::CompLightInfo {
                name: "Emitter".into(),
                kind: 2,
                pos: [600.0, 540.0, 0.0],
                dir: [0.0, 0.0, 1.0],
                color: [1.0, 0.0, 0.0],
                intensity: 1.0,
                cone: 90.0,
                feather: 0.5,
            }]),
        };
        assert_eq!(serde_json::from_str::<Request>(&request_line(&lights).unwrap()).unwrap(), lights);
        let t = Request::Transform {
            transform: Some(effectcraft_effects::plugin::LayerTransform {
                matrix: [[1.0, 0.0, 0.0, 2.0], [0.0, 1.0, 0.0, 3.0], [0.0, 0.0, 1.0, 0.0]],
                opacity: 0.5,
                rect: [0.0, 0.0, 400.0, 200.0],
            }),
        };
        assert_eq!(serde_json::from_str::<Request>(&request_line(&t).unwrap()).unwrap(), t);
    }

    #[test]
    fn image_sizes_are_checked() {
        let r = ImageRef { offset: 0, width: 4, height: 2, origin: [0.0; 2], scale: 1.0 };
        assert_eq!(r.bytes(), Some(128));
        assert_eq!(ImageRef { width: u32::MAX, height: u32::MAX, ..r }.pixels(), None);
    }
}
