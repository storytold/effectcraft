//! M13.6 panels and tools through the commands: Progress (jobs), Media Browser, Metadata,
//! Lumetri Scopes, the Footage panel's Overlay / Ripple Insert edits, Content-Aware Fill, Scene
//! Edit Detection, Auto-trace and Align Video to Data.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use effectcraft_color::Label;
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, LayerId, LayerSource, build};
use effectcraft_raster::Image;
use effectcraft_raster::inpaint::psnr;
use effectcraft_render::FootageSource;
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use crate::Session;

const FPS: u32 = 25;

/// Footage computed per frame.
struct Gen {
    make: Box<dyn Fn(u32) -> Image + Send + Sync>,
    cache: Mutex<HashMap<u32, Arc<Image>>>,
}

impl FootageSource for Gen {
    fn frame(&self, _: ItemId, _: &Footage, t: Tick) -> Option<Arc<Image>> {
        let f = (t.seconds() * FPS as f64).round().max(0.0) as u32;
        let mut c = self.cache.lock().unwrap();
        Some(c.entry(f).or_insert_with(|| Arc::new((self.make)(f))).clone())
    }
}

/// A comp `w × h`, `secs` long, with one footage layer drawn by `make` (returns session, comp,
/// layer, footage item).
fn setup(w: u32, h: u32, secs: f64, make: impl Fn(u32) -> Image + Send + Sync + 'static) -> (Session, ItemId, LayerId, ItemId) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Panels", "width": w, "height": h, "frameRate": FPS, "duration": secs})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let (lid, item) = s
        .edit("clip", None, |p, _| {
            let footage = Footage {
                path: "synthetic.mov".into(),
                kind: FootageKind::Video,
                width: w,
                height: h,
                frame_rate: FrameRate::new(FPS as i64, 1),
                duration: Tick::from_seconds_f64(secs),
                has_video: true,
                codec: "SYN".into(),
                ..Default::default()
            };
            let item = p.add_item("clip", Label::Aqua, None, ItemKind::Footage(footage));
            let comp = p.comp(cid).unwrap().clone();
            let l = build::layer(p, &comp, "Clip", LayerSource::Footage { item }, (w, h), None);
            let id = l.id;
            p.comp_mut(cid).unwrap().layers.insert(0, l);
            Ok((id, item))
        })
        .unwrap();
    s.footage = Arc::new(Gen { make: Box::new(make), cache: Mutex::default() });
    s.execute("layer.select", json!({"layers": [lid.0]})).unwrap();
    (s, cid, lid, item)
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ec-panels-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// ---------------------------------------------------------------- Progress

#[test]
fn progress_lists_and_cancels_jobs() {
    let mut s = Session::default();
    let r = s
        .spawn_task("test", "Endless job", false, |ctl| {
            let mut k = 0;
            while ctl.progress(k % 100, 100) {
                std::thread::sleep(std::time::Duration::from_millis(2));
                k += 1;
            }
            Err(crate::render_queue::CANCELLED.into())
        })
        .unwrap();
    let id = r["job"].as_str().unwrap().to_string();
    let listed = s.execute("jobs.list", json!({})).unwrap();
    let running = listed["running"].as_array().unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0]["id"], json!(id));
    assert_eq!(running[0]["label"], json!("Endless job"));
    assert_eq!(running[0]["cancellable"], json!(true));
    assert_eq!(s.execute("jobs.cancel", json!({"job": id})).unwrap()["cancelled"], json!(true));
    s.execute("jobs.wait", json!({})).unwrap();
    assert!(s.jobs().is_empty());
    assert_eq!(s.job_log.last().unwrap().status, "cancelled");
    // A finished job applies its edit (one undo step) and reports its result.
    let v = s
        .spawn_task("test", "Quick job", true, |ctl| {
            ctl.progress(1, 1);
            let apply: crate::jobs::Apply = Box::new(|s: &mut Session| {
                s.edit("Quick", None, |p, _| {
                    p.settings.comment = "done".into();
                    Ok(())
                })?;
                Ok(json!(42))
            });
            Ok(apply)
        })
        .unwrap();
    assert_eq!(v, json!(42));
    assert_eq!(s.project.settings.comment, "done");
    assert_eq!(s.job_log.last().unwrap().status, "done");
    assert!(s.undo());
    assert_eq!(s.project.settings.comment, "");
}

// ---------------------------------------------------------------- Media Browser & Metadata

