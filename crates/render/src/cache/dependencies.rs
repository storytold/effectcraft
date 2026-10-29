//! Content selected by layer parameters participates in the owner's cache key. Keep the
//! ordinary layer cache (including static self reads), and conservatively decline only
//! cyclic/over-budget graphs or expressions sampled at unknown times.
use super::{KeyHasher, hash_debug, hash_values};
use crate::EvalCtx;
use effectcraft_project::{ItemId, ItemKind, Layer, LayerId, LayerSource, LightKind, Node, PropGroup};
use std::hash::Hash;

pub(super) fn hash(h: &mut KeyHasher, ctx: &EvalCtx, layer: &Layer, effects: Option<usize>) -> Option<()> {
    let mut deps = Dependencies { h, path: vec![(ctx.comp_id, layer.id)], remaining: 1024, other_times: super::reads_other_times(layer, None) };
    deps.effects(ctx, layer, effects)
}

struct Dependencies<'a> {
    h: &'a mut KeyHasher,
    path: Vec<(ItemId, LayerId)>,
    remaining: usize,
    other_times: bool,
}

impl Dependencies<'_> {
    fn effects(&mut self, ctx: &EvalCtx, layer: &Layer, limit: Option<usize>) -> Option<()> {
        let previous = self.other_times;
        self.other_times |= super::reads_other_times(layer, None);
        if layer.switches.effects
            && let Some(fx) = layer.effects()
        {
            for g in fx.groups().take(limit.unwrap_or(usize::MAX)).filter(|g| g.enabled) {
                self.group(ctx, layer, g, 0)?;
            }
        }
        self.other_times = previous;
        Some(())
    }

    fn group(&mut self, ctx: &EvalCtx, layer: &Layer, g: &PropGroup, depth: usize) -> Option<()> {
        if depth >= 32 {
            return None;
        }
        for node in &g.children {
            match node {
                Node::Prop(p) => {
                    if let Some(id) = ctx.value(layer, p).as_layer() {
                        p.uid.hash(self.h);
                        id.hash(self.h);
                        let other = ctx.layer(LayerId(id));
                        other.is_some().hash(self.h);
                        if let Some(other) = other {
                            if other.id == layer.id {
                                // Source/Masks never enter the same stack again. The owner
                                // key already covers its source and mask properties.
                                hash_debug(self.h, &ctx.world_matrix(other));
                            } else {
                                self.layer(ctx, other)?;
                            }
                        }
                    }
                }
                Node::Group(sub) if sub.enabled => self.group(ctx, layer, sub, depth + 1)?,
                _ => {}
            }
        }
        Some(())
    }

    /// What a referenced 3D model or primitive layer's solo render depends on: the layer itself
    /// (geometry, materials, switches, in/out), its animated values and transform at the time,
    /// the mesh item, the comp's camera (with its depth of field) and its lights at the time
    /// (the lights are off in draft 3D, and the renderer setting changes the pixels), the model's
    /// layer time while it plays an animation clip, and the in/out gate (outside the range the
    /// read is transparent). An Environment light's image can be any layer's render: with one on
    /// the read is not cached. Comp size and render scale are in the owner's key already.
    fn soloed_model(&mut self, ctx: &EvalCtx, layer: &Layer) -> Option<()> {
        self.remaining = self.remaining.checked_sub(1)?;
        // Expressions sampled at other times: not cacheable, as in the ordinary path.
        if (self.other_times || super::reads_other_times(layer, None)) && super::has_expression(&layer.props) {
            return None;
        }
        hash_debug(self.h, layer);
        hash_values(self.h, ctx, layer, &layer.props);
        hash_debug(self.h, &ctx.world_matrix(layer));
        if let Some(item) = layer.source.item() {
            hash_debug(self.h, &ctx.project.item(item));
        }
        // The solo render goes through the comp's camera and lights. The camera's depth of
        // field blurs it (focus, aperture, iris, highlights) — none of that is in the
        // projection hashed above.
        hash_debug(self.h, &ctx.camera());
        hash_debug(self.h, &crate::three_d::active_camera(ctx).dof);
        hash_debug(self.h, &crate::three_d::light::lights_at(ctx));
        hash_debug(self.h, &ctx.comp.draft_3d);
        hash_debug(self.h, &ctx.comp.renderer);
        // An Environment light's image lights Advanced 3D models (its source can be any
        // layer's render, or any environment layer) — too open-ended to hash: while one is
        // on (and lighting: Draft 3D lights nothing), don't cache the read.
        if ctx.comp.renderer == effectcraft_project::Renderer::Advanced3D
            && !ctx.comp.draft_3d
            && ctx.comp.layers.iter().any(|l| {
                l.is_light() && l.switches.video && l.is_active_at(ctx.time) && matches!(l.source, LayerSource::Light { kind: LightKind::Environment })
            })
        {
            return None;
        }
        // A model with an animation clip selected is posed at its layer time (Animation Speed
        // and Looping are static, hashed above): every time is its own entry. Without one the
        // pose is the same at every time: keep the static key.
        if layer.props.sub("geometryOptions").map_or(0, |g| ctx.e(layer, g, "animation")) != 0 {
            layer.layer_time(ctx.time).0.hash(self.h);
        }
        // Outside the layer's in/out range the read is transparent: fold the gate in, so a
        // static model renders once inside it and again when it leaves.
        layer.is_active_at(ctx.time).hash(self.h);
        Some(())
    }

    fn layer(&mut self, ctx: &EvalCtx, layer: &Layer) -> Option<()> {
        let id = (ctx.comp_id, layer.id);
        if self.path.len() > crate::MAX_FX_DEPTH || self.path.contains(&id) {
            return None;
        }
        // A referenced 3D model or primitive layer renders through the comp's camera and lights
        // (an effect layer parameter reads it that way — Create 3D Layer Instance): hash what
        // moves its pixels, so the owner keeps a cache entry that changes when any of it does.
        if layer.source.is_model() {
            self.soloed_model(ctx, layer)?;
            return Some(());
        }
        self.remaining = self.remaining.checked_sub(1)?;
        if (self.other_times || super::reads_other_times(layer, None)) && super::has_expression(&layer.props) {
            return None;
        }
        self.path.push(id);
        // Include transforms/parents as well as pixels: selected-layer anchors and
        // continuously rasterized sources can depend on them.
        hash_debug(self.h, layer);
        // A referenced layer's static values can be overridden by the collapsed instance without
        // editing it (`hash_values` only sees animated ones), so the owner's key needs them too.
        super::hash_overrides(self.h, ctx, layer.id);
        hash_values(self.h, ctx, layer, &layer.props);
        hash_debug(self.h, &ctx.world_matrix(layer));
        // A cached owner never runs this layer's effects, so the comp camera and lights its
        // scene-reading effects see belong in the owner's key too (#1252).
        if super::reads_comp_scene(layer) {
            hash_debug(self.h, &crate::comp_scene(ctx, layer));
        }
        if super::time_dependent(layer, true) {
            layer.layer_time(ctx.time).0.hash(self.h);
        }
        match layer.source {
            LayerSource::Solid { item } | LayerSource::Footage { item } | LayerSource::Comp { item } => {
                let item = match layer.source {
                    LayerSource::Footage { .. } => ctx.media_item(layer, item),
                    _ => item,
                };
                let source = ctx.project.item(item);
                hash_debug(self.h, &source);
                if let Some(source) = source {
                    match &source.kind {
                        ItemKind::Footage(_) => ctx.source_time(layer).0.hash(self.h),
                        ItemKind::Comp(comp) => {
                            let time = ctx.source_time(layer);
                            time.0.hash(self.h);
                            let nested = EvalCtx::new(ctx.project, item, comp, time);
                            for child in &comp.layers {
                                self.layer(&nested, child)?;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        self.effects(ctx, layer, None)?;
        self.path.pop();
        Some(())
    }
}
