//! The effects library.
//!
//! Every effect is an [`EffectSpec`]: a stable id (`ec.blur.gaussian`), its display name and
//! category (the Effects & Presets tree), typed parameters with defaults and UI hints, and a CPU
//! render function. Effect instances on layers are ordinary property groups (see
//! [`instantiate`]), so parameters animate, take expressions and are addressable like any other
//! property.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod audio_fx;
mod blur2;
mod blur3;
pub mod camera_tracker;
mod card3d;
pub mod catalog;
mod channel;
mod channel2;
mod channel3d;
mod color2;
mod color3;
mod color_fx;
pub mod controls;
mod distort;
mod distort2;
mod distort3;
pub mod distort4;
pub mod face_track;
mod generate;
mod generate2;
mod generate3;
mod keying;
mod keying2;
pub mod keylight;
mod matte;
pub mod migrate;
mod misc;
pub mod mocha_shape;
mod noise;
mod noise2;
mod noise3;
mod obsolete;
mod ocio;
pub mod ocio_config;
pub mod paint;
mod perspective;
mod perspective2;
pub mod plugin;
pub mod psim;
pub mod puppet;
pub mod roto;
mod sim;
mod sim2;
mod sim3;
mod stylize2;
mod stylize3;
mod textfx;
mod time_fx;
mod transition;
mod transition2;
pub mod util;
mod utility;
mod utility2;
mod vr;
pub mod warp_stab;

use std::collections::HashMap;
use std::sync::OnceLock;

pub use card3d::{CompLight, CompScene};
pub use color_fx::{COLORAMA_PRESETS, ColoramaPalette};
pub use color_fx::{
    HUESAT_CHANNELS, HUESAT_RANGES, LEVELS_CHANNELS, PHOTO_FILTER_CUSTOM, PHOTO_FILTERS, exposure_settings, fill_uses_masks, huesat_ranges_identity,
    levels_channel_ids, levels_channel_settings, levels_channels_identity, levels_clip,
};
pub use color2::{Curve, auto_correct_settings, color_link_mode, color_link_sample, equalize_tables, luma_clip_points, shadow_highlight_amounts};
pub use color3::OffsetCurve;
pub use distort::transform_shutter;
pub use distort2::{MAGNIFY_MODES, bezier_grid, parse_mesh};
pub use distort3::{RESHAPE_POINTS, ReshapeSetup, SmearSetup, reshape_setup, smear_setup, warp_inverse_map};
pub use distort4::liquify_mesh;
use effectcraft_keyframe::Value;
use effectcraft_project::build::Ids;
use effectcraft_project::{GroupKind, ParamUi, PropGroup, Property};
pub use effectcraft_raster::{AuxChannels, Image};
pub use misc::{INVERT_ALPHA, INVERT_CHANNELS, glow_ab_t, glow_operation};
pub use sim::particle_state;
pub use sim3::{PgBlit, PgPlan, playground_plan, playground_state};
pub use stylize2::strobe_on;
// CPU helpers the GPU kernels share (effectcraft-gpu).
pub use blur2::{camera_lens_plain, camera_lens_spans};
pub use generate::gen_mode;
pub use generate2::pattern_kind as cell_pattern_kind;
pub use generate2::{BoltSeg, Contour, Ghost, RadioPlan, Wave, flare_ghosts, lightning_segments, radio_plan};
pub use generate3::{
    Coverage, MarksPlan, PaintPlan, Seg, eyedropper_color, marks_plan, paint_bucket_region, paint_plan, plan_reads_pixels, poly_segs, scribble_style,
};
pub use noise::GrainLook;
pub use textfx::{BoltPlan, TextLook, TextPass, bolt_plan, text_passes};
pub use transition::place_layer;
// effectcraft-gpu fx_noise.
pub use keying2::{InnerOuterPlan, inner_outer_plan};
pub use noise::remove_grain_level;
pub use noise3::{FractalGpu, fractal_gpu};
pub use stylize3::{BumpLight, bump_layer_id};
pub use textfx::{TextLayer, text_layer};
pub use time_fx::{PixelMotion, TimewarpPlan, TwStep, posterized_time, time_frames, timewarp_crop, timewarp_plan};
// effectcraft-gpu fx_tone.
pub use ocio::color_stabilizer_maps;
// GPU effects, part B (3D Channel, Immersive Video, colour management, simulation render passes).
pub use channel3d::{CRYPTO_LAYERS, id_color, selection_hashes};
pub use ocio::{ColorOp, Straight, Tf, color_program};
pub use sim::{Acc, Blob, Bubble, Post, Shape, Sprite, SpritePlan, Tint, bubble_list, drizzle_drops, mercury_blobs, sprite_plan};
pub use sim2::{
    CausticsSetup, FoamDisc, FoamEnv, FoamPlan, Piece, PiecePlan, PieceTex, PixelPlan, WavePlan, Waves, caustics_setup, foam_full_plan, piece_plan, pixel_plan,
    wave_world_plan,
};
pub use utility::{Lut, load_lut};
pub use vr::{equi_dir as vr_equi_dir, rotation as vr_rotation};

/// Effect categories in Effects & Presets order.
pub const CATEGORIES: &[&str] = &[
    "3D Channel",
    "Audio",
    "Blur & Sharpen",
    "Channel",
    "Color Correction",
    "Distort",
    "Expression Controls",
    "Generate",
    "Immersive Video",
    "Keying",
    "Matte",
    "Noise & Grain",
    "Obsolete",
    "Paint",
    "Perspective",
    "Simulation",
    "Stylize",
    "Text",
    "Time",
    "Transition",
    "Utility",
];

pub struct ParamSpec {
    /// Point defaults are fractions of the layer size (0.5, 0.5 = layer centre).
    pub id: &'static str,
    pub name: &'static str,
    pub default: Value,
    pub ui: ParamUi,
}

/// Parameter values evaluated at the current time.
#[derive(Clone, Debug, Default)]
pub struct Params {
    pub values: HashMap<String, Value>,
}

impl Params {
    pub fn get(&self, id: &str) -> Option<&Value> {
        self.values.get(id)
    }
    pub fn f(&self, id: &str) -> f64 {
        self.values.get(id).map(Value::as_f64).unwrap_or(0.0)
    }
    pub fn v2(&self, id: &str) -> [f64; 2] {
        self.values.get(id).map(Value::as_vec2).unwrap_or([0.0; 2])
    }
    pub fn color(&self, id: &str) -> [f32; 4] {
        self.values.get(id).map(Value::as_color).unwrap_or([1.0; 4])
    }
    pub fn b(&self, id: &str) -> bool {
        self.values.get(id).map(Value::as_bool).unwrap_or(false)
    }
    pub fn e(&self, id: &str) -> u32 {
        self.values.get(id).map(Value::as_enum).unwrap_or(0)
    }
    /// String parameter (`Value::Str`), empty when missing or of another kind.
    pub fn s(&self, id: &str) -> &str {
        match self.values.get(id) {
            Some(Value::Str(s)) => s,
            _ => "",
        }
    }
    /// Key prefixes (`{prefix}#1/`, `{prefix}#2/`, …) of the nested groups flattened under
    /// `prefix` by [`flatten_params`], in order.
    pub fn groups(&self, prefix: &str) -> Vec<String> {
        let mut out = vec![];
        for i in 1.. {
            let pre = format!("{prefix}#{i}/");
            if !self.values.contains_key(&format!("{pre}@match")) {
                break;
            }
            out.push(pre);
        }
        out
    }
    /// The first nested group under `prefix` with match id `m`.
    pub fn group(&self, prefix: &str, m: &str) -> Option<String> {
        self.groups(prefix).into_iter().find(|g| self.s(&format!("{g}@match")) == m)
    }
}

