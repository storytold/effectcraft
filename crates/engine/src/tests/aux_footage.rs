//! The footage files effects read the channels of (#474): the session gives the footage source
//! the whole set with every project change ([`Session::bump`]), so a multi-layer OpenEXR read by
//! EXtractoR or Cryptomatte decodes its picture and channels at once from the first frame, and
//! goes back to its picture alone once no effect reads it.

use std::sync::{Arc, Mutex};

use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, Project};
use effectcraft_raster::Image;
use effectcraft_time::{FrameRate, Tick};
use serde_json::json;

use crate::Session;
use crate::render::{FootageSource, RenderOpts};

/// Records every set it is given, and the set it had when it was first asked for a frame.
#[derive(Default)]
struct Recorder {
    sets: Mutex<Vec<Vec<String>>>,
    at_first_frame: Mutex<Option<Option<Vec<String>>>>,
    needs: Mutex<Vec<(String, effectcraft_raster::channels3d::AuxNeed)>>,
}

impl Recorder {
    fn last(&self) -> Option<Vec<String>> {
        self.sets.lock().unwrap().last().cloned()
    }
    fn calls(&self) -> usize {
        self.sets.lock().unwrap().len()
    }
}

impl FootageSource for Recorder {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        self.at_first_frame.lock().unwrap().get_or_insert_with(|| self.last());
        Some(Arc::new(Image::new(f.width, f.height)))
    }
    fn set_aux_footage(&self, paths: &[String]) {
        self.sets.lock().unwrap().push(paths.to_vec());
    }
    fn set_aux_needs(&self, needs: &[(String, effectcraft_raster::channels3d::AuxNeed)]) {
        *self.needs.lock().unwrap() = needs.to_vec();
    }
}

struct Probe;
impl crate::Importer for Probe {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        Ok(Footage {
            path: path.into(),
            kind: FootageKind::Still,
            width: 8,
            height: 4,
            has_video: true,
            frame_rate: FrameRate::from_f64(24.0),
            duration: Tick::from_seconds_f64(2.0),
            ..Default::default()
        })
    }
}

fn session(rec: &Arc<Recorder>) -> Session {
    let mut s = Session { footage: rec.clone(), importer: Some(Arc::new(Probe)), ..Default::default() };
    s.execute("comp.new", json!({"name": "C", "width": 8, "height": 4, "frameRate": 24, "duration": 2})).unwrap();
    s
}

fn import(s: &mut Session, path: &str) -> serde_json::Value {
    s.execute("file.import", json!({"paths": [path]})).unwrap()["items"][0].clone()
}

/// Add a layer of `item` to the active comp; its layer id.
fn add_layer(s: &mut Session, item: &serde_json::Value) -> u64 {
    s.execute("layer.addItem", json!({"item": item})).unwrap();
    s.state.selected_layers[0].0
}

fn paths(v: &[&str]) -> Option<Vec<String>> {
    Some(v.iter().map(|p| p.to_string()).collect())
}

/// Adding, disabling, removing an effect and undoing or redoing each change the set; edits that
/// don't change it don't send it again.
#[test]
fn effect_edits_undo_and_redo_update_the_set() {
    let rec = Arc::new(Recorder::default());
    let mut s = session(&rec);
    let item = import(&mut s, "/renders/a.exr");
    let l = add_layer(&mut s, &item);
    assert_eq!(rec.last(), paths(&[]), "no effect reads it yet");
    s.execute_checked("effect.apply", json!({"effect": "Gaussian Blur", "layers": [l]})).unwrap();
    assert_eq!(rec.last(), paths(&[]), "a blur doesn't read channels");
    s.execute_checked("effect.apply", json!({"effect": "EXtractoR", "layers": [l]})).unwrap();
    assert_eq!(rec.last(), paths(&["/renders/a.exr"]));
    assert!(s.undo());
    assert_eq!(rec.last(), paths(&[]), "undoing the add");
    assert!(s.redo());
    assert_eq!(rec.last(), paths(&["/renders/a.exr"]), "redoing it");
    let calls = rec.calls();
    s.execute_checked("effect.toggle", json!({"layer": l, "effect": "Gaussian Blur", "value": false})).unwrap();
    assert_eq!(rec.calls(), calls, "the set didn't change: it isn't sent again");
    s.execute_checked("effect.toggle", json!({"layer": l, "effect": "EXtractoR", "value": false})).unwrap();
    assert_eq!(rec.last(), paths(&[]), "a disabled effect reads nothing");
    s.execute_checked("effect.toggle", json!({"layer": l, "effect": "EXtractoR", "value": true})).unwrap();
    assert_eq!(rec.last(), paths(&["/renders/a.exr"]));
    s.execute_checked("effect.remove", json!({"layer": l, "effect": "EXtractoR"})).unwrap();
    assert_eq!(rec.last(), paths(&[]), "removed");
    assert!(s.undo());
    assert_eq!(rec.last(), paths(&["/renders/a.exr"]), "undoing the removal");
    assert!(s.redo());
    assert_eq!(rec.last(), paths(&[]), "redoing it");
    assert!(s.undo());
    s.execute_checked("effect.duplicate", json!({"layer": l, "effect": "EXtractoR"})).unwrap();
    s.execute_checked("effect.remove", json!({"layer": l, "effect": "EXtractoR"})).unwrap();
    assert_eq!(rec.last(), paths(&["/renders/a.exr"]), "the duplicate still reads it");
}

