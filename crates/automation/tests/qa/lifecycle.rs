//! Scenario 6 — project lifecycle: save and reopen, Save a Copy As XML, Collect Files, Reduce
//! Project, undo/redo across 50 operations, a jump across history branches, render templates
//! and a watch folder.

use serde_json::json;

use crate::harness::{Qa, mean_diff};

#[test]
fn project_lifecycle() {
    let mut qa = Qa::new("lifecycle");
    // Footage to collect later.
    std::fs::create_dir_all(qa.dir.join("media")).unwrap();
    image::RgbaImage::from_pixel(32, 32, image::Rgba([10, 200, 90, 255])).save(qa.dir.join("media/logo.png")).unwrap();
    let logo = qa.exec("file.import", json!({"paths": [qa.path("media/logo.png")]}))["items"][0].clone();

    let main = qa.exec("comp.new", json!({"name": "Main", "width": 96, "height": 54, "frameRate": 12, "duration": 1}))["comp"].clone();
    qa.exec("layer.addItem", json!({"item": logo.clone()}));
    let unused = qa.exec("comp.new", json!({"name": "Unused", "width": 64, "height": 64}))["comp"].clone();
    qa.exec("layer.newSolid", json!({"name": "Junk", "color": "#ff0000"}));
    qa.exec("comp.open", json!({"comp": main.clone()}));

    // 50 edits, one undo step each. Levels of Undo defaults to 32 (as in After Effects).
    assert_eq!(qa.exec("prefs.get", json!({"key": "general.undoLevels"})), json!(32));
    qa.exec("prefs.set", json!({"key": "general.undoLevels", "value": 99}));
    let box_id = qa.exec("layer.newSolid", json!({"name": "Box", "color": "#2040ff", "width": 20, "height": 20}))["layer"].clone();
    let steps_before = qa.tool("get_project", json!({}))["undo"].as_array().unwrap().len();
    for i in 0..50 {
        qa.tool("set_property", json!({"layer": box_id.clone(), "path": "transform/position", "value": [10 + i, 27]}));
    }
    let s = qa.tool("get_project", json!({}));
    let n = s["undo"].as_array().unwrap().len();
    assert_eq!(n, (steps_before + 50).min(99), "50 more undo steps recorded");
    let end = qa.frame(None, 0.0);
    let u = qa.tool("undo", json!({"steps": 50}));
    assert_eq!(u["steps"], 50);
    let pos = qa.tool("get_property", json!({"layer": box_id.clone(), "path": "transform/position"}))["value"].clone();
    assert_eq!(pos, json!([48.0, 27.0, 0.0]), "back to the solid's default position");
    let r = qa.tool("redo", json!({"steps": 50}));
    assert_eq!(r["steps"], 50);
    assert!(mean_diff(&end, &qa.frame(None, 0.0)) < 1e-9, "redo restores the same frame");

    // Branching history: undo 5, make a different edit, then jump back to the old branch's tip.
    qa.tool("undo", json!({"steps": 5}));
    qa.tool("set_property", json!({"layer": box_id.clone(), "path": "transform/opacity", "value": 25}));
    let h = qa.tool("history", json!({}));
    let nodes = h.as_array().unwrap_or_else(|| h["states"].as_array().expect("history list"));
    let branch_tip = nodes.iter().filter(|n| n["depth"].as_u64().unwrap_or(0) > 0).max_by_key(|n| n["index"].as_u64()).expect("a branch").clone();
    qa.tool("history", json!({"goto": branch_tip["id"].clone()}));
    let pos = qa.tool("get_property", json!({"layer": box_id.clone(), "path": "transform/position"}))["value"].clone();
    assert_eq!(pos, json!([59.0, 27.0, 0.0]), "the old branch's last edit is back");
    let op = qa.tool("get_property", json!({"layer": box_id.clone(), "path": "transform/opacity"}))["value"].as_f64().unwrap();
    assert_eq!(op, 100.0, "and the other branch's edit is not on this line");

    // Save, reopen, compare.
    let proj = qa.path("show.ecproj");
    let saved = qa.tool("save_project", json!({"path": proj}));
    assert_eq!(saved["dirty"], false);
    let before = qa.frame(Some(main.clone()), 0.0);
    qa.tool("open_project", json!({"new": true}));
    let reopened = qa.tool("open_project", json!({"path": proj}));
    assert_eq!(reopened["path"], json!(proj));
    let main2 = reopened["items"].as_array().unwrap().iter().find(|i| i["name"] == "Main").unwrap()["id"].clone();
    assert!(mean_diff(&before, &qa.frame(Some(main2.clone()), 0.0)) < 1e-9, "the reopened project renders the same");

    // Save a Copy As XML: reopens to the same frame and leaves the session's path alone.
    let xml = qa.path("show.ecprojx");
    qa.exec("file.saveCopyAsXml", json!({"path": xml}));
    assert!(std::fs::read_to_string(&xml).unwrap().trim_start().starts_with('<'));
    assert_eq!(qa.tool("get_project", json!({}))["path"], json!(proj));
    qa.tool("open_project", json!({"path": xml}));
    assert!(mean_diff(&before, &qa.frame(Some(main2.clone()), 0.0)) < 1e-9, "the XML copy renders the same");
    qa.tool("open_project", json!({"path": proj}));

    // Collect Files: the project and its footage in one folder that opens on its own.
    let collected = qa.path("collected");
    let c = qa.exec("file.collectFiles", json!({"folder": collected}));
    assert_eq!((c["copied"].clone(), c["errors"].clone()), (json!(1), json!([])), "Collect Files makes its folder: {c}");
    let files: Vec<String> = walk(&qa.dir.join("collected"));
    assert!(files.iter().any(|f| f.ends_with(".ecproj")) && files.iter().any(|f| f.ends_with("logo.png")), "{files:?}");
    std::fs::remove_file(qa.dir.join("media/logo.png")).unwrap();
    let cproj = files.iter().find(|f| f.ends_with(".ecproj")).unwrap().clone();
    let reo = qa.tool("open_project", json!({"path": cproj}));
    let cm = reo["items"].as_array().unwrap().iter().find(|i| i["name"] == "Main").unwrap()["id"].clone();
    assert!(mean_diff(&before, &qa.frame(Some(cm), 0.0)) < 1e-9, "the collected project finds its footage");

    // Reduce Project to Main: Unused goes, the footage Main uses stays.
    qa.exec("project.select", json!({"items": [main2.clone()]}));
    qa.exec("file.reduceProject", json!({}));
    let names: Vec<String> = qa.tool("get_project", json!({}))["items"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap().to_string()).collect();
    assert!(names.contains(&"Main".to_string()) && names.contains(&"logo.png".to_string()) && !names.contains(&"Unused".to_string()), "{names:?}");
    let _ = unused;

    // Render templates: save an output-module template, queue with it, render.
    qa.exec("renderQueue.saveTemplate", json!({"kind": "outputModule", "name": "QA PNG", "params": {"format": "png", "channels": "rgba"}}));
    let tpl = qa.exec("renderQueue.templates", json!({}));
    assert!(tpl.to_string().contains("QA PNG"), "{tpl}");
    qa.exec("renderQueue.add", json!({"comp": "Main", "output": qa.path("out/main_[##].png")}));
    qa.exec("renderQueue.applyTemplate", json!({"index": 1, "outputModule": "QA PNG"}));
    let list = qa.exec("renderQueue.list", json!({}));
    assert_eq!(list["items"][0]["output"]["format"], "PngSequence", "{list}");
    assert_eq!(list["items"][0]["output"]["channels"], "Rgba");
    let done = qa.exec("renderQueue.render", json!({"wait": true}));
    assert_eq!(done["items"][0]["status"], "Done", "{done}");
    assert_eq!(walk(&qa.dir.join("out")).iter().filter(|f| f.ends_with(".png")).count(), 12);

    // Watch folder: a project with a queued render dropped into the folder is rendered.
    let watch = qa.dir.join("watch");
    std::fs::create_dir_all(&watch).unwrap();
    qa.exec("renderQueue.setRender", json!({"index": 1, "render": true}));
    qa.exec("renderQueue.setOutput", json!({"index": 1, "path": watch.join("w_[##].png").to_string_lossy()}));
    qa.tool("save_project", json!({"path": watch.join("job.ecproj").to_string_lossy()}));
    // Starting the watch renders what is already there; a poll then finds nothing new.
    let started = qa.exec("file.watchFolder", json!({"folder": watch.to_string_lossy()}));
    assert_eq!(started["rendered"][0]["state"], "done", "{started}");
    let polled = qa.exec("file.watchFolder.poll", json!({}));
    assert_eq!(polled["rendered"], json!([]), "{polled}");
    assert!(watch.join("job.ecproj.status.json").exists() || walk(&watch).iter().any(|f| f.ends_with(".status.json")));
    assert_eq!(walk(&watch).iter().filter(|f| f.ends_with(".png")).count(), 12, "{:?}", walk(&watch));
    qa.exec("file.watchFolder", json!({"folder": watch.to_string_lossy(), "stop": true}));
}

