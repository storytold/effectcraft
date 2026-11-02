//! Which frames of a comp an edit can change: frame caches keep the others.

use super::comp::Comp;
use super::item::{Heard, ItemId};
use super::layer::{Layer, LayerSource};
use super::{Arc, Project, Tick};

/// Precomps nested deeper than this count as changed wherever they show.
const MAX_DEPTH: usize = 32;

impl Project {
    /// The comp-time spans of `comp` where `self` may draw other pixels than `old` (the project
    /// before an edit): [`Comp::changed_spans`] of its layers, and for a precomp whose comp
    /// changed, the spans that comp changed in, where the precomp shows them (#1641). Layers that
    /// can't change the picture ([`Project::silent_layers`]) don't count, nor does the audio of
    /// layers no effect draws it from (`heard`, best of both states). Expressions can read
    /// anything, so any change (`outside` these comps too: another comp, an item's name) reaches
    /// the layers that have one. `None` when the edit can reach any frame.
    pub fn dirty_spans(&self, old: &Project, comp: ItemId, heard: &Heard, outside: bool) -> Option<Vec<(Tick, Tick)>> {
        self.dirty_spans_at(old, comp, heard, outside, 0)
    }

    fn dirty_spans_at(&self, old: &Project, comp: ItemId, heard: &Heard, outside: bool, depth: usize) -> Option<Vec<(Tick, Tick)>> {
        if depth > MAX_DEPTH {
            return None;
        }
        let (new_c, old_c) = (self.comp_arc(comp)?, old.comp_arc(comp)?);
        // Expressions may read what changed elsewhere, even from comps that didn't change.
        let reads_elsewhere = heard.expressions && outside;
        let quiet = self.silent_layers(comp, &new_c, heard);
        // Precomps (drawing the same comp before and after) whose comp changed: where they show it.
        let mut touched = Vec::new();
        let mut nested = Vec::new();
        for l in new_c.layers.iter().filter(|l| !quiet.contains(&l.id)) {
            let LayerSource::Comp { item } = l.source else { continue };
            if old_c.layer(l.id).is_none_or(|before| before.source != l.source) {
                continue;
            }
            let (Some(n), Some(o)) = (self.comp_arc(item), old.comp_arc(item)) else { continue };
            if Arc::ptr_eq(&n, &o) && !reads_elsewhere {
                continue;
            }
            let inner = self.dirty_spans_at(old, item, heard, outside, depth + 1);
            if inner.as_ref().is_some_and(Vec::is_empty) {
                continue;
            }
            touched.push(l.id);
            nested.extend(shown(l, &new_c, &n, inner));
        }
        if Arc::ptr_eq(&new_c, &old_c) && touched.is_empty() && !reads_elsewhere {
            return Some(vec![]);
        }
        let picture = |p: &Project, c: &Comp| {
            let mut c = c.clone();
            let quiet = p.silent_layers(comp, &c, heard);
            c.layers.retain(|l| !quiet.contains(&l.id));
            c
        };
        let (a, b) = (picture(old, &old_c), picture(self, &new_c));
        let elsewhere = heard.expressions && (outside || old_c != new_c || !touched.is_empty());
        let mut spans = b.changed_spans(&a, false, &|l| heard.picked(comp, l), elsewhere, &touched)?;
        spans.extend(nested);
        Some(spans)
    }
}

/// The comp-time spans of `outer` where its precomp layer `l` shows the spans `inner` of its comp
/// `n` (`None`: all of it), widened by `outer`'s shutter. A time remap or Responsive Design
/// timing can show any nested time anywhere: then the layer's whole span.
fn shown(l: &Layer, outer: &Comp, n: &Comp, inner: Option<Vec<(Tick, Tick)>>) -> Vec<(Tick, Tick)> {
    let (before, after) = outer.shutter_margin();
    let whole = vec![(l.in_point - before, l.out_point + after)];
    let responsive = (l.stretch - 100.0).abs() > 1e-9 && n.markers.iter().any(|m| m.protected && m.duration > Tick::ZERO);
    let Some(inner) = inner else { return whole };
    if l.props.get("timeRemap").is_some() || responsive {
        return whole;
    }
    // (a nested frame either side: Preserve Frame Rate holds nested frames, Frame Blending mixes
    // neighbours)
    let fd = n.frame_duration();
    inner
        .into_iter()
        .filter_map(|(s, e)| {
            let (x, y) = (l.comp_time(s - fd), l.comp_time(e + fd));
            let (lo, hi) = (x.min(y).max(l.in_point), x.max(y).min(l.out_point));
            (lo <= hi).then(|| (lo - before, hi + after))
        })
        .collect()
}