#[test]
fn media_browser_and_metadata() {
    let dir = tmp("browser");
    std::fs::create_dir_all(dir.join("Footage")).unwrap();
    std::fs::write(dir.join("shot.mov"), b"0123").unwrap();
    std::fs::write(dir.join("data.csv"), "time,v\n0,1\n").unwrap();
    std::fs::write(dir.join("readme.txt"), "x").unwrap();
    let d = dir.to_string_lossy().to_string();
    let mut s = Session::default();
    let r = s.execute("mediaBrowser.go", json!({"path": d})).unwrap();
    let names: Vec<&str> = r["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Footage", "data.csv", "readme.txt", "shot.mov"]);
    assert_eq!(s.state.media_browser.folder.as_deref(), Some(d.as_str()));
    let r = s.execute("mediaBrowser.list", json!({"importableOnly": true})).unwrap();
    assert_eq!(r["entries"].as_array().unwrap().len(), 3);
    s.execute("mediaBrowser.addFavorite", json!({})).unwrap();
    assert_eq!(s.state.media_browser.favorites, vec![d.clone()]);
    s.execute("mediaBrowser.go", json!({"path": dir.join("Footage").to_string_lossy()})).unwrap();
    s.execute("mediaBrowser.go", json!({"path": ".."})).unwrap();
    assert_eq!(s.state.media_browser.folder.as_deref(), Some(d.as_str()));
    s.execute("mediaBrowser.removeFavorite", json!({"path": d})).unwrap();
    assert!(s.state.media_browser.favorites.is_empty());
    // Import a data file from the browser; its metadata reads back.
    let r = s.execute("mediaBrowser.import", json!({"paths": [dir.join("data.csv").to_string_lossy()]})).unwrap();
    let id = r["items"][0].as_u64().unwrap();
    let m = s.execute("item.metadata", json!({"item": id})).unwrap();
    assert_eq!(m["item"]["type"], json!("Data"));
    assert_eq!(m["item"]["codec"], json!("CSV"));
    assert_eq!(m["item"]["dataRows"], json!(1));
    assert!(m["item"]["modified"].as_str().unwrap().ends_with('Z'));
    assert_eq!(m["project"]["footage"], json!(1));
    // Editable comments: item (undoable) and project.
    s.execute("project.setComment", json!({"items": [id], "comment": "telemetry"})).unwrap();
    s.execute("project.setProjectComment", json!({"comment": "client cut"})).unwrap();
    let m = s.execute("item.metadata", json!({"item": id})).unwrap();
    assert_eq!(m["item"]["comment"], json!("telemetry"));
    assert_eq!(m["project"]["comment"], json!("client cut"));
    s.undo();
    assert_eq!(s.project.settings.comment, "");
    let fi = s.execute("mediaBrowser.fileInfo", json!({"path": dir.join("shot.mov").to_string_lossy()})).unwrap();
    assert_eq!(fi["size"], json!(4));
    assert_eq!(fi["kind"], json!("video"));
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------- Lumetri Scopes

#[test]
fn scopes_read_the_comp_frame() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 48, "frameRate": 25, "duration": 1})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    let v = s.execute("scopes.analyze", json!({"scope": "vectorscopeYuv", "size": 101})).unwrap();
    let st = effectcraft_raster::scopes::ColorStandard::Rec709;
    let [_, cb, cr] = st.ycbcr([1.0, 0.0, 0.0]);
    assert_eq!(v["peaks"][0], json!([((cb + 0.5) * 100.0).round() as u32, ((0.5 - cr) * 100.0).round() as u32]));
    let h = s.execute("scopes.analyze", json!({"scope": "histogram", "standard": "rec601"})).unwrap();
    let bars = &h["histogram"];
    assert_eq!(bars[0][255], json!(64 * 48));
    assert_eq!(bars[1][0], json!(64 * 48));
    assert!((h["luma"]["mean"].as_f64().unwrap() - 0.299).abs() < 1e-3);
    assert!(s.execute("scopes.analyze", json!({"scope": "bogus"})).is_err());
}

// ---------------------------------------------------------------- Footage panel

#[test]
fn footage_panel_overlay_keeps_full_length_past_the_comp_end() {
    let (mut s, cid, _clip, item) = setup(32, 24, 8.0, |_| Image::filled(32, 24, [0.5, 0.5, 0.5, 1.0]));
    s.execute("footage.open", json!({"item": item.0})).unwrap();
    let r = s.execute("footage.overlayEdit", json!({"item": item.0, "in": 0.0, "out": 4.0, "at": 6.0})).unwrap();
    let ov = LayerId(r["layer"].as_u64().unwrap());
    let l = s.project.comp(cid).unwrap().layer(ov).unwrap();
    assert_eq!((l.in_point.seconds(), l.out_point.seconds()), (6.0, 10.0));
}

