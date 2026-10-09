//! M14.5: settings that change behaviour, the keyframe assistants and labels, and the menu
//! entries added for After Effects parity (History, recent footage / presets, XML copies, VR,
//! shortcut slots, logging and the compatibility report).

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::render::RenderOpts;
use crate::{KeyRef, Session};
use effectcraft_keyframe::Value as KV;
use effectcraft_project::{ItemId, LayerId};
use effectcraft_time::Tick;

fn tmp(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!("ec-m145-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn session() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 48, "duration": 2, "frameRate": 10})).unwrap();
    s
}

fn solid(s: &mut Session, color: &str) -> u64 {
    s.execute("layer.newSolid", json!({"color": color, "width": 32, "height": 24})).unwrap()["layer"].as_u64().unwrap()
}

fn layer(s: &Session, id: u64) -> effectcraft_project::Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

// ---------------------------------------------------------------- General

#[test]
fn split_layers_go_above_or_below_the_original() {
    let mut s = session();
    let a = solid(&mut s, "#ff0000");
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    let below = s.execute("edit.splitLayer", json!({"layers": [a]})).unwrap()[0].as_u64().unwrap();
    let order: Vec<u64> = s.active_comp().unwrap().layers.iter().map(|l| l.id.0).collect();
    assert_eq!(order, vec![a, below], "off (default): the new part goes below");
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("prefs.set", json!({"key": "general.createSplitLayersAbove", "value": true})).unwrap();
    let above = s.execute("edit.splitLayer", json!({"layers": [a]})).unwrap()[0].as_u64().unwrap();
    let order: Vec<u64> = s.active_comp().unwrap().layers.iter().map(|l| l.id.0).collect();
    assert_eq!(order, vec![above, a]);
}

#[test]
fn new_masks_cycle_colours_only_when_asked() {
    let mut s = session();
    let a = solid(&mut s, "#ff0000");
    let colors = |s: &Session| -> Vec<[u8; 3]> {
        layer(s, a)
            .props
            .sub("masks")
            .unwrap()
            .groups()
            .map(|g| match g.kind {
                effectcraft_project::GroupKind::Mask { color, .. } => color,
                _ => [0; 3],
            })
            .collect()
    };
    s.execute("layer.addMask", json!({"layer": a})).unwrap();
    s.execute("layer.addMask", json!({"layer": a})).unwrap();
    let c = colors(&s);
    assert_ne!(c[0], c[1], "cycling by default");
    s.execute("prefs.set", json!({"key": "appearance.cycleMaskColors", "value": false})).unwrap();
    s.execute("layer.addMask", json!({"layer": a})).unwrap();
    s.execute("mask.new", json!({"layer": a, "vertices": [[0, 0], [5, 5]]})).unwrap();
    let c = colors(&s);
    assert_eq!(c[2], c[0]);
    assert_eq!(c[3], c[0]);
}

#[test]
fn preserve_constant_vertex_count_edits_every_mask_key() {
    let mut s = session();
    let a = solid(&mut s, "#ff0000");
    let m = s.execute("layer.addMask", json!({"layer": a})).unwrap()["mask"].as_u64().unwrap();
    s.execute("prop.select", json!({"layer": a, "prop": layer(&s, a).props.sub("masks").unwrap().groups().next().unwrap().get("path").unwrap().uid})).unwrap();
    let path_uid = layer(&s, a).props.sub("masks").unwrap().groups().next().unwrap().get("path").unwrap().uid;
    s.execute("prop.toggleAnimation", json!({"layer": a, "prop": path_uid})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("mask.setVertex", json!({"layer": a, "mask": m, "index": 0, "point": [1, 1]})).unwrap();
    let counts = |s: &Session| -> Vec<usize> {
        layer(s, a)
            .props
            .find(path_uid)
            .unwrap()
            .keys
            .iter()
            .map(|k| match &k.value {
                KV::Path(p) => p.vertices.len(),
                _ => 0,
            })
            .collect()
    };
    assert_eq!(counts(&s), vec![4, 4]);
    // On (the default): a vertex added at one key is added to the other.
    s.execute("mask.insertVertex", json!({"layer": a, "mask": m, "segment": 0})).unwrap();
    assert_eq!(counts(&s), vec![5, 5]);
    s.state.selected_vertices = vec![crate::VertexRef { layer: LayerId(a), mask: m, index: 1 }];
    s.execute("mask.deleteVertices", json!({})).unwrap();
    assert_eq!(counts(&s), vec![4, 4]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(counts(&s), vec![5, 5], "one undo step");
    // Off: only the current key changes.
    s.execute("prefs.set", json!({"key": "general.preserveConstantVertexCount", "value": false})).unwrap();
    s.execute("mask.addVertex", json!({"layer": a, "mask": m, "point": [3, 3]})).unwrap();
    assert_eq!(counts(&s), vec![5, 6]);
}

#[test]
fn switches_affect_nested_comps_passes_quality_down() {
    let mut s = Session::default();
    let inner = s.execute("comp.new", json!({"name": "Inner", "width": 40, "height": 40, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
    let a = solid(&mut s, "#ff8040");
    // A rotated, scaled layer: bilinear (Best) and nearest (Draft) sampling differ.
    s.execute("prop.set", json!({"layer": a, "path": "transform/rotation", "value": 17})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "transform/scale", "value": [131, 131]})).unwrap();
    let outer = s.execute("comp.new", json!({"name": "Outer", "width": 40, "height": 40, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
    let pre = s.execute("layer.addItem", json!({"item": inner})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.quality", json!({"layers": [pre], "quality": "draft"})).unwrap();
    let draw = |s: &Session| s.render(ItemId(outer), Tick::ZERO, RenderOpts::default());
    let on = draw(&s);
    s.execute("prefs.set", json!({"key": "general.switchesAffectNestedComps", "value": false})).unwrap();
    let off = draw(&s);
    assert_ne!(on.data, off.data, "the precomp's Draft switch reaches the nested layer only when on");
    // On = the nested layer itself set to Draft.
    s.execute("prefs.set", json!({"key": "general.switchesAffectNestedComps", "value": true})).unwrap();
    s.open_comp(ItemId(inner));
    s.execute("layer.quality", json!({"layers": [a], "quality": "draft"})).unwrap();
    s.execute("prefs.set", json!({"key": "general.switchesAffectNestedComps", "value": false})).unwrap();
    assert_eq!(draw(&s).data, on.data);
}

#[test]
fn related_comps_share_the_current_time() {
    let mut s = Session::default();
    let inner = s.execute("comp.new", json!({"name": "Inner", "duration": 10, "frameRate": 10})).unwrap()["comp"].as_u64().unwrap();
    let outer = s.execute("comp.new", json!({"name": "Outer", "duration": 10, "frameRate": 10})).unwrap()["comp"].as_u64().unwrap();
    let l = s.execute("layer.addItem", json!({"item": inner})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setTiming", json!({"layer": l, "startTime": 2.0})).ok();
    s.edit("t", None, |p, _| {
        p.comp_mut(ItemId(outer)).unwrap().layer_mut(LayerId(l)).unwrap().start_time = Tick::from_seconds_f64(2.0);
        Ok(())
    })
    .unwrap();
    s.set_time(Tick::from_seconds_f64(5.0));
    assert!((s.time_of(ItemId(inner)).seconds() - 3.0).abs() < 1e-9, "nested comp follows through the layer's start");
    s.open_comp(ItemId(inner));
    s.set_time(Tick::from_seconds_f64(1.0));
    assert!((s.time_of(ItemId(outer)).seconds() - 3.0).abs() < 1e-9, "and the other way");
    s.execute("prefs.set", json!({"key": "general.syncTimeRelatedItems", "value": false})).unwrap();
    s.set_time(Tick::from_seconds_f64(7.0));
    assert!((s.time_of(ItemId(outer)).seconds() - 3.0).abs() < 1e-9);
}

#[test]
fn pick_whip_writes_match_names_without_compact_english() {
    let mut s = session();
    let a = solid(&mut s, "#ff0000");
    let r = s.execute("layer.newSolid", json!({"name": "Other", "color": "#00ff00"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layer": r, "effect": "Gaussian Blur"})).unwrap();
    s.execute("prefs.set", json!({"key": "general.expressionPickWhipCompact", "value": false})).unwrap();
    let e = s.execute("prop.pickWhip", json!({"layer": a, "path": "transform/opacity", "target": {"layer": r, "path": "transform/opacity"}})).unwrap();
    assert_eq!(e, json!("thisComp.layer(\"Other\")(\"transform\")(\"opacity\")"));
    let e = s.execute("prop.pickWhip", json!({"layer": a, "path": "transform/rotation", "target": {"layer": a, "path": "transform/opacity"}})).unwrap();
    assert_eq!(e, json!("thisLayer(\"transform\")(\"opacity\")"));
}

// ---------------------------------------------------------------- pure setting helpers

#[test]
fn cache_budgets_leave_reserved_ram_and_shrink_when_low() {
    use crate::sysinfo::SysMemory;
    let mut p = crate::prefs::Prefs::default();
    p.memory.layer_cache_mb = 4096;
    p.memory.media_cache_mb = 4096;
    p.memory.preview_cache_mb = 8192;
    p.memory.ram_reserved_gb = 6;
    let b = p.cache_budgets(None);
    assert_eq!((b.layer >> 20, b.preview >> 20, b.capped), (4096, 8192, false));
    // 16 GB machine, 6 GB reserved: 16 GB of caches scaled into 10 GB.
    let m = SysMemory { total: 16 << 30, available: 12 << 30 };
    let b = p.cache_budgets(Some(m));
    assert!(b.capped && !b.reduced);
    assert!(((b.layer + b.media + b.preview) as i64 - (10i64 << 30)).abs() < 4, "{b:?}");
    // Low on memory: halved; off: not.
    let low = SysMemory { total: 16 << 30, available: 512 << 20 };
    let r = p.cache_budgets(Some(low));
    assert!(r.reduced && r.preview < b.preview / 2 + 2);
    p.memory.reduce_cache_when_low = false;
    assert!(!p.cache_budgets(Some(low)).reduced);
}

#[test]
fn motion_path_span_follows_the_composition_setting() {
    let mut p = crate::prefs::Prefs::default();
    let s = |x: f64| Tick::from_seconds_f64(x);
    let keys: Vec<Tick> = (0..40).map(|i| s(i as f64)).collect();
    // Default: no more than 15 keyframes, centred on the current time.
    assert_eq!(p.motion_path_span(&keys, s(20.0)), Some((s(13.0), s(27.0))));
    assert_eq!(p.motion_path_span(&keys, s(0.0)), Some((s(0.0), s(14.0))));
    p.composition.motion_path = "all".into();
    assert_eq!(p.motion_path_span(&keys, s(5.0)), Some((s(0.0), s(39.0))));
    p.composition.motion_path = "none".into();
    assert_eq!(p.motion_path_span(&keys, s(5.0)), None);
    p.composition.motion_path = "seconds".into();
    p.composition.motion_path_seconds = 4.0;
    assert_eq!(p.motion_path_span(&keys, s(10.0)), Some((s(8.0), s(12.0))));
}

#[test]
fn import_export_helpers() {
    use effectcraft_project::AlphaMode;
    let mut p = crate::prefs::Prefs::default();
    assert_eq!(p.unlabeled_alpha(false), None, "Ask User");
    for (v, movie, want) in [
        ("ignore", false, AlphaMode::Ignore),
        ("premultiplied", false, AlphaMode::Premultiplied),
        ("guess", true, AlphaMode::Premultiplied),
        ("guess", false, AlphaMode::Straight),
    ] {
        p.import.unlabeled_alpha = v.into();
        assert_eq!(p.unlabeled_alpha(movie), Some(want));
    }
    assert_eq!(p.drag_import_as(), "footage");
    p.import.drag_import_as = "compLayerSizes".into();
    assert_eq!(p.drag_import_as(), "compositionLayerSizes");
    assert_eq!(
        crate::sequence::missing_report(&["a_001.png".into(), "a_002.png".into(), "a_005.png".into(), "a_007.png".into()]).unwrap(),
        "3 missing frames (3–4, 6)"
    );
    assert!(crate::sequence::missing_report(&["a1.png".into(), "a2.png".into()]).is_none());
    // Append Bit Depth to File Name.
    assert_eq!(p.output_name("/out/Comp 1.png", 16), "/out/Comp 1.png");
    p.export.append_bits_to_name = true;
    assert_eq!(PathBuf::from(p.output_name("/out/Comp 1.png", 16)), PathBuf::from("/out/Comp 1_16bpc.png"));
    assert_eq!(p.output_name("Comp_[#####].exr", 32), "Comp_[#####]_32bpc.exr");
    // Segments: sequences by file count, movies by size at the data rate.
    assert_eq!(p.segment_frames(100, true, 0.0), None);
    p.export.segment_sequences = true;
    p.export.segment_sequence_files = 30;
    assert_eq!(p.segment_frames(100, true, 0.0), Some(30));
    p.export.segment_movies = true;
    p.export.segment_movie_mb = 1;
    assert_eq!(p.segment_frames(100, false, 1_048_576.0 / 10.0), Some(10));
    assert_eq!(p.segment_frames(5, false, 1_048_576.0 / 10.0), None);
    // Recent fonts.
    for f in ["A", "B", "C", "A"] {
        p.push_recent_font(f);
    }
    p.type_.recent_fonts = 2;
    assert_eq!(p.recent_fonts_shown(), ["A".to_string(), "C".to_string()]);
}

#[test]
fn render_queue_uses_default_folder_bit_depth_names_and_segments() {
    use effectcraft_project::render_queue::{OutputFormat, RenderQueueItem};
    let mut s = session();
    solid(&mut s, "#ff0000");
    let dir = tmp("out");
    s.execute("prefs.set", json!({"values": {"export.defaultOutputFolder": dir.to_string_lossy(), "export.appendBitsToName": true}})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let mut item = RenderQueueItem::new(1, cid);
    item.output.output = "Main.png".into();
    item.output.format = OutputFormat::PngSequence;
    let out = s.resolve_output(&item).unwrap();
    assert_eq!(PathBuf::from(&out), dir.join("Main.png"));
    assert_eq!(
        s.prefs.output_name(&out, crate::render_queue::output_bits(OutputFormat::PngSequence, s.project.settings.bit_depth)),
        dir.join("Main_8bpc.png").to_string_lossy()
    );
    // 20 frames in segments of 8 files: three folders, spans tiling the work area.
    s.prefs.export.segment_sequences = true;
    s.prefs.export.segment_sequence_files = 8;
    let segs = s.segments(&item, &out);
    assert_eq!(segs.len(), 3);
    let comp = s.active_comp().unwrap().clone();
    let n: Vec<u64> = segs.iter().map(|(i, _)| i.settings.frame_count(&comp)).collect();
    assert_eq!(n, vec![8, 8, 4]);
    assert!(segs[1].1.contains("Main_002"), "{}", segs[1].1);
    assert_eq!(segs[2].0.settings.first_frame(&comp), 16);
    // Movies: a ~1 MB file per segment at the H.264 bitrate.
    s.prefs.export.segment_movies = true;
    s.prefs.export.segment_movie_mb = 1;
    item.output.format = OutputFormat::H264;
    item.output.bitrate_kbps = 4000; // 50 kB per frame at 10 fps
    let segs = s.segments(&item, "/o/Main.mp4");
    assert_eq!(segs.len(), 1, "20 frames × 50 kB fit in 1 MB");
    item.output.bitrate_kbps = 8000; // 100 kB per frame: 10 frames per MB
    let segs = s.segments(&item, "/o/Main.mp4");
    assert_eq!(segs.iter().map(|(_, p)| PathBuf::from(p)).collect::<Vec<_>>(), [PathBuf::from("/o/Main_001.mp4"), PathBuf::from("/o/Main_002.mp4")]);
}

#[test]
fn every_wired_setting_is_live_and_display_only_ones_are_documented() {
    let todo = crate::prefs::todo_keys();
    assert_eq!(todo, ["general.useSystemColorPicker", "composition.hardwareAcceleratePanels", "scripting.enableJsDebugger"]);
    let docs = include_str!("../../../docs/preferences.md");
    assert!(docs.contains("### Display-only settings"));
    assert!(!docs.contains("Not wired yet"));
}

// ---------------------------------------------------------------- scripting gate

#[test]
fn execute_file_needs_the_gate_and_asks_first() {
    let mut s = session();
    let f = tmp("run.txt");
    std::fs::write(&f, "x").unwrap();
    let p = json!({"path": f.to_string_lossy()});
    assert!(s.execute("file.executeFile", p.clone()).is_err(), "off by default");
    s.execute("prefs.set", json!({"key": "scripting.allowScriptsWriteFiles", "value": true})).unwrap();
    s.drain_events();
    let r = s.execute("file.executeFile", p.clone()).unwrap();
    assert!(r.get("needsConfirmation").is_some());
    assert!(s.drain_events().iter().any(|e| matches!(e, crate::Event::Frontend { command, .. } if command == "app.confirmExecute")));
    let r = s.execute("file.executeFile", json!({"path": f.to_string_lossy(), "confirmed": true})).unwrap();
    assert!(r.get("opened").is_some());
    assert!(s.drain_events().iter().any(|e| matches!(e, crate::Event::OpenUrl(u) if u.starts_with("file://"))));
    s.execute("prefs.set", json!({"key": "scripting.warnExecutingFiles", "value": false})).unwrap();
    assert!(s.execute("file.executeFile", p).unwrap().get("opened").is_some());
}

// ---------------------------------------------------------------- Edit ▸ History, recent lists

#[test]
fn history_submenu_jumps_back_several_steps() {
    let mut s = session();
    solid(&mut s, "#ff0000");
    solid(&mut s, "#00ff00");
    solid(&mut s, "#0000ff");
    let list = s.execute("edit.history", json!({})).unwrap();
    assert_eq!(list["undo"][0], json!("New Solid"));
    let n = s.history.undo.len();
    let (entries, _) = crate::menus::dynamic(&s, "history", &Default::default());
    assert_eq!(entries.len(), n);
    assert_eq!(entries[1].params, json!({"steps": 2}));
    s.execute("edit.history", json!({"steps": 2})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 1);
    assert_eq!(s.history.redo.len(), 2);
    s.execute("edit.history", json!({"redo": 2})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 3);
    assert!(s.execute("edit.history", json!({"steps": 999})).is_err());
}

#[test]
fn recent_footage_and_presets_are_remembered() {
    let mut s = session();
    let a = solid(&mut s, "#ff0000");
    // Imports are remembered even when the build has no decoder (the error comes after).
    let _ = s.execute("file.import", json!({"paths": ["/x/clip.mov", "/x/still.png"]}));
    assert_eq!(s.prefs.recent_footage, ["/x/still.png", "/x/clip.mov"]);
    let (e, _) = crate::menus::dynamic(&s, "recentFootage", &Default::default());
    assert_eq!(e.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["still.png", "clip.mov"]);
    s.execute("file.clearRecentFootage", json!({})).unwrap();
    assert_eq!(crate::menus::dynamic(&s, "recentFootage", &Default::default()), (vec![], Some("No Recent Footage")));
    // Presets.
    let path = tmp("fade.ecpreset");
    s.execute("prop.select", json!({"layer": a, "path": "transform/opacity"})).unwrap();
    s.execute("anim.savePreset", json!({"path": path})).unwrap();
    s.execute("anim.applyPreset", json!({"path": path, "layers": [a]})).unwrap();
    assert_eq!(s.prefs.recent_presets, [path.to_string_lossy().to_string()]);
    let (e, _) = crate::menus::dynamic(&s, "recentPresets", &Default::default());
    assert_eq!(e[0].label, "fade");
    s.state.selected_layers = vec![LayerId(a)];
    s.execute("anim.applyRecentPreset", json!({"index": 0})).unwrap();
    s.execute("anim.clearRecentPresets", json!({})).unwrap();
    assert!(s.prefs.recent_presets.is_empty());
    // Recent lists survive a settings round trip and a reset.
    s.prefs.push_recent_footage("/y/a.wav");
    let back = crate::prefs::Prefs::from_json(&s.prefs.to_json());
    assert_eq!(back.recent_footage, ["/y/a.wav"]);
    s.execute("prefs.reset", json!({})).unwrap();
    assert_eq!(s.prefs.recent_footage, ["/y/a.wav"]);
}

#[test]
fn save_a_copy_as_xml_opens_again() {
    let mut s = session();
    solid(&mut s, "#ff0000");
    let path = tmp("copy.ecprojx");
    s.execute("file.saveCopyAsXml", json!({"path": path.to_string_lossy()})).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("<?xml") && text.contains("<EffectCraftProject"));
    let before = s.project.to_json();
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(t.project.to_json(), before);
    // Saving a project opened from XML writes XML.
    t.execute("file.save", json!({})).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().starts_with("<?xml"));
}

// ---------------------------------------------------------------- shortcut slots

#[test]
fn assign_shortcut_slots_move_keys_between_views_and_workspaces() {
    let mut s = session();
    s.execute("view.3d.top", json!({})).unwrap();
    let label = crate::menus::submenu_label(&s, "Assign Shortcut to 3D View", &Default::default());
    assert_eq!(label, "Assign Shortcut to “Top”");
    let (e, _) = crate::menus::dynamic(&s, "view3dShortcuts", &Default::default());
    assert_eq!(e[0].label, "F10 (Replace “Front”)");
    let r = s.execute("view.assign3dShortcut", json!({"slot": "F10"})).unwrap();
    assert_eq!(r["replaced"], json!(["Front"]));
    assert_eq!(s.shortcuts().shortcut_of("view.3d.top", &Value::Null), Some("F10"));
    assert_eq!(s.shortcuts().shortcut_of("view.3d.front", &Value::Null), None);
    // Workspaces: Shift+F11 now opens Animation instead of Standard.
    s.execute("window.assignWorkspaceShortcut", json!({"slot": "Shift+F11", "workspace": "Animation"})).unwrap();
    assert_eq!(s.shortcuts().shortcut_of("window.workspace", &json!({"name": "Animation"})), Some("Shift+F11"));
    assert_eq!(s.shortcuts().shortcut_of("window.workspace", &json!({"name": "Standard"})), None);
    assert!(s.execute("view.assign3dShortcut", json!({"slot": "F9"})).is_err());
    // Without a workspace the frontend fills it in.
    s.drain_events();
    s.execute("window.assignWorkspaceShortcut", json!({"slot": "F10"})).unwrap();
    assert!(s.drain_events().iter().any(|e| matches!(e, crate::Event::Frontend { command, .. } if command == "window.assignWorkspaceShortcut")));
}

// ---------------------------------------------------------------- Help

#[test]
fn compatibility_report_and_logging() {
    let mut s = session();
    let r = s.execute("help.systemReport", json!({"quiet": true})).unwrap();
    for k in ["os", "cpu", "cores", "memory", "gpu", "import", "export", "issues"] {
        assert!(r.get(k).is_some(), "{k} in {r}");
    }
    let text = crate::commands::report_text_for_tests(&r);
    assert!(text.contains("Operating system") && text.contains("Import:"));
    s.drain_events();
    s.execute("help.systemReport", json!({})).unwrap();
    assert!(s.drain_events().iter().any(|e| matches!(e, crate::Event::Frontend { command, .. } if command == "app.showReport")));
    // Logging into a file in the settings folder.
    let dir = tmp("cfg");
    std::fs::create_dir_all(&dir).unwrap();
    s.config = Some(std::sync::Arc::new(crate::config::DirConfig::new(dir.clone())));
    let r = s.execute("help.enableLogging", json!({"on": true})).unwrap();
    let file = PathBuf::from(r["path"].as_str().unwrap());
    assert!(file.starts_with(&dir));
    log::warn!(target: "effectcraft::tests", "hello log");
    assert!(std::fs::read_to_string(&file).unwrap().contains("logging enabled"));
    assert_eq!(crate::menus::checked(&s, "help.enableLogging", &Value::Null), Some(true));
    let r = s.execute("help.revealLogFile", json!({})).unwrap();
    assert!(r["url"].as_str().unwrap().starts_with("file://"));
    s.execute("help.enableLogging", json!({"on": false})).unwrap();
    assert!(!crate::logging::is_enabled());
}

// ---------------------------------------------------------------- VR

#[test]
fn create_vr_environment_and_extract_cubemap() {
    let mut s = session();
    let main = s.active_comp_id().unwrap();
    let a = solid(&mut s, "#ff0000");
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "threeD", "value": true})).unwrap();
    let undo = s.history.undo.len();
    let aname = layer(&s, a).name.clone();
    let r = s.execute("comp.vr.createEnvironment", json!({"size": 64})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    assert_eq!(s.history.undo.last().unwrap().0, "Create VR Environment");
    let faces: Vec<u64> = r["faces"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(faces.len(), 6);
    for f in &faces {
        let c = s.project.comp(ItemId(*f)).unwrap();
        assert_eq!((c.width, c.height), (64, 64));
        assert!(c.layers.iter().any(|l| l.is_camera()));
        let nested = c.layers.iter().find(|l| matches!(l.source, effectcraft_project::LayerSource::Comp { item } if item == main)).expect("the scene, nested");
        assert!(nested.switches.collapse && nested.is_3d(), "collapsed 3D precomp");
        let _ = &aname;
    }
    let cube = s.project.comp(ItemId(r["cubeMap"].as_u64().unwrap())).unwrap();
    assert_eq!((cube.width, cube.height, cube.layers.len()), (192, 128, 6));
    let out = s.project.comp(ItemId(r["output"].as_u64().unwrap())).unwrap();
    assert_eq!((out.width, out.height), (128, 64));
    let fx = out.layers[0].props.sub("effects").unwrap().groups().next().unwrap().clone();
    assert_eq!(fx.match_id, "ec.vr.converter");
    // Every face camera looks a different way.
    let mut dirs = vec![];
    for f in &faces {
        let c = s.project.comp(ItemId(*f)).unwrap();
        let cam = c.layers.iter().find(|l| l.is_camera()).unwrap();
        dirs.push(format!("{:?}", cam.props.prop("transform/orientation").unwrap().value));
    }
    dirs.sort();
    dirs.dedup();
    assert_eq!(dirs.len(), 6, "{dirs:?}");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.comp(ItemId(faces[0])).is_none());
    // Extract Cubemap of a 2:1 comp.
    let eq = s.execute("comp.new", json!({"name": "Equi", "width": 128, "height": 64, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
    let r = s.execute("comp.vr.extractCubemap", json!({"comp": eq})).unwrap();
    let c = s.project.comp(ItemId(r["cubeMap"].as_u64().unwrap())).unwrap();
    assert_eq!((c.width, c.height), (96, 64));
}

/// The equirectangular output sees each wall of a box around the rig in the right direction.
#[test]
fn vr_environment_faces_look_the_right_way() {
    // One wall at a time (centre 100,100,0; AE's −Y is up), and where the equirectangular output
    // must show it: front +Z, back −Z, right +X, left −X, up, down.
    let walls = [
        ("front", [100.0, 100.0, 150.0], [0.0, 0.0], (0.5, 0.5)),
        ("back", [100.0, 100.0, -150.0], [0.0, 180.0], (0.01, 0.5)),
        ("right", [250.0, 100.0, 0.0], [0.0, 90.0], (0.75, 0.5)),
        ("left", [-50.0, 100.0, 0.0], [0.0, 90.0], (0.25, 0.5)),
        ("up", [100.0, -50.0, 0.0], [90.0, 0.0], (0.5, 0.03)),
        ("down", [100.0, 250.0, 0.0], [90.0, 0.0], (0.5, 0.97)),
    ];
    for (name, pos, rot, (fx, fy)) in walls {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Room", "width": 200, "height": 200, "duration": 1, "frameRate": 10})).unwrap();
        let l = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 200, "height": 200})).unwrap()["layer"].as_u64().unwrap();
        s.execute("layer.setSwitch", json!({"layers": [l], "switch": "threeD", "value": true})).unwrap();
        s.execute("prop.set", json!({"layer": l, "path": "transform/position", "value": pos})).unwrap();
        s.execute("prop.set", json!({"layer": l, "path": "transform/rotationX", "value": rot[0]})).unwrap();
        s.execute("prop.set", json!({"layer": l, "path": "transform/rotationY", "value": rot[1]})).unwrap();
        let r = s.execute("comp.vr.createEnvironment", json!({"size": 64})).unwrap();
        let img = s.render(ItemId(r["output"].as_u64().unwrap()), Tick::ZERO, RenderOpts::default());
        let (w, h) = (img.width as f64, img.height as f64);
        let alpha =
            |x: f64, y: f64| img.data[((y * h) as usize).min(img.height as usize - 1) * img.width as usize + ((x * w) as usize).min(img.width as usize - 1)][3];
        assert!(alpha(fx, fy) > 0.5, "{name}: the wall shows in its direction");
        // The opposite direction is empty.
        let (ox, oy) = if fy != 0.5 { (fx, 1.0 - fy) } else { ((fx + 0.5) % 1.0, fy) };
        assert!(alpha(ox, oy) < 0.5, "{name}: nothing in the opposite direction");
    }
}

// ---------------------------------------------------------------- keyframes

#[test]
fn convert_audio_to_keyframes_makes_the_amplitude_null() {
    assert_eq!(crate::commands::amplitudes_for_tests(&[0.5, -0.25, -0.5, 0.25]), (50.0, 25.0, 37.5));
    let mut s = session();
    let undo = s.history.undo.len();
    let r = s.execute("keys.audioToKeyframes", json!({})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1);
    let l = layer(&s, r["layer"].as_u64().unwrap());
    assert_eq!(l.name, "Audio Amplitude");
    let fx: Vec<_> = l.props.sub("effects").unwrap().groups().map(|g| (g.name.clone(), g.get("slider").unwrap().keys.len())).collect();
    assert_eq!(fx, [("Left Channel".to_string(), 20), ("Right Channel".to_string(), 20), ("Both Channels".to_string(), 20)]);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_comp().unwrap().layers.is_empty());
}

#[test]
fn keyframe_labels_and_label_groups() {
    let mut s = session();
    let a = solid(&mut s, "#ff0000");
    for t in [0.0, 0.5, 1.0] {
        s.execute("prop.addKey", json!({"layer": a, "path": "transform/opacity", "time": t, "value": 50})).unwrap();
    }
    let uid = layer(&s, a).props.prop("transform/opacity").unwrap().uid;
    let k = |t: f64| KeyRef { layer: LayerId(a), prop: uid, time: Tick::from_seconds_f64(t) };
    s.state.selected_keys = vec![k(0.0), k(1.0)];
    // Edit ▸ Label colours the selected keyframes.
    s.execute("edit.label", json!({"label": "Blue"})).unwrap();
    let labels: Vec<u8> = layer(&s, a).props.prop("transform/opacity").unwrap().keys.iter().map(|k| k.label).collect();
    let blue = effectcraft_color::Label::ALL.iter().position(|l| *l == effectcraft_color::Label::Blue).unwrap() as u8;
    assert_eq!(labels, [blue, 0, blue]);
    assert_eq!(layer(&s, a).label, effectcraft_color::Label::Red, "the layer keeps its label");
    // Serde: labels round-trip; unlabeled keys don't write the field.
    let json = s.project.to_json();
    assert!(json.contains(&format!("\"label\": {blue}")) || json.contains(&format!("\"label\":{blue}")));
    let back = effectcraft_project::Project::from_json(&json).unwrap();
    assert_eq!(back.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(a)).unwrap().props.prop("transform/opacity").unwrap().keys[2].label, blue);
    // Moving a labelled key's value keeps the label.
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "transform/opacity", "value": 80})).unwrap();
    assert_eq!(layer(&s, a).props.prop("transform/opacity").unwrap().keys[2].label, blue);
    // Select Keyframe Label Group.
    s.state.selected_keys = vec![k(0.0)];
    let r = s.execute("keys.selectLabelGroup", json!({"scope": "all"})).unwrap();
    assert_eq!(r["keys"], 2);
    s.state.selected_keys = vec![k(0.5)];
    s.execute("keys.selectLabelGroup", json!({"scope": "selected"})).unwrap();
    assert_eq!(s.state.selected_keys, vec![k(0.5)]);
    s.state.selected_keys = vec![k(0.0)];
    let r = s.execute("keys.selectLabelGroup", json!({"scope": "visibleAll", "visible": [12345]})).unwrap();
    assert_eq!(r["keys"], 0, "only the given (visible) properties");
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(layer(&s, a).props.prop("transform/opacity").unwrap().keys.iter().all(|k| k.label == 0));
}

#[test]
fn rpf_camera_import_reads_json_and_csv_camera_data() {
    let mut s = session();
    let json_path = tmp("cam.json");
    std::fs::write(
        &json_path,
        r#"{"frameRate": 10, "frames": [{"frame": 0, "position": [0, 0, -500], "rotation": [0, 0, 0], "fov": 90}, {"frame": 10, "position": [100, 0, -500], "orientation": [0, 30, 0], "zoom": 400}]}"#,
    )
    .unwrap();
    let undo = s.history.undo.len();
    let r = s.execute("keys.rpfCameraImport", json!({"path": json_path.to_string_lossy()})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1);
    let cam = layer(&s, r["layer"].as_u64().unwrap());
    assert!(cam.is_camera());
    let pos = &cam.props.prop("transform/position").unwrap().keys;
    assert_eq!(pos.len(), 2);
    assert_eq!(pos[1].value, KV::Vec3([100.0, 0.0, -500.0]));
    assert!((pos[1].time.seconds() - 1.0).abs() < 1e-9);
    let zoom = &cam.props.prop("cameraOptions/zoom").unwrap().keys;
    assert!((zoom[0].value.as_f64() - 32.0).abs() < 1e-6, "90° across 64 px = zoom 32: {:?}", zoom[0].value);
    let csv = tmp("cam.csv");
    std::fs::write(&csv, "frame,px,py,pz,rx,ry,rz,zoom\n0,1,2,3,0,0,0,100\n5,4,5,6,10,20,30,200\n").unwrap();
    let r = s.execute("keys.rpfCameraImport", json!({"path": csv.to_string_lossy()})).unwrap();
    let cam = layer(&s, r["layer"].as_u64().unwrap());
    let o = &cam.props.prop("transform/orientation").unwrap().keys;
    assert_eq!(o[1].value, KV::Vec3([10.0, 20.0, 30.0]));
    assert!(s.execute("keys.rpfCameraImport", json!({"path": "/x/scene.rpf"})).unwrap_err().to_string().contains("JSON or CSV"));
}

// ---------------------------------------------------------------- menus

#[test]
fn dynamic_menus_and_window_viewer_labels() {
    let mut s = session();
    s.prefs.push_recent("/p/One.ecproj");
    let (e, _) = crate::menus::dynamic(&s, "recentProjects", &Default::default());
    assert_eq!((e[0].label.as_str(), e[0].command.as_str()), ("One.ecproj", "file.openRecent"));
    assert_eq!(crate::menus::dynamic(&s, "history", &Default::default()).0.len(), s.history.undo.len());
    let a = solid(&mut s, "#ff0000");
    s.state.selected_layers = vec![LayerId(a)];
    let entry = |cmd: &str, panel: &str| crate::menus::MenuEntry { label: "x".into(), command: cmd.into(), params: json!({"panel": panel}), shortcut: None };
    assert_eq!(crate::menus::entry_label(&s, &entry("window.panel", "composition")), "Composition: Main");
    assert_eq!(crate::menus::entry_label(&s, &entry("window.panel", "timeline")), "Timeline: Main");
    assert!(crate::menus::entry_label(&s, &entry("window.panel", "layer")).starts_with("Layer: "));
    assert_eq!(crate::menus::entry_label(&s, &entry("window.panel", "footage")), "Footage: (none)");
    // Other open comps are listed as viewers.
    let other = s.execute("comp.new", json!({"name": "Second"})).unwrap()["comp"].as_u64().unwrap();
    let (e, _) = crate::menus::dynamic(&s, "openViewers", &Default::default());
    assert_eq!(e.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Composition: Main"]);
    assert_eq!(e[0].command, "comp.open");
    let _ = other;
    // Every tree entry of the new menus resolves.
    for (path, e) in crate::menus::entries() {
        if ["History", "VR", "Recent Animation Presets", "Import Recent Footage"].iter().any(|p| path.contains(&p.to_string())) {
            assert!(crate::commands::find(&e.command).is_some());
        }
    }
    let labels: Vec<String> = crate::menus::entries().iter().map(|(_, e)| e.label.clone()).collect();
    for l in [
        "Save a Copy As XML...",
        "With Layered Comp",
        "Create VR Environment...",
        "Extract Cubemap...",
        "System Compatibility Report...",
        "Enable Logging",
        "Reveal Logging File",
    ] {
        assert!(labels.iter().any(|x| x == l), "{l}");
    }
}