/// Opening a project gives the set before the first frame is rendered; opening another sends
/// it again even when it is empty.
#[test]
fn opening_a_project_gives_the_set_before_the_first_frame() {
    let made = Arc::new(Recorder::default());
    let mut s = session(&made);
    let item = import(&mut s, "/renders/crypto.exr");
    let l = add_layer(&mut s, &item);
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte", "layers": [l]})).unwrap();
    let project: Project = (*s.project).clone();
    let comp = s.state.active_comp.unwrap();

    let rec = Arc::new(Recorder::default());
    let mut s = Session { footage: rec.clone(), ..Default::default() };
    s.replace_project(project, None);
    assert_eq!(rec.last(), paths(&["/renders/crypto.exr"]));
    s.render(comp, Tick::ZERO, RenderOpts::default());
    assert_eq!(*rec.at_first_frame.lock().unwrap(), Some(paths(&["/renders/crypto.exr"])), "given before the first frame");
    s.replace_project(Project::default(), None);
    assert_eq!(rec.last(), paths(&[]));
    let calls = rec.calls();
    s.replace_project(Project::default(), None);
    assert_eq!(rec.calls(), calls + 1, "each opened project gets its set");
}

/// Two footage items of one file: the file stays in while an effect reads either of them.
#[test]
fn two_items_of_one_file() {
    let rec = Arc::new(Recorder::default());
    let mut s = session(&rec);
    let a = import(&mut s, "/renders/shared.exr");
    let f = match &s.project.item(ItemId(a.as_u64().unwrap())).unwrap().kind {
        ItemKind::Footage(f) => f.clone(),
        k => panic!("{k:?}"),
    };
    let b = s.edit("Add", None, |p, _| Ok(p.add_item("shared copy", crate::color::Label::None, None, ItemKind::Footage(f)))).unwrap();
    let la = add_layer(&mut s, &a);
    let lb = add_layer(&mut s, &json!(b.0));
    s.execute_checked("effect.apply", json!({"effect": "EXtractoR", "layers": [la]})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "ID Matte", "layers": [lb]})).unwrap();
    assert_eq!(rec.last(), paths(&["/renders/shared.exr"]), "once");
    s.execute_checked("effect.remove", json!({"layer": la, "effect": "EXtractoR"})).unwrap();
    assert_eq!(rec.last(), paths(&["/renders/shared.exr"]), "the other item still reads it");
    s.execute_checked("effect.remove", json!({"layer": lb, "effect": "ID Matte"})).unwrap();
    assert_eq!(rec.last(), paths(&[]));
}

/// Replacing a layer's source or an item's file moves the set to the new file.
#[test]
fn source_and_footage_replacement_follow() {
    let rec = Arc::new(Recorder::default());
    let mut s = session(&rec);
    let a = import(&mut s, "/renders/a.exr");
    let b = import(&mut s, "/renders/b.exr");
    let l = add_layer(&mut s, &a);
    s.execute_checked("effect.apply", json!({"effect": "Depth Matte", "layers": [l]})).unwrap();
    assert_eq!(rec.last(), paths(&["/renders/a.exr"]));
    s.execute_checked("layer.replaceSource", json!({"layers": [l], "item": b})).unwrap();
    assert_eq!(rec.last(), paths(&["/renders/b.exr"]), "the layer's new source");
    s.execute_checked("file.replaceFootage", json!({"item": b, "path": "/renders/c.exr"})).unwrap();
    assert_eq!(rec.last(), paths(&["/renders/c.exr"]), "the item's new file");
}

