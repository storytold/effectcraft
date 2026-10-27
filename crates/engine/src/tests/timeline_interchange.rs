//! File ▸ Import / Export ▸ Adobe Premiere Pro Project… (timeline interchange) through the command
//! registry, without media decoding (pixels: `crates/host/tests/it/timeline_interop.rs`).

use serde_json::json;

use crate::Session;
use crate::project::{ItemId, ItemKind, LayerSource};

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("effectcraft-timeline-tests-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

#[test]
fn export_and_reimport_timeline_commands() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 25, "duration": 2})).unwrap();
    let full = s.execute("layer.newSolid", json!({"color": "#ff8000"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": full, "path": "transform/opacity", "value": 40})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#0080ff", "width": 50, "height": 50})).unwrap();
    s.execute("layer.newNull", json!({})).unwrap();
    let main = s.active_comp_id().unwrap().0;
    for (ext, format) in [("xml", "xml"), ("fcpxml", "fcpxml"), ("otio", "otio")] {
        let path = tmp(&format!("Main.{ext}"));
        // No exporter in a bare session: the small solid can't be pre-rendered and is left out.
        let r = s.execute_checked("file.exportTimeline", json!({"comp": main, "path": path})).unwrap();
        assert_eq!(r["format"], format);
        // Only Final Cut Pro XML carries colour mattes; the other formats would pre-render solids.
        assert_eq!(r["videoTracks"], if format == "xml" { 1 } else { 0 }, "{r}");
        assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("pre-render")), "{r}");
        let r = s.execute_checked("file.importTimeline", json!({"path": path})).unwrap();
        assert_eq!(r["format"], format);
        let cid = ItemId(r["comps"][0].as_u64().unwrap());
        assert_eq!(s.active_comp_id(), Some(cid));
        let c = s.project.comp(cid).unwrap();
        assert_eq!((c.width, c.height), (320, 180));
        if format != "xml" {
            assert!(c.layers.is_empty());
            continue;
        }
        assert_eq!(c.layers.len(), 1);
        let l = &c.layers[0];
        let LayerSource::Solid { item } = l.source else { panic!("{:?}", l.source) };
        let ItemKind::Solid(sol) = &s.project.item(item).unwrap().kind else { panic!() };
        assert!((sol.color[0] - 1.0).abs() < 0.01 && (sol.color[2]).abs() < 0.01, "{:?}", sol.color);
        assert!((l.transform().unwrap().get("opacity").unwrap().value.as_f64() - 40.0).abs() < 1e-6);
        let folder = ItemId(r["folder"].as_u64().unwrap());
        assert_eq!(s.project.item(folder).unwrap().name, format!("Main.{ext}"));
    }
    // A path without an extension gets the format's.
    let r = s.execute_checked("file.exportTimeline", json!({"comp": main, "path": tmp("NoExt"), "format": "otio", "prerender": "none"})).unwrap();
    assert!(r["path"].as_str().unwrap().ends_with("NoExt.otio"));
    assert!(s.execute_checked("file.exportTimeline", json!({"path": tmp("x.xml"), "prerender": "sometimes"})).is_err());
    assert!(s.execute_checked("file.importTimeline", json!({"path": "/nope/a.prproj"})).is_err());
    let formats = s.execute("file.timelineFormats", json!({})).unwrap();
    assert_eq!(formats.as_array().unwrap().len(), 6);
}

#[test]
fn premiere_menu_entries_keep_after_effects_labels() {
    let entries = crate::menus::entries();
    let find = |cmd: &str| entries.iter().find(|(_, e)| e.command == cmd).map(|(p, e)| (p.join(" > "), e.label.clone()));
    assert_eq!(find("file.exportTimeline"), Some(("File > Export".into(), "Adobe Premiere Pro Project...".into())));
    assert_eq!(find("file.importTimeline"), Some(("File > Import".into(), "Adobe Premiere Pro Project...".into())));
}