/// Evaluate an effect instance's parameters, including nested groups (Paint strokes, Puppet
/// meshes and pins). Direct properties keep their match id as the key. The `i`-th child group
/// (1-based, counting groups only) of a group with key prefix `pre` gets the prefix
/// `{pre}#{i}/`, its properties `{pre}#{i}/{match}`, and metadata keys `@match`, `@name`
/// (`Value::Str`), `@enabled` (`Value::Bool`) and `@uid` (`Value::Scalar`). Nested properties
/// are also keyed by their match path (`group/sub/param`, the first group with each match id),
/// which is how effects with twirl-down groups declare and read them.
pub fn flatten_params(g: &PropGroup, eval: &mut dyn FnMut(&Property) -> Value) -> Params {
    fn walk(g: &PropGroup, pre: &str, mpre: &str, eval: &mut dyn FnMut(&Property) -> Value, out: &mut HashMap<String, Value>) {
        let mut gi = 0;
        for c in &g.children {
            match c {
                effectcraft_project::Node::Prop(p) => {
                    let v = eval(p);
                    if !mpre.is_empty() {
                        out.entry(format!("{mpre}{}", p.match_id)).or_insert_with(|| v.clone());
                    }
                    out.insert(format!("{pre}{}", p.match_id), v);
                }
                effectcraft_project::Node::Group(sg) => {
                    gi += 1;
                    let sp = format!("{pre}#{gi}/");
                    out.insert(format!("{sp}@match"), Value::Str(sg.match_id.clone()));
                    out.insert(format!("{sp}@name"), Value::Str(sg.name.clone()));
                    out.insert(format!("{sp}@enabled"), Value::Bool(sg.enabled));
                    out.insert(format!("{sp}@uid"), Value::Scalar(sg.uid as f64));
                    walk(sg, &sp, &format!("{mpre}{}/", sg.match_id), eval, out);
                }
            }
        }
    }
    let mut values = HashMap::new();
    walk(g, "", "", eval, &mut values);
    Params { values }
}

/// The warning shown with effect instance `g` (Effect Controls, `effect.warning`), its
/// parameters evaluated by `eval`: settings it can't render as asked, such as an OCIO config it
/// can't read or transforms it passes through. `None` when it renders as asked.
pub fn warning(g: &PropGroup, eval: &mut dyn FnMut(&Property) -> Value) -> Option<String> {
    let GroupKind::Effect { effect } = &g.kind else { return None };
    // Only the effects that can warn evaluate their parameters.
    if !ocio::CUSTOM_CONFIG_FX.contains(&effect.as_str()) {
        return None;
    }
    ocio::warning(effect, &flatten_params(g, eval))
}

/// A layer image in flight: point `p` sits at pixel `p * scale + offset`. Points are in layer
/// space, or in effect space while the layer's effects run (see [`Buf::rebase`]).
#[derive(Clone, Debug, Default)]
pub struct Buf {
    pub img: Image,
    pub offset: [f64; 2],
    pub scale: f64,
}

impl Buf {
    pub fn to_px(&self, p: [f64; 2]) -> (f64, f64) {
        (p[0] * self.scale + self.offset[0], p[1] * self.scale + self.offset[1])
    }
    /// Measure from `origin` (in the current coordinates): a point `p` becomes `p − origin`.
    /// Moves between layer space and effect space (the renderer's `effect_bounds`).
    pub fn rebase(&mut self, origin: [f64; 2]) {
        self.offset[0] += origin[0] * self.scale;
        self.offset[1] += origin[1] * self.scale;
    }
    /// Grow by `pad` transparent pixels on every side.
    pub fn pad(&mut self, pad: u32) {
        if pad == 0 {
            return;
        }
        self.img = self.img.padded(pad);
        self.offset[0] += pad as f64;
        self.offset[1] += pad as f64;
    }
}

/// A layer mask at the current time, flattened to a polyline in layer coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaskShape {
    pub name: String,
    pub points: Vec<[f64; 2]>,
    pub closed: bool,
    pub inverted: bool,
}

/// Pixels of another layer referenced by a layer parameter: its buffer (layer space, see
/// [`Buf`]) and its source size in layer pixels.
#[derive(Clone, Debug, Default)]
pub struct LayerPixels {
    pub buf: Buf,
    pub size: [f64; 2],
}

/// Services the renderer offers effects that look beyond their own pixels.
pub trait EffectHost: Sync {
    /// Layer `id` of the same composition at the current comp time. `masks_and_effects`
    /// selects "Effects & Masks" over "Source". `None` for missing/self/recursive references.
    fn layer(&self, id: u64, masks_and_effects: bool) -> Option<LayerPixels>;
    /// `frames` stereo sample frames (interleaved L R …) of layer `id`'s audio starting at comp
    /// time `start` seconds, at `rate` Hz. `None` when the layer has no audio.
    fn audio(&self, id: u64, start: f64, frames: usize, rate: u32) -> Option<Vec<f32>>;
    /// Layer `id` with its masks but none of its effects (a layer parameter's "Masks" choice).
    /// Defaults to the plain source.
    fn layer_masks(&self, id: u64) -> Option<LayerPixels> {
        self.layer(id, false)
    }
    /// The effect's own layer at another **layer time** `layer_time` (seconds): its source with
    /// masks applied, followed by the first `effects` effects of its stack (0 = none, which is
    /// what After Effects' Time effects see). `None` when unavailable (no host, recursion
    /// limit, adjustment layers). Time effects use this to read neighbouring frames.
    fn self_at(&self, _layer_time: f64, _effects: usize) -> Option<Buf> {
        None
    }
    /// Layer `id` of the same composition at comp time `comp_time` (seconds), like
    /// [`EffectHost::layer`] but at another time.
    fn layer_at(&self, _id: u64, _comp_time: f64, _masks_and_effects: bool) -> Option<LayerPixels> {
        None
    }
    /// Auxiliary 3D channels (depth, object / material IDs, normals, Cryptomatte, any named
    /// EXR channel) of the effect's own layer source at the current time, in layer space (see
    /// [`AuxChannels`]). `None` when the source has none (the 3D Channel effects then pass the
    /// layer through).
    fn aux(&self) -> Option<std::sync::Arc<AuxChannels>> {
        None
    }
    /// The composition's camera and first light relative to the effect's layer (Card
    /// Dance / Shatter / Card Wipe's Comp Camera and First Comp Light). `None` when unknown.
    fn comp_scene(&self) -> Option<CompScene> {
        None
    }
    /// The running effect's own parameters evaluated at another **layer time** (Radio Waves'
    /// Parameters Are Set At: Birth). `None` when unavailable.
    fn params_at(&self, _layer_time: f64) -> Option<Params> {
        None
    }
    /// A particle simulation backend (GPU particles; see [`psim`]). `None` = simulate on the
    /// CPU.
    fn particles(&self) -> Option<&dyn psim::ParticleSim> {
        None
    }
}

