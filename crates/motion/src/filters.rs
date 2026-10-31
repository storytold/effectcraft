//! Motion filters ↔ EffectCraft effects.
//!
//! Motion's filters are plug-ins identified by a UUID; only a few have a close EffectCraft
//! equivalent. Bad TV has none, so it is rebuilt from Wave Warp (the wobble), Noise (the static)
//! and Hue/Saturation (the colour), an approximation of the look rather than a match.

use effectcraft_keyframe::Value;
use effectcraft_project::PropGroup;
use effectcraft_project::build::Ids;
use effectcraft_time::FrameRate;
use roxmltree::Node;

use crate::curve::{Channel, Mapped, merge, read_channel};
use crate::xml::{child_text, param};

/// Motion's Add Noise filter (the one filter we write).
pub(crate) const ADD_NOISE_UUID: &str = "C423A28F-C016-4133-B120-22FF2E9A7862";

/// Filters by plug-in UUID. `pluginName` is the name the author gave the instance (templates
/// rename filters to what they do, "Static", "Roll"…), so the UUID identifies the plug-in;
/// the name is only a fallback for filters whose UUID we have not seen.
const BY_UUID: &[(&str, &str)] =
    &[(ADD_NOISE_UUID, "Add Noise"), ("32AB5EE1-BACB-4B81-B44E-6D1E643C8D00", "Bad TV"), ("6EFE2B74-1702-4829-B98E-E619501D1F16", "Sharpen")];

/// The id of a filter's generic Mix parameter (blend of the filtered and unfiltered image).
pub(crate) const MIX_ID: u32 = 10001;

/// Add Noise `Amount` 1 (Motion's default) is a noise amount of 25 %.
const NOISE_SCALE: f64 = 25.0;

/// Reading context for one filter element.
pub(crate) struct FilterIn<'a, 'i> {
    pub node: Node<'a, 'i>,
    pub rate: FrameRate,
    pub size: [f64; 2],
}

impl FilterIn<'_, '_> {
    fn channel(&self, name: &str, fallback: f64, warn: &mut dyn FnMut(String)) -> Channel {
        match param(self.node, name) {
            Some(p) => read_channel(p, self.rate, fallback, warn),
            None => Channel::constant(fallback),
        }
    }

    /// The filter's Mix (1 = fully filtered), when it has one.
    pub fn mix(&self, warn: &mut dyn FnMut(String)) -> Option<Channel> {
        let p = self.node.children().find(|c| c.is_element() && c.tag_name().name() == "parameter" && c.attribute("id") == Some("10001"))?;
        Some(read_channel(p, self.rate, 1.0, warn))
    }

    /// The plug-in this filter is (from its UUID, else its name).
    pub fn plugin(&self) -> &str {
        let uuid = self.node.attribute("pluginUUID").unwrap_or("");
        BY_UUID.iter().find(|(u, _)| u.eq_ignore_ascii_case(uuid)).map(|(_, n)| *n).or(self.node.attribute("pluginName")).unwrap_or("Filter")
    }

    /// Whether the filter's `enabled` flag is off.
    pub fn disabled(&self) -> bool {
        child_text(self.node, "enabled") == Some("0")
    }
}

/// Set effect parameter `m` from a Motion channel mapped `ec = scale · motion + offset`.
fn set(g: &mut PropGroup, m: &str, ch: &Channel, scale: f64, offset: f64, warn: &mut dyn FnMut(String)) {
    let Some(p) = g.get_mut(m) else { return };
    let (v, keys) = merge(&[Mapped { ch, scale, offset }], &|v| Value::Scalar(v.first().copied().unwrap_or(0.0)), false, warn);
    p.value = v;
    p.keys = keys;
}

fn set_value(g: &mut PropGroup, m: &str, v: Value) {
    if let Some(p) = g.get_mut(m) {
        p.value = v;
    }
}

fn effect(ids: &mut Ids, id: &str, name: &str, size: [f64; 2]) -> Option<PropGroup> {
    effectcraft_effects::find(id).map(|spec| effectcraft_effects::instantiate(spec, ids, name, size))
}

