#![allow(dead_code)]
//! Builders shared by the exporter tests.

use std::sync::Arc;

use effectcraft_aep_export::{ExportOptions, ExportResult, export};
use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::{Comp, ItemId, ItemKind, Layer, LayerSource, Project, Solid, build};
use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};

pub fn secs(s: f64) -> Tick {
    Tick((s * TICKS_PER_SECOND as f64).round() as i64)
}

pub fn add_comp(p: &mut Project, name: &str, w: u32, h: u32, fps: FrameRate, dur: f64, parent: Option<ItemId>) -> ItemId {
    let comp = Comp::new(w, h, fps, secs(dur));
    p.add_item(name, Label::None, parent, ItemKind::Comp(Arc::new(comp)))
}

pub fn add_solid(p: &mut Project, name: &str, w: u32, h: u32, color: [f32; 3], parent: Option<ItemId>) -> ItemId {
    p.add_item(name, Label::Red, parent, ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }))
}

/// Adds a layer on top of the composition.
pub fn add_layer(p: &mut Project, comp: ItemId, name: &str, source: LayerSource, size: (u32, u32)) {
    let mut c = p.comp(comp).cloned().expect("comp");
    let mut l = build::layer(p, &c, name, source, size, None);
    l.name = name.to_string();
    c.layers.insert(0, l);
    if let Some(i) = p.item_mut(comp) {
        i.kind = ItemKind::Comp(Arc::new(c));
    }
}

pub fn with_layer(p: &mut Project, comp: ItemId, idx: usize, f: impl FnOnce(&mut Layer)) {
    let mut c = p.comp(comp).cloned().expect("comp");
    if let Some(l) = c.layers.get_mut(idx) {
        f(l);
    }
    if let Some(i) = p.item_mut(comp) {
        i.kind = ItemKind::Comp(Arc::new(c));
    }
}

pub fn with_comp(p: &mut Project, comp: ItemId, f: impl FnOnce(&mut Comp)) {
    let mut c = p.comp(comp).cloned().expect("comp");
    f(&mut c);
    if let Some(i) = p.item_mut(comp) {
        i.kind = ItemKind::Comp(Arc::new(c));
    }
}

pub fn key(t: f64, v: Value) -> Keyframe {
    Keyframe::new(secs(t), v)
}

pub fn export_all(p: &Project) -> ExportResult {
    export(p, &ExportOptions { allow_losses: true }).expect("export")
}

/// One comp `C` (1920x1080, 25 fps, 5 s) with one solid layer `L`.
pub fn simple() -> (Project, ItemId, ItemId) {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    let s = add_solid(&mut p, "L", 1920, 1080, [1.0, 0.0, 0.0], None);
    add_layer(&mut p, c, "L", LayerSource::Solid { item: s }, (1920, 1080));
    (p, c, s)
}