/// Extra context the renderer may supply (all optional; `Default` is "nothing known").
#[derive(Clone, Copy, Default)]
pub struct EffectEnv<'a> {
    /// The layer's masks (enabled ones, top to bottom).
    pub masks: &'a [MaskShape],
    pub host: Option<&'a dyn EffectHost>,
    /// Composition time in seconds.
    pub comp_time: f64,
    /// Composition frame rate (0 when unknown).
    pub frame_rate: f64,
    /// Index of the running effect in its layer's stack (for [`EffectHost::self_at`]).
    pub effect_index: usize,
    /// The project's working colour space (`None` = unmanaged, treated as sRGB).
    pub working_space: Option<effectcraft_color::ColorSpace>,
    /// Working-space pixels are linear light (Linearize Working Space).
    pub working_linear: bool,
    /// The composition's shutter (angle and phase in degrees, samples per frame) when both the
    /// comp's and the layer's motion blur switches are on (Transform's Use Composition's
    /// Shutter Angle).
    pub shutter: Option<(f64, f64, u32)>,
    /// The layer's source time range in **layer time** seconds as (first, end): the layer time of
    /// the first source frame and the time just past the last one (so the source lasts
    /// `end - first` seconds and its last frame starts one frame before `end`). `None` when the
    /// source has no duration (stills, solids, text, shapes) or the renderer does not know it.
    /// Retimers clamp their source requests to it.
    pub layer_span: Option<(f64, f64)>,
}

/// What an effect gets to render with.
pub struct EffectCtx<'a> {
    pub params: &'a Params,
    /// Layer time in seconds.
    pub time: f64,
    /// Layer source size in layer pixels (the effect's "layer bounds").
    pub layer_size: [f64; 2],
    /// Instance seed (stable per effect instance).
    pub seed: u32,
    /// Rendering for an adjustment layer (operating on the comp below).
    pub adjustment: bool,
    /// Masks, other layers, audio, comp timing.
    pub env: EffectEnv<'a>,
}

impl EffectCtx<'_> {
    /// The layer chosen in layer parameter `id`, via the host.
    /// `masks_and_effects` is the effect's built-in choice, used when the instance has no
    /// source companion parameter (see [`layer_source_id`]).
    pub fn layer_param(&self, id: &str, masks_and_effects: bool) -> Option<LayerPixels> {
        let lid = self.params.get(id)?.as_layer()?;
        let host = self.env.host?;
        match self.params.get(&layer_source_id(id)).map(Value::as_enum) {
            Some(0) => host.layer(lid, false),
            Some(1) => host.layer_masks(lid),
            Some(_) => host.layer(lid, true),
            None => host.layer(lid, masks_and_effects),
        }
    }
    /// Frame rate for frame-based maths (comp rate, else 30).
    pub fn fps(&self) -> f64 {
        if self.env.frame_rate > 0.0 { self.env.frame_rate } else { 30.0 }
    }
}

pub type RenderFn = fn(&EffectCtx, Buf) -> Buf;

pub struct EffectSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub params: Vec<ParamSpec>,
    pub render: RenderFn,
    /// Implemented on the GPU path too (shown with a badge).
    pub gpu: bool,
    /// Supports 32 bpc float (badge).
    pub float: bool,
}

pub(crate) fn p(id: &'static str, name: &'static str, default: Value, ui: ParamUi) -> ParamSpec {
    ParamSpec { id, name, default, ui }
}
pub(crate) fn slider(min: f64, max: f64, smin: f64, smax: f64, decimals: u8) -> ParamUi {
    ParamUi::Slider { min, max, slider_min: smin, slider_max: smax, decimals }
}
pub(crate) fn popup(opts: &[&str]) -> ParamUi {
    ParamUi::Popup { options: opts.iter().map(|s| s.to_string()).collect() }
}
pub(crate) fn num(v: f64) -> Value {
    Value::Scalar(v)
}
pub(crate) fn col(r: f64, g: f64, b: f64) -> Value {
    Value::Color([r, g, b, 1.0])
}

/// All effects, sorted by category then name.
pub fn registry() -> &'static [EffectSpec] {
    static R: OnceLock<Vec<EffectSpec>> = OnceLock::new();
    R.get_or_init(|| {
        let mut v = Vec::new();
        v.extend(misc::specs());
        v.extend(color_fx::specs());
        v.extend(generate::specs());
        v.extend(distort::specs());
        v.extend(controls::specs());
        v.extend(keying::specs());
        v.extend(matte::specs());
        v.extend(channel::specs());
        v.extend(blur2::specs());
        v.extend(color2::specs());
        v.extend(stylize2::specs());
        v.extend(noise::specs());
        v.extend(generate2::specs());
        v.extend(distort2::specs());
        v.extend(perspective::specs());
        v.extend(transition::specs());
        v.extend(utility::specs());
        v.extend(sim::specs());
        v.extend(sim2::specs());
        v.extend(distort3::specs());
        v.extend(perspective2::specs());
        v.extend(utility2::specs());
        v.extend(generate3::specs());
        v.extend(textfx::specs());
        v.extend(stylize3::specs());
        v.extend(transition2::specs());
        v.extend(channel2::specs());
        v.extend(keying2::specs());
        v.extend(noise2::specs());
        v.extend(noise3::specs());
        v.extend(color3::specs());
        v.extend(obsolete::specs());
        v.extend(ocio::specs());
        v.extend(vr::specs());
        v.extend(blur3::specs());
        v.extend(distort4::specs());
        v.extend(keylight::specs());
        v.extend(mocha_shape::specs());
        v.extend(sim3::specs());
        v.extend(channel3d::specs());
        v.extend(time_fx::specs());
        v.extend(audio_fx::specs());
        v.extend(paint::specs());
        v.extend(puppet::specs());
        v.extend(warp_stab::specs());
        v.extend(camera_tracker::specs());
        v.extend(face_track::specs());
        v.extend(roto::specs());
        v.sort_by(|a, b| a.category.cmp(b.category).then(a.name.cmp(b.name)));
        for s in v.iter_mut() {
            if GPU_EFFECTS.contains(&s.id) {
                s.gpu = true;
            }
        }
        v
    })
}

/// A built-in effect or a registered plug-in by id.
pub fn find(id: &str) -> Option<&'static EffectSpec> {
    registry().iter().find(|s| s.id == id).or_else(|| plugin::plugins().into_iter().find(|s| s.id == id))
}

/// Every effect: the built-ins ([`registry`]) and the registered plug-ins
/// ([`plugin::register_plugin`]), sorted by category then name.
pub fn all() -> Vec<&'static EffectSpec> {
    let plugins = plugin::plugins();
    let mut v: Vec<&'static EffectSpec> = registry().iter().collect();
    if !plugins.is_empty() {
        v.extend(plugins);
        v.sort_by(|a, b| a.category.cmp(b.category).then(a.name.cmp(b.name)));
    }
    v
}

/// Effects & Presets categories: [`CATEGORIES`] then the plug-ins' own, alphabetically.
pub fn categories() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = CATEGORIES.to_vec();
    let mut extra: Vec<&'static str> = plugin::plugins().iter().map(|s| s.category).filter(|c| !CATEGORIES.contains(c)).collect();
    extra.sort_unstable();
    extra.dedup();
    v.extend(extra);
    v
}

/// After Effects' third-party display names of effects we register under a generic name.
const ALIASES: &[(&str, &[&str])] = &[("ec.keying.keylight", keylight::KEYLIGHT_ALIASES)];

/// Other names effect `id` answers to (in [`lookup`] and searches).
pub fn aliases(id: &str) -> &'static [&'static str] {
    ALIASES.iter().find(|(i, _)| *i == id).map_or(&[], |(_, a)| a)
}

/// Search (Effects & Presets, `effect.list`): the display name or one of its [`aliases`]
/// contains `query` (lowercase).
pub fn name_matches(spec: &EffectSpec, query: &str) -> bool {
    std::iter::once(spec.name).chain(aliases(spec.id).iter().copied()).any(|n| n.to_lowercase().contains(query))
}

