//! Plug-in `effect` contributions as EffectCraft effects.
//!
//! A [`Surface`] of kind `effect` becomes an [`EffectPlugin`] in the effect registry
//! (`effectcraft_effects::plugin`). The registry hands it a premultiplied RGBA `f32` frame and the
//! parameters flattened to `f64`s. [`HostEffect::render`] gives the host the same bytes as one
//! [`ImageJob`], without a copy, and the host banding, hygiene and fault rules apply
//! ([`craft_plugin_host::process`]). A v1 `ec_` plug-in takes the same path through its shim.
//!
//! The registry has no parameter order for a craft-abi 1 contribution (`params` is a map), so
//! parameters are in id order.

use std::sync::Arc;

use craft_command::Owner;
use craft_plugin_abi::descriptor::{Alpha, Sample};
use craft_plugin_abi::manifest::{AlphaMode, Contribution, Kind, ParamSpec, ParamType, SampleType};
use craft_plugin_host::contrib::surface::Surface;
use craft_plugin_host::process::{ImageFormat, ImageJob, ProcessOpts};
use effectcraft_effects::plugin::{
    EffectPlugin, PLUGIN_API_VERSION, PluginFrame, PluginManifest, PluginParam, PluginParamKind, PluginParams, register_plugin, unregister_plugin,
};
use serde_json::{Map, Value, json};

/// Category of an effect whose contribution names none.
const DEFAULT_CATEGORY: &str = "Plug-ins";

/// The effect id of a contribution: the plug-in id, plus the contribution id unless it is the
/// only one a v1 shim describes (`effect`).
pub fn effect_id(owner: &Owner, contribution_id: &str) -> String {
    if contribution_id == "effect" { owner.plugin_id.to_string() } else { format!("{}.{contribution_id}", owner.plugin_id) }
}

fn number(v: &Option<Value>, fallback: f64) -> f64 {
    v.as_ref().and_then(Value::as_f64).unwrap_or(fallback)
}

fn floats<const N: usize>(v: &Option<Value>, fallback: [f64; N]) -> [f64; N] {
    let mut out = fallback;
    for (slot, x) in out.iter_mut().zip(v.iter().filter_map(Value::as_array).flatten()) {
        *slot = x.as_f64().unwrap_or(*slot);
    }
    out
}

/// The registry's kind for a parameter, or why the registry cannot show it.
fn kind_of(id: &str, p: &ParamSpec) -> Result<PluginParamKind, String> {
    Ok(match p.ty {
        ParamType::Number | ParamType::Int => {
            let (min, max) = (p.min.unwrap_or(0.0), p.max.unwrap_or(100.0));
            let decimals = if p.ty == ParamType::Int { 0 } else { p.decimals.map_or(2, |d| d.clamp(0, 6) as u8) };
            PluginParamKind::Slider {
                default: number(&p.default, min),
                min,
                max,
                slider_min: None,
                slider_max: None,
                decimals,
                integer: p.ty == ParamType::Int,
            }
        }
        ParamType::Angle => PluginParamKind::Angle { default: number(&p.default, 0.0) },
        ParamType::Bool => PluginParamKind::Checkbox { default: p.default.as_ref().and_then(Value::as_bool).unwrap_or(false) },
        ParamType::Choice => {
            let options = p.options.clone().unwrap_or_default();
            let default = p.default.as_ref().and_then(Value::as_str).and_then(|d| options.iter().position(|o| o == d)).unwrap_or(0) as u32;
            PluginParamKind::Popup { options, default }
        }
        ParamType::Point2 => PluginParamKind::Point { default: floats(&p.default, [0.5, 0.5]) },
        ParamType::Color => PluginParamKind::Color { default: floats(&p.default, [1.0; 4]) },
        other => return Err(format!("parameter `{id}` is {other:?}, which an effect cannot show")),
    })
}

/// A contribution as the registry describes an effect.
pub fn manifest_of(surface: &Surface) -> Result<PluginManifest, String> {
    let c = &surface.contribution;
    let id = effect_id(&surface.owner, &c.id);
    let params = c
        .params
        .iter()
        .flatten()
        .map(|(k, p)| Ok(PluginParam { id: k.clone(), name: p.label.clone().unwrap_or_else(|| k.clone()), group: String::new(), kind: kind_of(k, p)? }));
    Ok(PluginManifest {
        api: PLUGIN_API_VERSION,
        name: c.label.clone().unwrap_or_else(|| id.clone()),
        category: c.category.clone().unwrap_or_else(|| DEFAULT_CATEGORY.to_owned()),
        version: String::new(),
        author: String::new(),
        description: String::new(),
        params: params.collect::<Result<_, String>>()?,
        id,
    })
}

