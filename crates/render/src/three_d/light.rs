//! Lights and Classic 3D shading.
//!
//! Shading model (per light, Phong on the layer plane, lit from both sides):
//! `colour = texture · (ambient_mat · Σ ambient + diffuse_mat · Σ diffuse) + specular`, where a
//! light's diffuse term is `colour · intensity · max(N·L, 0) · falloff · cone · shadow` (falloff by
//! the distance from the light's position, for parallel lights too), light from
//! behind the layer passes through by Light Transmission, and the specular highlight
//! `(R·V)^shininess` (R the light direction mirrored about the normal, V towards the viewer;
//! After Effects' highlight is this narrow Phong lobe, not the wider Blinn `(N·H)` one) takes the
//! light's colour mixed towards the layer's colour by Metal.
//! A comp without lights shows 3D layers unlit.

use effectcraft_geom::{Vec3, vec3};
use effectcraft_project::{Layer, LightKind};

use super::camera::layer_frame;
use crate::EvalCtx;

/// An evaluated light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightState {
    pub kind: LightKind,
    /// World position (point/spot; parallel lights use it only for the shadow source).
    pub pos: Vec3,
    /// Unit direction the light travels (parallel/spot).
    pub dir: Vec3,
    /// Linear colour × intensity (1.0 = 100%).
    pub color: [f32; 3],
    /// Spot cone half-angle (radians) and feather (0..1).
    pub cone_half: f64,
    pub feather: f64,
    /// Falloff: 0 None, 1 Smooth, 2 Inverse Square Clamped.
    pub falloff: u32,
    pub radius: f64,
    pub falloff_distance: f64,
    pub casts_shadows: bool,
    /// 0..1.
    pub shadow_darkness: f64,
    /// Pixels.
    pub shadow_diffusion: f64,
}

impl LightState {
    /// Evaluate a light layer.
    pub fn from_layer(ctx: &EvalCtx, layer: &Layer) -> Option<LightState> {
        let effectcraft_project::LayerSource::Light { kind } = layer.source else { return None };
        let g = layer.props.sub("lightOptions")?;
        let intensity = ctx.f(layer, g, "intensity", 100.0) / 100.0;
        let c = ctx.color(layer, g, "color");
        let color = [(c[0] as f64 * intensity) as f32, (c[1] as f64 * intensity) as f32, (c[2] as f64 * intensity) as f32];
        let (pos, fwd, _) = layer_frame(ctx, layer);
        // Spot/parallel lights aim at their point of interest (auto-orient), then rotate.
        let dir = fwd;
        Some(LightState {
            kind,
            pos,
            dir,
            color,
            cone_half: (ctx.f(layer, g, "coneAngle", 90.0).clamp(0.0, 180.0) / 2.0).to_radians(),
            feather: ctx.f(layer, g, "coneFeather", 50.0).clamp(0.0, 100.0) / 100.0,
            falloff: ctx.e(layer, g, "falloff"),
            radius: ctx.f(layer, g, "radius", 500.0).max(0.0),
            falloff_distance: ctx.f(layer, g, "falloffDistance", 500.0).max(0.0),
            casts_shadows: ctx.b(layer, g, "castsShadows"),
            shadow_darkness: ctx.f(layer, g, "shadowDarkness", 100.0).clamp(0.0, 100.0) / 100.0,
            shadow_diffusion: ctx.f(layer, g, "shadowDiffusion", 0.0).max(0.0),
        })
    }

    /// Unit vector from a point towards the light, and the distance (∞ for parallel lights).
    pub fn to_light(&self, p: Vec3) -> (Vec3, f64) {
        match self.kind {
            LightKind::Parallel => (-self.dir, f64::INFINITY),
            _ => {
                let d = self.pos - p;
                let len = d.length();
                if len < 1e-9 { (vec3(0.0, 0.0, -1.0), 0.0) } else { (d / len, len) }
            }
        }
    }

    /// Distance attenuation × spot cone factor at `p` (0..1).
    pub fn attenuation(&self, p: Vec3) -> f64 {
        let (l, dist) = self.to_light(p);
        // A parallel light's rays all share one direction, but its falloff is still measured
        // from the light's position, as for a point light.
        let dist = if self.kind == LightKind::Parallel { (p - self.pos).length() } else { dist };
        let mut k = match self.falloff {
            1 if dist.is_finite() => {
                // Smooth: full within Radius, smooth to zero over Falloff Distance.
                if dist <= self.radius {
                    1.0
                } else if self.falloff_distance <= 0.0 {
                    0.0
                } else {
                    let t = ((dist - self.radius) / self.falloff_distance).clamp(0.0, 1.0);
                    1.0 - t * t * (3.0 - 2.0 * t)
                }
            }
            2 if dist.is_finite() => {
                // Inverse square, clamped to 1 inside Radius.
                if dist <= self.radius || self.radius <= 0.0 && dist <= 1.0 { 1.0 } else { (self.radius.max(1.0) / dist).powi(2) }
            }
            _ => 1.0,
        };
        if self.kind == LightKind::Spot {
            let cos_a = (-l).dot(self.dir).clamp(-1.0, 1.0);
            let a = cos_a.acos();
            let outer = self.cone_half;
            let inner = outer * (1.0 - self.feather);
            k *= if a <= inner {
                1.0
            } else if a >= outer {
                0.0
            } else {
                let t = (a - inner) / (outer - inner).max(1e-9);
                1.0 - t * t * (3.0 - 2.0 * t)
            };
        }
        k
    }
}

