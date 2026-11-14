//! Projects that used to cost quadratic time to export (#3299): many layers of one
//! precomposition in a composition that is itself shown many times, and a long parent chain. The
//! sizes are moderate, so the tests check what is written; the bounds are pinned by the unit tests
//! in `export::plan`.

use std::sync::Arc;

use crate::export::helpers::*;
use effectcraft_aep::export::LossCategory;
use effectcraft_project::{ItemId, ItemKind, LayerSource, Project, build};
use effectcraft_time::FrameRate;

/// Puts `n` layers of `source` into `comp` at once; `edit` changes layer `i`.
fn fill(p: &mut Project, comp: ItemId, n: usize, source: LayerSource, edit: &dyn Fn(usize, &mut effectcraft_project::Layer)) {
    let mut c = p.comp(comp).cloned().expect("comp");
    for i in 0..n {
        let mut l = build::layer(p, &c, &format!("L{i}"), source.clone(), (100, 100), None);
        l.name = format!("L{i}");
        edit(i, &mut l);
        c.layers.push(l);
    }
    if let Some(item) = p.item_mut(comp) {
        item.kind = ItemKind::Comp(Arc::new(c));
    }
}

#[test]
fn many_layers_of_one_precomp_in_a_shown_composition_are_written() {
    let n = 1500;
    let mut p = Project::default();
    let a = add_comp(&mut p, "A", 100, 100, FrameRate::new(25, 1), 1.0, None);
    let s = add_solid(&mut p, "S", 100, 100, [1.0, 0.0, 0.0], None);
    add_layer(&mut p, a, "S", LayerSource::Solid { item: s }, (100, 100));
    let b = add_comp(&mut p, "B", 100, 100, FrameRate::new(25, 1), 1.0, None);
    fill(&mut p, b, n, LayerSource::Comp { item: a }, &|_, _| {});
    let c = add_comp(&mut p, "C", 100, 100, FrameRate::new(25, 1), 1.0, None);
    fill(&mut p, c, n, LayerSource::Comp { item: b }, &|_, _| {});
    let r = export_all(&p);
    assert_eq!(r.report.exported.precomp_layers, 2 * n);
    assert_eq!(r.report.count(LossCategory::UnsupportedLayers), 0, "{:?}", r.report.losses);
}

#[test]
fn a_long_parent_chain_is_written_and_its_loop_cut_once() {
    let n = 1500;
    let mut p = Project::default();
    let s = add_solid(&mut p, "S", 100, 100, [1.0, 0.0, 0.0], None);
    let c = add_comp(&mut p, "C", 100, 100, FrameRate::new(25, 1), 1.0, None);
    fill(&mut p, c, n, LayerSource::Solid { item: s }, &|_, _| {});
    // Each layer parented to the next; with `closed`, the last to the first (a loop).
    for closed in [false, true] {
        let mut q = p.clone();
        with_comp(&mut q, c, |cc| {
            let ids: Vec<_> = cc.layers.iter().map(|l| l.id).collect();
            for i in 0..n {
                let next = if i + 1 < n {
                    Some(ids[i + 1])
                } else if closed {
                    Some(ids[0])
                } else {
                    None
                };
                if let Some(l) = cc.layers.get_mut(i) {
                    l.parent = next;
                }
            }
        });
        let r = export_all(&q);
        assert_eq!(r.report.exported.layers, n);
        assert_eq!(r.report.count(LossCategory::ParentLinks), usize::from(closed), "{:?}", r.report.losses);
    }
}
