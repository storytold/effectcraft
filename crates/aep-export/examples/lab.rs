//! Developer lab: exports small projects built in code, for opening in After Effects.
//!
//! `cargo run --release -p effectcraft-aep-export --example lab -- <case> <out.aep>` (cases `x0`..`z1`,
//! precompositions `p1`, effects `e1`, footage `f1`, text `t4` (media paths below `AEP_SYNTHETIC`);
//! masks: `m1`..`m4`)
use effectcraft_aep_export::{ExportOptions, export};
use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::text_doc::{CharStyle, Justify, ParaStyle, StyleRun as TStyleRun, TextDoc};
use effectcraft_keyframe::{Ease, Interp, Keyframe, ShapePath, Value};
use effectcraft_project::{
    Comp, Expression, Footage, FootageKind, GroupKind, ItemId, ItemKind, LayerSource, MaskMode, Node, Project, PropGroup, Quality, Solid, build,
};
use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};

fn secs(s: f64) -> Tick {
    Tick((s * TICKS_PER_SECOND as f64).round() as i64)
}

fn add_comp(p: &mut Project, name: &str, w: u32, h: u32, fps: FrameRate, dur: f64, parent: Option<ItemId>) -> ItemId {
    let comp = Comp::new(w, h, fps, secs(dur));
    p.add_item(name, Label::None, parent, ItemKind::Comp(std::sync::Arc::new(comp)))
}

fn add_solid(p: &mut Project, name: &str, w: u32, h: u32, color: [f32; 3], parent: Option<ItemId>) -> ItemId {
    p.add_item(name, Label::Red, parent, ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }))
}

fn add_layer(p: &mut Project, comp: ItemId, name: &str, source: LayerSource, size: (u32, u32)) {
    let mut c = p.comp(comp).cloned().expect("comp");
    let mut l = build::layer(p, &c, name, source, size, None);
    l.name = name.to_string();
    c.layers.insert(0, l);
    if let Some(i) = p.item_mut(comp) {
        i.kind = ItemKind::Comp(std::sync::Arc::new(c));
    }
}

fn with_layer(p: &mut Project, comp: ItemId, idx: usize, f: impl FnOnce(&mut effectcraft_project::Layer)) {
    let mut c = p.comp(comp).cloned().expect("comp");
    if let Some(l) = c.layers.get_mut(idx) {
        f(l);
    }
    if let Some(i) = p.item_mut(comp) {
        i.kind = ItemKind::Comp(std::sync::Arc::new(c));
    }
}

fn with_comp(p: &mut Project, comp: ItemId, f: impl FnOnce(&mut Comp)) {
    let mut c = p.comp(comp).cloned().expect("comp");
    f(&mut c);
    if let Some(i) = p.item_mut(comp) {
        i.kind = ItemKind::Comp(std::sync::Arc::new(c));
    }
}

fn key(t: f64, v: Value) -> Keyframe {
    Keyframe::new(secs(t), v)
}

/// Adds a mask to the layer at `idx` of `comp`; `f` edits the new mask group.
fn mask(p: &mut Project, comp: ItemId, idx: usize, name: &str, path: ShapePath, mode: MaskMode, f: impl FnOnce(&mut PropGroup)) {
    let mut next = p.next_id;
    with_layer(p, comp, idx, |l| {
        let i = l.masks().map_or(0, |m| m.children.len());
        let mut g = build::mask(&mut build::Ids(&mut next), name, path, mode, build::MASK_COLORS[i % 8]);
        f(&mut g);
        l.props.sub_mut("masks").expect("masks").children.push(g.into());
    });
    p.next_id = next;
}

/// The first effect group of the first layer of a fixture, imported.
fn fixture_effect(name: &str) -> PropGroup {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    let bytes = std::fs::read(dir.join(name)).expect("fixture");
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = effectcraft_aep_import::ImportOptions { allow_losses: true, ..effectcraft_aep_import::ImportOptions::new(&none) };
    effectcraft_aep_import::import(&mut p, &bytes, &opts).expect("import");
    p.items
        .values()
        .find_map(|i| if let ItemKind::Comp(c) = &i.kind { c.layers.first().and_then(|l| l.effects()).and_then(|e| e.groups().next().cloned()) } else { None })
        .expect("effect")
}

fn add_effect(p: &mut Project, comp: ItemId, idx: usize, g: PropGroup) {
    with_layer(p, comp, idx, |l| l.props.sub_mut("effects").expect("effects").children.push(g.into()));
}

fn still(path: &str, w: u32, h: u32) -> Footage {
    Footage { path: path.to_string(), kind: FootageKind::Still, width: w, height: h, has_video: true, ..Footage::default() }
}

/// A sequence of `n` files `<prefix><number><suffix>` in `<root><dir>`.
#[allow(clippy::too_many_arguments)]
fn seq(root: &str, dir: &str, prefix: &str, start: u32, n: u32, digits: usize, suffix: &str, w: u32, h: u32, rate: FrameRate) -> Footage {
    let files: Vec<String> = (start..start + n).map(|k| format!("{root}{dir}\\{prefix}{k:0digits$}{suffix}")).collect();
    let mut f = Footage { kind: FootageKind::Sequence, frame_rate: rate, sequence: files.clone(), ..still(&files[0], w, h) };
    f.sync_sequence_duration();
    f
}

