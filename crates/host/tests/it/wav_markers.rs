//! Markers a WAV file carries, through the fully wired session (media import included): the
//! layer made from the file starts with them, and Update Markers From Source copies them again,
//! through its bound `layers` parameter as well as the selection, as one undo step.

use effectcraft_engine::Session;
use effectcraft_project::LayerSource;
use serde_json::json;

/// The comments of a layer's markers, in time order.
fn names(s: &mut Session, layer: u64) -> Vec<String> {
    let list = s.execute_checked("markers.list", json!({"layer": layer})).unwrap();
    list.as_array().unwrap().iter().map(|m| m["comment"].as_str().unwrap().to_string()).collect()
}

/// A one-second mono 8 kHz WAV with the named cue points `(id, sample, name)` (a `cue ` chunk and a
/// `LIST` `adtl`).
fn marker_wav(points: &[(u32, u32, &str)]) -> Vec<u8> {
    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((body.len() as u32).to_le_bytes());
        v.extend(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }
    let mut fmt = Vec::new();
    fmt.extend(1u16.to_le_bytes());
    fmt.extend(1u16.to_le_bytes());
    fmt.extend(8000u32.to_le_bytes());
    fmt.extend(16000u32.to_le_bytes());
    fmt.extend(2u16.to_le_bytes());
    fmt.extend(16u16.to_le_bytes());
    let mut cue = (points.len() as u32).to_le_bytes().to_vec();
    let mut adtl = b"adtl".to_vec();
    for &(id, at, name) in points {
        for v in [id, 0] {
            cue.extend(v.to_le_bytes());
        }
        cue.extend(b"data");
        for v in [0, 0, at] {
            cue.extend(u32::to_le_bytes(v));
        }
        let mut labl = id.to_le_bytes().to_vec();
        labl.extend(name.as_bytes());
        labl.push(0);
        adtl.extend(chunk(b"labl", &labl));
    }
    let mut body = b"WAVE".to_vec();
    for c in [chunk(b"fmt ", &fmt), chunk(b"data", &[0; 16000]), chunk(b"cue ", &cue), chunk(b"LIST", &adtl)] {
        body.extend(c);
    }
    let mut file = b"RIFF".to_vec();
    file.extend((body.len() as u32).to_le_bytes());
    file.extend(body);
    file
}

/// A WAV's cue points become markers on the layer made from it, named as in the file, and
/// Update Markers From Source brings back one that was deleted (through the fully wired session,
/// media import included).
#[test]
fn a_wav_layer_starts_with_the_markers_in_its_file() {
    let mut s = effectcraft_host::session();
    s.execute_checked("comp.new", json!({"name": "C", "width": 64, "height": 48, "frameRate": 30, "duration": 2})).unwrap();
    let dir = std::env::temp_dir().join(format!("effectcraft-host-wav-markers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cues.wav");
    std::fs::write(&path, marker_wav(&[(1, 0, "Intro"), (2, 4000, "Middle")])).unwrap();
    let r = s.execute_checked("file.import", json!({"paths": [path.to_string_lossy()], "addToComp": true})).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    let layer = s.active_comp().unwrap().layers.iter().find(|l| matches!(l.source, LayerSource::Footage { .. })).unwrap().id.0;
    let list = s.execute_checked("markers.list", json!({"layer": layer})).unwrap();
    let marks: Vec<(String, f64)> =
        list.as_array().unwrap().iter().map(|m| (m["comment"].as_str().unwrap().to_string(), m["time"].as_f64().unwrap())).collect();
    assert_eq!(marks, [("Intro".to_string(), 0.0), ("Middle".to_string(), 0.5)]);
    s.execute_checked("markers.delete", json!({"layer": layer, "index": 0})).unwrap();
    assert_eq!(names(&mut s, layer), ["Middle"]);
    s.execute_checked("layer.select", json!({"layers": [layer]})).unwrap();
    // The bound `layers` parameter (what a script or the menu's own binding passes) brings the marker back...
    let r = s.execute_checked("layer.updateMarkersFromSource", json!({"layers": [layer]})).unwrap();
    assert_eq!(r["added"], 1);
    assert_eq!(names(&mut s, layer), ["Intro", "Middle"]);
    // ...as one undo step: undo removes it again, redo restores it.
    assert!(s.undo());
    assert_eq!(names(&mut s, layer), ["Middle"]);
    assert!(s.redo());
    assert_eq!(names(&mut s, layer), ["Intro", "Middle"]);
    // The selection route (no parameter) finds nothing more to add.
    assert_eq!(s.execute_checked("layer.updateMarkersFromSource", json!({})).unwrap()["added"], 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reload Footage re-reads the markers the file carries: a cue point added to the file after the
/// import is available to Update Markers From Source (and a layer already made keeps its own).
#[test]
fn reloading_a_wav_picks_up_cue_points_added_to_the_file() {
    let mut s = effectcraft_host::session();
    s.execute_checked("comp.new", json!({"name": "C", "width": 64, "height": 48, "frameRate": 30, "duration": 2})).unwrap();
    let dir = std::env::temp_dir().join(format!("effectcraft-host-wav-reload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cues.wav");
    std::fs::write(&path, marker_wav(&[(1, 0, "Intro")])).unwrap();
    let r = s.execute_checked("file.import", json!({"paths": [path.to_string_lossy()], "addToComp": true})).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    let (layer, item) = s
        .active_comp()
        .unwrap()
        .layers
        .iter()
        .find_map(|l| match l.source {
            LayerSource::Footage { item } => Some((l.id.0, item.0)),
            _ => None,
        })
        .unwrap();
    assert_eq!(names(&mut s, layer), ["Intro"]);
    // The file gains a cue point; the layer does not change until asked.
    std::fs::write(&path, marker_wav(&[(1, 0, "Intro"), (2, 4000, "Added later")])).unwrap();
    let r = s.execute_checked("file.reloadFootage", json!({"items": [item]})).unwrap();
    assert_eq!(r["reloaded"], 1, "{r}");
    assert_eq!(names(&mut s, layer), ["Intro"]);
    let r = s.execute_checked("layer.updateMarkersFromSource", json!({"layers": [layer]})).unwrap();
    assert_eq!(r["added"], 1);
    assert_eq!(names(&mut s, layer), ["Intro", "Added later"]);
    let _ = std::fs::remove_dir_all(&dir);
}