/// The effects equivalent to a Motion filter (empty, with a warning, when there is none).
pub(crate) fn import_filter(f: &FilterIn, ids: &mut Ids, warn: &mut dyn FnMut(String)) -> Vec<PropGroup> {
    let name = f.node.attribute("name").unwrap_or("Filter");
    let plugin = f.plugin();
    let size = f.size;
    let deg = 180.0 / std::f64::consts::PI;
    let mut out = vec![];
    match plugin {
        "Gaussian Blur" => {
            if let Some(mut g) = effect(ids, "ec.blur.gaussian", name, size) {
                set(&mut g, "blurriness", &f.channel("Amount", 4.0, warn), 1.0, 0.0, warn);
                out.push(g);
            }
        }
        "Sharpen" => {
            if let Some(mut g) = effect(ids, "ec.blur.sharpen", name, size) {
                set(&mut g, "amount", &f.channel("Amount", 2.5, warn), 1.0, 0.0, warn);
                out.push(g);
            }
        }
        "Directional Blur" => {
            if let Some(mut g) = effect(ids, "ec.blur.directional", name, size) {
                set(&mut g, "length", &f.channel("Amount", 10.0, warn), 1.0, 0.0, warn);
                // Motion: blur line at a counter-clockwise angle from horizontal (radians).
                // EffectCraft: clockwise from vertical (degrees).
                set(&mut g, "direction", &f.channel("Angle", 0.0, warn), -deg, 90.0, warn);
                out.push(g);
            }
        }
        "Add Noise" => {
            if let Some(mut g) = effect(ids, "ec.noise.noise", name, size) {
                set(&mut g, "amount", &f.channel("Amount", 1.0, warn), NOISE_SCALE, 0.0, warn);
                let mono = f.channel("Monochrome", 0.0, warn).value != 0.0;
                set_value(&mut g, "color", Value::Bool(!mono));
                out.push(g);
            }
        }
        "Invert" => {
            if let Some(mut g) = effect(ids, "ec.channel.invert", name, size) {
                set(&mut g, "blend", &f.channel("Mix", 1.0, warn), -100.0, 100.0, warn);
                out.push(g);
            }
        }
        "Tint" => {
            if let Some(mut g) = effect(ids, "ec.color.tint", name, size) {
                if let Some(c) = param(f.node, "Color") {
                    let ch = |n: &str, w: &mut dyn FnMut(String)| param(c, n).map(|p| read_channel(p, f.rate, 0.5, w).value).unwrap_or(0.5);
                    let (r, gr, b) = (ch("Red", warn), ch("Green", warn), ch("Blue", warn));
                    set_value(&mut g, "white", Value::Color([r, gr, b, 1.0]));
                }
                set(&mut g, "amount", &f.channel("Intensity", 1.0, warn), 100.0, 0.0, warn);
                out.push(g);
            }
        }
        "Bad TV" => {
            if let Some(mut g) = effect(ids, "ec.distort.wavewarp", &format!("{name} (Wobble)"), size) {
                set(&mut g, "height", &f.channel("Waviness", 10.0, warn), 1.0, 0.0, warn);
                set_value(&mut g, "width", Value::Scalar(120.0));
                set_value(&mut g, "direction", Value::Scalar(0.0));
                set(&mut g, "speed", &f.channel("Roll", 45.0, warn), 1.0 / 15.0, 0.0, warn);
                out.push(g);
            }
            if let Some(mut g) = effect(ids, "ec.noise.noise", &format!("{name} (Static)"), size) {
                set(&mut g, "amount", &f.channel("Static", 0.1, warn), 6.25, 0.0, warn);
                set_value(&mut g, "color", Value::Bool(false));
                out.push(g);
            }
            if let Some(mut g) = effect(ids, "ec.color.huesaturation", &format!("{name} (Saturate)"), size) {
                set(&mut g, "saturation", &f.channel("Saturate", -25.0, warn), 1.0, 0.0, warn);
                out.push(g);
            }
            warn("Bad TV filters are approximated with Wave Warp, Noise and Hue/Saturation; scan lines are not reproduced".into());
        }
        other => warn(format!("filter `{name}` ({other}) has no EffectCraft equivalent (skipped)")),
    }
    if f.disabled() {
        for g in &mut out {
            g.enabled = false;
        }
    }
    out
}

/// A filter to write: plug-in identity and its parameters (name, id, channel).
pub(crate) struct FilterOut {
    pub name: String,
    /// Mix (from the adjustment layer's opacity), when not a constant 1.
    pub mix: Option<Channel>,
    pub plugin: &'static str,
    pub uuid: &'static str,
    pub enabled: bool,
    pub params: Vec<(&'static str, u32, Channel)>,
}

/// The Motion filter for an EffectCraft effect, if Motion has one we can name.
pub(crate) fn export_effect(g: &PropGroup) -> Option<FilterOut> {
    let effectcraft_project::GroupKind::Effect { effect } = &g.kind else { return None };
    match effect.as_str() {
        "ec.noise.noise" => {
            let amount = g.get("amount").map(|p| crate::curve::channel_of(p, 0, NOISE_SCALE, 0.0)).unwrap_or_default();
            let color = g.get("color").map(|p| matches!(p.value, Value::Bool(true))).unwrap_or(true);
            Some(FilterOut {
                name: g.name.clone(),
                mix: None,
                plugin: "Add Noise",
                uuid: ADD_NOISE_UUID,
                enabled: g.enabled,
                params: vec![
                    ("Amount", 1, amount),
                    ("Type", 2, Channel::constant(0.0)),
                    ("Monochrome", 3, Channel::constant(if color { 0.0 } else { 1.0 })),
                    ("Random Seed", 6, Channel::constant(25.0)),
                ],
            })
        }
        _ => None,
    }
}