fn walk(dir: &std::path::Path) -> Vec<String> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p.to_string_lossy().into_owned());
            }
        }
    }
    out
}

#[test]
fn collect_files_keeps_footage_and_composition_proxies() {
    use effectcraft_engine::project::{ItemId, ItemKind, Project};

    let mut qa = Qa::new("collect-proxies");
    let originals = qa.dir.join("originals");
    std::fs::create_dir_all(&originals).unwrap();
    let assets = [
        ("plain.png", [255, 0, 0, 255]),
        ("enabled.png", [0, 255, 0, 255]),
        ("disabled.png", [255, 255, 0, 255]),
        ("enabled-proxy.png", [0, 255, 255, 255]),
        ("disabled-proxy.png", [0, 0, 255, 255]),
        ("comp-proxy.png", [255, 255, 255, 255]),
    ];
    for (name, color) in &assets {
        image::RgbaImage::from_pixel(32, 32, image::Rgba(*color)).save(originals.join(name)).unwrap();
    }
    let source_bytes: Vec<_> = assets.iter().map(|(name, _)| std::fs::read(originals.join(name)).unwrap()).collect();
    let mut imported = vec![];
    for (name, _) in &assets[..3] {
        imported.push(qa.exec("file.import", json!({"paths": [originals.join(name).to_string_lossy()]}))["items"][0].clone());
    }
    let (plain, enabled, disabled) = (imported[0].clone(), imported[1].clone(), imported[2].clone());
    let inner = qa.exec("comp.new", json!({"name": "Inner", "width": 32, "height": 32, "duration": 1}))["comp"].clone();
    qa.exec("layer.newSolid", json!({"name": "Magenta", "color": "#ff00ff", "width": 32, "height": 32}));
    for (item, name) in [(&enabled, "enabled-proxy.png"), (&disabled, "disabled-proxy.png"), (&inner, "comp-proxy.png")] {
        qa.exec("file.setProxy", json!({"item": item, "path": originals.join(name).to_string_lossy()}));
    }
    let main = qa.exec("comp.new", json!({"name": "Main", "width": 128, "height": 32, "duration": 1}))["comp"].clone();
    for (tile, item) in [&plain, &enabled, &disabled, &inner].into_iter().enumerate() {
        let layer = qa.exec("layer.addItem", json!({"item": item}))["layer"].clone();
        qa.tool("set_property", json!({"layer": layer, "path": "transform/position", "value": [16 + 32 * tile, 16]}));
    }
    let switches = |qa: &mut Qa, alternate: bool| {
        for (item, on) in [(&enabled, !alternate), (&disabled, alternate), (&inner, !alternate)] {
            qa.exec("file.useProxy", json!({"item": item, "on": on}));
        }
    };
    let pixels = |frame: &crate::harness::Img, expected: [[u8; 4]; 4]| {
        assert_eq!(frame.dimensions(), (128, 32));
        for (tile, color) in expected.into_iter().enumerate() {
            assert_eq!(frame.get_pixel(16 + 32 * tile as u32, 16).0, color, "tile {tile}");
        }
    };
    switches(&mut qa, false);
    let before = qa.frame(Some(main.clone()), 0.0);
    pixels(&before, [[255, 0, 0, 255], [0, 255, 255, 255], [255, 255, 0, 255], [255, 255, 255, 255]]);
    switches(&mut qa, true);
    let alternate = qa.frame(Some(main.clone()), 0.0);
    pixels(&alternate, [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 0, 255, 255]]);
    assert!(mean_diff(&before, &alternate) > 0.0, "both proxy states have observable positive controls");
    switches(&mut qa, false);

    let saved = qa.path("original.ecproj");
    qa.tool("save_project", json!({"path": saved}));
    let project_bytes = std::fs::read(&saved).unwrap();
    let project = Project::from_json(std::str::from_utf8(&project_bytes).unwrap()).unwrap();
    let archive = qa.path("collected");
    let collected = qa.exec("file.collectFiles", json!({"folder": archive}));
    assert_eq!(collected["errors"], json!([]), "{collected}");
    let live_copy = qa.path("live-after.ecproj");
    qa.exec("file.saveCopy", json!({"path": live_copy}));
    let live_project = Project::from_json(&std::fs::read_to_string(&live_copy).unwrap()).unwrap();
    assert_eq!(live_project, project, "collection preserves the full live project, including source paths and proxy switches");
    assert_eq!(qa.tool("get_project", json!({}))["path"], json!(saved), "collection and the snapshot leave the session path alone");
    assert_eq!(std::fs::read(&saved).unwrap(), project_bytes, "collection leaves the original project file alone");
    for (index, (name, _)) in assets.iter().enumerate() {
        assert_eq!(std::fs::read(originals.join(name)).unwrap(), source_bytes[index], "collection leaves {name} unchanged");
    }
    assert_eq!(mean_diff(&before, &qa.frame(Some(main.clone()), 0.0)), 0.0, "the live session still renders its original media");

    let archive_root = std::path::Path::new(&archive).canonicalize().unwrap();
    let archived_path = collected["project"].as_str().unwrap();
    assert!(std::path::Path::new(archived_path).canonicalize().unwrap().starts_with(&archive_root));
    let mut archived = Project::from_json(&std::fs::read_to_string(archived_path).unwrap()).unwrap();
    let entries = [(&plain, false), (&enabled, false), (&disabled, false), (&enabled, true), (&disabled, true), (&inner, true)];
    for (index, (id, is_proxy)) in entries.into_iter().enumerate() {
        let item = archived.items.get_mut(&ItemId(id.as_u64().unwrap())).unwrap();
        let footage = if is_proxy {
            &mut item.proxy.as_mut().expect("the proxy remains attached").footage
        } else {
            let ItemKind::Footage(footage) = &mut item.kind else { panic!("the source remains footage") };
            footage
        };
        let copied = std::path::Path::new(&footage.path).canonicalize().unwrap();
        assert!(copied.starts_with(&archive_root), "asset {} must live inside the archive: {}", assets[index].0, copied.display());
        assert_eq!(std::fs::read(copied).unwrap(), source_bytes[index], "archive preserves bytes for {}", assets[index].0);
        // Normalize only the six relocated file paths; every other item field must survive unchanged.
        footage.path = originals.join(assets[index].0).to_string_lossy().into_owned();
    }
    assert_eq!(serde_json::to_value(&archived.items).unwrap(), serde_json::to_value(&project.items).unwrap());
    assert_eq!(collected["copied"], json!(6), "three originals and all three attached proxies are collected");

    // Remove only the fixture's own originals. A new headless session prevents cached source frames from hiding broken links.
    std::fs::remove_dir_all(&originals).unwrap();
    assert!(!originals.exists());
    let mut reopened = Qa::new("collect-proxies-reopen");
    reopened.tool("open_project", json!({"path": archived_path}));
    assert_eq!(mean_diff(&before, &reopened.frame(Some(main.clone()), 0.0)), 0.0, "saved proxy switches still work without originals");
    switches(&mut reopened, true);
    assert_eq!(mean_diff(&alternate, &reopened.frame(Some(main), 0.0)), 0.0, "both proxies and full sources remain usable after relocation");
}

