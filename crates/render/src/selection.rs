//! Geometric bounds for viewer controls, separate from source/render allocation bounds.

use effectcraft_keyframe::Value;
use effectcraft_path::{FillRule, Join, boolean::try_boolean_n};
use effectcraft_project::{GroupKind, Layer, MaskMode};
use kurbo::{BezPath, PathEl, Rect, Shape};
use std::cell::RefCell;
use std::collections::VecDeque;

use crate::EvalCtx;

// Cache evaluated geometry, rather than project IDs or revisions: this also works across
// separate evaluation contexts and cannot retain stale expression/time-dependent results.
// Exact equality avoids hash collisions; entries and total retained path elements are bounded.
const MAX_ELEMENTS: usize = 4096;
const CACHE_ELEMENTS: usize = 16_384;
const CACHE_ENTRIES: usize = 64;

#[derive(PartialEq)]
struct MaskInput {
    path: BezPath,
    mode: MaskMode,
    inverted: bool,
    opacity: f64,
    expansion: f64,
}

#[derive(PartialEq)]
struct BoundsInput {
    source: [f64; 4],
    masks: Vec<MaskInput>,
}

struct CachedBounds {
    input: BoundsInput,
    result: Option<[f64; 4]>,
    elements: usize,
}

thread_local! {
    static BOUNDS_CACHE: RefCell<VecDeque<CachedBounds>> = const { RefCell::new(VecDeque::new()) };
    #[cfg(test)]
    static ARRANGEMENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn cached_bounds(input: BoundsInput, elements: usize) -> Option<[f64; 4]> {
    if let Some(hit) = BOUNDS_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let index = cache.iter().position(|entry| entry.input == input)?;
        let hit = cache.remove(index)?;
        let result = hit.result;
        cache.push_front(hit);
        Some(result)
    }) {
        return hit;
    }
    let result = compute_bounds(&input);
    BOUNDS_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        while cache.len() >= CACHE_ENTRIES || cache.iter().map(|e| e.elements).sum::<usize>().saturating_add(elements) > CACHE_ELEMENTS {
            if cache.pop_back().is_none() {
                break;
            }
        }
        cache.push_front(CachedBounds { input, result, elements });
    });
    result
}

/// Layer-space selection bounds after closed, enabled masks at the current time. The box
/// follows mask geometry (including expansion), not feather tails, effects or source alpha.
/// Keeping this separate from `content_bounds` prevents masks from changing raster allocation
/// limits or effect coordinates. Fully masked layers have no selection box.
pub fn selection_bounds(ctx: &EvalCtx, layer: &Layer) -> Option<[f64; 4]> {
    let b = crate::content_bounds(ctx, layer)?;
    let Some(masks) = layer.masks() else { return Some(b) };
    if !b.iter().all(|v| v.is_finite()) || b[2] <= b[0] || b[3] <= b[1] {
        return Some(b);
    }
    let mut input = BoundsInput { source: b, masks: Vec::new() };
    let mut elements = 0usize;
    for g in masks.groups().filter(|g| g.enabled) {
        let GroupKind::Mask { mode, inverted, .. } = g.kind else { continue };
        if mode == MaskMode::None {
            continue;
        }
        let Some(Value::Path(sp)) = ctx.group_value(layer, g, "path") else { continue };
        if !sp.closed {
            continue;
        }
        elements = elements.saturating_add(sp.vertices.len().saturating_add(2));
        // Keep interactive geometry work bounded for adversarial or unusually large paths.
        // A conservative source box leaves the layer selectable without affecting rendering.
        if elements > MAX_ELEMENTS {
            return Some(b);
        }
        let path = effectcraft_path::to_kurbo(&sp);
        let expansion = ctx.f(layer, g, "expansion", 0.0);
        let opacity = ctx.f(layer, g, "opacity", 100.0) / 100.0;
        if !path.is_finite() || !expansion.is_finite() || !opacity.is_finite() {
            return Some(b);
        }
        input.masks.push(MaskInput { path, mode, inverted, opacity: opacity.clamp(0.0, 1.0), expansion });
    }
    if input.masks.is_empty() {
        return Some(b);
    }
    cached_bounds(input, elements)
}

// An exact cheap branch for rectangular masks (including clipping) and contained convex
// Add masks. Other curves and mask algebras keep the arrangement on a cache miss.
fn rectangle(path: &BezPath) -> Option<Rect> {
    let [PathEl::MoveTo(a), PathEl::LineTo(b), PathEl::LineTo(c), PathEl::LineTo(d), PathEl::LineTo(e), PathEl::ClosePath] = path.elements() else {
        return None;
    };
    if a != e || a.x != d.x || a.y != b.y || b.x != c.x || c.y != d.y || a.x == c.x || a.y == c.y {
        return None;
    }
    Some(Rect::from_points(*a, *c))
}

