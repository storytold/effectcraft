//! Developer lab: exports small projects built in code, for opening in After Effects.
//!
//! `cargo run --release -p effectcraft-aep-export --example lab -- <case> <out.aep>`
use effectcraft_aep_export::{ExportOptions, export};
use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Ease, Interp, Keyframe, Value};
use effectcraft_project::{Comp, Expression, ItemId, ItemKind, LayerSource, Project, Quality, Solid, build};
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
        _ => {
            eprintln!("unknown case");
            std::process::exit(2);
        }
    }
    let res = export(&p, &ExportOptions { allow_losses: true }).expect("export");
    eprintln!("{}", serde_json::to_string(&res.report.losses).unwrap_or_default());
    std::fs::write(&a[2], res.bytes).expect("write");
}
