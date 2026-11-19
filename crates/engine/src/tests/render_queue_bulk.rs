//! #3726: several compositions go into the Render Queue at once, and one change reaches every
//! selected item (`items`) as one undo step, keeping each item's output name.

use effectcraft_project::render_queue::{OutputFormat, RenderLog, RenderQuality, RenderStatus};
use serde_json::{Value, json};

use crate::Session;

/// Three comps, `Alpha`, `Beta` and `Gamma`, and their ids.
fn three_comps() -> (Session, Vec<u64>) {
    let mut s = Session::default();
    let comps = ["Alpha", "Beta", "Gamma"]
        .iter()
        .map(|name| s.execute("comp.new", json!({"name": name, "width": 64, "height": 36, "duration": 1})).unwrap()["comp"].as_u64().unwrap())
        .collect();
    (s, comps)
}

fn ids(s: &Session) -> Vec<u64> {
    s.project.render_queue.iter().map(|i| i.id).collect()
}

fn outputs(s: &Session) -> Vec<String> {
    s.project.render_queue.iter().map(|i| i.output.output.clone()).collect()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap()
}

/// Every selected comp gets an item, in the order given, as one undo step.
#[test]
fn comps_are_added_together_and_undone_together() {
    let (mut s, comps) = three_comps();
    let steps = s.history.undo.len();
    let r = s.execute("renderQueue.add", json!({"comps": comps})).unwrap();
    let queue = &s.project.render_queue;
    assert_eq!(queue.iter().map(|i| i.comp.0).collect::<Vec<_>>(), comps, "one item per comp, in order");
    assert_eq!(r["added"], json!(ids(&s)));
    assert_eq!(r["items"].as_array().map(Vec::len), Some(3));
    let names: Vec<String> = queue.iter().filter_map(|i| s.resolve_output(i)).map(|o| file_name(&o).to_string()).collect();
    assert_eq!(names, ["Alpha.mp4", "Beta.mp4", "Gamma.mp4"], "each output is named after its comp");
    assert_eq!(s.history.undo.len(), steps + 1, "one undo step");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.render_queue.is_empty(), "one undo removes all three");
}

/// A comp named twice is queued once; an unknown comp or an empty list queues nothing.
#[test]
fn bad_comp_lists_queue_nothing() {
    let (mut s, comps) = three_comps();
    s.execute("renderQueue.add", json!({"comps": [comps[0], "Alpha", comps[1]]})).unwrap();
    assert_eq!(s.project.render_queue.len(), 2);
    for bad in [json!({"comps": [comps[2], 9999]}), json!({"comps": []}), json!({"comps": [null]}), json!({"comps": "Alpha"})] {
        assert!(s.execute("renderQueue.add", bad.clone()).is_err(), "{bad}");
        assert_eq!(s.project.render_queue.len(), 2, "{bad} queued nothing");
    }
}

/// Render Settings, Output Module and template changes reach every selected item and leave the
/// others; each item keeps its output name; one undo step puts all of them back.
#[test]
fn settings_reach_every_selected_item_and_keep_their_names() {
    let (mut s, comps) = three_comps();
    s.execute("renderQueue.add", json!({"comps": comps})).unwrap();
    let all = ids(&s);
    let picked = json!([all[0], all[2]]);
    for (id, name) in all.iter().zip(["one", "two", "three"]) {
        s.execute("renderQueue.setOutput", json!({"item": id, "path": format!("/out/{name}.mp4")})).unwrap();
    }
    let names = ["one", "two", "three"];
    let before = s.project.render_queue.clone();

    let r = s.execute("renderQueue.setRenderSettings", json!({"items": picked, "template": "Draft Settings"})).unwrap();
    assert_eq!(r["items"].as_array().map(Vec::len), Some(2));
    let q = &s.project.render_queue;
    assert_eq!([q[0].settings.quality, q[1].settings.quality, q[2].settings.quality], [RenderQuality::Draft, RenderQuality::Best, RenderQuality::Draft]);

    let steps = s.history.undo.len();
    s.execute("renderQueue.setOutputModule", json!({"items": all, "format": "prores"})).unwrap();
    assert_eq!(s.history.undo.len(), steps + 1);
    for (it, name) in s.project.render_queue.iter().zip(&names) {
        assert_eq!(it.output.format, OutputFormat::ProRes);
        assert_eq!(it.output.output, format!("/out/{name}.mov"), "the name is kept, the extension follows the format");
    }

    s.execute("renderQueue.applyTemplate", json!({"items": picked, "outputModule": "Lossless with Alpha", "renderSettings": "Best Settings"})).unwrap();
    let q = &s.project.render_queue;
    assert_eq!([q[0].output.name.as_str(), q[2].output.name.as_str()], ["Lossless with Alpha", "Lossless with Alpha"]);
    assert_ne!(q[1].output.name, "Lossless with Alpha", "the item not selected is left alone");
    assert_eq!(q[0].settings.quality, RenderQuality::Best);
    assert!(s.project.render_queue.iter().zip(&names).all(|(it, n)| it.output.output.starts_with(&format!("/out/{n}."))), "names kept: {:?}", outputs(&s));

    s.execute("renderQueue.setLog", json!({"items": all, "log": "plusSettings"})).unwrap();
    assert!(s.project.render_queue.iter().all(|i| i.log == RenderLog::PlusSettings));

    for _ in 0..4 {
        s.execute("edit.undo", json!({})).unwrap();
    }
    assert_eq!(s.project.render_queue, before, "four changes, four undo steps");
}

