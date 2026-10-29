#![allow(dead_code)]
//! Builders shared by the exporter tests.

use std::sync::Arc;

use effectcraft_aep::export::{ExportOptions, ExportResult, export};
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

/// `c` as far as the exporter writes property records: in every `tdbs` the user name is left out
/// and the header bytes that are not decoded are zero (aep-format.md 3.4; After Effects reads
/// them as zero, read-back e1-ztdb4d).
pub fn decoded(c: &effectcraft_aep::Chunk<'static>) -> effectcraft_aep::Chunk<'static> {
    use effectcraft_aep::{Body, Chunk};
    const KEEP: &[(usize, usize)] = &[(0, 4), (5, 6), (7, 16), (68, 69), (76, 78), (79, 80), (119, 121)];
    let record = c.is_list(b"tdbs");
    let kids = |children: &[Chunk<'static>]| -> Vec<Chunk<'static>> {
        children
            .iter()
            .filter(|k| !(record && k.is(b"tdsn")))
            .map(|k| {
                if record && k.is(b"tdb4") {
                    let d = k.bytes().unwrap_or_default();
                    Chunk::data(
                        *b"tdb4",
                        d.iter().enumerate().map(|(i, b)| if KEEP.iter().any(|(f, e)| (*f..*e).contains(&i)) { *b } else { 0 }).collect::<Vec<u8>>(),
                    )
                } else {
                    decoded(k)
                }
            })
            .collect()
    };
    match &c.body {
        Body::List { list_type, children } => Chunk { id: c.id, body: Body::List { list_type: *list_type, children: kids(children) }, pad: c.pad },
        Body::Chunks(children) => Chunk { id: c.id, body: Body::Chunks(kids(children)), pad: c.pad },
        _ => c.clone(),
    }
}