/// All lights switched on and active at the context time.
pub fn lights_at(ctx: &EvalCtx) -> Vec<LightState> {
    // Environment lights only light Advanced 3D comps (image-based light).
    ctx.comp
        .layers
        .iter()
        .filter(|l| {
            l.is_light()
                && l.switches.video
                && l.is_active_at(ctx.time)
                && l.source != (effectcraft_project::LayerSource::Light { kind: LightKind::Environment })
        })
        .filter_map(|l| LightState::from_layer(ctx, l))
        .collect()
}

/// Material Options of a 3D layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// 0 Off, 1 On, 2 Only.
    pub casts_shadows: u32,
    /// 0..1.
    pub light_transmission: f32,
    /// 0 Off, 1 On, 2 Only.
    pub accepts_shadows: u32,
    pub accepts_lights: bool,
    pub ambient: f32,
    pub diffuse: f32,
    pub specular: f32,
    /// 0..1.
    pub shininess: f32,
    pub metal: f32,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            casts_shadows: 0,
            light_transmission: 0.0,
            accepts_shadows: 1,
            accepts_lights: true,
            ambient: 1.0,
            diffuse: 0.5,
            specular: 0.5,
            shininess: 0.05,
            metal: 1.0,
        }
    }
}

impl Material {
    pub fn of(ctx: &EvalCtx, layer: &Layer) -> Material {
        let Some(g) = layer.props.sub("materialOptions") else { return Material::default() };
        let d = Material::default();
        let pct = |m: &str, def: f32| (ctx.f(layer, g, m, def as f64 * 100.0) / 100.0) as f32;
        Material {
            casts_shadows: g.get("castsShadows").map(|p| ctx.value(layer, p).as_enum()).unwrap_or(d.casts_shadows),
            light_transmission: pct("lightTransmission", d.light_transmission).clamp(0.0, 1.0),
            accepts_shadows: g.get("acceptsShadows").map(|p| ctx.value(layer, p).as_enum()).unwrap_or(d.accepts_shadows),
            accepts_lights: g.get("acceptsLights").map(|p| ctx.value(layer, p).as_bool()).unwrap_or(true),
            ambient: pct("ambient", d.ambient),
            diffuse: pct("diffuse", d.diffuse),
            specular: pct("specularIntensity", d.specular),
            shininess: pct("specularShininess", d.shininess).clamp(0.0, 1.0),
            metal: pct("metal", d.metal).clamp(0.0, 1.0),
        }
    }
    /// Phong exponent for Specular Shininess (0% → 1, 100% → 256).
    pub fn exponent(&self) -> f64 {
        256f64.powf(self.shininess as f64).max(1.0)
    }
}

/// Shade one straight-alpha texel at world point `p` with unit normal `n`.
///
/// `to_viewer` is the unit direction towards the viewer; `shadow(light_index, p)` returns the
/// RGB light transmittance (1 = unshadowed). Returns the lit straight RGB colour and the summed
/// shadow amount (0..1, used by "Accepts Shadows: Only").
pub fn shade(
    rgb: [f32; 3],
    p: Vec3,
    n: Vec3,
    to_viewer: Vec3,
    mat: &Material,
    lights: &[LightState],
    shadow: &dyn Fn(usize, Vec3) -> [f32; 3],
) -> ([f32; 3], f32) {
    if lights.is_empty() || !mat.accepts_lights {
        return (rgb, 0.0);
    }
    let n = if n.dot(to_viewer) < 0.0 { -n } else { n };
    let mut amb = [0.0f32; 3];
    let mut diff = [0.0f32; 3];
    let mut spec = [0.0f32; 3];
    let mut shadow_amt = 0.0f32;
    let exp = mat.exponent();
    for (i, l) in lights.iter().enumerate() {
        if l.kind == LightKind::Ambient {
            for c in 0..3 {
                amb[c] += l.color[c];
            }
            continue;
        }
        let k = l.attenuation(p);
        if k <= 0.0 {
            continue;
        }
        let (lv, _) = l.to_light(p);
        let ndl = n.dot(lv);
        let front = ndl > 0.0;
        let lambert = if front { ndl } else { -ndl * mat.light_transmission as f64 };
        if lambert <= 0.0 {
            continue;
        }
        let sh = if l.casts_shadows && mat.accepts_shadows != 0 { shadow(i, p) } else { [1.0; 3] };
        shadow_amt = shadow_amt.max(1.0 - (sh[0] + sh[1] + sh[2]) / 3.0);
        let kd = (lambert * k) as f32;
        for c in 0..3 {
            diff[c] += l.color[c] * kd * sh[c];
        }
        if front && mat.specular > 0.0 {
            // Phong: the light mirrored about the normal against the view direction.
            let r = n * (2.0 * ndl) - lv;
            let s = (r.dot(to_viewer).max(0.0).powf(exp) * k) as f32;
            for c in 0..3 {
                // Metal: highlight colour from the light (0%) to the layer colour (100%).
                let tint = 1.0 + (rgb[c] - 1.0) * mat.metal;
                spec[c] += l.color[c] * s * sh[c] * tint;
            }
        }
    }
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        out[c] = rgb[c] * (mat.ambient * amb[c] + mat.diffuse * diff[c]) + mat.specular * spec[c];
    }
    (out, shadow_amt)
}