#[test]
fn collect_files_keeps_existing_proxy_sources_and_destination_files() {
    use effectcraft_engine::project::{ItemId, ItemKind, Project};

    let mut qa = Qa::new("collect-proxy-collision");
    let originals = qa.dir.join("originals");
    let archive = qa.dir.join("collected");
    let footage_dir = archive.join("(Footage)");
    std::fs::create_dir_all(&originals).unwrap();
    std::fs::create_dir_all(&footage_dir).unwrap();
    let sources = [originals.join("shared.png"), footage_dir.join("shared.png")];
    for (path, color) in sources.iter().zip([[255, 0, 0, 255], [0, 255, 255, 255]]) {
        image::RgbaImage::from_pixel(32, 32, image::Rgba(color)).save(path).unwrap();
    }
    let source_bytes: Vec<_> = sources.iter().map(|p| std::fs::read(p).unwrap()).collect();
    assert_ne!(source_bytes[0], source_bytes[1], "same basename, independent media bytes");
    // Occupy a likely renamed destination and the old writer's sibling temporary path.
    let sentinels = [
        (footage_dir.join("2_shared.png"), b"existing destination, keep these bytes".to_vec()),
        (footage_dir.join("3_2_shared.png.tmp"), b"existing temporary-path occupant, keep these bytes".to_vec()),
    ];
    for (path, bytes) in &sentinels {
        std::fs::write(path, bytes).unwrap();
    }

    let first = qa.exec("comp.new", json!({"name": "First", "width": 32, "height": 32, "duration": 1}))["comp"].clone();
    qa.exec("layer.newSolid", json!({"name": "Yellow", "color": "#ffff00", "width": 32, "height": 32}));
    let later = qa.exec("comp.new", json!({"name": "Later", "width": 32, "height": 32, "duration": 1}))["comp"].clone();
    qa.exec("layer.newSolid", json!({"name": "Magenta", "color": "#ff00ff", "width": 32, "height": 32}));
    assert!(first.as_u64().unwrap() < later.as_u64().unwrap(), "BTreeMap traversal must visit the external proxy first");
    for (item, source) in [&first, &later].into_iter().zip(&sources) {
        qa.exec("file.setProxy", json!({"item": item, "path": source.to_string_lossy()}));
    }
    let main = qa.exec("comp.new", json!({"name": "Main", "width": 64, "height": 32, "duration": 1}))["comp"].clone();
    for (tile, item) in [&first, &later].into_iter().enumerate() {
        let layer = qa.exec("layer.addItem", json!({"item": item}))["layer"].clone();
        qa.tool("set_property", json!({"layer": layer, "path": "transform/position", "value": [16 + 32 * tile, 16]}));
    }
    let pixels = |frame: &crate::harness::Img, expected: [[u8; 4]; 2]| {
        assert_eq!(frame.dimensions(), (64, 32));
        for (tile, color) in expected.into_iter().enumerate() {
            assert_eq!(frame.get_pixel(16 + 32 * tile as u32, 16).0, color, "tile {tile}");
        }
    };
    let switches = |qa: &mut Qa, on: bool| {
        for item in [&first, &later] {
            qa.exec("file.useProxy", json!({"item": item, "on": on}));
        }
    };
    let before = qa.frame(Some(main.clone()), 0.0);
    pixels(&before, [[255, 0, 0, 255], [0, 255, 255, 255]]);
    switches(&mut qa, false);
    pixels(&qa.frame(Some(main.clone()), 0.0), [[255, 255, 0, 255], [255, 0, 255, 255]]);
    switches(&mut qa, true);
    let saved = qa.path("original.ecproj");
    qa.tool("save_project", json!({"path": saved}));
    let project_bytes = std::fs::read(&saved).unwrap();
    let project = Project::from_json(std::str::from_utf8(&project_bytes).unwrap()).unwrap();
    assert!(project.items.values().all(|item| !matches!(&item.kind, ItemKind::Footage(_))), "isolate the new proxy traversal");

    let collected = qa.exec("file.collectFiles", json!({"folder": archive.to_string_lossy()}));
    // Check original disk bytes before metadata or rendering: a warm media cache must not hide replacement.
    for (index, source) in sources.iter().enumerate() {
        assert_eq!(std::fs::read(source).unwrap(), source_bytes[index], "collection must preserve original proxy {index}: {}", source.display());
    }
    for (path, bytes) in &sentinels {
        assert_eq!(&std::fs::read(path).unwrap(), bytes, "collection must preserve existing destination entry: {}", path.display());
    }
    assert_eq!((collected["copied"].clone(), collected["errors"].clone()), (json!(2), json!([])), "{collected}");
    assert_eq!(std::fs::read(&saved).unwrap(), project_bytes);
    let live_copy = qa.path("live-after.ecproj");
    qa.exec("file.saveCopy", json!({"path": live_copy}));
    assert_eq!(Project::from_json(&std::fs::read_to_string(&live_copy).unwrap()).unwrap(), project, "live paths and proxy switches stay unchanged");
    assert_eq!(qa.tool("get_project", json!({}))["path"], json!(saved));

    let archive_root = archive.canonicalize().unwrap();
    let archived_path = collected["project"].as_str().unwrap();
    assert!(std::path::Path::new(archived_path).canonicalize().unwrap().starts_with(&archive_root));
    let mut archived = Project::from_json(&std::fs::read_to_string(archived_path).unwrap()).unwrap();
    let mut copied_paths = std::collections::BTreeSet::new();
    for (index, id) in [&first, &later].into_iter().enumerate() {
        let proxy = archived.items.get_mut(&ItemId(id.as_u64().unwrap())).unwrap().proxy.as_mut().unwrap();
        assert!(proxy.enabled);
        assert!(proxy.footage.sequence.is_empty(), "this case covers single-image proxies");
        let copied = std::path::Path::new(&proxy.footage.path).canonicalize().unwrap();
        assert!(copied.starts_with(&archive_root), "proxy {index} must be inside the archive");
        assert!(copied_paths.insert(copied.clone()), "distinct media must not share an archive path");
        assert_eq!(std::fs::read(copied).unwrap(), source_bytes[index], "archive preserves proxy {index}");
        proxy.footage.path = sources[index].to_string_lossy().into_owned();
    }
    assert_eq!(archived, project, "only the two archived proxy paths may change");

    // Remove only external originals; the later source is already part of the archive and must stay.
    std::fs::remove_dir_all(&originals).unwrap();
    assert!(!sources[0].exists());
    let mut reopened = Qa::new("collect-proxy-collision-reopen");
    reopened.tool("open_project", json!({"path": archived_path}));
    pixels(&reopened.frame(Some(main.clone()), 0.0), [[255, 0, 0, 255], [0, 255, 255, 255]]);
    switches(&mut reopened, false);
    pixels(&reopened.frame(Some(main), 0.0), [[255, 255, 0, 255], [255, 0, 255, 255]]);
}
