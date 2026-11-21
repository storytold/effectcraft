use super::frames::frame_blend_mode;
use super::styles;
use super::{
    Arc, Buf, EffectHost, EvalCtx, FrameBlend, Image, ItemKind, Layer, LayerPixels, LayerSource, Params, Quality, Renderer, Tick, frame_size, three_d,
};

/// Layer parameters and audio for effects (see [`effectcraft_effects::EffectHost`]). What it
/// hands out is in effect space (see [`effect_bounds`]): the other layer's for its pixels, the
/// running layer's for its own frames and the comp scene.
pub(crate) struct FxHost<'r, 'a, 'c> {
    pub(crate) r: &'r Renderer<'a>,
    pub(crate) ctx: &'c EvalCtx<'a>,
    pub(crate) layer: &'c Layer,
    /// Where the stack's effect space starts in layer coordinates (zero on adjustment layers,
    /// whose effects run on the comp below).
    pub(crate) origin: [f64; 2],
    /// Index of the effect being rendered (bounds `self_at` to effects before it).
    pub(crate) index: usize,
    pub(crate) original: Option<&'c Buf>,
    /// Set when a [`self_at`](EffectHost::self_at) call had to decline: the composite below could
    /// not be reproduced, so a time effect must pass its input through instead of reading a frame.
    /// Shared with the effect instances this host serves (atomically, as effects may run on any
    /// worker); owned by the render that created it.
    pub(crate) unsafe_sample: &'c std::sync::atomic::AtomicBool,
}

impl FxHost<'_, '_, '_> {
    /// Render the composite below an adjustment layer at comp time `t` (the whole frame), as After
    /// Effects feeds a time effect there: an adjustment layer's effects run on the comp below, so
    /// Posterize Time (and Echo and the rest) hold *that* composite.
    ///
    /// `None` when this cannot be answered safely — a region of interest, a nested render past the
    /// effect-depth limit, an enclosing collapsed precomp (the adjustment runs on the parent's
    /// canvas, not the layers below here), an effect earlier in the same stack (the composite below
    /// is not what reaches the time effect then), or a layer below that needs more than its own
    /// content (a mask, an effect, a layer style, 3D, a track matte, a wireframe outline). The
    /// caller then passes its input through, which is what happened before.
    fn below_at(&self, t: f64) -> Option<Buf> {
        let decline = || {
            self.unsafe_sample.store(true, std::sync::atomic::Ordering::Relaxed);
            None
        };
        if self.r.opts.roi.is_some() || self.r.depth > MAX_FX_DEPTH {
            return decline();
        }
        // Inside a collapsed precomp the adjustment runs on the parent's canvas: the layers below
        // here are not the composite that reaches it, so keep the previous pass-through.
        if self.r.outer.is_some() {
            return decline();
        }
        // A time effect holds what reaches it: on an ordinary layer that is its own source with the
        // effects before it. Here only the composite below is available, so with an effect earlier in
        // the same stack the input would differ — leave those stacks as they were.
        let earlier = self.layer.effects().is_some_and(|fx| fx.groups().take(self.index).any(|g| g.enabled));
        if earlier {
            return decline();
        }
        // Colour-managed projects blend in linear light; reproducing those conversions here would
        // need the compositor's blending pipeline, so they keep the previous pass-through.
        if self.r.pipe.to_blend().is_some() {
            return decline();
        }
        let ctx = self.ctx.at(Tick::from_seconds_f64(t));
        let (w, h) = frame_size(ctx.comp.width, ctx.comp.height, self.r.opts.scale);
        // Only layers whose processed buffer is exactly their own content are composited here; the
        // rest would need the full compositor (masks, effects, styles, 3D, mattes, another
        // adjustment, a collapsed precomp or a wireframe outline).
        let any_solo = ctx.comp.any_solo();
        let mut plain = Vec::new();
        for other in ctx.comp.layers.iter().rev() {
            if other.id == self.layer.id {
                break;
            }
            // The same visibility rules the compositor uses ([`Renderer::visible_layers`]).
            if !other.has_video()
                || !other.is_active_at(ctx.time)
                || other.environment
                || (!self.r.opts.guides && other.switches.guide)
                || (any_solo && !other.switches.solo)
            {
                continue;
            }
            if other.masks().is_some_and(|m| m.groups().next().is_some())
                || styles::active(&ctx, other)
                || other.is_3d()
                || other.track_matte.is_some()
                || other.preserve_transparency
                || other.switches.adjustment
                || other.switches.collapse
                || self.r.quality(other) == Quality::Wireframe
                || (other.switches.effects && other.effects().is_some_and(|fx| fx.groups().any(|g| g.enabled)))
            {
                return decline();
            }
            plain.push(other);
        }
        let sub = self.r.nested();
        let mut img = Image::new(w, h);
        for other in plain {
            let Some(buf) = sub.content_buf(&ctx, other) else {
                return decline();
            };
            let buf = (*buf).clone();
            let opacity = ctx.opacity(other) as f32 * self.r.opacity_mul;
            self.r.place(&ctx, other, &buf, &mut img, other.blend_mode, opacity);
            self.r.pipe.quantize(&mut img);
        }
        Some(Buf { img, offset: [0.0; 2], scale: self.r.opts.scale })
    }

    /// Another layer's pixels for a layer parameter, in its effect space; `layer_space`: `buf`
    /// is still in layer space (its source or its content).
    fn pixels(&self, other: &Layer, mut buf: Buf, layer_space: bool) -> LayerPixels {
        let (size, origin) = self.ctx.effect_bounds(other);
        if layer_space {
            buf.rebase(origin);
        }
        LayerPixels { buf, size }
    }
}