// A convex control polygon bounds one nondegenerate filled contour without arrangement work.
// Every edge must support the entire polygon (a consistent local turn alone admits stars).
// Limit this quadratic proof to small paths; complex/concave paths keep the general solver.
fn convex_bounds(path: &BezPath) -> Option<Rect> {
    let mut points = Vec::new();
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) if points.is_empty() => points.push(p),
            PathEl::LineTo(p) => points.push(p),
            PathEl::QuadTo(c, p) => points.extend([c, p]),
            PathEl::CurveTo(a, b, p) => points.extend([a, b, p]),
            PathEl::ClosePath => {}
            _ => return None,
        }
        if points.len() > 64 {
            return None;
        }
    }
    points.dedup();
    if points.first() == points.last() {
        points.pop();
    }
    if points.len() < 3 {
        return None;
    }
    let area: f64 = points.iter().zip(points.iter().cycle().skip(1)).take(points.len()).map(|(a, b)| a.x * b.y - a.y * b.x).sum();
    if !area.is_finite() || area == 0.0 {
        return None;
    }
    let sign = area.signum();
    for (a, b) in points.iter().zip(points.iter().cycle().skip(1)).take(points.len()) {
        for p in &points {
            let cross = ((b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)) * sign;
            if !cross.is_finite() || cross < 0.0 {
                return None;
            }
        }
    }
    let b = path.bounding_box();
    (b.width() > 0.0 && b.height() > 0.0).then_some(b)
}