fn set(g: &mut PropGroup, id: &str, v: Value) {
    g.get_mut(id).expect(id).value = v;
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let case = a[1].as_str();
    let mut p = Project::default();
    let r25 = FrameRate::new(25, 1);
    match case {
        "x1" => {
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "L", 1920, 1080, [1.0, 0.0, 0.0], None);
            add_layer(&mut p, c, "L", LayerSource::Solid { item: s }, (1920, 1080));
        }
        "x2" => {
            let c = add_comp(&mut p, "Comp 2", 1280, 720, FrameRate::new(30, 1), 4.0, None);
            let s = add_solid(&mut p, "S2", 400, 300, [0.0, 1.0, 0.0], None);
            add_layer(&mut p, c, "Layer X", LayerSource::Solid { item: s }, (400, 300));
        }
        "x3" => {
            let f = p.add_item("Folder A", Label::None, None, ItemKind::Folder);
            let c1 = add_comp(&mut p, "Comp 1", 1920, 1080, r25, 5.0, Some(f));
            let c2 = add_comp(&mut p, "Comp 10", 640, 360, FrameRate::FPS_29_97, 2.0, None);
            let s = add_solid(&mut p, "Blue", 640, 360, [0.0, 0.0, 1.0], Some(f));
            add_layer(&mut p, c1, "Blue", LayerSource::Solid { item: s }, (640, 360));
            add_layer(&mut p, c1, "Null X", LayerSource::Null, (100, 100));
            add_layer(&mut p, c2, "Blue2", LayerSource::Solid { item: s }, (640, 360));
        }
        "x4" => {
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "L", 400, 300, [1.0, 0.5, 0.0], None);
            add_layer(&mut p, c, "L", LayerSource::Solid { item: s }, (400, 300));
            with_layer(&mut p, c, 0, |l| {
                let tr = l.transform_mut().expect("tr");
                tr.get_mut("anchor").expect("a").value = Value::Vec3([100.0, 50.0, 0.0]);
                tr.get_mut("position").expect("a").value = Value::Vec3([300.0, 200.0, 0.0]);
                tr.get_mut("scale").expect("a").value = Value::Vec3([50.0, 60.0, 100.0]);
                tr.get_mut("rotation").expect("a").value = Value::Scalar(45.0);
                tr.get_mut("opacity").expect("a").value = Value::Scalar(25.0);
            });
        }
        "x5" => {
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "S2", 400, 300, [0.0, 1.0, 0.0], None);
            add_layer(&mut p, c, "S2", LayerSource::Solid { item: s }, (400, 300));
            with_layer(&mut p, c, 0, |l| {
                let tr = l.transform_mut().expect("tr");
                let set = |tr: &mut effectcraft_project::PropGroup, id: &str, keys: Vec<Keyframe>| {
                    let pr = tr.get_mut(id).expect("prop");
                    pr.value = keys[0].value.clone();
                    pr.keys = keys;
                };
                set(tr, "anchor", vec![key(0.0, Value::Vec3([200.0, 150.0, 0.0])), key(2.0, Value::Vec3([100.0, 100.0, 0.0]))]);
                set(tr, "position", vec![key(0.0, Value::Vec3([100.0, 100.0, 0.0])), key(2.0, Value::Vec3([500.0, 300.0, 0.0]))]);
                set(tr, "scale", vec![key(0.0, Value::Vec3([100.0, 100.0, 100.0])), key(2.0, Value::Vec3([200.0, 150.0, 100.0]))]);
                set(tr, "rotation", vec![key(0.0, Value::Scalar(0.0)), key(2.0, Value::Scalar(90.0))]);
                set(tr, "opacity", vec![key(0.0, Value::Scalar(100.0)), key(2.0, Value::Scalar(0.0))]);
            });
        }
        "x6" => {
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "S", 400, 300, [0.2, 0.4, 0.6], None);
            add_layer(&mut p, c, "S", LayerSource::Solid { item: s }, (400, 300));
            with_layer(&mut p, c, 0, |l| {
                let tr = l.transform_mut().expect("tr");
                let o = tr.get_mut("opacity").expect("o");
                let mut k0 = key(0.0, Value::Scalar(0.0));
                let mut k1 = key(1.0, Value::Scalar(100.0));
                k0.out_interp = Interp::Bezier;
                k0.out_ease = vec![Ease { speed: 0.0, influence: 0.5 }];
                k1.in_interp = Interp::Bezier;
                k1.in_ease = vec![Ease { speed: 0.0, influence: 0.25 }];
                k1.out_interp = Interp::Hold;
                let k2 = key(2.0, Value::Scalar(50.0));
                o.keys = vec![k0, k1, k2];
                let pos = tr.get_mut("position").expect("p");
                let mut a = key(0.0, Value::Vec3([100.0, 100.0, 0.0]));
                let mut b = key(1.0, Value::Vec3([400.0, 500.0, 0.0]));
                let c3 = key(2.0, Value::Vec3([800.0, 100.0, 0.0]));
                a.out_interp = Interp::Bezier;
                a.out_ease = vec![Ease { speed: 0.0, influence: 0.3333 }];
                b.in_interp = Interp::Bezier;
                b.in_ease = vec![Ease { speed: 100.0, influence: 0.5 }];
                b.out_interp = Interp::Bezier;
                b.out_ease = vec![Ease { speed: 100.0, influence: 0.5 }];
                pos.keys = vec![a, b, c3];
                let rot = tr.get_mut("rotation").expect("r");
                rot.value = Value::Scalar(10.0);
                rot.expr = Some(Expression { text: "time*20".into(), enabled: true });
            });
        }
        "y1" => {
            let c = add_comp(&mut p, "Flags", 800, 600, FrameRate::new(30, 1), 10.0, None);
            with_comp(&mut p, c, |cc| {
                cc.work_area = (secs(1.0), secs(3.0));
                cc.background = [0.5, 0.25, 1.0];
                cc.pixel_aspect = 0.9091;
                cc.shutter_angle = 90.0;
                cc.shutter_phase = 0.0;
                cc.display_start = secs(4.0);
                cc.enable_motion_blur = false;
            });
            let s = add_solid(&mut p, "Sol", 800, 600, [0.1, 0.2, 0.3], None);
            let long = "L".repeat(80);
            add_layer(&mut p, c, "\u{dc}n\u{ef}c\u{f6}d\u{e9} \u{65e5}\u{672c}\u{8a9e}", LayerSource::Solid { item: s }, (800, 600));
            add_layer(&mut p, c, &long, LayerSource::Solid { item: s }, (800, 600));
            add_layer(&mut p, c, "Hidden", LayerSource::Solid { item: s }, (800, 600));
            add_layer(&mut p, c, "Locked", LayerSource::Solid { item: s }, (800, 600));
            add_layer(&mut p, c, "Adj", LayerSource::Solid { item: s }, (800, 600));
            let parent_id = p.comp(c).and_then(|cc| cc.layers.last().map(|l| l.id)).expect("last");
            let child_id = p.comp(c).and_then(|cc| cc.layers.get(1).map(|l| l.id)).expect("child");
            with_layer(&mut p, c, 0, |l| {
                l.switches.adjustment = true;
                l.blend_mode = BlendMode::Multiply;
                l.label = Label::Lavender;
                l.switches.quality = Quality::Draft;
            });
            with_layer(&mut p, c, 1, |l| {
                l.switches.locked = true;
                l.switches.shy = true;
                l.switches.solo = true;
                l.blend_mode = BlendMode::Screen;
                l.parent = Some(parent_id);
            });
            with_layer(&mut p, c, 2, |l| {
                l.switches.video = false;
                l.switches.audio = false;
                l.switches.effects = false;
                l.switches.motion_blur = true;
                l.switches.guide = true;
                l.blend_mode = BlendMode::Overlay;
                l.start_time = secs(1.5);
                l.in_point = secs(2.0);
                l.out_point = secs(6.0);
            });
            with_layer(&mut p, c, 3, |l| {
                l.blend_mode = BlendMode::Add;
                l.parent = Some(child_id);
            });
            with_layer(&mut p, c, 4, |l| {
                l.blend_mode = BlendMode::Difference;
            });
        }
        "y2" => {
            let c = add_comp(&mut p, "Keys", 1920, 1080, FrameRate::new(24, 1), 10.0, None);
            let s = add_solid(&mut p, "S", 200, 100, [0.9, 0.9, 0.9], None);
            add_layer(&mut p, c, "K", LayerSource::Solid { item: s }, (200, 100));
            with_layer(&mut p, c, 0, |l| {
                let tr = l.transform_mut().expect("tr");
                let pos = tr.get_mut("position").expect("p");
                let pts = [[100.0, 100.0], [400.0, 700.0], [900.0, 200.0], [1300.0, 800.0], [1800.0, 300.0]];
                let mut ks = Vec::new();
                for (i, v) in pts.iter().enumerate() {
                    let mut k = key(i as f64 * 1.5, Value::Vec3([v[0], v[1], 0.0]));
                    k.label = (i as u8 + 1) * 3;
                    if i == 2 {
                        k.spatial_auto = false;
                        k.spatial_in = [-100.0, 50.0, 0.0];
                        k.spatial_out = [100.0, -50.0, 0.0];
                    }
                    if i == 3 {
                        k.roving = true;
                    }
                    ks.push(k);
                }
                pos.keys = ks;
                let sc = tr.get_mut("scale").expect("s");
                let mut k0 = key(0.0, Value::Vec3([100.0, 100.0, 100.0]));
                let mut k1 = key(3.0, Value::Vec3([50.0, 200.0, 100.0]));
                k0.out_interp = Interp::Bezier;
                k0.out_ease = vec![Ease { speed: 0.0, influence: 0.4 }, Ease { speed: 0.1, influence: 0.5 }, Ease { speed: 0.0, influence: 0.2 }];
                k1.in_interp = Interp::Bezier;
                k1.in_ease = vec![Ease { speed: 0.0, influence: 0.6 }, Ease { speed: 0.0, influence: 0.7 }, Ease { speed: 0.0, influence: 0.8 }];
                sc.keys = vec![k0, k1];
                let an = tr.get_mut("anchor").expect("a");
                an.keys = vec![key(0.5, Value::Vec3([100.0, 50.0, 0.0])), key(4.5, Value::Vec3([0.0, 0.0, 0.0]))];
                let op = tr.get_mut("opacity").expect("o");
                op.value = Value::Scalar(40.0);
                op.expr = Some(Expression { text: "wiggle(2, 30)".into(), enabled: false });
                let ro = tr.get_mut("rotation").expect("r");
                let mut ks = vec![key(0.0, Value::Scalar(0.0)), key(1.0 / 3.0, Value::Scalar(360.0)), key(9.0, Value::Scalar(-720.0))];
                ks[0].out_interp = Interp::Hold;
                ks[1].in_interp = Interp::Hold;
                ro.keys = ks;
            });
        }
        "y3" => {
            let c = add_comp(&mut p, "Many", 640, 360, FrameRate::new(50, 1), 20.0, None);
            let s = add_solid(&mut p, "S", 64, 64, [0.5, 0.5, 0.5], None);
            for i in 0..300 {
                add_layer(&mut p, c, &format!("Layer {i}"), if i % 7 == 0 { LayerSource::Null } else { LayerSource::Solid { item: s } }, (64, 64));
                with_layer(&mut p, c, 0, |l| {
                    let tr = l.transform_mut().expect("tr");
                    let pos = tr.get_mut("position").expect("p");
                    pos.keys = (0..20).map(|k| key(k as f64, Value::Vec3([(i * 2) as f64, (k * 10) as f64, 0.0]))).collect();
                    tr.get_mut("opacity").expect("o").value = Value::Scalar((i % 100) as f64);
                });
            }
        }
        "x0" => {}
        "x7" => {
            // No layers, an empty folder, duplicate names, the Solids folder of the project.
            let sf = p.add_item("Solids", Label::None, None, ItemKind::Folder);
            p.add_item("Empty", Label::None, None, ItemKind::Folder);
            let s = add_solid(&mut p, "Dup", 64, 64, [0.3, 0.3, 0.3], Some(sf));
            add_solid(&mut p, "Dup", 32, 32, [0.6, 0.6, 0.6], Some(sf));
            let a = add_comp(&mut p, "Dup", 640, 360, r25, 1.0, None);
            add_comp(&mut p, "Dup", 640, 360, r25, 1.0, None);
            add_comp(&mut p, "No layers", 640, 360, FrameRate::new(24, 1), 0.04, None);
            add_layer(&mut p, a, "Dup", LayerSource::Solid { item: s }, (64, 64));
            add_layer(&mut p, a, "Null", LayerSource::Null, (100, 100));
        }
        "z1" => {
            let f = p.add_item("Rates", Label::Cyan, None, ItemKind::Folder);
            let long = "S".repeat(100);
            let s = add_solid(&mut p, &long, 128, 72, [0.25, 0.5, 0.75], Some(f));
            let s2 = add_solid(&mut p, "\u{dc}ml\u{e4}ut \u{65e5}\u{672c}", 50, 50, [1.0, 1.0, 0.0], None);
            for (i, (n, d)) in [(12, 1), (60000, 1001), (240, 1), (24000, 1001), (48, 1), (15, 1)].into_iter().enumerate() {
                let c = add_comp(&mut p, &format!("Rate {i}"), 320 + i as u32, 180, FrameRate::new(n, d), 3.0, Some(f));
                add_layer(&mut p, c, "S", LayerSource::Solid { item: s }, (128, 72));
                add_layer(&mut p, c, "U", LayerSource::Solid { item: s2 }, (50, 50));
                add_layer(&mut p, c, "N", LayerSource::Null, (100, 100));
                with_layer(&mut p, c, 0, |l| {
                    let tr = l.transform_mut().expect("tr");
                    let pos = tr.get_mut("position").expect("p");
                    pos.keys =
                        vec![key(0.0, Value::Vec3([10.0, 10.0, 0.0])), key(1.0, Value::Vec3([100.0, 50.0, 0.0])), key(2.5, Value::Vec3([20.0, 90.0, 0.0]))];
                    let rot = tr.get_mut("rotation").expect("r");
                    rot.keys = vec![key(0.0, Value::Scalar(0.0)), key(2.5, Value::Scalar(180.0))];
                });
                with_layer(&mut p, c, 2, |l| {
                    let tr = l.transform_mut().expect("tr");
                    tr.get_mut("opacity").expect("o").expr = Some(Expression { text: "Math.sin(time)*50+50".into(), enabled: true });
                    l.start_time = secs(0.5);
                    l.in_point = secs(0.5);
                    l.out_point = secs(2.0);
                });
            }
        }
        "m1" => {
            // Every mode, inverted, feather / opacity / expansion, an open path and a hexagon with tangents.
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "L", 1920, 1080, [1.0, 0.0, 0.0], None);
            add_layer(&mut p, c, "L", LayerSource::Solid { item: s }, (1920, 1080));
            mask(&mut p, c, 0, "Rect", ShapePath::rect([960.0, 540.0], 800.0, 400.0), MaskMode::Add, |_| {});
            mask(&mut p, c, 0, "Ellipse", ShapePath::ellipse([960.0, 540.0], 600.0, 300.0), MaskMode::Subtract, |g| {
                if let GroupKind::Mask { inverted, .. } = &mut g.kind {
                    *inverted = true;
                }
                set(g, "feather", Value::Vec2([20.0, 10.0]));
                set(g, "opacity", Value::Scalar(50.0));
                set(g, "expansion", Value::Scalar(5.0));
            });
            let open = ShapePath {
                vertices: vec![[200.0, 200.0], [600.0, 150.0], [900.0, 500.0], [700.0, 900.0], [300.0, 800.0]],
                in_tangents: vec![[0.0, 0.0], [-100.0, -20.0], [0.0, -150.0], [120.0, 0.0], [40.0, 60.0]],
                out_tangents: vec![[50.0, -30.0], [100.0, 20.0], [0.0, 150.0], [-120.0, 0.0], [0.0, 0.0]],
                closed: false,
                feather: Vec::new(),
            };
            mask(&mut p, c, 0, "Open", open, MaskMode::Intersect, |_| {});
            let mut hex = ShapePath::polygon(&[[960.0, 200.0], [1260.0, 370.0], [1260.0, 710.0], [960.0, 880.0], [660.0, 710.0], [660.0, 370.0]], true);
            hex.in_tangents[1] = [-40.0, -40.0];
            hex.out_tangents[1] = [40.0, 40.0];
            mask(&mut p, c, 0, "Hex", hex, MaskMode::Difference, |g| set(g, "expansion", Value::Scalar(-12.5)));
            mask(&mut p, c, 0, "Lighten", ShapePath::rect([300.0, 300.0], 200.0, 100.0), MaskMode::Lighten, |_| {});
            mask(&mut p, c, 0, "Darken", ShapePath::rect([1600.0, 300.0], 200.0, 100.0), MaskMode::Darken, |_| {});
            mask(&mut p, c, 0, "None", ShapePath::ellipse([300.0, 900.0], 100.0, 100.0), MaskMode::None, |_| {});
        }
        "m2" => {
            // Path keys (linear, hold out, label 3) and Mask Feather keys with bezier ease.
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "L", 1920, 1080, [1.0, 0.0, 0.0], None);
            add_layer(&mut p, c, "L", LayerSource::Solid { item: s }, (1920, 1080));
            mask(&mut p, c, 0, "Keyed", ShapePath::rect([960.0, 540.0], 400.0, 200.0), MaskMode::Add, |g| {
                let mut k1 = key(1.0, Value::Path(ShapePath::rect([960.0, 540.0], 800.0, 400.0)));
                k1.out_interp = Interp::Hold;
                k1.label = 3;
                g.get_mut("path").expect("path").keys = vec![
                    key(0.0, Value::Path(ShapePath::rect([960.0, 540.0], 400.0, 200.0))),
                    k1,
                    key(2.0, Value::Path(ShapePath::rect([900.0, 500.0], 600.0, 300.0))),
                ];
                let mut a = key(0.0, Value::Vec2([0.0, 0.0]));
                a.out_interp = Interp::Bezier;
                a.out_ease = vec![Ease { speed: 15.0, influence: 0.66 }, Ease { speed: 5.0, influence: 0.5 }];
                let mut b = key(2.0, Value::Vec2([50.0, 30.0]));
                b.in_interp = Interp::Bezier;
                b.in_ease = vec![Ease { speed: 8.0, influence: 0.66 }, Ease { speed: 8.0, influence: 0.66 }];
                g.get_mut("feather").expect("feather").keys = vec![a, b];
            });
        }
        "m3" => {
            // A 400x300 solid in a 1280x720 composition, and an adjustment layer with a feathered mask.
            let c = add_comp(&mut p, "Small", 1280, 720, r25, 3.0, None);
            let s = add_solid(&mut p, "Green 400x300", 400, 300, [0.0, 1.0, 0.0], None);
            let adj = add_solid(&mut p, "Adjustment", 1280, 720, [1.0, 1.0, 1.0], None);
            add_layer(&mut p, c, "G", LayerSource::Solid { item: s }, (400, 300));
            add_layer(&mut p, c, "Adj", LayerSource::Solid { item: adj }, (1280, 720));
            with_layer(&mut p, c, 0, |l| l.switches.adjustment = true);
            mask(&mut p, c, 0, "Vignette", ShapePath::ellipse([640.0, 360.0], 1000.0, 560.0), MaskMode::Add, |g| {
                if let GroupKind::Mask { inverted, .. } = &mut g.kind {
                    *inverted = true;
                }
                set(g, "feather", Value::Vec2([150.0, 150.0]));
            });
            mask(&mut p, c, 1, "Mask 1", ShapePath::rect([200.0, 150.0], 200.0, 100.0), MaskMode::Add, |_| {});
        }
        "m4" => {
            // Two layers with two masks each (the running mask counter per layer).
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "L", 1920, 1080, [1.0, 0.0, 0.0], None);
            add_layer(&mut p, c, "A", LayerSource::Solid { item: s }, (1920, 1080));
            add_layer(&mut p, c, "B", LayerSource::Solid { item: s }, (1920, 1080));
            for idx in 0..2 {
                mask(&mut p, c, idx, "Mask 1", ShapePath::ellipse([960.0, 540.0], 800.0, 400.0), MaskMode::Add, |_| {});
                mask(&mut p, c, idx, "Mask 2", ShapePath::rect([700.0, 500.0], 400.0, 300.0), MaskMode::Subtract, |_| {});
            }
        }
        "p1" => {
            // A 640x360 30 fps 3 s precomposition (green solid) in C, with anchor and position;
            // a composition B that uses the 100x100 composition A twice, B and A both in C2.
            let pre = add_comp(&mut p, "Pre", 640, 360, FrameRate::new(30, 1), 3.0, None);
            let g = add_solid(&mut p, "Green", 640, 360, [0.0, 1.0, 0.0], None);
            add_layer(&mut p, pre, "Green", LayerSource::Solid { item: g }, (640, 360));
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "L", 1920, 1080, [1.0, 0.0, 0.0], None);
            add_layer(&mut p, c, "L", LayerSource::Solid { item: s }, (1920, 1080));
            add_layer(&mut p, c, "Pre", LayerSource::Comp { item: pre }, (640, 360));
            with_layer(&mut p, c, 0, |l| {
                let tr = l.transform_mut().expect("tr");
                tr.get_mut("anchor").expect("a").value = Value::Vec3([160.0, 90.0, 0.0]);
                tr.get_mut("position").expect("p").value = Value::Vec3([300.0, 200.0, 0.0]);
            });
            let a_ = add_comp(&mut p, "A", 100, 100, r25, 1.0, None);
            let blue = add_solid(&mut p, "Blue", 100, 100, [0.0, 0.0, 1.0], None);
            add_layer(&mut p, a_, "Blue", LayerSource::Solid { item: blue }, (100, 100));
            let b = add_comp(&mut p, "B", 400, 200, r25, 2.0, None);
            add_layer(&mut p, b, "A left", LayerSource::Comp { item: a_ }, (100, 100));
            add_layer(&mut p, b, "A right", LayerSource::Comp { item: a_ }, (100, 100));
            with_layer(&mut p, b, 0, |l| l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([300.0, 100.0, 0.0]));
            with_layer(&mut p, b, 1, |l| l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([100.0, 100.0, 0.0]));
            let c2 = add_comp(&mut p, "C2", 1280, 720, r25, 4.0, None);
            add_layer(&mut p, c2, "B", LayerSource::Comp { item: b }, (400, 200));
            add_layer(&mut p, c2, "A", LayerSource::Comp { item: a_ }, (100, 100));
            with_layer(&mut p, c2, 0, |l| {
                let tr = l.transform_mut().expect("tr");
                tr.get_mut("position").expect("p").value = Value::Vec3([1000.0, 500.0, 0.0]);
                tr.get_mut("scale").expect("s").keys = vec![key(0.0, Value::Vec3([100.0, 100.0, 100.0])), key(2.0, Value::Vec3([200.0, 200.0, 100.0]))];
            });
        }
        "e1" => {
            // Tint on a red solid, Gradient Ramp on a 640x360 solid, Exposure (keyed) and a
            // second Tint on a grey solid.
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let red = add_solid(&mut p, "Red", 1920, 1080, [1.0, 0.0, 0.0], None);
            let green = add_solid(&mut p, "Green 640", 640, 360, [0.0, 1.0, 0.0], None);
            let grey = add_solid(&mut p, "Grey", 400, 400, [0.5, 0.5, 0.5], None);
            add_layer(&mut p, c, "Red", LayerSource::Solid { item: red }, (1920, 1080));
            add_layer(&mut p, c, "Ramp", LayerSource::Solid { item: green }, (640, 360));
            add_layer(&mut p, c, "Grey", LayerSource::Solid { item: grey }, (400, 400));
            with_layer(&mut p, c, 1, |l| l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([400.0, 300.0, 0.0]));
            with_layer(&mut p, c, 0, |l| l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([1400.0, 700.0, 0.0]));
            let mut tint = fixture_effect("356-fx-tint-default.aep");
            set(&mut tint, "black", Value::Color([0.2, 0.4, 0.6, 1.0]));
            set(&mut tint, "white", Value::Color([1.0, 1.0, 0.5, 1.0]));
            set(&mut tint, "amount", Value::Scalar(60.0));
            add_effect(&mut p, c, 2, tint);
            let mut ramp = fixture_effect("368-fx-ramp-default.aep");
            set(&mut ramp, "start", Value::Vec2([100.0, 50.0]));
            set(&mut ramp, "end", Value::Vec2([500.0, 300.0]));
            set(&mut ramp, "startColor", Value::Color([1.0, 0.0, 0.0, 1.0]));
            set(&mut ramp, "endColor", Value::Color([0.0, 0.0, 1.0, 1.0]));
            set(&mut ramp, "blend", Value::Scalar(25.0));
            add_effect(&mut p, c, 1, ramp);
            let mut exposure = fixture_effect("389-fx-exposure-default.aep");
            {
                let e = exposure.prop_mut("master/exposure").expect("exposure");
                e.keys = vec![key(0.0, Value::Scalar(-1.0)), key(2.0, Value::Scalar(1.0))];
                e.value = Value::Scalar(-1.0);
            }
            exposure.prop_mut("master/offset").expect("offset").value = Value::Scalar(0.05);
            exposure.prop_mut("master/gamma").expect("gamma").value = Value::Scalar(1.5);
            exposure.prop_mut("green/greenExposure").expect("g").value = Value::Scalar(0.5);
            add_effect(&mut p, c, 0, exposure);
            let mut tint2 = fixture_effect("356-fx-tint-default.aep");
            tint2.name = "Tint 2".into();
            set(&mut tint2, "amount", Value::Scalar(50.0));
            add_effect(&mut p, c, 0, tint2);
        }
        "s1" => {
            // Time stretch: a precomposition whose 200x200 white square moves left to right
            // over 4 s, shown at 200 % (top half) and at 50 % from 1 s (bottom half).
            let mv = add_comp(&mut p, "Move", 1920, 540, r25, 4.0, None);
            let w = add_solid(&mut p, "Square", 200, 200, [1.0, 1.0, 1.0], None);
            add_layer(&mut p, mv, "Square", LayerSource::Solid { item: w }, (200, 200));
            with_layer(&mut p, mv, 0, |l| {
                l.transform_mut().expect("tr").get_mut("position").expect("p").keys =
                    vec![key(0.0, Value::Vec3([100.0, 270.0, 0.0])), key(4.0, Value::Vec3([1820.0, 270.0, 0.0]))];
            });
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 10.0, None);
            let red = add_solid(&mut p, "Red", 1920, 1080, [1.0, 0.0, 0.0], None);
            add_layer(&mut p, c, "BG", LayerSource::Solid { item: red }, (1920, 1080));
            for (name, pct, start, y) in [("Slow 200%", 200.0, 0.0, 270.0), ("Fast 50%", 50.0, 1.0, 810.0)] {
                add_layer(&mut p, c, name, LayerSource::Comp { item: mv }, (1920, 540));
                with_layer(&mut p, c, 0, |l| {
                    l.stretch = pct;
                    l.start_time = secs(start);
                    l.in_point = secs(start);
                    l.out_point = secs(start + 4.0 * pct / 100.0);
                    l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([960.0, y, 0.0]);
                });
            }
        }
        "d1" => {
            // 3D layers without a camera: a 400x400 green solid at z 0, a blue one at z 500 and a
            // white one at z -300 (closer), over a 2D red background.
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let red = add_solid(&mut p, "Red", 1920, 1080, [1.0, 0.0, 0.0], None);
            add_layer(&mut p, c, "BG", LayerSource::Solid { item: red }, (1920, 1080));
            for (name, rgb, x, z) in
                [("Green z0", [0.0, 1.0, 0.0], 400.0, 0.0), ("Blue z500", [0.0, 0.0, 1.0], 960.0, 500.0), ("White z-300", [1.0, 1.0, 1.0], 1520.0, -300.0)]
            {
                let s = add_solid(&mut p, name, 400, 400, rgb, None);
                add_layer(&mut p, c, name, LayerSource::Solid { item: s }, (400, 400));
                with_layer(&mut p, c, 0, |l| {
                    l.switches.three_d = true;
                    l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([x, 540.0, z]);
                });
            }
        }
        "f2" => {
            // The kinds added after f1: a jpeg sequence, a ProRes mov, a TIFF still.
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let red = add_solid(&mut p, "L", 1920, 1080, [1.0, 0.0, 0.0], None);
            add_layer(&mut p, c, "L", LayerSource::Solid { item: red }, (1920, 1080));
            let syn = std::env::var("AEP_SYNTHETIC").unwrap_or_else(|_| r"C:\fixtures\".to_string());
            let media = format!(r"{syn}test-media\");
            let mov = Footage { kind: FootageKind::Video, frame_rate: r25, duration: secs(1.0), has_video: true, ..still(&format!("{media}clip.mov"), 64, 48) };
            let items: Vec<(&str, Footage, f64)> = vec![
                ("j_[000-002].jpg", seq(&media, "jpgseq", "j_", 0, 3, 3, ".jpg", 64, 48, r25), 0.12),
                ("clip.mov", mov, 1.0),
                ("still.tif", still(&format!("{media}still.tif"), 64, 48), 5.0),
            ];
            for (i, (name, f, dur)) in items.into_iter().enumerate() {
                let size = (f.width.max(1), f.height.max(1));
                let id = p.add_item(name, Label::Lavender, None, ItemKind::Footage(f));
                add_layer(&mut p, c, name, LayerSource::Footage { item: id }, size);
                let x = 100.0 + 200.0 * i as f64;
                with_layer(&mut p, c, 0, |l| {
                    l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([x, 100.0, 0.0]);
                    l.out_point = secs(dur);
                });
            }
        }
        "f1" => {
            // Every footage kind the exporter writes, side by side over a red solid.
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let red = add_solid(&mut p, "L", 1920, 1080, [1.0, 0.0, 0.0], None);
            add_layer(&mut p, c, "L", LayerSource::Solid { item: red }, (1920, 1080));
            // The synthetic corpus folder (with `test-media`, `test-seq`, `test8x8.png`, `test-tone.wav`).
            let syn = std::env::var("AEP_SYNTHETIC").unwrap_or_else(|_| r"C:\fixtures\".to_string());
            let media = format!(r"{syn}test-media\");
            let (syn, media) = (syn.as_str(), media.as_str());
            let f30 = FrameRate::new(30, 1);
            let items: Vec<(&str, Footage)> = vec![
                ("still.png", still(&format!("{media}still.png"), 64, 48)),
                ("still.jpg", still(&format!("{media}still.jpg"), 64, 48)),
                ("still.exr", still(&format!("{media}still.exr"), 64, 48)),
                ("shot_[0101-0103].exr", seq(media, "exrseq", "shot_", 101, 3, 4, ".exr", 64, 48, f30)),
                ("seq_[001-003].png", seq(syn, "test-seq", "seq_", 1, 3, 3, ".png", 64, 64, f30)),
                (
                    "clip.mp4",
                    Footage { kind: FootageKind::Video, frame_rate: r25, duration: secs(1.0), has_video: true, ..still(&format!("{media}clip.mp4"), 64, 48) },
                ),
                (
                    "test-tone.wav",
                    Footage { kind: FootageKind::Audio, duration: secs(1.0), has_video: false, has_audio: true, ..still(&format!("{syn}test-tone.wav"), 0, 0) },
                ),
                // An 8x8 RGBA png written with the 64x48 RGB png prototype (stale colour type).
                ("test8x8.png", still(&format!("{syn}test8x8.png"), 8, 8)),
            ];
            for (i, (name, f)) in items.into_iter().enumerate() {
                let size = (f.width.max(1), f.height.max(1));
                let id = p.add_item(name, Label::Lavender, None, ItemKind::Footage(f));
                add_layer(&mut p, c, name, LayerSource::Footage { item: id }, size);
                let x = 100.0 + 200.0 * i as f64;
                with_layer(&mut p, c, 0, |l| l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([x, 100.0, 0.0]));
            }
        }
        "t4" => {
            // Text layers: point, multi-paragraph centred, box right-aligned with another font,
            // two style runs with tracking, faux bold + all caps, faux italic + small caps with
            // automatic leading, an anchor.
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "BG", 1920, 1080, [0.1, 0.1, 0.3], None);
            add_layer(&mut p, c, "BG", LayerSource::Solid { item: s }, (1920, 1080));
            let style = |font: &str, size: f64, fill: [f32; 4]| CharStyle {
                font: font.into(),
                style: "Regular".into(),
                size,
                fill,
                leading: Some(size * 1.2),
                ..CharStyle::default()
            };
            let mut docs: Vec<(&str, TextDoc, [f64; 2])> = Vec::new();
            {
                let mut d = TextDoc::plain("Hello");
                d.set_runs(vec![TStyleRun { len: 5, style: style("TimesNewRomanPSMT", 100.0, [1.0, 1.0, 1.0, 1.0]) }]);
                docs.push(("Hello", d, [200.0, 200.0]));
            }
            {
                let mut d = TextDoc::plain("First\rSecond line\rThird");
                d.set_runs(vec![TStyleRun { len: 23, style: CharStyle { leading: Some(60.0), ..style("TimesNewRomanPSMT", 50.0, [0.0, 1.0, 0.0, 1.0]) } }]);
                d.set_paras(vec![ParaStyle { justify: Justify::Center, ..d.base_para() }; 3]);
                docs.push(("Paragraphs", d, [960.0, 300.0]));
            }
            {
                let mut d = TextDoc::plain("Box text that wraps across the box");
                d.set_runs(vec![TStyleRun { len: 34, style: style("ArialMT", 40.0, [1.0, 1.0, 0.0, 1.0]) }]);
                d.set_paras(vec![ParaStyle { justify: Justify::Right, ..d.base_para() }]);
                d.box_size = Some([400.0, 200.0]);
                d.box_pos = [-200.0, -100.0];
                docs.push(("Box", d, [1500.0, 300.0]));
            }
            {
                let mut d = TextDoc::plain("Red Blue");
                let red = CharStyle { tracking: 50.0, ..style("TimesNewRomanPSMT", 80.0, [1.0, 0.0, 0.0, 1.0]) };
                let blue = CharStyle { tracking: 50.0, ..style("TimesNewRomanPSMT", 80.0, [0.2, 0.4, 1.0, 1.0]) };
                d.set_runs(vec![TStyleRun { len: 4, style: red }, TStyleRun { len: 4, style: blue }]);
                docs.push(("Runs", d, [200.0, 600.0]));
            }
            {
                let mut d = TextDoc::plain("Bold Caps");
                d.set_runs(vec![TStyleRun {
                    len: 9,
                    style: CharStyle { faux_bold: true, all_caps: true, ..style("TimesNewRomanPSMT", 70.0, [1.0, 0.5, 0.0, 1.0]) },
                }]);
                docs.push(("Bold", d, [800.0, 600.0]));
            }
            {
                let mut d = TextDoc::plain("Italic small\rcaps auto");
                d.set_runs(vec![TStyleRun {
                    len: 22,
                    style: CharStyle { faux_italic: true, small_caps: true, leading: None, ..style("TimesNewRomanPSMT", 60.0, [1.0, 1.0, 1.0, 1.0]) },
                }]);
                docs.push(("Italic", d, [1300.0, 600.0]));
            }
            {
                let mut d = TextDoc::plain("Anchor");
                d.set_runs(vec![TStyleRun { len: 6, style: style("TimesNewRomanPSMT", 60.0, [0.0, 1.0, 1.0, 1.0]) }]);
                docs.push(("Anchor", d, [300.0, 900.0]));
            }
            for (name, doc, pos) in docs {
                add_layer(&mut p, c, name, LayerSource::Text, (1920, 1080));
                with_layer(&mut p, c, 0, |l| {
                    if let Some(st) = l.props.prop_mut("text/sourceText") {
                        st.value = Value::Text(Box::new(doc));
                    }
                    let tr = l.transform_mut().expect("tr");
                    tr.get_mut("position").expect("p").value = Value::Vec3([pos[0], pos[1], 0.0]);
                    if name == "Anchor" {
                        tr.get_mut("anchor").expect("a").value = Value::Vec3([10.0, 20.0, 0.0]);
                    }
                });
            }
        }
        "t5" => {
            // Style slot /13: three `HHHH` 100 px point texts, normal, superscript and subscript
            // (compare 527 / 526 / 528: 288.9 px wide normal, 168.4 px super / subscript).
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            for (name, b, y) in [
                ("Normal", effectcraft_keyframe::text_doc::BaselineOption::Normal, 300.0),
                ("Super", effectcraft_keyframe::text_doc::BaselineOption::Superscript, 600.0),
                ("Sub", effectcraft_keyframe::text_doc::BaselineOption::Subscript, 900.0),
            ] {
                let mut d = TextDoc::plain("HHHH");
                let st = CharStyle {
                    font: "ArialMT".into(),
                    style: "Regular".into(),
                    size: 100.0,
                    fill: [1.0, 1.0, 1.0, 1.0],
                    leading: None,
                    baseline: b,
                    ..CharStyle::default()
                };
                d.set_runs(vec![TStyleRun { len: 4, style: st }]);
                add_layer(&mut p, c, name, LayerSource::Text, (1920, 1080));
                with_layer(&mut p, c, 0, |l| {
                    if let Some(st) = l.props.prop_mut("text/sourceText") {
                        st.value = Value::Text(Box::new(d));
                    }
                    l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([200.0, y, 0.0]);
                });
            }
        }
        "t6" => {
            // The character and paragraph settings written since the importer decodes them:
            // scales, baseline shift, auto kern none / optical, ligatures off, tsume, no fill +
            // stroke, stroke under fill, the four justified variants, indents and spacing,
            // single-line composer, right to left.
            use effectcraft_keyframe::text_doc::{Composer, Direction, Kerning};
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "BG", 1920, 1080, [0.1, 0.1, 0.3], None);
            add_layer(&mut p, c, "BG", LayerSource::Solid { item: s }, (1920, 1080));
            let st = |size: f64| CharStyle {
                font: "ArialMT".into(),
                style: "Regular".into(),
                size,
                fill: [1.0, 1.0, 1.0, 1.0],
                leading: None,
                ..CharStyle::default()
            };
            let mut docs: Vec<(&str, TextDoc, [f64; 2])> = Vec::new();
            let one = |text: &str, style: CharStyle| {
                let mut d = TextDoc::plain(text);
                d.set_runs(vec![TStyleRun { len: text.chars().count(), style }]);
                d
            };
            docs.push(("Scale", one("Scale 150 80", CharStyle { h_scale: 150.0, v_scale: 80.0, ..st(60.0) }), [100.0, 100.0]));
            docs.push(("Shift", one("Shift 20", CharStyle { baseline_shift: 20.0, ..st(60.0) }), [1100.0, 100.0]));
            docs.push(("KernNone", one("AVAWAT none", CharStyle { kerning: Kerning::Manual(0.0), ..st(60.0) }), [100.0, 200.0]));
            docs.push(("KernOptical", one("AVAWAT optical", CharStyle { kerning: Kerning::Optical, ..st(60.0) }), [1100.0, 200.0]));
            docs.push(("LigOff", one("office fi off", CharStyle { ligatures: false, ..st(60.0) }), [100.0, 300.0]));
            docs.push(("Tsume", one("Tsume 50", CharStyle { tsume: 50.0, ..st(60.0) }), [1100.0, 300.0]));
            docs.push((
                "Stroke",
                one("Stroke only", CharStyle { apply_fill: false, apply_stroke: true, stroke: [1.0, 1.0, 0.0, 1.0], stroke_width: 4.0, ..st(80.0) }),
                [100.0, 420.0],
            ));
            {
                let mut d = one("Under", CharStyle { apply_stroke: true, stroke: [0.0, 1.0, 0.0, 1.0], stroke_width: 10.0, ..st(80.0) });
                d.stroke_over_fill = false;
                docs.push(("UnderFill", d, [1100.0, 420.0]));
            }
            let box_text = "One two three four five six seven eight nine ten eleven twelve\rSecond paragraph here";
            for (i, (name, j)) in [
                ("JustLeft", Justify::JustifyLastLeft),
                ("JustRight", Justify::JustifyLastRight),
                ("JustCenter", Justify::JustifyLastCenter),
                ("JustAll", Justify::JustifyAll),
            ]
            .into_iter()
            .enumerate()
            {
                let mut d = one(box_text, st(30.0));
                d.set_paras(vec![ParaStyle { justify: j, ..d.base_para() }; 2]);
                d.box_size = Some([400.0, 250.0]);
                d.box_pos = [0.0, 0.0];
                docs.push((name, d, [60.0 + 470.0 * i as f64, 500.0]));
            }
            {
                let mut d = one(box_text, st(30.0));
                let ps = ParaStyle {
                    indent_first: 30.0,
                    indent_left: 20.0,
                    indent_right: 40.0,
                    space_before: 15.0,
                    space_after: 25.0,
                    composer: Composer::SingleLine,
                    hanging_punctuation: true,
                    ..d.base_para()
                };
                d.set_paras(vec![ps; 2]);
                d.box_size = Some([500.0, 250.0]);
                d.box_pos = [0.0, 0.0];
                docs.push(("Indents", d, [60.0, 800.0]));
            }
            {
                let mut d = one(box_text, st(30.0));
                d.set_paras(vec![ParaStyle { composer: Composer::EveryLine, ..d.base_para() }; 2]);
                d.box_size = Some([500.0, 250.0]);
                d.box_pos = [0.0, 0.0];
                docs.push(("EveryLine", d, [660.0, 800.0]));
            }
            {
                let mut d = one("Right to left", st(50.0));
                d.set_paras(vec![ParaStyle { direction: Direction::Rtl, justify: Justify::Right, ..d.base_para() }]);
                docs.push(("Rtl", d, [1800.0, 900.0]));
            }
            for (name, doc, pos) in docs {
                add_layer(&mut p, c, name, LayerSource::Text, (1920, 1080));
                with_layer(&mut p, c, 0, |l| {
                    if let Some(st) = l.props.prop_mut("text/sourceText") {
                        st.value = Value::Text(Box::new(doc));
                    }
                    l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([pos[0], pos[1] + 60.0, 0.0]);
                });
            }
        }
        "e2" => {
            // Every effect the table maps, each in its own 480 x 270 composition on a solid with
            // a default Gradient Ramp (black to white, top to bottom) under it, with the values
            // of the one-change fixtures; an expression control renamed and driving the layer's
            // rotation by expression.
            let list: &[(&str, &str, &str)] = &[
                ("Invert", "410-fx-invert-channel-2.aep", "ec.channel.invert"),
                ("FastBox", "413-fx-fastbox-radius-20.aep", "ec.blur.fastbox"),
                ("Gaussian", "418-fx-gaussian-dimensions-3.aep", "ec.blur.gaussian"),
                ("DropShadow", "422-fx-dropshadow-opacity-204.aep", "ec.perspective.dropshadow"),
                ("Glow", "429-fx-glow-threshold-76.5.aep", "ec.stylize.glow"),
                ("Fill", "215-fx-fill-default.aep", "ec.generate.fill"),
                ("HueSat", "444-fx-huesat-hue-90.aep", "ec.color.huesaturation"),
                ("Colorize", "448-fx-huesat-colorizehue-120.aep", "ec.color.huesaturation"),
                ("Levels", "514-fx-levels-gamma-2.aep", "ec.color.levels"),
                ("Timewarp", "530-fx-timewarp-speed-25.aep", "ec.time.timewarp"),
                ("Directional", "711-fx-directional-blur-30-40-on-M.aep", "ec.blur.directional"),
                ("Radial", "713-fx-radial-blur-zoom-20-on-M.aep", "ec.blur.radial"),
                ("CCRadial", "714-fx-ccradial-type2-30-60-on-M.aep", "ec.blur.ccradial"),
                ("Venetian", "715-fx-venetian-40-30-50-5-on-M.aep", "ec.transition.venetian"),
                ("ChannelBlur", "716-fx-channel-blur-5-10-15-20-edge-horiz-on-M.aep", "ec.blur.channel"),
                ("BlackWhite", "717-fx-blackwhite-set-tint-on-L.aep", "ec.color.blackwhite"),
                ("Tritone", "718-fx-tritone-set-blend25-on-L.aep", "ec.color.tritone"),
                ("LumaKey", "719-fx-lumakey-darker-140-20-1-2.5-on-M.aep", "ec.key.luma"),
                ("SimpleChoker", "721-fx-simplechoker-3.5-view-matte-on-M.aep", "ec.matte.simplechoker"),
                ("MatteChoker", "722-fx-mattechoker-set-on-M.aep", "ec.matte.mattechoker"),
                ("Transform", "723-fx-transform-set-on-M.aep", "ec.distort.transform"),
                ("Slider", "534-fx-slider-expression-time10.aep", "ec.control.slider"),
                ("Color", "073-fx-color-0.2-0.6-1.aep", "ec.control.color"),
                ("Checkbox", "217-fx-checkbox-on.aep", "ec.control.checkbox"),
                ("Point", "218-fx-point-111-222.aep", "ec.control.point"),
                ("Angle", "219-fx-angle-90.aep", "ec.control.angle"),
                ("SliderKeys", "221-fx-slider-2key.aep", "ec.control.slider"),
            ];
            let ramp = fixture_effect("368-fx-ramp-default.aep");
            for (name, fixture, effect) in list {
                let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
                let bytes = std::fs::read(dir.join(fixture)).expect("fixture");
                let mut q = Project::default();
                let none = |_: &str| false;
                let opts = effectcraft_aep_import::ImportOptions { allow_losses: true, ..effectcraft_aep_import::ImportOptions::new(&none) };
                effectcraft_aep_import::import(&mut q, &bytes, &opts).expect("import");
                let mut g = q
                    .items
                    .values()
                    .filter_map(|i| if let ItemKind::Comp(c) = &i.kind { Some(c.clone()) } else { None })
                    .flat_map(|c| c.layers.clone())
                    .filter_map(|l| l.effects().cloned())
                    .flat_map(|e| e.groups().cloned().collect::<Vec<_>>())
                    .find(|g| matches!(&g.kind, GroupKind::Effect { effect: e } if e == effect))
                    .expect("effect");
                let c = add_comp(&mut p, &format!("C {name}"), 480, 270, r25, 2.0, None);
                let s = add_solid(&mut p, &format!("S {name}"), 480, 270, [1.0, 0.0, 0.0], None);
                add_layer(&mut p, c, "L", LayerSource::Solid { item: s }, (480, 270));
                let mut r = ramp.clone();
                set(&mut r, "start", Value::Vec2([240.0, 0.0]));
                set(&mut r, "end", Value::Vec2([240.0, 270.0]));
                add_effect(&mut p, c, 0, r);
                if *name == "Fill" {
                    set(&mut g, "color", Value::Color([0.0, 0.5, 1.0, 1.0]));
                    set(&mut g, "opacity", Value::Scalar(50.0));
                }
                if *name == "Slider" {
                    g.name = "Speed".into();
                    with_layer(&mut p, c, 0, |l| {
                        let rot = l.transform_mut().expect("tr").get_mut("rotation").expect("r");
                        rot.expr = Some(Expression { text: "effect(\"Speed\")(\"Slider\")".into(), enabled: true });
                    });
                }
                add_effect(&mut p, c, 0, g);
            }
        }
        "t8" => {
            // Text animators: four 80 px ArialMT texts. A: Position [0,-60] + Opacity 0 with Start
            // keyed 0 -> 100 over 2 s (a reveal); B: Scale 150 + Rotation 20 on words (Based On
            // Words, End 50); C: Fill Color green, Ramp Up shape, Offset 20; D: Tracking 30 and
            // Blur 4 with Index units (characters 2 to 5) and Mode Subtract on a second selector.
            use effectcraft_project::build;
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 3.0, None);
            let s = add_solid(&mut p, "BG", 1920, 1080, [0.1, 0.1, 0.2], None);
            add_layer(&mut p, c, "BG", LayerSource::Solid { item: s }, (1920, 1080));
            let st =
                CharStyle { font: "ArialMT".into(), style: "Regular".into(), size: 80.0, fill: [1.0, 1.0, 1.0, 1.0], leading: None, ..CharStyle::default() };
            let mut next = p.next_id;
            let mut texts: Vec<(&str, &str, Vec<PropGroup>, [f64; 2])> = Vec::new();
            {
                let mut ids = build::Ids(&mut next);
                let anim = |ids: &mut build::Ids, name: &str, kinds: &[&str]| -> PropGroup {
                    let props: Vec<_> = kinds.iter().flat_map(|k| build::text_anim_props(ids, k, false)).collect();
                    build::text_animator(ids, name, props)
                };
                let mut a = anim(&mut ids, "Reveal", &["position", "opacity"]);
                if let Some(pg) = a.sub_mut("properties") {
                    pg.get_mut("position").unwrap().value = Value::Vec3([0.0, -60.0, 0.0]);
                    pg.get_mut("opacity").unwrap().value = Value::Scalar(0.0);
                }
                if let Some(Node::Group(sel)) = a.sub_mut("selectors").and_then(|s| s.children.first_mut()) {
                    sel.get_mut("start").unwrap().keys = vec![key(0.0, Value::Scalar(0.0)), key(2.0, Value::Scalar(100.0))];
                }
                texts.push(("A", "Reveal text", vec![a], [200.0, 200.0]));
                let mut b = anim(&mut ids, "Words", &["scale", "rotation"]);
                if let Some(pg) = b.sub_mut("properties") {
                    pg.get_mut("scale").unwrap().value = Value::Vec3([150.0, 150.0, 100.0]);
                    pg.get_mut("rotation").unwrap().value = Value::Scalar(20.0);
                }
                if let Some(Node::Group(sel)) = b.sub_mut("selectors").and_then(|s| s.children.first_mut()) {
                    sel.get_mut("end").unwrap().value = Value::Scalar(50.0);
                    sel.sub_mut("advanced").unwrap().get_mut("basedOn").unwrap().value = Value::Enum(2);
                }
                texts.push(("B", "Words are big", vec![b], [1000.0, 200.0]));
                let mut cc = anim(&mut ids, "Colour", &["fillColor"]);
                if let Some(pg) = cc.sub_mut("properties") {
                    pg.get_mut("fillColor").unwrap().value = Value::Color([0.0, 1.0, 0.0, 1.0]);
                }
                if let Some(Node::Group(sel)) = cc.sub_mut("selectors").and_then(|s| s.children.first_mut()) {
                    sel.get_mut("offset").unwrap().value = Value::Scalar(20.0);
                    sel.sub_mut("advanced").unwrap().get_mut("shape").unwrap().value = Value::Enum(1);
                }
                texts.push(("C", "Ramped colour", vec![cc], [200.0, 600.0]));
                let mut dd = anim(&mut ids, "Track", &["tracking", "blur"]);
                if let Some(pg) = dd.sub_mut("properties") {
                    pg.get_mut("tracking").unwrap().value = Value::Scalar(30.0);
                    pg.get_mut("blur").unwrap().value = Value::Vec2([4.0, 4.0]);
                }
                if let Some(sels) = dd.sub_mut("selectors") {
                    if let Some(Node::Group(sel)) = sels.children.first_mut() {
                        sel.sub_mut("advanced").unwrap().get_mut("units").unwrap().value = Value::Enum(1);
                        sel.get_mut("start").unwrap().value = Value::Scalar(2.0);
                        sel.get_mut("end").unwrap().value = Value::Scalar(5.0);
                    }
                    let mut second = build::range_selector(&mut ids, "Range Selector 2");
                    second.get_mut("start").unwrap().value = Value::Scalar(80.0);
                    second.sub_mut("advanced").unwrap().get_mut("mode").unwrap().value = Value::Enum(1);
                    sels.children.push(second.into());
                }
                texts.push(("D", "Tracked blurred", vec![dd], [1000.0, 600.0]));
            }
            p.next_id = next;
            for (name, text, anims, pos) in texts {
                let mut d = TextDoc::plain(text);
                d.set_runs(vec![TStyleRun { len: text.chars().count(), style: st.clone() }]);
                add_layer(&mut p, c, name, LayerSource::Text, (1920, 1080));
                with_layer(&mut p, c, 0, |l| {
                    if let Some(stp) = l.props.prop_mut("text/sourceText") {
                        stp.value = Value::Text(Box::new(d));
                    }
                    l.props.sub_mut("text").unwrap().sub_mut("animators").unwrap().children = anims.into_iter().map(Node::Group).collect();
                    l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([pos[0], pos[1], 0.0]);
                });
            }
        }
        "o2" => {
            // Shape operators and keyed shape properties: four shape layers on a dark solid.
            // A: star (7 points, rotation 15) + gradient fill radial + stroke; B: rectangle with
            // round corners 40, trim paths (End keyed 0 -> 100 over 2 s, start 10), stroke 8;
            // C: two rectangles merged (subtract) + offset paths -10 round + fill with keyed
            // colour; D: a small rectangle repeated 5 times (position 90 / 0, rotation 10, end
            // opacity 20) with keyed group position and an expression on the repeater copies.
            use effectcraft_project::build;
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 3.0, None);
            let s = add_solid(&mut p, "BG", 1920, 1080, [0.1, 0.1, 0.15], None);
            add_layer(&mut p, c, "BG", LayerSource::Solid { item: s }, (1920, 1080));
            let mut next = p.next_id;
            let mut layers: Vec<(&str, PropGroup, [f64; 2])> = Vec::new();
            {
                let mut ids = build::Ids(&mut next);
                let mut star = build::shape_star(&mut ids, true, 7.0, [0.0, 0.0], 180.0, 80.0);
                star.get_mut("rotation").unwrap().value = Value::Scalar(15.0);
                let gfill = build::shape_gradient_fill(&mut ids, true, [0.0, 0.0], [180.0, 0.0], effectcraft_keyframe::Gradient::default());
                let stroke = build::shape_stroke(&mut ids, [1.0, 0.5, 0.0, 1.0], 6.0);
                layers.push(("A", build::shape_group(&mut ids, "Group 1", vec![star, stroke, gfill]), [480.0, 270.0]));

                let rect = build::shape_rect(&mut ids, [500.0, 300.0], [0.0, 0.0], 0.0);
                let mut round = build::shape_simple_op(&mut ids, "round").unwrap();
                round.get_mut("radius").unwrap().value = Value::Scalar(40.0);
                let mut trim = build::shape_trim(&mut ids, 10.0, 100.0, 0.0);
                trim.get_mut("end").unwrap().keys = vec![key(0.0, Value::Scalar(0.0)), key(2.0, Value::Scalar(100.0))];
                let stroke = build::shape_stroke(&mut ids, [0.2, 0.8, 1.0, 1.0], 8.0);
                layers.push(("B", build::shape_group(&mut ids, "Group 1", vec![rect, round, trim, stroke]), [1440.0, 270.0]));

                let r1 = build::shape_rect(&mut ids, [400.0, 300.0], [0.0, 0.0], 0.0);
                let r2 = build::shape_rect(&mut ids, [200.0, 200.0], [60.0, 40.0], 0.0);
                let mut merge = build::shape_simple_op(&mut ids, "merge").unwrap();
                merge.get_mut("mode").unwrap().value = Value::Enum(2);
                let mut offset = build::shape_simple_op(&mut ids, "offset").unwrap();
                offset.get_mut("amount").unwrap().value = Value::Scalar(-10.0);
                offset.get_mut("join").unwrap().value = Value::Enum(1);
                let mut fill = build::shape_fill(&mut ids, [0.0, 1.0, 0.0, 1.0]);
                fill.get_mut("color").unwrap().keys = vec![key(0.0, Value::Color([1.0, 0.0, 0.0, 1.0])), key(2.0, Value::Color([0.0, 0.0, 1.0, 1.0]))];
                layers.push(("C", build::shape_group(&mut ids, "Group 1", vec![r1, r2, merge, offset, fill]), [480.0, 810.0]));

                let small = build::shape_rect(&mut ids, [60.0, 60.0], [0.0, 0.0], 0.0);
                let fill = build::shape_fill(&mut ids, [1.0, 1.0, 1.0, 1.0]);
                let mut rep = build::shape_repeater(&mut ids, 5.0, [90.0, 0.0]);
                if let Some(t) = rep.sub_mut("transform") {
                    t.get_mut("rotation").unwrap().value = Value::Scalar(10.0);
                    t.get_mut("endOpacity").unwrap().value = Value::Scalar(20.0);
                }
                rep.get_mut("copies").unwrap().expr = Some(Expression { text: "3 + Math.floor(time * 2)".into(), enabled: true });
                let mut g = build::shape_group(&mut ids, "Group 1", vec![small, fill, rep]);
                if let Some(t) = g.sub_mut("transform") {
                    t.get_mut("position").unwrap().keys = vec![key(0.0, Value::Vec2([-200.0, 0.0])), key(2.0, Value::Vec2([0.0, 80.0]))];
                }
                layers.push(("D", g, [1300.0, 810.0]));
            }
            p.next_id = next;
            for (name, g, pos) in layers {
                add_layer(&mut p, c, name, LayerSource::Shape, (1920, 1080));
                with_layer(&mut p, c, 0, |l| {
                    l.props.sub_mut("contents").expect("contents").children = vec![g.into()];
                    l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([pos[0], pos[1], 0.0]);
                });
            }
        }
        "r1" => {
            // Time remapping: a 4 s precomposition with a 100 x 100 square moving left to right,
            // shown four times (rows): reversed, held at 1 s then eased back, frozen at 2.5 s (no
            // keys), and driven by an expression; plus one row remapped on a 200 % stretched layer.
            let mv = add_comp(&mut p, "Move", 1920, 200, r25, 4.0, None);
            let w = add_solid(&mut p, "Square", 100, 100, [1.0, 1.0, 1.0], None);
            add_layer(&mut p, mv, "Square", LayerSource::Solid { item: w }, (100, 100));
            with_layer(&mut p, mv, 0, |l| {
                l.transform_mut().expect("tr").get_mut("position").expect("p").keys =
                    vec![key(0.0, Value::Vec3([50.0, 100.0, 0.0])), key(4.0, Value::Vec3([1870.0, 100.0, 0.0]))];
            });
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let red = add_solid(&mut p, "Red", 1920, 1080, [0.5, 0.0, 0.0], None);
            add_layer(&mut p, c, "BG", LayerSource::Solid { item: red }, (1920, 1080));
            let k = |t: f64, v: f64| key(t, Value::Scalar(v));
            let rows: Vec<(&str, Vec<Keyframe>, f64, Option<&str>, f64)> = vec![
                ("Reverse", vec![k(0.0, 4.0), k(4.0, 0.0)], 0.0, None, 100.0),
                (
                    "HoldEase",
                    vec![
                        k(0.0, 0.0),
                        Keyframe { out_interp: Interp::Hold, ..k(1.0, 1.0) },
                        Keyframe {
                            in_interp: Interp::Bezier,
                            out_interp: Interp::Bezier,
                            in_ease: vec![Ease { speed: 0.0, influence: 0.5 }],
                            out_ease: vec![Ease { speed: 0.0, influence: 0.5 }],
                            ..k(2.0, 3.5)
                        },
                        k(4.0, 0.5),
                    ],
                    0.0,
                    None,
                    100.0,
                ),
                ("Freeze", vec![], 2.5, None, 100.0),
                ("Expr", vec![k(0.0, 0.0), k(4.0, 4.0)], 0.0, Some("time * 0.5"), 100.0),
                ("Stretched", vec![k(0.0, 0.0), k(2.0, 4.0)], 0.0, None, 200.0),
            ];
            for (i, (name, keys, value, expr, stretch)) in rows.into_iter().enumerate() {
                add_layer(&mut p, c, name, LayerSource::Comp { item: mv }, (1920, 200));
                let mut r = effectcraft_project::Property::new(90_000 + i as u64, "timeRemap", "Time Remap", Value::Scalar(value));
                r.keys = keys;
                r.expr = expr.map(|e| Expression { text: e.into(), enabled: true });
                with_layer(&mut p, c, 0, |l| {
                    l.props.children.insert(0, r.into());
                    l.stretch = stretch;
                    l.out_point = secs(5.0);
                    l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([960.0, 108.0 + 216.0 * i as f64, 0.0]);
                });
            }
        }
        "t7" => {
            // Right-to-left paragraphs with every justification, as point text (one line) and as
            // box text (two lines and a second paragraph), to see how After Effects reads the
            // stored code in a right-to-left paragraph.
            use effectcraft_keyframe::text_doc::Direction;
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "BG", 1920, 1080, [0.1, 0.1, 0.3], None);
            add_layer(&mut p, c, "BG", LayerSource::Solid { item: s }, (1920, 1080));
            let st =
                CharStyle { font: "ArialMT".into(), style: "Regular".into(), size: 30.0, fill: [1.0, 1.0, 1.0, 1.0], leading: None, ..CharStyle::default() };
            let js = [
                ("L", Justify::Left),
                ("R", Justify::Right),
                ("C", Justify::Center),
                ("JL", Justify::JustifyLastLeft),
                ("JR", Justify::JustifyLastRight),
                ("JC", Justify::JustifyLastCenter),
                ("JA", Justify::JustifyAll),
            ];
            for (i, (code, j)) in js.into_iter().enumerate() {
                for boxed in [false, true] {
                    let text = if boxed { "One two three four five six seven eight nine\rSecond one" } else { "Right to left" };
                    let mut d = TextDoc::plain(text);
                    d.set_runs(vec![TStyleRun { len: text.chars().count(), style: st.clone() }]);
                    d.set_paras(vec![ParaStyle { justify: j, direction: Direction::Rtl, ..d.base_para() }; if boxed { 2 } else { 1 }]);
                    let name = format!("{}{code}", if boxed { "B" } else { "P" });
                    let pos = if boxed {
                        d.box_size = Some([300.0, 150.0]);
                        d.box_pos = [0.0, 0.0];
                        [1000.0, 60.0 + 145.0 * i as f64]
                    } else {
                        [600.0, 60.0 + 145.0 * i as f64]
                    };
                    add_layer(&mut p, c, &name, LayerSource::Text, (1920, 1080));
                    with_layer(&mut p, c, 0, |l| {
                        if let Some(stp) = l.props.prop_mut("text/sourceText") {
                            stp.value = Value::Text(Box::new(d));
                        }
                        l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([pos[0], pos[1], 0.0]);
                    });
                }
            }
        }
        "g5" => {
            // Working space Rec.709 Gamma 2.4 (our own profile, R49), 16 bpc: a horizontal grey
            // ramp (Gradient Ramp black to white) over a 1920 x 540 solid and a mid-grey and a red
            // solid below.
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let red = add_solid(&mut p, "Red", 960, 540, [1.0, 0.0, 0.0], None);
            let grey = add_solid(&mut p, "Grey", 960, 540, [0.5, 0.5, 0.5], None);
            let ramp = add_solid(&mut p, "Ramp", 1920, 540, [0.0, 0.0, 0.0], None);
            add_layer(&mut p, c, "Red", LayerSource::Solid { item: red }, (960, 540));
            with_layer(&mut p, c, 0, |l| l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([1440.0, 810.0, 0.0]));
            add_layer(&mut p, c, "Grey", LayerSource::Solid { item: grey }, (960, 540));
            with_layer(&mut p, c, 0, |l| l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([480.0, 810.0, 0.0]));
            add_layer(&mut p, c, "Ramp", LayerSource::Solid { item: ramp }, (1920, 540));
            with_layer(&mut p, c, 0, |l| l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([960.0, 270.0, 0.0]));
            let mut r = fixture_effect("368-fx-ramp-default.aep");
            set(&mut r, "start", Value::Vec2([0.0, 270.0]));
            set(&mut r, "end", Value::Vec2([1920.0, 270.0]));
            add_effect(&mut p, c, 0, r);
            p.settings.bit_depth = effectcraft_project::BitDepth::Bpc16;
            p.settings.working_space = Some(effectcraft_color::ColorSpace::Rec709);
        }
        "g1" | "g2" | "g3" | "g4" => {
            // Project settings: g1 8 bpc, no working space, gamma 2.4, frames from 1; g2 16 bpc,
            // sRGB, linearize + blend linear, feet + frames 35 mm; g3 32 bpc, sRGB, feet + frames 16 mm.
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let s = add_solid(&mut p, "L", 1920, 1080, [0.5, 0.25, 0.75], None);
            add_layer(&mut p, c, "L", LayerSource::Solid { item: s }, (1920, 1080));
            use effectcraft_project::{BitDepth, TimeDisplayStyle};
            let st = &mut p.settings;
            match case {
                "g1" => {
                    st.bit_depth = BitDepth::Bpc8;
                    st.working_space = None;
                    st.working_gamma = 2.4;
                    st.time_display = TimeDisplayStyle::Frames;
                    st.frame_start = 1;
                }
                "g2" => {
                    st.bit_depth = BitDepth::Bpc16;
                    st.working_space = Some(effectcraft_color::ColorSpace::Srgb);
                    st.linearize = true;
                    st.blend_linear = true;
                    st.time_display = TimeDisplayStyle::Feet35;
                }
                _ => {
                    st.bit_depth = BitDepth::Bpc32;
                    st.working_space = Some(effectcraft_color::ColorSpace::Srgb);
                    st.time_display = TimeDisplayStyle::Feet16;
                }
            }
        }
        "s2" => {
            // Keys on stretched layers: three 200 x 200 solids moving left to right, stretched
            // 200 % (keys at layer 0 / 2 s), 50 % (keys at layer 0 / 4 s) and 150 % from 0.4 s
            // (keys at layer 1 / 3 s).
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 10.0, None);
            for (i, (name, stretch, start, k0, k1, y)) in
                [("S200", 200.0, 0.0, 0.0, 2.0, 200.0), ("S50", 50.0, 0.0, 0.0, 4.0, 540.0), ("S150", 150.0, 0.4, 1.0, 3.0, 880.0)].into_iter().enumerate()
            {
                let s = add_solid(&mut p, name, 200, 200, [[1.0, 0.2, 0.2], [0.2, 1.0, 0.2], [0.2, 0.4, 1.0]][i], None);
                add_layer(&mut p, c, name, LayerSource::Solid { item: s }, (200, 200));
                with_layer(&mut p, c, 0, |l| {
                    l.stretch = stretch;
                    l.start_time = secs(start);
                    l.in_point = secs(0.0);
                    l.out_point = secs(10.0);
                    let pos = l.transform_mut().expect("tr").get_mut("position").expect("p");
                    pos.keys = vec![key(k0, Value::Vec3([200.0, y, 0.0])), key(k1, Value::Vec3([1700.0, y, 0.0]))];
                    pos.value = pos.keys[0].value.clone();
                });
            }
        }
        "f3" => {
            // The kinds added in round 2: tga, flat psd, ai and eps stills and the 25 fps mp4
            // conformed to 30 fps (25 frames in 0.8333 s).
            let c = add_comp(&mut p, "C", 1920, 1080, r25, 5.0, None);
            let red = add_solid(&mut p, "L", 1920, 1080, [1.0, 0.0, 0.0], None);
            add_layer(&mut p, c, "L", LayerSource::Solid { item: red }, (1920, 1080));
            let syn = std::env::var("AEP_SYNTHETIC").unwrap_or_else(|_| r"C:ixtures\".to_string());
            let media = format!(r"{syn}test-media\");
            let r30 = FrameRate::new(30, 1);
            let conformed = Footage {
                kind: FootageKind::Video,
                frame_rate: r30,
                native_rate: Some(r25),
                duration: r30.tick_of(25),
                has_video: true,
                ..still(&format!("{media}clip.mp4"), 64, 48)
            };
            let mut items: Vec<(String, Footage, f64)> =
                ["tga", "psd", "ai", "eps"].iter().map(|e| (format!("still.{e}"), still(&format!("{media}still.{e}"), 64, 48), 5.0)).collect();
            items.push(("clip.mp4".to_string(), conformed, 25.0 / 30.0));
            for (i, (name, f, dur)) in items.into_iter().enumerate() {
                let size = (f.width.max(1), f.height.max(1));
                let id = p.add_item(&name, Label::Lavender, None, ItemKind::Footage(f));
                add_layer(&mut p, c, &name, LayerSource::Footage { item: id }, size);
                let x = 100.0 + 200.0 * i as f64;
                with_layer(&mut p, c, 0, |l| {
                    l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([x, 100.0, 0.0]);
                    l.out_point = secs(dur);
                });
            }
        }
        "o1" => {
            // Round 2 objects: shapes, a luma track matte and markers in a 2D composition; a
            // camera, a spot and a point light over a 3D card in a second one.
            use effectcraft_keyframe::ShapePath;
            use effectcraft_project::{CuePoint, LightKind, Marker, MatteKind, TrackMatte};
            let c = add_comp(&mut p, "Shapes", 1920, 1080, r25, 5.0, None);
            let grey = add_solid(&mut p, "BG", 1920, 1080, [0.3, 0.3, 0.3], None);
            add_layer(&mut p, c, "BG", LayerSource::Solid { item: grey }, (1920, 1080));
            with_layer(&mut p, c, 0, |l| {
                l.markers = vec![
                    Marker { time: secs(1.0), comment: "LM".into(), duration: secs(1.0), chapter: "Chap".into(), label: Label::Aqua, ..Default::default() },
                    Marker {
                        time: secs(2.0),
                        comment: "Cue".into(),
                        cue_point: Some(CuePoint { name: "CueX".into(), navigation: true, params: vec![("k".into(), "v".into())] }),
                        ..Default::default()
                    },
                ];
            });
            let blue = add_solid(&mut p, "Target", 600, 400, [0.2, 0.4, 1.0], None);
            add_layer(&mut p, c, "Target", LayerSource::Solid { item: blue }, (600, 400));
            let white = add_solid(&mut p, "MatteSolid", 300, 300, [1.0, 1.0, 1.0], None);
            add_layer(&mut p, c, "MatteSolid", LayerSource::Solid { item: white }, (300, 300));
            for i in 0..2 {
                with_layer(&mut p, c, i, |l| l.transform_mut().expect("tr").get_mut("position").expect("p").value = Value::Vec3([1400.0, 750.0, 0.0]));
            }
            let matte_id = p.comp(c).expect("c").layers[0].id;
            with_layer(&mut p, c, 1, |l| l.track_matte = Some(TrackMatte { layer: matte_id, kind: MatteKind::Luma }));
            add_layer(&mut p, c, "S", LayerSource::Shape, (1920, 1080));
            let mut next = p.next_id;
            {
                let mut ids = build::Ids(&mut next);
                let rect = build::shape_rect(&mut ids, [400.0, 300.0], [10.0, -20.0], 15.0);
                let mut fill = build::shape_fill(&mut ids, [0.0, 1.0, 0.0, 1.0]);
                fill.get_mut("opacity").expect("o").value = Value::Scalar(50.0);
                let mut stroke = build::shape_stroke(&mut ids, [1.0, 0.0, 0.0, 1.0], 6.0);
                stroke.get_mut("join").expect("j").value = Value::Enum(1);
                let mut g1 = build::shape_group(&mut ids, "Group 1", vec![rect, stroke, fill]);
                if let Some(t) = g1.sub_mut("transform") {
                    t.get_mut("position").expect("pos").value = Value::Vec2([-400.0, -200.0]);
                    t.get_mut("scale").expect("scale").value = Value::Vec2([50.0, 80.0]);
                    t.get_mut("rotation").expect("rot").value = Value::Scalar(30.0);
                    t.get_mut("opacity").expect("op").value = Value::Scalar(80.0);
                }
                let ellipse = build::shape_ellipse(&mut ids, [500.0, 200.0], [100.0, 50.0]);
                let path = build::shape_path(
                    &mut ids,
                    ShapePath {
                        vertices: vec![[0.0, -100.0], [100.0, 100.0], [-100.0, 100.0]],
                        in_tangents: vec![[-10.0, -20.0], [30.0, 20.0], [-60.0, 10.0]],
                        out_tangents: vec![[40.0, 0.0], [-20.0, 30.0], [0.0, -50.0]],
                        closed: true,
                        feather: Vec::new(),
                    },
                );
                let mut f2 = build::shape_fill(&mut ids, [1.0, 0.8, 0.0, 1.0]);
                f2.get_mut("rule").expect("r").value = Value::Enum(1);
                let blue_fill = build::shape_fill(&mut ids, [0.0, 0.0, 1.0, 1.0]);
                let inner = build::shape_group(&mut ids, "Inner", vec![ellipse, blue_fill]);
                let mut g2 = build::shape_group(&mut ids, "Group 2", vec![path, f2]);
                if let Some(t) = g2.sub_mut("transform") {
                    t.get_mut("position").expect("pos").value = Value::Vec2([300.0, 100.0]);
                }
                with_layer(&mut p, c, 0, |l| {
                    l.props.sub_mut("contents").expect("contents").children = vec![g1.into(), inner.into(), g2.into()];
                });
            }
            p.next_id = next;
            with_comp(&mut p, c, |cc| {
                cc.markers = vec![
                    Marker { time: secs(1.0), comment: "M1".into(), label: Label::Yellow, ..Default::default() },
                    Marker { time: secs(2.5), comment: "M2".into(), duration: secs(0.5), label: Label::None, ..Default::default() },
                ];
            });
            // 3D: a card lit by a spot and a point light, seen by a two-node camera.
            let d = add_comp(&mut p, "Scene", 1920, 1080, r25, 5.0, None);
            let card = add_solid(&mut p, "Card", 800, 600, [0.8, 0.8, 0.8], None);
            add_layer(&mut p, d, "Card", LayerSource::Solid { item: card }, (800, 600));
            with_layer(&mut p, d, 0, |l| l.switches.three_d = true);
            add_layer(&mut p, d, "Point", LayerSource::Light { kind: LightKind::Point }, (1920, 1080));
            with_layer(&mut p, d, 0, |l| {
                l.props.prop_mut("lightOptions/intensity").expect("i").value = Value::Scalar(50.0);
                l.props.prop_mut("transform/position").expect("p").value = Value::Vec3([1500.0, 300.0, -600.0]);
            });
            add_layer(&mut p, d, "Spot", LayerSource::Light { kind: LightKind::Spot }, (1920, 1080));
            with_layer(&mut p, d, 0, |l| {
                l.props.prop_mut("lightOptions/intensity").expect("i").value = Value::Scalar(80.0);
                l.props.prop_mut("lightOptions/color").expect("c").value = Value::Color([1.0, 0.8, 0.6, 1.0]);
                l.props.prop_mut("lightOptions/coneAngle").expect("a").value = Value::Scalar(60.0);
                l.props.prop_mut("lightOptions/coneFeather").expect("f").value = Value::Scalar(30.0);
                l.props.prop_mut("transform/position").expect("p").value = Value::Vec3([600.0, 300.0, -800.0]);
            });
            add_layer(&mut p, d, "Cam", LayerSource::Camera, (1920, 1080));
            with_layer(&mut p, d, 0, |l| {
                l.props.prop_mut("transform/position").expect("p").value = Value::Vec3([960.0, 540.0, -2000.0]);
                l.props.prop_mut("cameraOptions/zoom").expect("z").value = Value::Scalar(2000.0);
                l.props.prop_mut("cameraOptions/dof").expect("d").value = Value::Bool(true);
                l.props.prop_mut("cameraOptions/focusDistance").expect("f").value = Value::Scalar(1800.0);
                l.props.prop_mut("cameraOptions/aperture").expect("a").value = Value::Scalar(30.0);
            });
        }
        _ => {
            eprintln!("unknown case");
            std::process::exit(2);
        }
    }
    let res = export(&p, &ExportOptions { allow_losses: true }).expect("export");
    eprintln!("{}", serde_json::to_string(&res.report.losses).unwrap_or_default());
    let mut bytes = res.bytes;
    if case == "g4" {
        // Experiment: a working space named without its profile data (does After Effects find
        // the profile by name?). g4 = g3 with the JSON after PwCs replaced.
        let mut f = effectcraft_aep::parse(&bytes).expect("parse").into_owned();
        let at = f.chunks.iter().position(|c| c.is(b"PwCs")).expect("PwCs") + 1;
        f.chunks[at] = effectcraft_aep::Chunk::data(*b"Utf8", br#"{"baseColorProfile":{"colorProfileName":"Rec.709 Gamma 2.4"},"baseProfileType":2}"#.to_vec());
        bytes = effectcraft_aep::write(&f).expect("write");
    }
    std::fs::write(&a[2], bytes).expect("write");
    // The EffectCraft project next to it (for renders of the same scene).
    std::fs::write(std::path::Path::new(&a[2]).with_extension("ecproj"), p.to_json()).expect("write project");
}