/// The comp's camera and first light relative to `layer`, in its layer pixels (see
/// [`EffectHost::comp_scene`]): the comp camera's view of the layer (as if 3D at its transform),
/// brought back into the layer's own pixel grid through the inverse of how the layer itself
/// composites.
pub(crate) fn comp_scene(ctx: &EvalCtx, layer: &Layer) -> Option<effectcraft_effects::CompScene> {
    let world = ctx.world_matrix(layer);
    let (cam, _, _) = ctx.camera();
    let p = (cam * world).0;
    let back = ctx.layer_to_comp(layer).0.inverse()?.0;
    let rows = [p[0], p[1], p[3]];
    let camera: [[f64; 4]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| back[i][k] * rows[k][j]).sum()));
    let inv = world.inverse();
    let light = three_d::light::lights_at(ctx).first().and_then(|l| {
        let inv = inv.as_ref()?;
        let pos = inv.apply(l.pos);
        let dir = inv.apply_vec(l.dir);
        let len = (dir.x * dir.x + dir.y * dir.y + dir.z * dir.z).sqrt().max(1e-12);
        let kind = match l.kind {
            effectcraft_project::LightKind::Parallel => 0,
            effectcraft_project::LightKind::Ambient => 2,
            _ => 1,
        };
        Some(effectcraft_effects::CompLight { pos: [pos.x, pos.y, pos.z], dir: [dir.x / len, dir.y / len, dir.z / len], color: l.color, kind })
    });
    let camera_layer = ctx.comp.active_camera(ctx.time).is_some();
    Some(effectcraft_effects::CompScene { camera: Some(camera), camera_layer, light })
}

/// Nesting limit for effects that render layers (other layers, other times).
pub(crate) const MAX_FX_DEPTH: usize = 8;