/// Find by id or (case-insensitive) display name.
pub fn lookup(name_or_id: &str) -> Option<&'static EffectSpec> {
    find(name_or_id)
        .or_else(|| all().into_iter().find(|s| s.name.eq_ignore_ascii_case(name_or_id)))
        .or_else(|| ALIASES.iter().find(|(_, names)| names.iter().any(|a| a.eq_ignore_ascii_case(name_or_id))).and_then(|(id, _)| find(id)))
        .or_else(|| {
            // Display names of earlier versions.
            migrate::EFFECT_NAME_ALIASES.iter().find(|(old, _)| old.eq_ignore_ascii_case(name_or_id)).and_then(|(_, id)| find(id))
        })
}

/// A parameter's default value on a layer of `layer_size` (point defaults are fractions of the
/// layer size).
pub fn default_value(ps: &ParamSpec, layer_size: [f64; 2]) -> Value {
    match (&ps.ui, &ps.default) {
        (ParamUi::Point, Value::Vec2(f)) => Value::Vec2([f[0] * layer_size[0], f[1] * layer_size[1]]),
        _ => ps.default.clone(),
    }
}

/// Options of a layer parameter's source popup (After Effects shows it next to the layer
/// popup): what of the chosen layer the effect sees.
pub const LAYER_SOURCE_OPTIONS: [&str; 3] = ["Source", "Masks", "Effects & Masks"];

/// Id of the hidden companion parameter holding layer parameter `id`'s source choice (an index
/// into [`LAYER_SOURCE_OPTIONS`]).
pub fn layer_source_id(id: &str) -> String {
    format!("{id}Source")
}

/// Build the property group for a new instance of an effect.
pub fn instantiate(spec: &EffectSpec, ids: &mut Ids, instance_name: &str, layer_size: [f64; 2]) -> PropGroup {
    let mut g = ids.group(spec.id, instance_name);
    g.kind = GroupKind::Effect { effect: spec.id.to_string() };
    for ps in &spec.params {
        let default = default_value(ps, layer_size);
        // `group/sub/param` ids nest the parameter in twirl-down groups (Warp Stabilizer's
        // Stabilization / Borders / Advanced).
        let (path, leaf) = ps.id.rsplit_once('/').map(|(g, l)| (Some(g), l)).unwrap_or((None, ps.id));
        let mut pr = Property::new(ids.alloc(), leaf, ps.name, default).with_ui(ps.ui.clone());
        if matches!(ps.ui, ParamUi::Point) {
            pr.spatial = true;
        }
        if matches!(ps.ui, ParamUi::Checkbox | ParamUi::Popup { .. } | ParamUi::Layer) {
            pr.hold_only = true;
        }
        match path.and_then(|path| nested_group(&mut g, ids, path)) {
            Some(sub) => sub.children.push(pr.into()),
            None => g.children.push(pr.into()),
        }
        // Layer parameters carry a source choice (Source / Masks / Effects & Masks) unless the
        // effect declares its own (with its own default).
        // A grouped layer parameter's companion sits next to it in its group, so it flattens
        // to `group/paramSource` = `layer_source_id("group/param")`.
        let sid = layer_source_id(ps.id);
        if matches!(ps.ui, ParamUi::Layer) && !spec.params.iter().any(|q| q.id == sid) {
            let mut src = Property::new(ids.alloc(), &layer_source_id(leaf), &format!("{} Source", ps.name), Value::Enum(2)).with_ui(ParamUi::Hidden);
            src.hold_only = true;
            match path.and_then(|path| nested_group(&mut g, ids, path)) {
                Some(sub) => sub.children.push(src.into()),
                None => g.children.push(src.into()),
            }
        }
    }
    g
}