#[test]
fn footage_panel_overlay_and_ripple_insert() {
    let (mut s, cid, clip, item) = setup(32, 24, 8.0, |_| Image::filled(32, 24, [0.5, 0.5, 0.5, 1.0]));
    s.execute("footage.open", json!({"item": item.0})).unwrap();
    assert!(
        s.drain_events().iter().any(|e| matches!(e, crate::Event::Frontend { command, params } if command == "window.panel" && params["panel"] == "footage"))
    );
    s.execute("footage.setTime", json!({"time": 1.0})).unwrap();
    s.execute("footage.setIn", json!({})).unwrap();
    s.execute("footage.setOut", json!({"frame": 49})).unwrap();
    let info = s.execute("footage.info", json!({})).unwrap();
    assert_eq!(info["in"], json!(1.0));
    assert_eq!(info["out"], json!(2.0));
    // Overlay Edit at 0.6 s: a 1 s layer on top showing source 1–2 s.
    s.set_time(Tick::from_seconds_f64(0.6));
    let r = s.execute("footage.overlayEdit", json!({})).unwrap();
    let ov = LayerId(r["layer"].as_u64().unwrap());
    let comp = s.project.comp(cid).unwrap();
    assert_eq!(comp.layers[0].id, ov);
    let l = comp.layer(ov).unwrap();
    assert_eq!((l.in_point.seconds(), l.out_point.seconds(), l.start_time.seconds()), (0.6, 1.6, -0.4));
    assert_eq!(comp.layer(clip).unwrap().out_point.seconds(), 8.0, "overlay leaves other layers alone");
    // Ripple Insert at 3 s: layers crossing 3 s are split, the rest moves back by 1 s.
    s.set_time(Tick::from_seconds_f64(3.0));
    let r = s.execute("footage.rippleInsertEdit", json!({})).unwrap();
    let ins = LayerId(r["layer"].as_u64().unwrap());
    let comp = s.project.comp(cid).unwrap();
    assert_eq!(comp.layers.len(), 4, "overlay, insert, clip split in two");
    let l = comp.layer(ins).unwrap();
    assert_eq!((l.in_point.seconds(), l.out_point.seconds()), (3.0, 4.0));
    let head = comp.layer(clip).unwrap();
    assert_eq!((head.in_point.seconds(), head.out_point.seconds()), (0.0, 3.0));
    let tail = comp.layers.iter().find(|x| x.id != clip && x.id != ins && x.id != ov).unwrap();
    assert_eq!((tail.in_point.seconds(), tail.out_point.seconds(), tail.start_time.seconds()), (4.0, 9.0, 1.0));
    // Source time continuity: the tail shows source 3 s at comp 4 s.
    assert_eq!(tail.layer_time(Tick::from_seconds_f64(4.0)).seconds(), 3.0);
    // The overlay layer (in before 3 s, out before 3 s) didn't move.
    assert_eq!(comp.layer(ov).unwrap().in_point.seconds(), 0.6);
    // One undo step each.
    assert!(s.undo());
    assert_eq!(s.project.comp(cid).unwrap().layers.len(), 2);
    assert!(s.execute("footage.setOut", json!({"time": 0.5})).is_ok());
    assert_eq!(s.state.footage_panel.as_ref().unwrap().in_point, None, "an Out before the In clears the In");
}

// ---------------------------------------------------------------- Content-Aware Fill

fn bg(x: f32, y: f32) -> [f32; 4] {
    let s = 0.5 + 0.3 * (x * std::f32::consts::TAU / 10.0).sin();
    let n = effectcraft_raster::hash_noise(((x.floor() as i64).rem_euclid(20) / 4) as u32, ((y as u32) % 20) / 4, 3);
    [s, 0.3 + 0.4 * n, 0.6 - 0.3 * s * n, 1.0]
}

/// Panning textured background with a red "object" walking through the masked region.
fn caf_frame(f: u32, with_object: bool) -> Image {
    let (w, h) = (96u32, 64u32);
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut p = bg(x as f32 + 2.0 * f as f32, y as f32);
            let ox = 40 + (f % 4);
            if with_object && (ox..ox + 10).contains(&x) && (26..36).contains(&y) {
                p = [0.9, 0.1, 0.1, 1.0];
            }
            img.set(x, y, p);
        }
    }
    img
}

