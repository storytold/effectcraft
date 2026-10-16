//! Content selected by layer parameters participates in the owner's cache key. Keep the
//! ordinary layer cache (including static self reads), and conservatively decline only
//! cyclic/over-budget graphs or expressions sampled at unknown times.
use super::{KeyHasher, hash_debug, hash_values};
use crate::EvalCtx;
use effectcraft_project::{ItemId, ItemKind, Layer, LayerId, LayerSource, Node, PropGroup};
use std::hash::Hash;

pub(super) fn hash(h: &mut KeyHasher, ctx: &EvalCtx, layer: &Layer, effects: Option<usize>) -> Option<()> {
    let mut deps = Dependencies { h, path: vec![(ctx.comp_id, layer.id)], remaining: 1024, other_times: super::reads_other_times(layer) };
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
        self.other_times |= super::reads_other_times(layer);
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

    fn layer(&mut self, ctx: &EvalCtx, layer: &Layer) -> Option<()> {
        let id = (ctx.comp_id, layer.id);
        if self.path.len() > crate::MAX_FX_DEPTH || self.path.contains(&id) {
            return None;
        }
        self.remaining = self.remaining.checked_sub(1)?;
        if (self.other_times || super::reads_other_times(layer)) && super::has_expression(&layer.props) {
            return None;
        }
        self.path.push(id);
        // Include transforms/parents as well as pixels: selected-layer anchors and
        // continuously rasterized sources can depend on them.
        hash_debug(self.h, layer);
        hash_values(self.h, ctx, layer, &layer.props);
        hash_debug(self.h, &ctx.world_matrix(layer));
        if super::time_dependent(layer, true) {
            layer.layer_time(ctx.time).0.hash(self.h);
        }
        match layer.source {
            LayerSource::Solid { item } | LayerSource::Footage { item } | LayerSource::Comp { item } => {
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