/// Display names of twirl-down parameter groups (`group/param` spec ids) by match id, for
/// effects without their own table.
pub const PARAM_GROUPS: &[(&str, &str)] = &[
    // Color Correction: Exposure, Levels (Individual Controls), Change to Color, Colorama,
    // Shadow/Highlight, Lumetri Color.
    ("master", "Master"),
    ("rgb", "RGB"),
    ("red", "Red"),
    ("green", "Green"),
    ("blue", "Blue"),
    ("alpha", "Alpha"),
    ("toleranceGroup", "Tolerance"),
    ("inputPhase", "Input Phase"),
    ("outputCycle", "Output Cycle"),
    ("modify", "Modify"),
    ("pixelSelection", "Pixel Selection"),
    ("masking", "Masking"),
    ("details", "Details"),
    ("reds", "Reds"),
    ("yellows", "Yellows"),
    ("greens", "Greens"),
    ("cyans", "Cyans"),
    ("blues", "Blues"),
    ("magentas", "Magentas"),
    ("whites", "Whites"),
    ("neutrals", "Neutrals"),
    ("blacks", "Blacks"),
    ("moreOptions", "More Options"),
    ("basicCorrection", "Basic Correction"),
    ("whiteBalance", "White Balance"),
    ("tone", "Tone"),
    ("creative", "Creative"),
    ("adjustments", "Adjustments"),
    ("curves", "Curves"),
    ("colorWheels", "Color Wheels"),
    ("vignette", "Vignette"),
    ("rgbCurves", "RGB Curves"),
    ("hueSaturationCurves", "Hue Saturation Curves"),
    ("hslSecondary", "HSL Secondary"),
    ("key", "Key"),
    ("refine", "Refine"),
    ("correction", "Correction"),
    // Camera Lens Blur.
    ("irisProperties", "Iris Properties"),
    ("highlight", "Highlight"),
    ("blurMap", "Blur Map"),
    // Numbers.
    ("format", "Format"),
    ("fillAndStroke", "Fill and Stroke"),
    // Stylize: Cartoon, Roughen Edges.
    ("fill", "Fill"),
    ("edge", "Edge"),
    ("evolutionOptions", "Evolution Options"),
    // Noise & Grain: Fractal / Turbulent Noise, Add / Match Grain, Remove Grain, Noise Alpha.
    ("transform", "Transform"),
    ("subSettings", "Sub Settings"),
    ("tweaking", "Tweaking"),
    ("channelIntensities", "Channel Intensities"),
    ("channelSize", "Channel Size"),
    ("color", "Color"),
    ("application", "Application"),
    ("animation", "Animation"),
    ("noiseReductionSettings", "Noise Reduction Settings"),
    ("fineTuning", "Fine Tuning"),
    ("unsharpMask", "Unsharp Mask"),
    ("noiseOptions", "Noise Options (Animation)"),
    ("previewRegion", "Preview Region"),
    ("sampling", "Sampling"),
    ("temporalFiltering", "Temporal Filtering"),
    // Time: Timewarp.
    ("tuning", "Tuning"),
    ("motionBlur", "Motion Blur"),
    ("smoothing", "Smoothing"),
    ("weighting", "Weighting"),
    ("sourceCrops", "Source Crops"),
    // Simulation.
    ("extras", "Extras"),
    ("effectCamera", "Effect Camera"),
    ("wiggle", "Wiggle"),
    ("light", "Light"),
    ("shading", "Shading"),
    ("producer", "Producer"),
    ("physics", "Physics"),
    ("directionAxis", "Direction Axis"),
    ("gravityVector", "Gravity Vector"),
    ("particle", "Particle"),
    ("hairfallMap", "Hairfall Map"),
    ("hairColor", "Hair Color"),
    ("bottom", "Bottom"),
    ("water", "Water"),
    ("sky", "Sky"),
    ("lighting", "Lighting"),
    ("material", "Material"),
    ("shape", "Shape"),
    ("force1", "Force 1"),
    ("force2", "Force 2"),
    ("bubbles", "Bubbles"),
    ("rendering", "Rendering"),
    ("flowMap", "Flow Map"),
    ("heightMapControls", "Height Map Controls"),
    ("wireframeControls", "Wireframe Controls"),
    ("ground", "Ground"),
    ("geometric", "Geometric Distortion"),
    ("target", "Target Point"),
    ("simulation", "Simulation"),
    ("producer1", "Producer 1"),
    ("producer2", "Producer 2"),
    ("xPosition", "X Position"),
    ("yPosition", "Y Position"),
    ("zPosition", "Z Position"),
    ("xRotation", "X Rotation"),
    ("yRotation", "Y Rotation"),
    ("zRotation", "Z Rotation"),
    ("xScale", "X Scale"),
    ("yScale", "Y Scale"),
    ("cameraPosition", "Camera Position"),
    ("cornerPins", "Corner Pins"),
    ("positionJitter", "Position Jitter"),
    ("rotationJitter", "Rotation Jitter"),
    ("textures", "Textures"),
    ("gradient", "Gradient"),
    ("cannon", "Cannon"),
    ("grid", "Grid"),
    ("layerExploder", "Layer Exploder"),
    ("layerMap", "Layer Map"),
    ("gravity", "Gravity"),
    ("repel", "Repel"),
    ("wall", "Wall"),
    ("persistentPropertyMapper", "Persistent Property Mapper"),
    ("ephemeralPropertyMapper", "Ephemeral Property Mapper"),
    ("particleExploder", "Particle Exploder"),
    ("affects", "Affects"),
    ("options", "Options"),
    // Keying, Matte, Obsolete (Key Light's groups are in `keylight::GROUPS`).
    ("ultraSettings", "Ultra Settings"),
    ("additionalForeground", "Additional Foreground"),
    ("additionalBackground", "Additional Background"),
    ("cleanupForeground", "Cleanup Foreground"),
    ("cleanupBackground", "Cleanup Background"),
    ("cleanupForeground1", "Cleanup Foreground 1"),
    ("cleanupForeground2", "Cleanup Foreground 2"),
    ("cleanupForeground3", "Cleanup Foreground 3"),
    ("cleanupForeground4", "Cleanup Foreground 4"),
    ("cleanupForeground5", "Cleanup Foreground 5"),
    ("cleanupForeground6", "Cleanup Foreground 6"),
    ("cleanupForeground7", "Cleanup Foreground 7"),
    ("cleanupForeground8", "Cleanup Foreground 8"),
    ("cleanupForeground9", "Cleanup Foreground 9"),
    ("cleanupForeground10", "Cleanup Foreground 10"),
    ("cleanupBackground1", "Cleanup Background 1"),
    ("cleanupBackground2", "Cleanup Background 2"),
    ("cleanupBackground3", "Cleanup Background 3"),
    ("cleanupBackground4", "Cleanup Background 4"),
    ("cleanupBackground5", "Cleanup Background 5"),
    ("cleanupBackground6", "Cleanup Background 6"),
    ("cleanupBackground7", "Cleanup Background 7"),
    ("cleanupBackground8", "Cleanup Background 8"),
    ("cleanupBackground9", "Cleanup Background 9"),
    ("cleanupBackground10", "Cleanup Background 10"),
    ("fillAndStroke", "Fill and Stroke"),
    ("pathOptions", "Path Options"),
    ("controlPoints", "Control Points"),
    ("character", "Character"),
    ("orientation", "Orientation"),
    ("paragraph", "Paragraph"),
    ("preview", "Preview"),
    ("jitterSettings", "Jitter Settings"),
    // Generate.
    ("positionsColors", "Positions & Colors"),
    ("feather", "Feather"),
    ("tilingOptions", "Tiling Options"),
    ("polygon", "Polygon"),
    ("waveMotion", "Wave Motion"),
    ("waveStroke", "Stroke"),
    ("imageContour", "Image Contour"),
    ("waveMask", "Mask"),
    ("coreSettings", "Core Settings"),
    ("glowSettings", "Glow Settings"),
    ("expertSettings", "Expert Settings"),
    ("mandelbrot", "Mandelbrot"),
    ("julia", "Julia"),
    ("postInversionOffset", "Post-Inversion Offset"),
    ("fractalColor", "Color"),
    ("highQualitySettings", "High Quality Settings"),
    ("edgeOptions", "Edge Options"),
    ("strokeOptions", "Stroke Options"),
];

/// Display names of nested parameter groups by match id.
fn group_name(m: &str) -> &str {
    warp_stab::GROUPS.iter().chain(roto::GROUPS).chain(keylight::GROUPS).chain(PARAM_GROUPS).find(|(id, _)| *id == m).map(|(_, n)| *n).unwrap_or(m)
}

/// The nested group at `path` (`borders/autoScale`) under `g`, created on first use.
fn nested_group<'a>(g: &'a mut PropGroup, ids: &mut Ids, path: &str) -> Option<&'a mut PropGroup> {
    let (first, rest) = match path.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (path, None),
    };
    if g.sub(first).is_none() {
        g.children.push(ids.group(first, group_name(first)).into());
    }
    let sub = g.sub_mut(first)?;
    match rest {
        Some(r) => nested_group(sub, ids, r),
        None => Some(sub),
    }
}

