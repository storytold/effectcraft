//! The helper protocol, version [`PROTOCOL_VERSION`] (`docs/external-helpers.md`).
//!
//! Control messages are JSON, one per line. EffectCraft writes requests ([`Request`]) to the
//! helper's stdin; the helper writes replies ([`Reply`]) to its stdout, each line starting with
//! [`MAGIC`] so that anything else a helper (or a library it loads) prints is ignored. Pixels
//! never travel over the pipe: they live in the **frame exchange file** named in
//! [`Request::Hello`] and are addressed by [`ImageRef`]s (byte offset + size).

use effectcraft_effects::MaskPath;
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

/// The most masks one message carries (more are left out).
pub const MAX_MASKS: usize = 1024;
/// The most mask vertices one message carries, all masks together (a mask that doesn't fit is
/// left out with those after it): about 8 MB of JSON, far below [`MAX_LINE`].
pub const MAX_MASK_VERTICES: usize = 1 << 16;

/// The masks as they are sent: masks whose numbers aren't all finite, or whose tangent lists
/// don't match their vertices, are left out; then at most [`MAX_MASKS`] masks and
/// [`MAX_MASK_VERTICES`] vertices in all, in order.
pub fn bounded_masks(masks: Vec<MaskPath>) -> Vec<MaskPath> {
    let finite = |m: &MaskPath| {
        let pts = m.vertices.iter().chain(&m.in_tangents).chain(&m.out_tangents).chain(std::iter::once(&m.feather));
        pts.flatten().chain([&m.opacity, &m.expansion]).all(|v| v.is_finite())
    };
    let (mut out, mut vertices) = (Vec::new(), 0usize);
    for m in masks {
        if !finite(&m) || m.in_tangents.len() != m.vertices.len() || m.out_tangents.len() != m.vertices.len() {
            continue;
        }
        vertices = vertices.saturating_add(m.vertices.len());
        if out.len() >= MAX_MASKS || vertices > MAX_MASK_VERTICES {
            log::warn!("external effect helper: masks beyond {MAX_MASKS} masks / {MAX_MASK_VERTICES} vertices are not passed on");
            break;
        }
        out.push(m);
    }
    out
}

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
    /// The layer's masks in buffer pixels of `frame` (effects with [`RemoteEffect::masks`] of
    /// helpers with [`FEATURE_MASKS`]; absent otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masks: Option<Vec<MaskPath>>,
}

/// EffectCraft → helper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// First message: the protocol version and the frame exchange file to open (read + write).
    Hello { version: u32, frames: String },
    /// List the effects the helper offers → [`Reply::Effects`].
    Describe,
    /// Load more effects from a file or folder the user picked → [`Reply::Effects`] (only the
    /// new ones).
    Load { path: String },
    /// Transparent pixels to add around the buffer before [`Request::Render`] → [`Reply::Padding`].
    /// `masks` as in [`RenderRequest::masks`], in buffer pixels of the buffer before padding
    /// (where `flat`'s points are).
    Padding {
        effect: String,
        instance: u64,
        time: f64,
        scale: f64,
        layer_size: [f64; 2],
        flat: Vec<f64>,
        texts: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        masks: Option<Vec<MaskPath>>,
    },
    /// Render → callbacks ([`Reply::InputAt`], ...) answered with [`Request::Image`] /
    /// [`Request::Params`], then [`Reply::Done`] or [`Reply::Error`].
    Render(RenderRequest),
    /// The answer to [`Reply::InputAt`] / [`Reply::LayerInput`]: the image, or `None`.
    Image { image: Option<ImageRef> },
    /// The answer to [`Reply::ParamsAt`]: flattened values and texts, or `None`.
    Params { values: Option<(Vec<f64>, Vec<String>)> },
    /// The answer to [`Reply::LayerMasks`]: the masks in buffer pixels of the render's frame,
    /// or `None`.
    Masks { masks: Option<Vec<MaskPath>> },
    /// The user changed parameter `param` of an effect that asked for it
    /// ([`RemoteEffect::notify_params`]; only to helpers that list [`FEATURE_PARAM_CHANGED`] in
    /// their `hello`) → [`Reply::SetParams`] or [`Reply::Error`]. `values` holds every parameter's
    /// value in property units ([`effectcraft_effects::plugin::param_values`]), `texts` the text /
    /// hidden ones as in [`RenderRequest::texts`]. Sent between renders, never into one.
    ParamChanged {
        effect: String,
        instance: u64,
        param: String,
        /// Layer time, seconds.
        time: f64,
        layer_size: [f64; 2],
        values: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        texts: Vec<String>,
    },
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
    /// Parameter ids whose changes by the user EffectCraft reports with
    /// [`Request::ParamChanged`] (helpers that announce [`FEATURE_PARAM_CHANGED`]). Read
    /// leniently: anything but an array of the manifest's parameter ids is ignored, entry by entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_params: Option<serde_json::Value>,
    /// `true`: every `render` / `padding` of the effect carries its layer's masks (helpers that
    /// announce [`FEATURE_MASKS`]). Anything else means no.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masks: Option<serde_json::Value>,
}