/// Output To with `folder` moves every selected output there and keeps each file name.
#[test]
fn a_folder_keeps_each_output_name() {
    let (mut s, comps) = three_comps();
    s.execute("renderQueue.add", json!({"comps": comps})).unwrap();
    let all = ids(&s);
    for (id, name) in all.iter().zip(["a.mp4", "b.mp4", "c.mp4"]) {
        s.execute("renderQueue.setOutput", json!({"item": id, "path": format!("/first/{name}")})).unwrap();
    }
    s.execute("renderQueue.setOutput", json!({"items": [all[0], all[1]], "folder": "/renders/final/"})).unwrap();
    assert_eq!(outputs(&s), ["/renders/final/a.mp4", "/renders/final/b.mp4", "/first/c.mp4"]);
    s.execute("renderQueue.setOutput", json!({"items": all, "folder": "C:\\out"})).unwrap();
    assert_eq!(outputs(&s), ["C:\\out\\a.mp4", "C:\\out\\b.mp4", "C:\\out\\c.mp4"]);
    assert!(s.execute("renderQueue.setOutput", json!({"items": all})).is_err(), "neither path nor folder");
}

/// Remove, the Render checkbox and Move act on every selected item.
#[test]
fn remove_render_and_move_act_on_the_selection() {
    let (mut s, comps) = three_comps();
    s.execute("renderQueue.add", json!({"comps": comps})).unwrap();
    s.execute("renderQueue.add", json!({"comp": comps[0]})).unwrap();
    let all = ids(&s);

    s.execute("renderQueue.setRender", json!({"items": [all[1], all[3]]})).unwrap();
    let on: Vec<bool> = s.project.render_queue.iter().map(|i| i.render).collect();
    assert_eq!(on, [true, false, true, false], "the first selected item's checkbox decides for all");
    assert_eq!(s.project.render_queue[3].status, RenderStatus::Unqueued);

    s.execute("renderQueue.move", json!({"items": [all[1], all[3]], "to": 1})).unwrap();
    assert_eq!(ids(&s), [all[1], all[3], all[0], all[2]], "moved together, in queue order");
    s.execute("renderQueue.move", json!({"items": [all[1], all[3]], "to": 99})).unwrap();
    assert_eq!(ids(&s), [all[0], all[2], all[1], all[3]], "past the end goes to the end");
    s.execute("renderQueue.move", json!({"item": all[3], "to": 1})).unwrap();
    assert_eq!(ids(&s), [all[3], all[0], all[2], all[1]], "a single item still moves on its own");

    let r = s.execute("renderQueue.remove", json!({"items": [all[0], all[1], all[0]]})).unwrap();
    assert_eq!(r, json!({"remaining": 2, "removed": 2}));
    assert_eq!(ids(&s), [all[3], all[2]]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(ids(&s), [all[3], all[0], all[2], all[1]], "one undo brings both back in place");
}

/// An unknown or malformed `items` changes nothing.
#[test]
fn bad_item_lists_change_nothing() {
    let (mut s, comps) = three_comps();
    s.execute("renderQueue.add", json!({"comps": comps})).unwrap();
    let all = ids(&s);
    let before = s.project.render_queue.clone();
    for (cmd, p) in [
        ("renderQueue.remove", json!({"items": [all[0], 9999]})),
        ("renderQueue.remove", json!({"items": []})),
        ("renderQueue.remove", json!({"items": all[0]})),
        ("renderQueue.setRenderSettings", json!({"items": ["x"], "quality": "draft"})),
        ("renderQueue.setOutputModule", json!({"items": all, "module": 2, "format": "prores"})),
        ("renderQueue.move", json!({"items": [all[2]]})),
    ] {
        let r: Result<Value, _> = s.execute(cmd, p.clone());
        assert!(r.is_err(), "{cmd} {p}");
        assert_eq!(s.project.render_queue, before, "{cmd} {p} changed nothing");
    }
}