/// Effects the GPU compositor (`effectcraft-gpu`) implements with the CPU effect's semantics
/// (Mercury GPU Acceleration; the Effects & Presets GPU badge). Expression controls are
/// pass-throughs and count as GPU effects too.
pub const GPU_EFFECTS: &[&str] = &[
    "ec.blur.gaussian",
    "ec.blur.fastbox",
    "ec.blur.directional",
    "ec.stylize.glow",
    "ec.color.levels",
    "ec.color.curves",
    "ec.color.huesaturation",
    "ec.color.tint",
    "ec.generate.fill",
    "ec.generate.gradientramp",
    "ec.noise.fractal",
    "ec.perspective.dropshadow",
    "ec.color.brightnesscontrast",
    "ec.color.exposure",
    "ec.channel.invert",
    "ec.distort.transform",
    "ec.distort.turbulentdisplace",
    "ec.distort.displacementmap",
    "ec.distort.wavewarp",
    "ec.distort.ripple",
    "ec.distort.twirl",
    "ec.distort.bulge",
    "ec.distort.cclens",
    "ec.distort.meshwarp",
    "ec.stylize.mosaic",
    "ec.stylize.findedges",
    "ec.stylize.emboss",
    // Blur, transition and generate family (effectcraft-gpu `fx_generate`).
    "ec.blur.radial",
    "ec.blur.cameralens",
    "ec.blur.ccradialfast",
    "ec.transition.venetian",
    "ec.transition.linearwipe",
    "ec.transition.radialwipe",
    "ec.transition.gradientwipe",
    "ec.generate.cellpattern",
    "ec.generate.checkerboard",
    "ec.generate.grid",
    "ec.generate.fourcolor",
    "ec.noise.noise",
    "ec.noise.addgrain",
    // effectcraft-gpu fx_color
    "ec.color.colorbalance",
    "ec.color.vibrance",
    "ec.color.lumetri",
    "ec.color.blackwhite",
    "ec.color.tritone",
    "ec.color.colorama",
    "ec.color.channelmixer",
    "ec.color.selectivecolor",
    "ec.key.linearcolor",
    "ec.keying.keylight",
    // effectcraft-gpu fx_key (keying, matte, channel)
    "ec.key.colorkey",
    "ec.key.luma",
    "ec.key.colorrange",
    "ec.key.extract",
    "ec.key.differencematte",
    "ec.key.colordifference",
    "ec.key.screen",
    "ec.key.spill",
    "ec.key.advancedspill",
    "ec.key.keycleaner",
    "ec.key.unmult",
    "ec.matte.simplechoker",
    "ec.matte.mattechoker",
    "ec.matte.refinesoft",
    "ec.matte.refinehard",
    "ec.channel.setmatte",
    "ec.channel.setchannels",
    "ec.channel.shiftchannels",
    "ec.channel.removecolormatting",
    "ec.channel.arithmetic",
    "ec.channel.solidcomposite",
    "ec.channel.combiner",
    "ec.channel.blend",
    "ec.channel.calculations",
    "ec.channel.compoundarithmetic",
    // effectcraft-gpu fx_tone (colour correction)
    "ec.color.levelsic",
    "ec.color.gammapedestalgain",
    "ec.color.photofilter",
    "ec.color.changecolor",
    "ec.color.changetocolor",
    "ec.color.leavecolor",
    "ec.color.broadcast",
    "ec.color.colorbalancehls",
    "ec.color.videolimiter",
    "ec.color.psarbitrarymap",
    "ec.color.cctoner",
    "ec.color.cccoloroffset",
    "ec.color.cckernel",
    "ec.color.autolevels",
    "ec.color.autocontrast",
    "ec.color.autocolor",
    "ec.color.equalize",
    "ec.color.shadowhighlight",
    "ec.color.cccolorneutralizer",
    "ec.color.colorstabilizer",
    // effectcraft-gpu fx_stylize (stylize, distort)
    "ec.stylize.posterize",
    "ec.stylize.threshold",
    "ec.stylize.ccthreshold",
    "ec.stylize.ccthresholdrgb",
    "ec.stylize.strobe",
    "ec.stylize.ccvignette",
    "ec.stylize.scatter",
    "ec.stylize.brushstrokes",
    "ec.stylize.roughenedges",
    "ec.stylize.texturize",
    "ec.stylize.motiontile",
    "ec.stylize.cckaleida",
    "ec.stylize.ccrepetile",
    "ec.distort.mirror",
    "ec.distort.offset",
    "ec.distort.polar",
    "ec.distort.spherize",
    "ec.distort.cornerpin",
    "ec.distort.opticscompensation",
    "ec.distort.magnify",
    "ec.distort.ccslant",
    "ec.distort.ccsmear",
    "ec.distort.ccsplit",
    "ec.distort.ccsplit2",
    "ec.distort.cctiler",
    "ec.distort.ccgriddler",
    "ec.distort.liquify",
    "ec.distort.twirllegacy",
    "ec.distort.ccripplepulse",
    "ec.distort.ccpowerpin",
    "ec.distort.ccflomotion",
    // effectcraft-gpu fx_noise (noise, blur, time)
    "ec.noise.turbulent",
    "ec.noise.median",
    "ec.noise.medianlegacy",
    "ec.noise.dustscratches",
    "ec.noise.removegrain",
    "ec.noise.noisealpha",
    "ec.noise.noisehls",
    "ec.noise.noisehlsauto",
    "ec.blur.smart",
    "ec.blur.bilateral",
    "ec.blur.sharpen",
    "ec.blur.unsharp",
    "ec.blur.compound",
    "ec.blur.channel",
    "ec.channel.minimax",
    "ec.time.echo",
    "ec.time.posterizetime",
    // effectcraft-gpu fx_warp (warps, cartoon)
    "ec.distort.warp",
    "ec.distort.bezierwarp",
    "ec.distort.ccbendit",
    "ec.distort.ccpageturn",
    "ec.distort.smear",
    "ec.distort.reshape",
    "ec.stylize.coloremboss",
    "ec.stylize.cartoon",
    // effectcraft-gpu fx_extra (shapes, bevels)
    "ec.generate.circle",
    "ec.generate.ellipse",
    "ec.transition.iriswipe",
    "ec.perspective.bevelalpha",
    "ec.perspective.beveledges",
    "ec.obsolete.gaussianlegacy",
    // GPU effects, part B: 3D Channel.
    "ec.3d.channelextract",
    "ec.3d.cryptomatte",
    "ec.3d.depthmatte",
    "ec.3d.depthoffield",
    "ec.3d.extractor",
    "ec.3d.fog3d",
    "ec.3d.idmatte",
    "ec.3d.identifier",
    // Colour management.
    "ec.utility.applylut",
    "ec.color.ociocdl",
    "ec.color.ociocolorspace",
    "ec.color.ociodisplay",
    "ec.color.ociofile",
    "ec.color.ociolook",
    "ec.utility.colorprofileconverter",
    // Immersive Video.
    "ec.vr.blur",
    "ec.vr.chromaticaberrations",
    "ec.vr.colorgradients",
    "ec.vr.converter",
    "ec.vr.denoise",
    "ec.vr.digitalglitch",
    "ec.vr.fractalnoise",
    "ec.vr.glow",
    "ec.vr.planetosphere",
    "ec.vr.rotatesphere",
    "ec.vr.sharpen",
    "ec.vr.spheretoplane",
    // Simulation render passes.
    "ec.sim.ccrainfall",
    "ec.sim.ccsnowfall",
    "ec.sim.ccstarburst",
    "ec.sim.ccbubbles",
    "ec.sim.ccdrizzle",
    "ec.sim.cchair",
    "ec.sim.ccmrmercury",
    "ec.sim.caustics",
    "ec.sim.waveworld",
    "ec.sim.foam",
    "ec.sim.shatter",
    "ec.sim.carddance",
    "ec.transition.cardwipe",
    // effectcraft-gpu fx_particles (particle render passes)
    "ec.sim.ccparticleworld",
    "ec.sim.ccparticlesystems2",
    "ec.sim.particleplayground",
    "ec.sim.ccballaction",
    "ec.sim.ccpixelpolly",
    "ec.sim.ccscatterize",
    "ec.noise.curlnoise",
    // effectcraft-gpu fx_light (the CC light family)
    "ec.generate.cclightrays",
    "ec.generate.cclightburst",
    "ec.generate.cclightsweep",
    "ec.transition.cclightwipe",
    // effectcraft-gpu fx_transition (transitions, perspective)
    "ec.transition.blockdissolve",
    "ec.transition.ccglasswipe",
    "ec.transition.ccgridwipe",
    "ec.transition.ccimagewipe",
    "ec.transition.ccjaws",
    "ec.transition.cclinesweep",
    "ec.transition.ccradialscalewipe",
    "ec.transition.ccscalewipe",
    "ec.transition.cctwister",
    "ec.transition.ccwarpomatic",
    "ec.perspective.radialshadow",
    "ec.distort.ccbender",
    "ec.distort.ccblobbylize",
    "ec.perspective.cccylinder",
    "ec.perspective.ccsphere",
    "ec.perspective.ccspotlight",
    "ec.perspective.ccenvironment",
    "ec.perspective.3dglasses",
    // effectcraft-gpu fx_text (glyph coverage rasterised on the CPU)
    "ec.text.numbers",
    "ec.text.timecode",
    // effectcraft-gpu fx_time (frames fetched by the host, combined on the GPU)
    "ec.time.timedifference",
    "ec.time.timedisplacement",
    "ec.time.ccforcemotionblur",
    "ec.time.ccwidetime",
    "ec.time.pixelmotionblur",
    "ec.time.timewarp",
    // effectcraft-gpu fx_pixel2 (pixel ports, part C)
    "ec.blur.cccross",
    "ec.blur.ccradial",
    "ec.blur.ccvector",
    "ec.blur.reduceflicker",
    "ec.channel.cccomposite",
    "ec.color.colorlink",
    "ec.utility.cineon",
    "ec.utility.hdrcompander",
    "ec.utility.hdrcompression",
    "ec.utility.growbounds",
    "ec.utility.ccoverbrights",
    "ec.stylize.ccblockload",
    "ec.stylize.ccburnfilm",
    "ec.stylize.ccglass",
    "ec.stylize.cchextile",
    "ec.stylize.ccmrsmoothie",
    "ec.stylize.ccplastic",
    "ec.key.innerouter",
    "ec.key.ccsimplewireremoval",
    "ec.obsolete.basic3d",
    // effectcraft-gpu fx_gen2 (generators, part C: CPU plans rasterised on the GPU)
    "ec.obsolete.lightning",
    "ec.generate.advancedlightning",
    "ec.generate.beam",
    "ec.generate.lensflare",
    "ec.generate.radiowaves",
    "ec.generate.vegas",
    "ec.generate.stroke",
    "ec.generate.scribble",
    "ec.generate.writeon",
    "ec.generate.paintbucket",
    "ec.generate.eyedropperfill",
    "ec.generate.ccgluegun",
    "ec.generate.ccthreads",
    "ec.generate.audiospectrum",
    "ec.generate.audiowaveform",
    "ec.obsolete.basictext",
    "ec.obsolete.pathtext",
];

