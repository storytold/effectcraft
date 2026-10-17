//! Cheap identities for immutable composition snapshots. Weak handles keep allocation identities
//! unique without retaining old documents. `Arc::make_mut` dissociates weak handles even when it
//! can move, rather than clone, a composition: editing a snapshot always changes its identity.
use effectcraft_project::{Comp, ItemKind, Layer};
use effectcraft_render::EvalCtx;
use effectcraft_time::Tick;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, Weak};

const MAX_SNAPSHOTS: usize = 8;
const MAX_COMPS: usize = 4096;
const MAX_METADATA_BYTES: usize = 1 << 20;

struct Snapshot {
    digest: u64,
    id: u64,
    // Do not replace these with raw pointers: a freed allocation could otherwise be reused.
    comps: Vec<Weak<Comp>>,
}

struct Cache {
    next: u64,
    snapshots: Vec<Snapshot>,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache { next: 1, snapshots: Vec::new() });

/// Conservative tail proof: every property in this 2D comp is past all of its keys and every
/// expression belongs to the closed current-comp language. This includes hidden path/slider
/// dependencies. Do not extend the sampling horizon or use this for arbitrary JavaScript.
pub(super) fn hold_start(ctx: &EvalCtx) -> Option<Tick> {
    use effectcraft_project::{Node, PropGroup};
    fn group(g: &PropGroup, l: &Layer, last: &mut Tick, left: &mut usize, depth: usize) -> Option<()> {
        if depth >= 32 {
            return None;
        }
        for n in &g.children {
            *left = left.checked_sub(1)?;
            match n {
                Node::Group(g) => group(g, l, last, left, depth + 1)?,
                Node::Prop(p) => {
                    if p.has_expression() && !p.expr.as_ref().is_some_and(|e| super::rewrite::current_comp_only(&e.text)) {
                        return None;
                    }
                    for k in &p.keys {
                        *left = left.checked_sub(1)?;
                        *last = (*last).max(l.comp_time(k.time));
                    }
                }
            }
        }
        Some(())
    }
    let mut last = Tick::MIN;
    let mut left = 65_536usize;
    for l in &ctx.comp.layers {
        left = left.checked_sub(1)?;
        if l.is_3d() {
            return None;
        }
        group(&l.props, l, &mut last, &mut left, 0)?;
    }
    (last != Tick::MIN && last < ctx.time).then_some(last)
}

struct Fingerprint {
    hash: DefaultHasher,
    remaining: usize,
}

impl std::io::Write for Fingerprint {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.remaining = self.remaining.checked_sub(bytes.len()).ok_or_else(|| std::io::Error::other("motion metadata budget exceeded"))?;
        self.hash.write(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn key(ctx: &EvalCtx, layer: &Layer) -> Option<u64> {
    let ItemKind::Comp(current) = &ctx.project.item(ctx.comp_id)?.kind else { return None };
    // Synthetic/overridden contexts need an explicit identity of their own; never pretend that
    // they are the project's stored snapshot or reuse another layer's entry.
    if !std::ptr::eq(Arc::as_ptr(current), ctx.comp) || !std::ptr::eq(ctx.comp.layer(layer.id)?, layer) {
        return None;
    }
    let mut h = Fingerprint { hash: DefaultHasher::new(), remaining: MAX_METADATA_BYTES };
    serde_json::to_writer(&mut h, &ctx.project.settings).ok()?;
    let mut comps = Vec::new();
    for (id, item) in &ctx.project.items {
        // Names and ordering can affect comp(name), footage(name) and layer source metadata.
        serde_json::to_writer(&mut h, &(id, item.id, &item.name)).ok()?;
        match &item.kind {
            ItemKind::Comp(comp) => {
                if comps.len() >= MAX_COMPS {
                    return None;
                }
                Arc::as_ptr(comp).hash(&mut h.hash);
                comps.push(Arc::downgrade(comp));
            }
            kind => serde_json::to_writer(&mut h, kind).ok()?,
        }
    }
    let digest = h.hash.finish();
    let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let snapshot = if let Some(s) =
        cache.snapshots.iter().find(|s| s.digest == digest && s.comps.len() == comps.len() && s.comps.iter().zip(&comps).all(|(a, b)| Weak::ptr_eq(a, b)))
    {
        s.id
    } else {
        let id = cache.next;
        cache.next = id.checked_add(1)?;
        if cache.snapshots.len() >= MAX_SNAPSHOTS {
            cache.snapshots.remove(0);
        }
        cache.snapshots.push(Snapshot { digest, id, comps });
        id
    };
    let mut h = DefaultHasher::new();
    (snapshot, ctx.comp_id, layer.id).hash(&mut h);
    Some(h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_project::{Project, build};
    use effectcraft_time::{FrameRate, Tick};

    #[test]
    fn weak_snapshot_identity_changes_on_edit_without_retaining_the_comp() {
        let mut p = Project::default();
        let cid = p.add_item(
            "Main",
            effectcraft_color::Label::None,
            None,
            ItemKind::Comp(Arc::new(Comp::new(64, 64, FrameRate::new(30, 1), Tick::from_seconds_f64(10.0)))),
        );
        let comp = p.comp(cid).unwrap().clone();
        let layer = build::layer(&mut p, &comp, "Layer", effectcraft_project::LayerSource::Null, (64, 64), None);
        p.comp_mut(cid).unwrap().layers.push(layer);
        let get = |p: &Project| {
            let c = p.comp(cid).unwrap();
            key(&EvalCtx::new(p, cid, c, Tick::ZERO), &c.layers[0]).unwrap()
        };
        let before = get(&p);
        assert_eq!(get(&p), before);
        let weak = Arc::downgrade(&p.comp_arc(cid).unwrap());
        p.comp_mut(cid).unwrap().layers[0].name = "Edited".into();
        assert_ne!(get(&p), before);
        assert!(weak.upgrade().is_none(), "motion cache must not retain old compositions");
        let copy = p.clone();
        assert_eq!(get(&copy), get(&p), "unchanged worker snapshots can share motion history");
        p.comp_mut(cid).unwrap().layers[0].comment = "x".repeat(2 * MAX_METADATA_BYTES);
        assert!(get(&p) != get(&copy), "the identity changes without serializing the 2 MiB layer");
        let before = get(&p);
        p.item_mut(cid).unwrap().name = "Renamed".into();
        assert_ne!(get(&p), before, "item names are not part of the Arc but affect comp lookups");
        let before = get(&p);
        p.settings.blend_linear = !p.settings.blend_linear;
        assert_ne!(get(&p), before, "project settings participate in expressions");
        // Large external metadata disables caching rather than allocating an unbounded digest.
        p.item_mut(cid).unwrap().name = "x".repeat(MAX_METADATA_BYTES + 1);
        let c = p.comp(cid).unwrap();
        assert!(key(&EvalCtx::new(&p, cid, c, Tick::ZERO), &c.layers[0]).is_none());
    }
}