/// The `params` object of a host call, from the flat values the registry evaluated.
fn params_json(m: &PluginManifest, flat: &[f64]) -> Value {
    let mut at = 0;
    let mut out = Map::new();
    for p in &m.params {
        let w = p.kind.width();
        let v = flat.get(at..at + w).unwrap_or(&[]);
        at += w;
        let first = v.first().copied().unwrap_or(0.0);
        let value = match &p.kind {
            PluginParamKind::Slider { decimals: 0, .. } => json!(first.round() as i64),
            PluginParamKind::Slider { .. } | PluginParamKind::Angle { .. } => json!(first),
            PluginParamKind::Checkbox { .. } => json!(first != 0.0),
            PluginParamKind::Popup { options, .. } => json!(options.get(first.max(0.0) as usize).cloned().unwrap_or_default()),
            PluginParamKind::Point { .. } | PluginParamKind::Point3 { .. } | PluginParamKind::Color { .. } => json!(v),
            PluginParamKind::Layer => json!(first.round() as i64),
            // `kind_of` never makes these two: a craft-abi parameter has no text type here.
            PluginParamKind::Text { .. } | PluginParamKind::Hidden { .. } => continue,
        };
        out.insert(p.id.clone(), value);
    }
    Value::Object(out)
}

/// The buffer a plug-in gets. The registry's frame is premultiplied `f32`; a contribution that
/// declares other samples or straight alpha gets a converted copy.
#[derive(Clone, Copy)]
struct Wire {
    sample: Sample,
    alpha: Alpha,
}

fn wire_of(c: &Contribution) -> Result<Wire, String> {
    let process = c.process.as_ref();
    let samples = process.and_then(|p| p.sample.as_ref());
    let sample = match samples {
        None => Sample::F32,
        Some(s) if s.contains(&SampleType::F32) => Sample::F32,
        Some(s) if s.contains(&SampleType::U8) => Sample::U8,
        Some(s) => return Err(format!("it takes {s:?} samples, and an effect gives f32 or u8")),
    };
    let alpha = match process.and_then(|p| p.alpha) {
        None | Some(AlphaMode::Premultiplied) => Alpha::Premultiplied,
        Some(AlphaMode::Straight) => Alpha::Straight,
        Some(AlphaMode::None) => return Err("it takes images without alpha, and an effect has RGBA".into()),
    };
    Ok(Wire { sample, alpha })
}

fn straight(p: [f32; 4]) -> [f32; 4] {
    if p[3] > 0.0 { [p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3]] } else { [0.0; 4] }
}

fn premultiplied(p: [f32; 4]) -> [f32; 4] {
    [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

struct HostEffect {
    manifest: PluginManifest,
    surface: Surface,
    wire: Wire,
}

impl HostEffect {
    fn run(&self, format: ImageFormat, data: &mut [u8], scale: f64, params: &Value, time: f64) -> Result<(), String> {
        let mut job = ImageJob::new(format, data);
        job.scale = scale;
        job.time = time;
        self.surface.process_image(&mut job, params, &ProcessOpts::default()).map(drop).map_err(|e| e.to_string())
    }
}

impl EffectPlugin for HostEffect {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String> {
        let Wire { sample, alpha } = self.wire;
        let format = ImageFormat {
            width: frame.width,
            height: frame.height,
            channels: 4,
            sample,
            alpha,
            color_mode: 0,
            depth: if sample == Sample::U8 { 8 } else { 32 },
        };
        let (scale, params) = (frame.scale, params_json(&self.manifest, params.flat));
        let convert = |p: [f32; 4]| if alpha == Alpha::Straight { straight(p) } else { p };
        match sample {
            // The registry's own buffer, no copy.
            Sample::F32 if alpha == Alpha::Premultiplied => self.run(format, bytemuck::cast_slice_mut(frame.pixels), scale, &params, time),
            Sample::F32 => {
                let mut copy: Vec<[f32; 4]> = frame.pixels.iter().map(|p| convert(*p)).collect();
                self.run(format, bytemuck::cast_slice_mut(&mut copy), scale, &params, time)?;
                frame.pixels.iter_mut().zip(copy).for_each(|(p, q)| *p = premultiplied(q));
                Ok(())
            }
            _ => {
                let mut bytes: Vec<u8> = frame.pixels.iter().flat_map(|p| convert(*p).map(to_u8)).collect();
                self.run(format, &mut bytes, scale, &params, time)?;
                for (p, q) in frame.pixels.iter_mut().zip(bytes.chunks_exact(4)) {
                    let q = [q[0], q[1], q[2], q[3]].map(|b| f32::from(b) / 255.0);
                    *p = if alpha == Alpha::Straight { premultiplied(q) } else { q };
                }
                Ok(())
            }
        }
    }

    fn source(&self) -> String {
        format!("plug-in {}", self.surface.owner.plugin_id)
    }
}

/// Put an `effect` surface in the effect registry. A surface of another kind is ignored.
pub fn register(surface: &Surface) -> Result<(), String> {
    if surface.contribution.kind != Kind::Effect {
        return Ok(());
    }
    let wire = wire_of(&surface.contribution)?;
    let manifest = manifest_of(surface)?;
    register_plugin(Arc::new(HostEffect { manifest, surface: surface.clone(), wire })).map(drop)
}

/// Take an `effect` surface out of the effect registry.
pub fn unregister(owner: &Owner, contribution_id: &str) -> bool {
    unregister_plugin(&effect_id(owner, contribution_id))
}