/// Effects that can read [`EffectHost::comp_scene`] (the comp camera or the first comp light):
/// their output changes when the camera, the lights or the layer's own transform move, so the
/// renderer's layer cache folds the comp scene into the key for these.
pub const READS_COMP_SCENE: &[&str] = &["ec.sim.carddance", "ec.sim.shatter", "ec.transition.cardwipe", "ec.sim.caustics", "ec.sim.ccparticleworld"];

/// Effects whose output depends on [`EffectCtx::time`] directly (not only through animated
/// parameters). The renderer's layer cache folds the layer time into the key for these.
/// The `time_dependence_is_declared` test checks the list against every registered effect.
pub const TIME_DEPENDENT: &[&str] = &[
    "ec.distort.wavewarp",
    "ec.distort.ripple",
    "ec.generate.radiowaves",
    "ec.stylize.scatter",
    "ec.stylize.strobe",
    "ec.noise.noise",
    "ec.noise.addgrain",
    // Remove Grain's temporal filtering reads neighbouring frames.
    "ec.noise.removegrain",
    "ec.noise.noisealpha",
    "ec.noise.noisehlsauto",
    "ec.obsolete.lightning",
    "ec.text.timecode",
    "ec.text.numbers",
    // The audio visualisers read the Audio Layer's samples around the frame time.
    "ec.generate.audiospectrum",
    "ec.generate.audiowaveform",
    // Path Text's jitter changes every frame.
    "ec.obsolete.pathtext",
    // Time effects read neighbouring frames of the layer.
    "ec.time.echo",
    "ec.time.posterizetime",
    "ec.time.timedifference",
    "ec.time.timedisplacement",
    "ec.time.timewarp",
    "ec.time.ccforcemotionblur",
    "ec.time.ccwidetime",
    "ec.time.pixelmotionblur",
    // Refine mattes' Reduce Chatter / motion blur read neighbouring frames.
    "ec.matte.refinesoft",
    "ec.matte.refinehard",
    // Card Wipe's position / rotation jitter moves with time.
    "ec.transition.cardwipe",
    // Transform's motion blur and Radio Waves' birth parameters read the parameters at
    // other times.
    "ec.distort.transform",
    // Temporal Smoothing reads neighbouring frames.
    "ec.color.autolevels",
    "ec.color.autocontrast",
    "ec.color.autocolor",
    "ec.color.shadowhighlight",
    "ec.vr.digitalglitch",
    "ec.blur.camerashakedeblur",
    "ec.distort.rollingshutterrepair",
    "ec.obsolete.mochashape",
    // Each frame gets its own stabilizing warp.
    warp_stab::ID,
    // Render Track Points draws each frame's solved points.
    camera_tracker::ID,
    // Each frame has its own segmentation.
    roto::ID,
];

/// See [`TIME_DEPENDENT`].
pub fn is_time_dependent(id: &str) -> bool {
    // Simulations replay from layer time 0, so every frame differs even when the defaults
    // happen to look static at the test's two sample times.
    id.starts_with("ec.sim.") || TIME_DEPENDENT.contains(&id)
}

/// Run an effect on a buffer.
pub fn apply(spec: &EffectSpec, ctx: &EffectCtx, buf: Buf) -> Buf {
    (spec.render)(ctx, buf)
}

