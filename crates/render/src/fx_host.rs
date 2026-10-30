use super::{Arc, Buf, EffectHost, EvalCtx, ItemKind, Layer, LayerPixels, LayerSource, Params, Renderer, Tick, three_d};

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
}

impl FxHost<'_, '_, '_> {
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

    fn audio(&self, id: u64, start: f64, frames: usize, rate: u32) -> Option<Vec<f32>> {
        let other = self.ctx.layer(effectcraft_project::LayerId(id))?;
        let LayerSource::Footage { item } = &other.source else { return None };
        let item = self.ctx.media_item(other, *item);
        let ItemKind::Footage(f) = &self.r.project.item(item)?.kind else { return None };
        if !f.has_audio {
            return None;
        }
        let st = other.layer_time(Tick::from_seconds_f64(start));
        self.r.footage.audio(item, f, st, frames, rate)
    }

    fn self_at(&self, layer_time: f64, effects: usize) -> Option<Buf> {
        if self.r.depth > MAX_FX_DEPTH || self.layer.switches.adjustment {
            return None;
        }
        let n = effects.min(self.index);
        let t = self.layer.comp_time(Tick::from_seconds_f64(layer_time));
        let sub = self.r.nested();
        sub.layer_input(&self.ctx.at(t), self.layer, n).map(|b| (*b).clone())
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

/// The Essential Properties overrides of a precomp layer, evaluated at the context time.
pub fn essential_overrides(ctx: &EvalCtx, layer: &Layer) -> Vec<effectcraft_project::essential::Override> {
    use effectcraft_project::essential;
    let over = essential::overridden(layer);
    let Some(g) = essential::group(layer).filter(|_| !over.is_empty()) else { return vec![] };
    let mut out = vec![];
    g.walk("", &mut |_, p| {
        if over.contains(&p.uid)
            && let Some(control) = essential::control_of(&p.match_id)
        {
            out.push(essential::Override { control, value: ctx.value(layer, p) });
        }
    });
    out
}