impl RemoteEffect {
    /// An effect without the protocol extensions.
    pub fn new(manifest: PluginManifest, padding: bool) -> RemoteEffect {
        RemoteEffect { manifest, padding, notify_params: None, masks: None }
    }

    /// Whether the effect asked for its layer's masks ([`RemoteEffect::masks`] is `true`).
    pub fn wants_masks(&self) -> bool {
        self.masks == Some(serde_json::Value::Bool(true))
    }

    /// The valid [`RemoteEffect::notify_params`]: ids of the manifest's parameters, each once.
    pub fn notify_ids(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for id in self.notify_params.as_ref().and_then(serde_json::Value::as_array).into_iter().flatten().filter_map(serde_json::Value::as_str) {
            if self.manifest.params.iter().any(|p| p.id == id) && !out.iter().any(|o| o == id) {
                out.push(id.to_string());
            }
        }
        out
    }
}

/// The `hello` feature of helpers that answer [`Request::ParamChanged`].
pub const FEATURE_PARAM_CHANGED: &str = "param_changed";
/// The `hello` feature of helpers that take masks ([`RemoteEffect::masks`],
/// [`Reply::LayerMasks`]).
pub const FEATURE_MASKS: &str = "masks";

/// The feature names in a helper's `hello` (`features`): the strings of an array, at most 64,
/// each at most 64 bytes; anything else is ignored.
pub fn feature_list(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|s| s.len() <= 64)
        .take(64)
        .map(str::to_string)
        .collect()
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
        /// How many processes of this helper may run at once (a whole number, 1 when absent or
        /// anything else; clamped by EffectCraft). Each process has its own conversation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_processes: Option<serde_json::Value>,
        /// The protocol extensions the helper understands (`["param_changed"]`, see
        /// [`feature_list`]). Absent: none, the helper is spoken to exactly as before.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        features: Option<serde_json::Value>,
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
    /// During a render: the masks of the layer chosen in a layer parameter, at a layer time
    /// → [`Request::Masks`].
    LayerMasks {
        param: String,
        time: f64,
    },
    /// The render finished; the result is in the request's frame.
    Done,
    /// Answer to [`Request::ParamChanged`]: parameter values to set with the user's change
    /// (only those the helper changes, same units; ids that aren't the effect's, values of the
    /// wrong type and numbers that aren't finite are ignored). `ui` (per-parameter UI state such
    /// as `{"p12": {"enabled": false}}`) is accepted and ignored for now.
    SetParams {
        #[serde(default)]
        values: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ui: Option<serde_json::Value>,
    },
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

    #[test]
    fn image_sizes_are_checked() {
        let r = ImageRef { offset: 0, width: 4, height: 2, origin: [0.0; 2], scale: 1.0 };
        assert_eq!(r.bytes(), Some(128));
        assert_eq!(ImageRef { width: u32::MAX, height: u32::MAX, ..r }.pixels(), None);
    }
}