/// Test helper: run effect `id` on `img` (scale 1, layer size = image size) with its defaults
/// overridden by `vals`, at layer time `time`, with an optional environment.
#[cfg(test)]
pub(crate) fn run_fx(id: &str, vals: &[(&str, Value)], img: Image, time: f64, env: EffectEnv) -> Buf {
    let s = find(id).unwrap_or_else(|| panic!("no effect {id}"));
    let size = [img.width as f64, img.height as f64];
    let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), default_value(p, size))).collect() };
    for (k, v) in vals {
        assert!(params.values.contains_key(*k), "{id}: unknown param {k}");
        params.values.insert(k.to_string(), v.clone());
    }
    let ctx = EffectCtx { params: &params, time, layer_size: [img.width as f64, img.height as f64], seed: 1, adjustment: false, env };
    apply(s, &ctx, Buf { img, offset: [0.0, 0.0], scale: 1.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_fx_passthrough() {
        let img = Image::filled(4, 3, [0.5, 0.25, 0.125, 1.0]);
        let out = run_fx("ec.control.slider", &[("slider", num(3.0))], img.clone(), 0.0, EffectEnv::default());
        assert_eq!(out.img.data, img.data);
    }

    #[test]
    #[ignore]
    fn dump_registry() {
        for s in registry() {
            println!("{} | {} | {}", s.category, s.name, s.id);
        }
    }

    #[test]
    fn registry_is_consistent() {
        let r = registry();
        assert!(r.len() >= 40, "{}", r.len());
        let mut ids = std::collections::HashSet::new();
        for s in r {
            assert!(ids.insert(s.id), "duplicate {}", s.id);
            assert!(CATEGORIES.contains(&s.category), "{} in unknown category {}", s.id, s.category);
            let mut pids = std::collections::HashSet::new();
            for p in &s.params {
                assert!(pids.insert(p.id), "{}: duplicate param {}", s.id, p.id);
            }
        }
    }

    /// Registry-wide parameter sanity: unique ids and names, non-empty display names, defaults
    /// of the right kind and inside their ranges, popup defaults inside their options.
    #[test]
    fn every_param_is_well_formed() {
        let mut problems = vec![];
        let mut names = std::collections::HashSet::new();
        for s in registry() {
            if s.name.trim().is_empty() {
                problems.push(format!("{}: empty display name", s.id));
            }
            if !names.insert(s.name) {
                problems.push(format!("{}: duplicate display name {}", s.id, s.name));
            }
            let mut ids = std::collections::HashSet::new();
            for p in &s.params {
                let at = format!("{} / {}", s.id, p.id);
                if !ids.insert(p.id) {
                    problems.push(format!("{at}: duplicate id"));
                }
                if p.id.is_empty() || p.id.contains(char::is_whitespace) {
                    problems.push(format!("{at}: bad id"));
                }
                if p.name.trim().is_empty() {
                    problems.push(format!("{at}: empty display name"));
                }
                match (&p.ui, &p.default) {
                    (ParamUi::Slider { min, max, slider_min, slider_max, .. }, d) => {
                        if !(min <= max && slider_min <= slider_max) {
                            problems.push(format!("{at}: inverted range"));
                        }
                        if !(slider_min >= min && slider_max <= max) {
                            problems.push(format!("{at}: slider range {slider_min}..{slider_max} outside valid range {min}..{max}"));
                        }
                        match d {
                            Value::Scalar(v) if v < min || v > max => problems.push(format!("{at}: default {v} outside {min}..{max}")),
                            Value::Scalar(_) => {}
                            d => problems.push(format!("{at}: slider default {d:?} is not a number")),
                        }
                    }
                    (ParamUi::Popup { options }, d) => {
                        if options.is_empty() || options.iter().any(|o| o.trim().is_empty()) {
                            problems.push(format!("{at}: empty popup option"));
                        }
                        let uniq: std::collections::HashSet<_> = options.iter().collect();
                        if uniq.len() != options.len() {
                            problems.push(format!("{at}: duplicate popup option"));
                        }
                        match d {
                            Value::Enum(i) if (*i as usize) < options.len() => {}
                            d => problems.push(format!("{at}: popup default {d:?} not among {} options", options.len())),
                        }
                    }
                    (ParamUi::Checkbox, d) if !matches!(d, Value::Bool(_)) => problems.push(format!("{at}: checkbox default {d:?}")),
                    (ParamUi::Color, d) if !matches!(d, Value::Color(_)) => problems.push(format!("{at}: color default {d:?}")),
                    (ParamUi::Point, d) if !matches!(d, Value::Vec2(_)) => problems.push(format!("{at}: point default {d:?}")),
                    (ParamUi::Point3, d) if !matches!(d, Value::Vec3(_)) => problems.push(format!("{at}: 3D point default {d:?}")),
                    (ParamUi::Layer, d) if !matches!(d, Value::Layer(_)) => problems.push(format!("{at}: layer default {d:?}")),
                    (ParamUi::Percent | ParamUi::Angle | ParamUi::Pixels | ParamUi::Number, d) if !matches!(d, Value::Scalar(_) | Value::Vec2(_)) => {
                        problems.push(format!("{at}: numeric default {d:?}"))
                    }
                    _ => {}
                }
            }
        }
        assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
    }

    /// Every effect runs on a small image with its default parameters without panicking and
    /// keeps alpha in range.
    #[test]
    fn all_effects_run_with_defaults() {
        let mut img = Image::new(48, 32);
        for y in 0..32 {
            for x in 0..48 {
                let a = if (8..40).contains(&x) && (6..26).contains(&y) { 1.0 } else { 0.0 };
                img.set(x, y, [x as f32 / 48.0 * a, y as f32 / 32.0 * a, 0.5 * a, a]);
            }
        }
        for s in registry() {
            let params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
            let ctx = EffectCtx { params: &params, time: 0.5, layer_size: [48.0, 32.0], seed: 1, adjustment: false, env: Default::default() };
            let out = apply(s, &ctx, Buf { img: img.clone(), offset: [0.0, 0.0], scale: 1.0 });
            assert!(!out.img.is_empty(), "{}", s.id);
            assert!(out.img.data.iter().all(|p| p[3].is_finite() && p[3] >= -1e-4 && p[3] <= 1.0001), "{} alpha out of range", s.id);
        }
    }

    /// Effects that read the clock must be listed in `TIME_DEPENDENT`, or the layer cache would
    /// serve stale pixels. Render every effect at two times with default parameters (plus
    /// "random…" switches turned on) and require identical output from unlisted ones.
    #[test]
    fn time_dependence_is_declared() {
        let mut img = Image::new(40, 30);
        for y in 0..30 {
            for x in 0..40 {
                let a = if (6..34).contains(&x) && (5..25).contains(&y) { 1.0 } else { 0.3 };
                img.set(x, y, [((x * 7 + y * 3) % 11) as f32 / 11.0 * a, y as f32 / 30.0 * a, 0.4 * a, a]);
            }
        }
        let mut missing = vec![];
        for s in registry() {
            let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
            for (k, v) in params.values.iter_mut() {
                if k.to_lowercase().contains("random") && matches!(v, Value::Bool(_)) {
                    *v = Value::Bool(true);
                }
            }
            let run = |t: f64| {
                let ctx = EffectCtx { params: &params, time: t, layer_size: [40.0, 30.0], seed: 7, adjustment: false, env: Default::default() };
                apply(s, &ctx, Buf { img: img.clone(), offset: [0.0, 0.0], scale: 1.0 }).img
            };
            let (a, b) = (run(0.0), run(1.37));
            if a != b && !is_time_dependent(s.id) {
                missing.push(s.id);
            }
        }
        assert!(missing.is_empty(), "these depend on time but are not in TIME_DEPENDENT: {missing:?}");
    }

    /// A host that tags the returned pixels with the requested mode (R = 0 source, 0.5 masks,
    /// 1 effects & masks).
    struct ModeHost;
    impl EffectHost for ModeHost {
        fn layer(&self, _: u64, me: bool) -> Option<LayerPixels> {
            let v = if me { 1.0 } else { 0.0 };
            Some(LayerPixels { buf: Buf { img: Image::filled(1, 1, [v, 0.0, 0.0, 1.0]), offset: [0.0; 2], scale: 1.0 }, size: [1.0, 1.0] })
        }
        fn layer_masks(&self, _: u64) -> Option<LayerPixels> {
            Some(LayerPixels { buf: Buf { img: Image::filled(1, 1, [0.5, 0.0, 0.0, 1.0]), offset: [0.0; 2], scale: 1.0 }, size: [1.0, 1.0] })
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
    }

    #[test]
    fn layer_param_source_companion_selects_the_mode() {
        let host = ModeHost;
        let mode = |companion: Option<u32>, builtin: bool| {
            let mut params = Params::default();
            params.values.insert("l".into(), Value::Layer(Some(3)));
            if let Some(c) = companion {
                params.values.insert(layer_source_id("l"), Value::Enum(c));
            }
            let env = EffectEnv { host: Some(&host), ..Default::default() };
            let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [1.0, 1.0], seed: 0, adjustment: false, env };
            ctx.layer_param("l", builtin).unwrap().buf.img.get(0, 0)[0]
        };
        assert_eq!(mode(None, true), 1.0);
        assert_eq!(mode(None, false), 0.0);
        assert_eq!(mode(Some(0), true), 0.0);
        assert_eq!(mode(Some(1), true), 0.5);
        assert_eq!(mode(Some(2), false), 1.0);
        // Instances get a companion (default Effects & Masks) unless the effect declares one.
        let mut next = 0;
        let g = instantiate(find("ec.channel.blend").unwrap(), &mut Ids(&mut next), "Blend", [10.0, 10.0]);
        assert_eq!(g.get("blendWithLayerSource").map(|p| p.value.clone()), Some(Value::Enum(2)));
        let g = instantiate(find("ec.noise.matchgrain").unwrap(), &mut Ids(&mut next), "Match Grain", [10.0, 10.0]);
        assert_eq!(g.get("noiseSourceLayerSource").map(|p| p.value.clone()), Some(Value::Enum(0)));
        assert_eq!(g.props().filter(|p| p.match_id == "noiseSourceLayerSource").count(), 1);
    }
}