/// A watch-folder render opens its project in a session sharing the host's footage source: the
/// host's set is given back afterwards.
#[test]
fn a_watch_folder_render_gives_the_host_set_back() {
    let dir = std::env::temp_dir().join(format!("ec-aux-footage-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut other = Session::default();
    other.execute("comp.new", json!({"name": "Idle", "width": 8, "height": 8, "duration": 1})).unwrap();
    other.execute("file.saveAs", json!({"path": dir.join("idle.ecproj").to_string_lossy()})).unwrap();

    let rec = Arc::new(Recorder::default());
    let mut s = session(&rec);
    let item = import(&mut s, "/renders/a.exr");
    let l = add_layer(&mut s, &item);
    s.execute_checked("effect.apply", json!({"effect": "EXtractoR", "layers": [l]})).unwrap();
    let calls = rec.calls();
    let r = s.execute("file.watchFolder", json!({"folder": dir.to_string_lossy()})).unwrap();
    assert_eq!(r["rendered"].as_array().map(Vec::len), Some(1), "{r}");
    assert!(rec.calls() > calls + 1, "the watched project gave its own set");
    assert_eq!(rec.last(), paths(&["/renders/a.exr"]), "the host's set is back");
    s.execute("file.watchFolder", json!({"stop": true})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// #3271: the parameters that pick the channels an effect reads can't be animated, as in After
/// Effects (no stopwatch there either): a keyframe or an expression is refused, the value still
/// changes; what a frame reads is exactly the channels picked.
#[test]
fn channel_selection_is_static_and_says_what_is_read() {
    use effectcraft_raster::channels3d::{AuxLookup, AuxNeed};
    let rec = Arc::new(Recorder::default());
    let mut s = session(&rec);
    let item = import(&mut s, "/renders/static.exr");
    let l = add_layer(&mut s, &item);
    s.execute_checked("effect.apply", json!({"effect": "EXtractoR", "layers": [l]})).unwrap();
    let red = "effects/#1/red";
    let prop = |s: &Session, path: &str| {
        let c = s.active_comp().unwrap();
        c.layer(effectcraft_project::LayerId(l)).unwrap().props.prop(path).unwrap().clone()
    };
    assert!(prop(&s, red).static_only, "no stopwatch");
    assert!(s.execute_checked("prop.addKey", json!({"layer": l, "path": red})).is_err(), "no keyframe");
    assert!(s.execute_checked("prop.toggleKey", json!({"layer": l, "path": red})).is_err(), "no keyframe");
    assert!(s.execute_checked("prop.toggleAnimation", json!({"layer": l, "path": red})).is_err(), "no stopwatch");
    assert!(s.execute_checked("prop.setExpression", json!({"layer": l, "path": red, "expression": "'Z'"})).is_err(), "no expression");
    assert!(prop(&s, red).keys.is_empty() && prop(&s, red).expr.is_none());
    // The value changes; the channels read are exactly those picked.
    s.execute_checked("prop.set", json!({"layer": l, "path": red, "value": "Z"})).unwrap();
    for c in ["green", "blue", "alpha"] {
        s.execute_checked("prop.set", json!({"layer": l, "path": format!("effects/#1/{c}"), "value": ""})).unwrap();
    }
    assert_eq!(prop(&s, red).value, effectcraft_keyframe::Value::Str("Z".into()));
    let needs = rec.needs.lock().unwrap().clone();
    assert_eq!(needs, [("/renders/static.exr".to_string(), AuxNeed::Lookups(vec![AuxLookup::Name("Z".into())]))]);
}

/// #3271: a document that animated a channel selection (it can't come from After Effects, nor
/// from EffectCraft now) opens with the first key's value as the static one, its keys and
/// expression dropped, and says so.
#[test]
fn a_keyed_channel_selection_opens_static() {
    use effectcraft_keyframe::{Keyframe, Value as K};
    let rec = Arc::new(Recorder::default());
    let mut s = session(&rec);
    let item = import(&mut s, "/renders/keyed.exr");
    let l = add_layer(&mut s, &item);
    s.execute_checked("effect.apply", json!({"effect": "EXtractoR", "layers": [l]})).unwrap();
    let cid = s.active_comp_id().unwrap();
    {
        let p = Arc::make_mut(&mut s.project);
        let pr = p.comp_mut(cid).unwrap().layer_mut(effectcraft_project::LayerId(l)).unwrap().props.prop_mut("effects/#1/red").unwrap();
        // (as a document from before)
        pr.static_only = false;
        pr.keys = vec![Keyframe::new(Tick::ZERO, K::Str("Z".into())), Keyframe::new(Tick::from_seconds_f64(1.0), K::Str("N.X".into()))];
    }
    let dir = std::env::temp_dir().join(format!("ec-3271-static-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("keyed.ecproj");
    s.execute_checked("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    let mut o = session(&rec);
    o.drain_events();
    o.execute_checked("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    let c = o.active_comp().unwrap();
    let pr = c.layer(effectcraft_project::LayerId(l)).unwrap().props.prop("effects/#1/red").unwrap();
    assert!(pr.static_only && pr.keys.is_empty() && pr.expr.is_none());
    assert_eq!(pr.value, K::Str("Z".into()), "the first key's value");
    let said = o.drain_events().into_iter().any(|e| matches!(e, crate::Event::Toast { message, .. } if message.contains("Channel selection")));
    assert!(said, "the loss is reported");
    let _ = std::fs::remove_dir_all(&dir);
}