#[test]
fn content_aware_fill_removes_an_object() {
    let (mut s, cid, clip, _) = setup(96, 64, 0.48, |f| caf_frame(f, true));
    // Cut the object out with a Subtract mask; fill the work area (12 frames).
    s.execute("layer.addMask", json!({"rect": [36.0, 22.0, 20.0, 18.0], "mode": "subtract"})).unwrap();
    let out = tmp("caf");
    s.execute("contentFill.set", json!({"method": "object", "range": "workArea", "alphaExpansion": 1})).unwrap();
    let r = s.execute("contentFill.generate", json!({"outputDir": out.to_string_lossy(), "wait": true})).unwrap();
    let fill = LayerId(r["layer"].as_u64().unwrap());
    let files: Vec<String> = r["files"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(files.len(), 12);
    // The Fill layer sits right above the source with the source's timing.
    let comp = s.project.comp(cid).unwrap();
    let i = comp.layers.iter().position(|l| l.id == fill).unwrap();
    assert_eq!(comp.layers[i + 1].id, clip);
    let fl = comp.layer(fill).unwrap();
    assert_eq!((fl.in_point, fl.out_point.seconds()), (Tick::ZERO, 0.48));
    let LayerSource::Footage { item } = fl.source else { panic!("footage") };
    let Some(ItemKind::Footage(f)) = s.project.item(item).map(|i| &i.kind) else { panic!() };
    assert_eq!((f.kind, f.sequence.len()), (FootageKind::Sequence, 12));
    assert!(s.project.item(item).unwrap().name.starts_with("Fill 1"));
    // The filled frames match the clean background inside the mask, and stay steady over time.
    let region: Vec<bool> = (0..96 * 64).map(|i| (36..56).contains(&(i % 96)) && (22..40).contains(&(i / 96))).collect();
    let mut prev: Option<Image> = None;
    for (k, path) in files.iter().enumerate() {
        let png = image::open(path).unwrap().to_rgba8();
        let img = Image::from_rgba8(96, 64, png.as_raw());
        let truth = caf_frame(k as u32, false);
        let db = psnr(&img, &truth, Some(&region));
        assert!(db > 25.0, "frame {k}: {db:.1} dB");
        // Outside the hole the frame is the source itself.
        let outside: Vec<bool> = region.iter().map(|r| !r).collect();
        assert!(psnr(&img, &caf_frame(k as u32, true), Some(&outside)) > 40.0);
        if let Some(p) = &prev {
            // Temporal consistency: the change in the hole follows the 2 px/frame pan (compare
            // against the previous fill shifted).
            let mut shifted = p.clone();
            for y in 0..64 {
                for x in 0..94 {
                    shifted.data[y * 96 + x] = p.data[y * 96 + x + 2];
                }
            }
            let inner: Vec<bool> = (0..96 * 64).map(|i| (38..54).contains(&(i % 96)) && (24..38).contains(&(i / 96))).collect();
            assert!(psnr(&img, &shifted, Some(&inner)) > 24.0, "frame {k} jumps");
        }
        prev = Some(img);
    }
    // One undo step removes the fill layer.
    assert!(s.undo());
    assert!(s.project.comp(cid).unwrap().layer(fill).is_none());
    std::fs::remove_dir_all(&out).unwrap();
    // Without a hole there is nothing to fill.
    let (mut s2, _, _, _) = setup(32, 24, 0.2, |_| Image::filled(32, 24, [0.5, 0.5, 0.5, 1.0]));
    assert!(s2.execute("contentFill.generate", json!({"wait": true, "outputDir": tmp("caf2").to_string_lossy()})).is_err());
}

#[test]
fn content_aware_fill_runs_in_the_background() {
    let (mut s, _, _, _) = setup(48, 32, 0.2, |f| {
        let mut img = Image::new(48, 32);
        for y in 0..32 {
            for x in 0..48 {
                img.set(x, y, bg(x as f32 + f as f32, y as f32));
            }
        }
        img
    });
    s.execute("layer.addMask", json!({"rect": [20.0, 10.0, 8.0, 8.0], "mode": "subtract"})).unwrap();
    let out = tmp("cafbg");
    let r = s.execute("contentFill.generate", json!({"method": "edgeBlend", "outputDir": out.to_string_lossy()})).unwrap();
    assert!(r["job"].as_str().unwrap().starts_with("task:"));
    assert_eq!(s.jobs().len(), 1);
    assert_eq!(s.jobs()[0].kind, "contentFill");
    s.wait_jobs();
    let rec = s.job_log.last().unwrap();
    assert_eq!(rec.status, "done", "{}", rec.message);
    assert_eq!(rec.result["frames"], json!(5));
    std::fs::remove_dir_all(&out).unwrap();
}

/// The browser: Content-Aware Fill goes to a job worker (its files written through the
/// worker's services), and the page adds the fill layer when the plan comes back.
#[test]
fn content_aware_fill_runs_in_a_job_worker() {
    #[derive(Default)]
    struct Manual(std::sync::Mutex<Vec<(crate::offload::WorkerRequest, Arc<crate::offload::Inbox>)>>);
    impl crate::offload::Offload for Manual {
        fn start(&self, req: crate::offload::WorkerRequest, inbox: Arc<crate::offload::Inbox>) -> std::result::Result<(), String> {
            self.0.lock().unwrap().push((req, inbox));
            Ok(())
        }
        fn cancel(&self, _: u64) {}
    }
    let (mut s, cid, clip, _) = setup(48, 32, 0.2, |f| {
        let mut img = Image::new(48, 32);
        for y in 0..32 {
            for x in 0..48 {
                img.set(x, y, bg(x as f32 + f as f32, y as f32));
            }
        }
        img
    });
    let off = Arc::new(Manual::default());
    s.offload = Some(off.clone());
    s.execute("layer.addMask", json!({"rect": [20.0, 10.0, 8.0, 8.0], "mode": "subtract"})).unwrap();
    let out = tmp("cafworker");
    let r = s.execute("contentFill.generate", json!({"method": "edgeBlend", "outputDir": out.to_string_lossy()})).unwrap();
    assert_eq!((r["job"].as_str(), r["worker"].as_bool()), (Some("contentFill"), Some(true)), "{r}");
    assert!(s.jobs().iter().any(|j| j.id == "contentFill"));
    let (req, inbox) = off.0.lock().unwrap().pop().unwrap();
    let back: crate::offload::WorkerRequest = serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
    // The worker: the same footage, the default (file system) services.
    let mut w = Session { footage: s.footage.clone(), ..Default::default() };
    let post: crate::offload::Post = std::rc::Rc::new(move |r| inbox.push(r));
    crate::offload::run_request(&mut w, back, &post);
    s.poll_offload();
    let comp = s.project.comp(cid).unwrap().clone();
    let i = comp.layers.iter().position(|l| l.id == clip).unwrap();
    assert!(i > 0, "a fill layer above the source");
    let fill = &comp.layers[i - 1];
    assert!(fill.name.starts_with("Fill 1"), "{}", fill.name);
    let LayerSource::Footage { item } = fill.source else { panic!("footage") };
    let Some(ItemKind::Footage(f)) = s.project.item(item).map(|i| &i.kind) else { panic!() };
    assert_eq!(f.sequence.len(), 5);
    assert!(f.sequence.iter().all(|p| std::path::Path::new(p).exists()));
    assert!(s.jobs().is_empty());
    // One undo step.
    assert!(s.undo());
    assert_eq!(s.project.comp(cid).unwrap().layers.len(), comp.layers.len() - 1);
    std::fs::remove_dir_all(&out).unwrap();
}

// ---------------------------------------------------------------- Scene Edit Detection

fn shot_frame(f: u32) -> Image {
    // Shots: frames 0–9, 10–17, 18–24.
    let (kind, k) = if f < 10 {
        (0, f)
    } else if f < 18 {
        (1, f - 10)
    } else {
        (2, f - 18)
    };
    let mut img = Image::new(80, 48);
    for y in 0..48 {
        for x in 0..80 {
            let fx = x as f32 + 2.0 * k as f32;
            let v = match kind {
                0 => 0.5 + 0.4 * (fx * 0.15).sin() * (y as f32 * 0.11).cos(),
                1 => {
                    if ((fx / 10.0).floor() as i32 + y as i32 / 10) % 2 == 0 {
                        0.85
                    } else {
                        0.15
                    }
                }
                _ => effectcraft_raster::hash_noise((fx.floor() as u32) / 4, y / 4, 5),
            };
            let c = match kind {
                0 => [v, v * 0.6, 0.2],
                1 => [0.1, v * 0.8, v],
                _ => [v * 0.9, v * 0.9, 0.5 + v * 0.4],
            };
            img.set(x, y, [c[0], c[1], c[2], 1.0]);
        }
    }
    img
}

#[test]
fn scene_edit_detection_finds_cuts() {
    let (mut s, cid, clip, _) = setup(80, 48, 1.0, shot_frame);
    // Markers.
    let r = s.execute("layer.sceneEditDetection", json!({"mode": "markers", "wait": true})).unwrap();
    assert_eq!(r["cuts"], json!([0.4, 0.72]));
    let l = s.project.comp(cid).unwrap().layer(clip).unwrap();
    assert_eq!(l.markers.iter().map(|m| m.time.seconds()).collect::<Vec<_>>(), vec![0.4, 0.72]);
    s.undo();
    // Split Layers.
    let r = s.execute("layer.sceneEditDetection", json!({"mode": "split", "wait": true})).unwrap();
    let ids: Vec<u64> = r["layers"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(ids.len(), 3);
    let comp = s.project.comp(cid).unwrap();
    let spans: Vec<(f64, f64)> = ids.iter().map(|i| comp.layer(LayerId(*i)).unwrap()).map(|l| (l.in_point.seconds(), l.out_point.seconds())).collect();
    assert_eq!(spans, vec![(0.0, 0.4), (0.4, 0.72), (0.72, 1.0)]);
    s.undo();
    assert_eq!(s.project.comp(cid).unwrap().layers.len(), 1);
    // Split and Precompose: three precomp layers, one undo step.
    let r = s.execute("layer.sceneEditDetection", json!({"mode": "splitPrecompose", "wait": true})).unwrap();
    let comp = s.project.comp(cid).unwrap();
    assert_eq!(comp.layers.len(), 3);
    for v in r["layers"].as_array().unwrap() {
        let l = comp.layer(LayerId(v.as_u64().unwrap())).unwrap();
        assert!(matches!(l.source, LayerSource::Comp { .. }));
    }
    assert_eq!(s.history.undo.last().unwrap().0, "Scene Edit Detection");
    s.undo();
    assert_eq!(s.project.comp(cid).unwrap().layers.len(), 1);
}

// ---------------------------------------------------------------- Auto-trace

fn disc_frame(f: u32) -> Image {
    let mut img = Image::new(80, 60);
    let (cx, cy) = (30.0 + 2.0 * f as f64, 30.0);
    for y in 0..60 {
        for x in 0..80 {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            // Anti-aliased disc of radius 15 plus a 12 × 12 square, on transparency.
            let a = (15.5 - d).clamp(0.0, 1.0) as f32;
            let sq = (60..72).contains(&x) && (8..20).contains(&y);
            let a = if sq { 1.0 } else { a };
            img.set(x, y, [a, a, a, a]);
        }
    }
    img
}

fn iou_with(path_set: &[effectcraft_keyframe::ShapePath], truth: &Image) -> f64 {
    let paths: Vec<_> = path_set.iter().map(effectcraft_path::to_kurbo).collect();
    let cov = effectcraft_path::fill_coverage(&paths, &effectcraft_geom::Mat3::IDENTITY, truth.width, truth.height, effectcraft_path::FillRule::EvenOdd);
    let (mut i, mut u) = (0.0, 0.0);
    for (c, p) in cov.data.iter().zip(&truth.data) {
        let (a, b) = (*c >= 0.5, p[3] >= 0.5);
        i += (a && b) as u8 as f64;
        u += (a || b) as u8 as f64;
    }
    i / u
}

fn mask_paths(s: &Session, cid: ItemId, lid: LayerId, lt: Tick) -> Vec<effectcraft_keyframe::ShapePath> {
    let l = s.project.comp(cid).unwrap().layer(lid).unwrap();
    l.masks()
        .unwrap()
        .groups()
        .filter(|g| g.get("opacity").unwrap().value_at(lt).as_f64() > 0.0)
        .map(|g| match g.get("path").unwrap().value_at(lt) {
            effectcraft_project::Value::Path(p) => p,
            _ => panic!("path"),
        })
        .collect()
}

#[test]
fn auto_trace_matches_shapes() {
    let (mut s, cid, clip, _) = setup(80, 60, 0.4, disc_frame);
    let r = s.execute("layer.autoTrace", json!({"layer": clip.0, "channel": "alpha", "tolerance": 0.5, "cornerRoundness": 30})).unwrap();
    assert_eq!(r["masks"], json!(2));
    let iou = iou_with(&mask_paths(&s, cid, clip, Tick::ZERO), &disc_frame(0));
    assert!(iou > 0.95, "IoU {iou}");
    s.undo();
    // Work area onto a new layer: one key per frame following the moving disc.
    let r = s.execute("layer.autoTrace", json!({"layer": clip.0, "timeSpan": "workArea", "applyToNewLayer": true, "minimumArea": 20})).unwrap();
    let nl = LayerId(r["layer"].as_u64().unwrap());
    assert_ne!(nl, clip);
    assert_eq!(r["frames"], json!(10));
    for f in [0u32, 5, 9] {
        let t = FrameRate::new(FPS as i64, 1).tick_of(f as i64);
        let iou = iou_with(&mask_paths(&s, cid, nl, t), &disc_frame(f));
        assert!(iou > 0.93, "frame {f}: IoU {iou}");
    }
    // Luminance with Invert traces the background instead (one outline with holes).
    s.undo();
    let r = s.execute("layer.autoTrace", json!({"layer": clip.0, "channel": "luminance", "invert": true, "threshold": 50})).unwrap();
    assert!(r["masks"].as_u64().unwrap() >= 1);
}

// ---------------------------------------------------------------- Align Video to Data

#[test]
fn align_video_to_data() {
    let (mut s, cid, clip, _) = setup(16, 16, 20.0, |_| Image::filled(16, 16, [0.2, 0.2, 0.2, 1.0]));
    let add_data = |s: &mut Session, name: &str, ext: &str, text: &str| -> u64 {
        s.edit("data", None, |p, _| {
            let f =
                Footage { path: format!("{name}.{ext}"), kind: FootageKind::Data, codec: ext.to_uppercase(), data: Some(text.into()), ..Default::default() };
            Ok(p.add_item(name, Label::Sandstone, None, ItemKind::Footage(f)).0)
        })
        .unwrap()
    };
    let gps = add_data(&mut s, "gps", "csv", "lat,lon,timestamp\n1,2,2026-05-01T10:00:00Z\n1,2,2026-05-01T10:00:01Z\n");
    // The video started 5 s after the first sample: it moves 5 s later.
    let r = s.execute("layer.alignVideoToData", json!({"data": gps, "videoStart": "2026-05-01T10:00:05Z"})).unwrap();
    assert_eq!(r["startTime"], json!(5.0));
    let l = s.project.comp(cid).unwrap().layer(clip).unwrap();
    assert_eq!((l.start_time.seconds(), l.in_point.seconds()), (5.0, 5.0));
    s.undo();
    // Data placed at 2 s, timestamps as times of day, video start as timecode.
    let log = add_data(&mut s, "log", "json", r#"{"samples": [{"t": "09:59:58.0", "v": 1}, {"t": "09:59:59.0", "v": 2}]}"#);
    let r = s.execute("layer.alignVideoToData", json!({"data": "log", "videoStart": "10:00:00:00", "dataStart": 2.0})).unwrap();
    assert_eq!(r["key"], json!("t"));
    assert_eq!(r["startTime"], json!(4.0));
    // A video that started before the data moves earlier (negative start).
    let r = s.execute("layer.alignVideoToData", json!({"data": log, "videoStart": "2026-05-01T09:59:57Z"})).unwrap();
    assert_eq!(r["startTime"], json!(-1.0));
    // Errors: unknown key, no data.
    assert!(s.execute("layer.alignVideoToData", json!({"data": gps, "key": "nope", "videoStart": 0})).is_err());
    let _: Value = Value::Null;
}

#[test]
fn panels_commands_are_wired() {
    // The Window menu opens real panels now, and the former stubs are enabled.
    let menus = crate::menus::entries();
    for panel in ["lumetriScopes", "footage", "mediaBrowser", "metadata", "progress", "contentAwareFill"] {
        assert!(menus.iter().any(|(_, e)| e.command == "window.panel" && e.params["panel"] == panel), "{panel}");
    }
    assert!(crate::find_command("window.unavailablePanel").is_none());
    let (s, _, _, _) = setup(8, 8, 1.0, |_| Image::filled(8, 8, [0.0; 4]));
    for id in ["layer.autoTrace", "layer.sceneEditDetection", "layer.alignVideoToData", "layer.newContentAwareFill", "contentFill.generate"] {
        assert!(s.is_enabled(id), "{id}");
    }
}

/// A browser like the web app's: a virtual tree whose files are read on demand.
#[derive(Default)]
struct VirtualBrowser {
    tree: Mutex<crate::media_browser::VirtualTree>,
    fetched: Mutex<Vec<String>>,
    actions: Mutex<Vec<String>>,
}

impl crate::media_browser::Browser for VirtualBrowser {
    fn home(&self) -> String {
        "/Browser Storage".into()
    }
    fn places(&self) -> Vec<(String, String)> {
        vec![("Browser Storage".into(), self.home()), ("Shoot".into(), "/Folders/Shoot".into())]
    }
    fn list(&self, dir: &str, only: bool) -> Result<Vec<crate::media_browser::Entry>, String> {
        self.tree.lock().unwrap().list(dir, only)
    }
    fn fetch(&self, paths: &[String], _: &Value) -> bool {
        // Opened folders' files arrive later.
        let pending: Vec<String> = paths.iter().filter(|p| p.starts_with("/Folders/")).cloned().collect();
        self.fetched.lock().unwrap().extend(pending.iter().cloned());
        pending.is_empty()
    }
    fn actions(&self) -> Vec<(String, String)> {
        vec![("openFolder".into(), "Open Folder…".into())]
    }
    fn action(&self, id: &str) -> Result<Value, String> {
        self.actions.lock().unwrap().push(id.into());
        Ok(json!({"pending": true}))
    }
}

#[test]
fn media_browser_over_a_virtual_tree() {
    let b = Arc::new(VirtualBrowser::default());
    {
        let mut t = b.tree.lock().unwrap();
        t.add_dir("/Browser Storage");
        t.add_file("/Folders/Shoot/a.csv", "/Folders/Shoot/a.csv", 12, Some(1));
    }
    let mut s = Session { browser: Some(b.clone()), ..Default::default() };
    let r = s.execute("mediaBrowser.list", json!({})).unwrap();
    assert_eq!(r["path"], "/Browser Storage");
    assert_eq!(r["places"][1]["path"], "/Folders/Shoot");
    assert_eq!(r["actions"][0]["id"], "openFolder");
    let r = s.execute("mediaBrowser.go", json!({"path": "/Folders/Shoot"})).unwrap();
    assert_eq!(r["entries"][0]["name"], "a.csv");
    assert_eq!(r["parent"], "/Folders");
    assert!(s.execute("mediaBrowser.go", json!({"path": "/Nope"})).is_err());
    // Importing a file that is not read yet: pending (the host imports it once it arrives).
    let r = s.execute("mediaBrowser.import", json!({"paths": ["/Folders/Shoot/a.csv"]})).unwrap();
    assert_eq!(r["pending"], true);
    assert_eq!(*b.fetched.lock().unwrap(), ["/Folders/Shoot/a.csv"]);
    assert!(s.project.items.values().all(|i| !matches!(i.kind, ItemKind::Footage(_))));
    let r = s.execute("mediaBrowser.action", json!({"action": "openFolder"})).unwrap();
    assert_eq!(r["pending"], true);
    assert_eq!(*b.actions.lock().unwrap(), ["openFolder"]);
    assert!(s.execute("mediaBrowser.action", json!({})).is_err());
}

/// Footage the source can't decode is reported with its reason by `project.summary` and
/// `footage.info`, so an agent can tell the user why the picture is black.
#[test]
fn footage_that_cannot_be_decoded_reports_why() {
    struct Broken;
    impl FootageSource for Broken {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            None
        }
        fn problem(&self, f: &Footage) -> Option<String> {
            Some(format!("{} isn't supported by the built-in decoder", f.codec))
        }
    }
    let mut s = Session { footage: Arc::new(Broken), ..Default::default() };
    s.execute("comp.new", json!({"name": "C", "width": 64, "height": 36, "duration": 2})).unwrap();
    let f = Footage {
        kind: FootageKind::Video,
        path: "a.mp4".into(),
        codec: "H.264 (High 4:2:2)".into(),
        width: 64,
        height: 36,
        has_video: true,
        ..Default::default()
    };
    let item = std::sync::Arc::make_mut(&mut s.project).add_item("Clip", Default::default(), None, ItemKind::Footage(f));
    let sum = s.execute("project.summary", json!({})).unwrap();
    let it = sum["items"].as_array().unwrap().iter().find(|i| i["id"] == json!(item.0)).unwrap();
    assert_eq!(it["problem"], json!("H.264 (High 4:2:2) isn't supported by the built-in decoder"));
    assert!(sum["items"].as_array().unwrap().iter().filter(|i| i["id"] != json!(item.0)).all(|i| i.get("problem").is_none()), "only footage reports one");
    s.execute("footage.open", json!({"item": item.0})).unwrap();
    assert_eq!(s.execute("footage.info", json!({})).unwrap()["problem"], it["problem"]);
}

/// A proxy in use that can't be decoded leaves its item without a picture although the item's
/// own file is fine: `project.summary` and `footage.info` give the proxy's reason, said to be the
/// proxy's, for footage and compositions alike, and nothing once Use Proxy is off.
#[test]
fn a_proxy_in_use_that_cannot_be_decoded_reports_why() {
    struct BrokenProxies;
    impl FootageSource for BrokenProxies {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
            None
        }
        fn problem(&self, f: &Footage) -> Option<String> {
            f.path.ends_with(".avi").then(|| format!("{} isn't supported by the built-in decoder", f.codec))
        }
    }
    let mut s = Session { footage: Arc::new(BrokenProxies), ..Default::default() };
    let comp = s.execute("comp.new", json!({"name": "C", "width": 64, "height": 36, "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    let still = Footage { kind: FootageKind::Still, path: "a.png".into(), width: 64, height: 36, has_video: true, ..Default::default() };
    let item = std::sync::Arc::make_mut(&mut s.project).add_item("Plate", Default::default(), None, ItemKind::Footage(still));
    let proxy = Footage {
        kind: FootageKind::Video,
        path: "plate.avi".into(),
        codec: "MPEG-4 Part 2".into(),
        width: 32,
        height: 18,
        has_video: true,
        ..Default::default()
    };
    let want = json!("Proxy: MPEG-4 Part 2 isn't supported by the built-in decoder");
    let problem = |s: &mut Session, id: u64| {
        let sum = s.execute("project.summary", json!({})).unwrap();
        sum["items"].as_array().unwrap().iter().find(|i| i["id"] == json!(id)).unwrap().get("problem").cloned()
    };
    for id in [item.0, comp] {
        let p = std::sync::Arc::make_mut(&mut s.project);
        p.items.get_mut(&ItemId(id)).unwrap().proxy = Some(Box::new(effectcraft_project::Proxy { footage: proxy.clone(), enabled: true }));
        assert_eq!(problem(&mut s, id), Some(want.clone()), "item {id}");
        std::sync::Arc::make_mut(&mut s.project).items.get_mut(&ItemId(id)).unwrap().proxy.as_mut().unwrap().enabled = false;
        assert_eq!(problem(&mut s, id), None, "item {id}: its own file shows, and is fine");
    }
    std::sync::Arc::make_mut(&mut s.project).items.get_mut(&item).unwrap().proxy.as_mut().unwrap().enabled = true;
    s.execute("footage.open", json!({"item": item.0})).unwrap();
    assert_eq!(s.execute("footage.info", json!({})).unwrap()["problem"], want);
}