impl EffectHost for FxHost<'_, '_, '_> {
    fn original(&self) -> Option<&Buf> {
        self.original
    }

    fn particles(&self) -> Option<&dyn effectcraft_effects::psim::ParticleSim> {
        self.r.active_accel().and_then(|a| a.particles())
    }
    fn layer(&self, id: u64, masks_and_effects: bool) -> Option<LayerPixels> {
        let other = self.ctx.layer(effectcraft_project::LayerId(id))?;
        if (other.id == self.layer.id && masks_and_effects) || self.r.depth > MAX_FX_DEPTH {
            return None;
        }
        let sub = self.r.nested();
        // A 3D model or primitive layer has no 2D source of its own: an effect referencing one
        // reads it as After Effects does — the layer rendered on its own through the comp's
        // camera (Create 3D Layer Instance's effect), in the layer's effect space (the
        // comp, centred on the layer's anchor).
        if other.source.is_model() {
            let buf = sub.solo_3d(self.ctx, other);
            return Some(self.pixels(other, buf, false));
        }
        // The cached layer buffer is shared; effects get their own copy.
        let buf = if masks_and_effects { (*sub.content_buf(self.ctx, other)?).clone() } else { sub.source(self.ctx, other)? };
        Some(self.pixels(other, buf, true))
    }

    fn layer_masks(&self, id: u64) -> Option<LayerPixels> {
        let other = self.ctx.layer(effectcraft_project::LayerId(id))?;
        if self.r.depth > MAX_FX_DEPTH {
            return None;
        }
        let sub = self.r.nested();
        let buf = (*sub.layer_input(self.ctx, other, 0)?).clone();
        Some(self.pixels(other, buf, false))
    }

    /// The audio of footage layer `id` from `start`, or `None` when the layer is not footage.
    fn audio(&self, id: u64, start: f64, frames: usize, rate: u32) -> Option<Vec<f32>> {
        let other = self.ctx.layer(effectcraft_project::LayerId(id))?;
        let LayerSource::Footage { item } = &other.source else { return None };
        let item = self.ctx.media_item(other, *item);
        let ItemKind::Footage(f) = &self.r.project.item(item)?.kind else { return None };
        if !f.has_audio {
            return None;
        }
        let st = other.layer_time(Tick::from_seconds_f64(start));
        self.r.read(|| self.r.footage.audio(item, f, st, frames, rate))
    }

    fn self_at(&self, layer_time: f64, effects: usize) -> Option<Buf> {
        if self.r.depth > MAX_FX_DEPTH || !layer_time.is_finite() {
            self.unsafe_sample.store(true, std::sync::atomic::Ordering::Relaxed);
            return None;
        }
        if self.layer.switches.adjustment {
            // An adjustment layer's stack runs on the comp below, so a time effect there holds that
            // composite. `layer_time` is the adjustment's own layer time; the composite below plays
            // at the matching comp time.
            let t = self.layer.comp_time(Tick::from_seconds_f64(layer_time));
            return self.below_at(t.seconds());
        }
        let n = effects.min(self.index);
        let t = self.layer.comp_time(Tick::from_seconds_f64(layer_time));
        let sub = self.r.nested();
        sub.layer_input(&self.ctx.at(t), self.layer, n).map(|b| (*b).clone())
    }

    fn unsafe_time_sample(&self) -> bool {
        self.unsafe_sample.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn aux(&self) -> Option<Arc<effectcraft_raster::AuxChannels>> {
        self.r.layer_aux(self.ctx, self.layer)
    }

    fn params_at(&self, layer_time: f64) -> Option<Params> {
        let g = self.layer.effects()?.groups().nth(self.index)?;
        let ctx = self.ctx.at(self.layer.comp_time(Tick::from_seconds_f64(layer_time)));
        Some(effectcraft_effects::flatten_params(g, &mut |pr| ctx.value(self.layer, pr)))
    }

    fn param_at(&self, effect_index: usize, id: &str, layer_time: f64) -> Option<effectcraft_keyframe::Value> {
        // Only the one property: a simulation samples it at every step. Its keyframes are read
        // at the requested layer time exactly (mapping it to comp time and back rounds under a
        // fractional stretch, so a Hold key there could read its previous value).
        let p = effect_param(self.layer, effect_index, id)?;
        let lt = Tick::from_seconds_f64(layer_time);
        Some(self.ctx.at(self.layer.comp_time(lt)).value_at_layer_time(self.layer, p, lt))
    }

    fn param_is_static(&self, effect_index: usize, id: &str) -> bool {
        effect_param(self.layer, effect_index, id).is_some_and(|p| p.keys.is_empty() && !p.has_expression())
    }

    fn comp_scene(&self) -> Option<effectcraft_effects::CompScene> {
        Some(comp_scene(self.ctx, self.layer)?.rebased(self.origin))
    }

    fn layer_at(&self, id: u64, comp_time: f64, masks_and_effects: bool) -> Option<LayerPixels> {
        let other = self.ctx.layer(effectcraft_project::LayerId(id))?;
        if (other.id == self.layer.id && masks_and_effects) || !comp_time.is_finite() || self.r.depth > MAX_FX_DEPTH {
            return None;
        }
        let ctx = self.ctx.at(Tick::from_seconds_f64(comp_time));
        let sub = self.r.nested();
        Some(if masks_and_effects {
            self.pixels(other, (*sub.content_buf(&ctx, other)?).clone(), true)
        } else if other.id == self.layer.id {
            self.pixels(other, sub.source(&ctx, other)?, true)
        } else {
            // Preserve other-layer reads: time effects historically see source + masks.
            self.pixels(other, (*sub.layer_input(&ctx, other, 0)?).clone(), false)
        })
    }
}