fn compute_bounds(input: &BoundsInput) -> Option<[f64; 4]> {
    let b = input.source;
    let source = Rect::new(b[0], b[1], b[2], b[3]);
    let simple = input.masks.iter().enumerate().all(|(i, m)| {
        !m.inverted
            && m.expansion.abs() <= 0.01
            && (matches!(m.mode, MaskMode::Add | MaskMode::Lighten)
                || (i == 0 && matches!(m.mode, MaskMode::Intersect | MaskMode::Darken | MaskMode::Difference)))
            && (rectangle(&m.path).is_some() || convex_bounds(&m.path).is_some_and(|r| source.contains_rect(r)))
    });
    if simple {
        let mut bounds: Option<Rect> = None;
        for mask in &input.masks {
            if mask.opacity <= 0.0 {
                continue;
            }
            let r = rectangle(&mask.path).or_else(|| convex_bounds(&mask.path))?.intersect(source);
            if r.width() > 0.0 && r.height() > 0.0 {
                bounds = Some(bounds.map_or(r, |b| b.union(r)));
            }
        }
        return bounds.map(|r| [r.x0, r.y0, r.x1, r.y1]);
    }
    let mut paths = vec![(source.to_path(0.01), FillRule::NonZero)];
    let mut modes = Vec::new();
    let mut expanded_elements = 0usize;
    for mask in &input.masks {
        let path =
            if mask.expansion.abs() > 0.01 { effectcraft_path::offset::offset_path(&mask.path, mask.expansion, Join::Round, 4.0) } else { mask.path.clone() };
        expanded_elements = expanded_elements.saturating_add(path.elements().len());
        if !path.is_finite() || expanded_elements > MAX_ELEMENTS {
            return Some(b);
        }
        paths.push((path, FillRule::NonZero));
        modes.push((mask.mode, mask.inverted, mask.opacity));
    }
    #[cfg(test)]
    ARRANGEMENTS.with(|n| n.set(n.get() + 1));
    // Evaluate the same ordered mask algebra as masks::apply on each region of the curve
    // arrangement. Combining rectangles instead would give wrong bounds for subtract/invert
    // and for disjoint paths whose bounding rectangles overlap.
    let first_subtract = modes.first().is_some_and(|m| m.0 == MaskMode::Subtract);
    let visible = try_boolean_n(&paths, |inside| {
        if !inside.first().copied().unwrap_or(false) {
            return false;
        }
        let mut alpha: f64 = if first_subtract { 1.0 } else { 0.0 };
        for (i, ((mode, inverted, opacity), covered)) in modes.iter().zip(inside.iter().skip(1)).enumerate() {
            let c = if *covered != *inverted { *opacity } else { 0.0 };
            let mode = if i == 0 && matches!(mode, MaskMode::Intersect | MaskMode::Darken) { MaskMode::Add } else { *mode };
            alpha = match mode {
                MaskMode::Add => alpha + c - alpha * c,
                MaskMode::Subtract => alpha * (1.0 - c),
                MaskMode::Intersect => alpha * c,
                MaskMode::Lighten => alpha.max(c),
                MaskMode::Darken => alpha.min(c),
                MaskMode::Difference => (alpha - c).abs(),
                MaskMode::None => alpha,
            };
        }
        alpha > 0.0
    });
    let Some(visible) = visible else { return Some(b) };
    if visible.is_empty() {
        return None;
    }
    let r = visible.bounding_box();
    Some([r.x0, r.y0, r.x1, r.y1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_color::Label;
    use effectcraft_keyframe::{Keyframe, ShapePath};
    use effectcraft_project::{
        Comp, ItemKind, LayerSource, Project, Solid,
        build::{self, Ids},
    };
    use effectcraft_time::{FrameRate, Tick};

    fn bounds(masks: &[(MaskMode, bool, f64, ShapePath)], time: Tick, animated: bool) -> Option<[f64; 4]> {
        bounds_config(masks, time, animated, |_| {})
    }

    fn bounds_config(
        masks: &[(MaskMode, bool, f64, ShapePath)],
        time: Tick,
        animated: bool,
        configure: impl Fn(&mut effectcraft_project::PropGroup),
    ) -> Option<[f64; 4]> {
        let mut p = Project::default();
        let comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let cid = p.add_item("Comp", Label::Red, None, ItemKind::Comp(comp.clone().into()));
        let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0; 3], width: 200, height: 100, pixel_aspect: 1.0 }));
        let mut l = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (200, 100), None);
        let mut next = p.next_id;
        for (mode, inverted, opacity, path) in masks {
            let mut g = build::mask(&mut Ids(&mut next), "Mask", path.clone(), *mode, [255, 0, 0]);
            if let GroupKind::Mask { inverted: inv, .. } = &mut g.kind {
                *inv = *inverted;
            }
            g.get_mut("opacity").unwrap().value = Value::Scalar(*opacity);
            if animated {
                let prop = g.get_mut("path").unwrap();
                prop.keys = vec![
                    Keyframe::new(Tick::ZERO, Value::Path(path.clone())),
                    Keyframe::new(Tick::from_seconds_f64(1.0), Value::Path(ShapePath::rect([100.0, 50.0], 40.0, 40.0))),
                ];
            }
            configure(&mut g);
            l.props.sub_mut("masks").unwrap().children.push(g.into());
        }
        p.next_id = next;
        let ctx = EvalCtx::new(&p, cid, &comp, time);
        assert_eq!(crate::content_bounds(&ctx, &l), Some([0.0, 0.0, 200.0, 100.0]), "render/source bounds stay unchanged");
        selection_bounds(&ctx, &l)
    }

    #[test]
    fn evaluated_cache_reuses_geometry_and_tracks_animation_and_mask_settings() {
        BOUNDS_CACHE.with(|c| c.borrow_mut().clear());
        ARRANGEMENTS.with(|n| n.set(0));
        let mask = (MaskMode::Add, false, 100.0, ShapePath::ellipse([10.0, 50.0], 40.0, 40.0));
        let origin = bounds(std::slice::from_ref(&mask), Tick::ZERO, true);
        for _ in 0..10 {
            assert_eq!(bounds(std::slice::from_ref(&mask), Tick::ZERO, true), origin);
        }
        assert_eq!(ARRANGEMENTS.with(|n| n.get()), 1, "reuse across separately constructed contexts and projects");
        assert_eq!(bounds_config(std::slice::from_ref(&mask), Tick::ZERO, true, |g| g.get_mut("feather").unwrap().value = Value::Vec2([50.0; 2])), origin);
        assert_eq!(ARRANGEMENTS.with(|n| n.get()), 1, "feather does not affect geometric bounds");
        assert_ne!(bounds(std::slice::from_ref(&mask), Tick::from_seconds_f64(0.1), true), origin);
        assert_eq!(ARRANGEMENTS.with(|n| n.get()), 2);
        let inverted = (mask.0, true, mask.2, mask.3.clone());
        assert_ne!(bounds(&[inverted], Tick::ZERO, false), origin);
        let transparent = (mask.0, false, 0.0, mask.3.clone());
        assert_eq!(bounds(&[transparent], Tick::ZERO, false), None);
        assert_ne!(bounds_config(&[mask], Tick::ZERO, false, |g| g.get_mut("expansion").unwrap().value = Value::Scalar(10.0)), origin);
        assert_eq!(ARRANGEMENTS.with(|n| n.get()), 5);
    }

    #[test]
    fn rectangular_add_masks_skip_arrangements_and_cache_storage_stays_bounded() {
        BOUNDS_CACHE.with(|c| c.borrow_mut().clear());
        ARRANGEMENTS.with(|n| n.set(0));
        let masks: Vec<_> = (0..50).map(|i| (MaskMode::Add, false, 100.0, rect(25.0 + i as f64, 50.0, 20.0, 30.0))).collect();
        assert_eq!(bounds(&masks, Tick::ZERO, false), Some([15.0, 35.0, 84.0, 65.0]));
        assert_eq!(ARRANGEMENTS.with(|n| n.get()), 0);
        for i in 0..80 {
            let mask = (MaskMode::Add, false, 100.0, rect(i as f64, 50.0, 20.0, 30.0));
            bounds(&[mask], Tick::ZERO, false);
        }
        BOUNDS_CACHE.with(|cache| {
            let cache = cache.borrow();
            assert!(cache.len() <= CACHE_ENTRIES);
            assert!(cache.iter().map(|e| e.elements).sum::<usize>() <= CACHE_ELEMENTS);
        });
    }

    #[test]
    fn convex_fast_path_rejects_spikes_concavity_and_self_crossing_stars() {
        assert!(convex_bounds(&effectcraft_path::to_kurbo(&ShapePath::ellipse([50.0, 50.0], 40.0, 40.0))).is_some());
        for vertices in [
            vec![[0.0, 0.0], [10.0, 0.0], [5.0, 5.0], [10.0, 10.0], [0.0, 10.0]],
            vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            vec![[0.0, 0.0], [10.0, 10.0], [0.0, 10.0], [10.0, 0.0]],
        ] {
            let mut path = rect(0.0, 0.0, 10.0, 10.0);
            path.vertices = vertices;
            path.in_tangents.clear();
            path.out_tangents.clear();
            assert!(convex_bounds(&effectcraft_path::to_kurbo(&path)).is_none());
        }
        BOUNDS_CACHE.with(|c| c.borrow_mut().clear());
        ARRANGEMENTS.with(|n| n.set(0));
        let masks: Vec<_> = (0..50).map(|i| (MaskMode::Add, false, 100.0, ShapePath::ellipse([25.0 + i as f64, 50.0], 20.0, 30.0))).collect();
        let b = bounds(&masks, Tick::ZERO, false).unwrap();
        assert!(b.iter().zip([15.0, 35.0, 84.0, 65.0]).all(|(a, b)| (a - b).abs() < 0.01));
        assert_eq!(ARRANGEMENTS.with(|n| n.get()), 0, "convex Add masks do not require a curve arrangement");
    }

    #[test]
    fn excessive_selection_geometry_uses_conservative_source_box() {
        let mut path = rect(50.0, 50.0, 20.0, 30.0);
        path.vertices = vec![[50.0, 50.0]; MAX_ELEMENTS + 1];
        assert_eq!(bounds(&[(MaskMode::Add, false, 100.0, path)], Tick::ZERO, false), Some([0.0, 0.0, 200.0, 100.0]));
    }

    #[test]
    #[ignore = "diagnostic timing probe; run explicitly with --nocapture"]
    fn selection_bounds_timing_probe() {
        for curved in [false, true] {
            let masks: Vec<_> = (0..50)
                .map(|i| {
                    (
                        MaskMode::Add,
                        false,
                        100.0,
                        if curved { ShapePath::ellipse([25.0 + i as f64, 50.0], 20.0, 30.0) } else { rect(25.0 + i as f64, 50.0, 20.0, 30.0) },
                    )
                })
                .collect();
            BOUNDS_CACHE.with(|c| c.borrow_mut().clear());
            let start = std::time::Instant::now();
            let result = bounds(&masks, Tick::ZERO, false);
            let cold = start.elapsed();
            let start = std::time::Instant::now();
            for _ in 0..100 {
                assert_eq!(bounds(&masks, Tick::ZERO, false), result);
            }
            eprintln!("50 masks curved={curved}: cold={cold:?}, cached average={:?}", start.elapsed() / 100);
        }
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> ShapePath {
        ShapePath::rect([x, y], w, h)
    }

    #[test]
    fn selection_bounds_follow_add_and_first_intersect_masks() {
        for mode in [MaskMode::Add, MaskMode::Intersect, MaskMode::Darken, MaskMode::Lighten, MaskMode::Difference] {
            assert_eq!(bounds(&[(mode, false, 100.0, rect(50.0, 50.0, 40.0, 40.0))], Tick::ZERO, false), Some([30.0, 30.0, 70.0, 70.0]));
        }
    }

    #[test]
    fn subtract_inversion_and_partial_opacity() {
        let full = Some([0.0, 0.0, 200.0, 100.0]);
        let cut_left = rect(50.0, 50.0, 100.0, 100.0);
        assert_eq!(bounds(&[(MaskMode::Subtract, false, 100.0, cut_left.clone())], Tick::ZERO, false), Some([100.0, 0.0, 200.0, 100.0]));
        assert_eq!(bounds(&[(MaskMode::Add, true, 100.0, cut_left.clone())], Tick::ZERO, false), Some([100.0, 0.0, 200.0, 100.0]));
        assert_eq!(bounds(&[(MaskMode::Subtract, false, 50.0, cut_left)], Tick::ZERO, false), full);
        assert_eq!(bounds(&[(MaskMode::None, false, 100.0, rect(50.0, 50.0, 40.0, 40.0))], Tick::ZERO, false), full);
    }

    #[test]
    fn masks_are_combined_in_order_and_clipped_to_source() {
        let a = (MaskMode::Add, false, 100.0, rect(0.0, 50.0, 100.0, 100.0));
        let b = (MaskMode::Add, false, 100.0, rect(150.0, 50.0, 40.0, 40.0));
        assert_eq!(bounds(&[a.clone(), b.clone()], Tick::ZERO, false), Some([0.0, 0.0, 170.0, 100.0]));
        assert_eq!(bounds(&[a, (MaskMode::Intersect, b.1, b.2, b.3)], Tick::ZERO, false), None);
        assert_eq!(bounds(&[(MaskMode::Add, false, 0.0, rect(50.0, 50.0, 40.0, 40.0))], Tick::ZERO, false), None);
    }

    #[test]
    fn curved_paths_and_disjoint_geometry() {
        let ellipse = ShapePath::ellipse([50.0, 50.0], 40.0, 40.0);
        let b = bounds(&[(MaskMode::Add, false, 100.0, ellipse)], Tick::ZERO, false).unwrap();
        assert!(b.iter().zip([30.0, 30.0, 70.0, 70.0]).all(|(a, b)| (a - b).abs() < 0.01), "{b:?}");
        let mut a = rect(50.0, 50.0, 80.0, 80.0);
        a.vertices = vec![[10.0, 10.0], [90.0, 10.0], [10.0, 90.0]];
        a.in_tangents = vec![[0.0; 2]; 3];
        a.out_tangents = a.in_tangents.clone();
        let mut b = a.clone();
        b.vertices = vec![[90.0, 90.0], [90.0, 30.0], [30.0, 90.0]];
        assert_eq!(
            bounds(&[(MaskMode::Add, false, 100.0, a), (MaskMode::Intersect, false, 100.0, b)], Tick::ZERO, false),
            None,
            "overlapping bounding boxes do not imply overlapping masks"
        );
    }

    #[test]
    fn disabled_masks_expansion_and_feather() {
        let masks = [(MaskMode::Add, false, 100.0, rect(50.0, 50.0, 40.0, 40.0))];
        assert_eq!(bounds_config(&masks, Tick::ZERO, false, |g| g.enabled = false), Some([0.0, 0.0, 200.0, 100.0]));
        let grown = bounds_config(&masks, Tick::ZERO, false, |g| g.get_mut("expansion").unwrap().value = Value::Scalar(10.0)).unwrap();
        assert!(grown.iter().zip([20.0, 20.0, 80.0, 80.0]).all(|(a, b)| (a - b).abs() < 0.01), "{grown:?}");
        assert_eq!(
            bounds_config(&masks, Tick::ZERO, false, |g| g.get_mut("feather").unwrap().value = Value::Vec2([30.0, 30.0])),
            Some([30.0, 30.0, 70.0, 70.0])
        );
    }

    #[test]
    fn animated_paths_and_open_paths() {
        let mask = (MaskMode::Add, false, 100.0, rect(50.0, 50.0, 40.0, 40.0));
        assert_eq!(bounds(std::slice::from_ref(&mask), Tick::from_seconds_f64(1.0), true), Some([80.0, 30.0, 120.0, 70.0]));
        let mut open = mask.3;
        open.closed = false;
        assert_eq!(bounds(&[(MaskMode::Add, false, 100.0, open)], Tick::ZERO, false), Some([0.0, 0.0, 200.0, 100.0]));
    }
}