/// Parameter `id` (a flattened id: match ids joined by `/`) of effect `effect_index` in
/// `layer`'s stack.
fn effect_param<'a>(layer: &'a Layer, effect_index: usize, id: &str) -> Option<&'a effectcraft_project::Property> {
    let mut g = layer.effects()?.groups().nth(effect_index)?;
    let mut path = id.split('/').peekable();
    while let Some(m) = path.next() {
        if path.peek().is_none() {
            return g.children.iter().find_map(|c| match c {
                effectcraft_project::Node::Prop(p) if p.match_id == m => Some(p),
                _ => None,
            });
        }
        g = g.children.iter().find_map(|c| match c {
            effectcraft_project::Node::Group(sg) if sg.match_id == m => Some(sg),
            _ => None,
        })?;
    }
    None
}

/// Whether an instance's keys are in the nested comp's time, so they can go into the comp as they
/// are: not with Time Remap, Responsive Design - Time (a stretched precomp with protected
/// regions), a nested comp that preserves its frame rate (its time is floored to its own frames)
/// or a layer with Frame Blending (it renders the nested comp at two other times and mixes them,
/// so keys would pull the next frame's value in early).
fn keys_in_nested_time(ctx: &EvalCtx, layer: &Layer) -> bool {
    if layer.props.get("timeRemap").is_some() || frame_blend_mode(ctx, layer) != FrameBlend::Off {
        return false;
    }
    let LayerSource::Comp { item } = layer.source else { return true };
    let Some(nc) = ctx.project.comp(item) else { return true };
    let responsive = (layer.stretch - 100.0).abs() > 1e-9 && !super::eval::protected_regions(nc).is_empty();
    !responsive && !nc.preserve_frame_rate
}

/// The Essential Properties overrides of a precomp layer, evaluated at the context time.
pub fn essential_overrides(ctx: &EvalCtx, layer: &Layer) -> Vec<effectcraft_project::essential::Override> {
    use effectcraft_project::essential;
    let over = essential::overridden(layer);
    let Some(g) = essential::group(layer).filter(|_| !over.is_empty()) else { return vec![] };
    let same_time = keys_in_nested_time(ctx, layer);
    let mut out = vec![];
    g.walk("", &mut |_, p| {
        if over.contains(&p.uid)
            && let Some(control) = essential::control_of(&p.match_id)
        {
            // An expression on the instance property replaces its keys, and so does an override
            // from a collapsed parent (only its value reaches here, the keys are the stale own ones).
            let from_parent = ctx.property_overrides.iter().any(|o| o.layer == layer.id && o.prop == p.uid);
            let keys = if same_time && !from_parent && !p.has_expression() { p.keys.clone() } else { vec![] };
            out.push(essential::Override { control, value: ctx.value(layer, p), keys });
        }
    });
    out
}
