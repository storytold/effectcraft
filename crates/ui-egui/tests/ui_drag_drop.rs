//! Drag and drop between panels (egui_kittest, real pointer drags): Project
//! items land in the Timeline where they are dropped, between layers and, over the time graph,
//! starting there (#89); dropped on the Composition viewer they are centred where they land
//! (#85); an effect dropped on the viewer goes on the layer under the pointer (#88). Files
//! dropped on the window import with visible progress (#270).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::Value as KV;
use effectcraft_engine::time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::panels::viewer::{comp_to_screen, last_fit};
use egui::{Event, Modifiers, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// Comp "Main" (4 s at 30 fps, current time 1 s) with solids Top, Middle and Bottom, and comp
/// "Clip" in the Project panel to drag in.
fn harness() -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    let clip = s.execute("comp.new", json!({"name": "Clip", "width": 160, "height": 90, "frameRate": 30, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    for name in ["Bottom", "Middle", "Top"] {
        s.execute("layer.newSolid", json!({"name": name, "color": "#406080"})).unwrap();
    }
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    (h, clip)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

/// Press on `from`, move to `to` in steps, release there (with `modifiers` held).
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::ModifiersChanged(modifiers));
    for k in 1..=10 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 10.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(3);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

/// Layer names top to bottom, and the In point of `name`.
fn stack(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.state().session.active_comp().unwrap().layers.iter().map(|l| l.name.clone()).collect()
}

fn in_point(h: &Harness<'_, EffectcraftApp>, index: usize) -> f64 {
    h.state().session.active_comp().unwrap().layers[index].in_point.seconds()
}

fn layer_id(h: &Harness<'_, EffectcraftApp>, name: &str) -> u64 {
    h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == name).unwrap().id.0
}

/// #89: dropped on the layer outline, an item goes in between the layers there; over the time
/// graph it also starts where it was dropped, or at the current time with Shift.
#[test]
fn project_items_land_where_they_are_dropped_in_the_timeline() {
    let (mut h, clip) = harness();
    let item = rect(&h, &format!("project.item.{clip}.name")).center();
    let frame = 1.0 / 30.0;

    // Between Top and Middle in the outline: the lower half of Top's row.
    let top = rect(&h, &format!("timeline.layer.{}.row", layer_id(&h, "Top")));
    drag(&mut h, item, pos2(top.center().x, top.max.y - 2.0), Modifiers::NONE);
    assert_eq!(stack(&h), ["Top", "Clip", "Middle", "Bottom"]);
    // (Settings ▸ General ▸ Create Layers at Composition Start Time, on by default.)
    assert_eq!(in_point(&h, 1), 0.0, "starts where new layers start");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);

    // Over the time graph at 2 s, on the upper half of Bottom's row: above Bottom, starting at 2 s.
    let bottom = layer_id(&h, "Bottom");
    let row = rect(&h, &format!("timeline.layer.{bottom}.row"));
    let bar = rect(&h, &format!("timeline.layer.{bottom}.bar"));
    let at_2s = bar.min.x + bar.width() * 0.5;
    drag(&mut h, item, pos2(at_2s, row.min.y + 2.0), Modifiers::NONE);
    assert_eq!(stack(&h), ["Top", "Middle", "Clip", "Bottom"]);
    assert!((in_point(&h, 2) - 2.0).abs() <= frame + 1e-9, "starts where it was dropped: {}", in_point(&h, 2));
    assert_eq!(
        Tick::from_seconds_f64(in_point(&h, 2)),
        h.state().session.active_comp().unwrap().frame_rate.snap_nearest(Tick::from_seconds_f64(in_point(&h, 2))),
        "on a frame"
    );
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);

    // Shift: at the current time; below the last layer: at the bottom of the stack.
    let row = rect(&h, &format!("timeline.layer.{bottom}.row"));
    drag(&mut h, item, pos2(at_2s, row.max.y + 30.0), Modifiers::SHIFT);
    assert_eq!(stack(&h), ["Top", "Middle", "Bottom", "Clip"]);
    assert!((in_point(&h, 3) - 1.0).abs() < 1e-9, "Shift starts it at the current time");
}

/// #85: a Project item dropped on the Composition viewer becomes a layer centred where it was
/// dropped (above the selected layer, like any new layer).
#[test]
fn project_items_dropped_on_the_viewer_land_under_the_pointer() {
    let (mut h, clip) = harness();
    let item = rect(&h, &format!("project.item.{clip}.name")).center();
    let to = comp_to_screen(&h.ctx, [80.0, 45.0]).unwrap();
    drag(&mut h, item, to, Modifiers::NONE);
    assert_eq!(stack(&h), ["Clip", "Top", "Middle", "Bottom"]);
    let comp = h.state().session.active_comp().unwrap();
    let Some(KV::Vec3(p)) = comp.layers[0].props.prop("transform/position").map(|p| p.value.clone()) else { panic!("no position") };
    // One screen point is up to 1/zoom comp pixels.
    let tol = 1.0 / last_fit(&h.ctx) as f64 + 1e-6;
    assert!((p[0] - 80.0).abs() <= tol && (p[1] - 45.0).abs() <= tol, "{p:?}");
}

/// #88: an effect dragged from Effects & Presets onto the viewer goes on the layer under the
/// pointer, not the selected one.
#[test]
fn effects_dropped_on_the_viewer_go_on_the_layer_under_the_pointer() {
    let (mut h, _) = harness();
    let (top, middle) = (layer_id(&h, "Top"), layer_id(&h, "Middle"));
    h.state_mut().session.execute("layer.select", json!({"layers": [middle]})).unwrap();
    h.state_mut().show_panel(PanelKind::EffectsPresets);
    h.state_mut().ui.effects_search = "Gaussian Blur".into();
    h.run_steps(3);
    let fx = rect(&h, "effects.item.ec.blur.gaussian").center();
    let to = comp_to_screen(&h.ctx, [160.0, 90.0]).unwrap();
    drag(&mut h, fx, to, Modifiers::NONE);
    let comp = h.state().session.active_comp().unwrap();
    let effects = |id: u64| comp.layer(effectcraft_engine::project::LayerId(id)).unwrap().effects().map_or(0, |fx| fx.groups().count());
    assert_eq!((effects(top), effects(middle)), (1, 0), "on Top, which is under the pointer");
}

/// #227: Project items dropped on Create a new Composition (the Project panel's footer) make a
/// composition from them, as File ▸ New Comp from Selection does; several items ask how in its
/// dialog.
#[test]
fn project_items_dropped_on_new_comp_make_a_composition() {
    let (mut h, clip) = harness();
    let comps = |h: &Harness<'_, EffectcraftApp>| h.state().session.project.comps().count();
    let before = comps(&h);
    let item = rect(&h, &format!("project.item.{clip}.name")).center();
    let button = rect(&h, "project.newComp").center();
    drag(&mut h, item, button, Modifiers::NONE);
    assert_eq!(comps(&h), before + 1);
    let comp = h.state().session.active_comp().unwrap();
    assert_eq!((comp.width, comp.height, comp.duration.seconds()), (160, 90, 1.0), "the item's settings");
    assert_eq!(stack(&h), ["Clip"], "holding the item");
    assert_ne!(h.state().session.active_comp_id().map(|c| c.0), Some(clip), "a new comp, open");
    assert!(h.state().dialog.is_none());

    // Two selected items, one dragged: New Composition from Selection asks how.
    let main = h.state().session.project.items.values().find(|i| i.name == "Main").unwrap().id;
    h.state_mut().session.state.project_selection = vec![effectcraft_engine::project::ItemId(clip), main];
    h.run_steps(2);
    let item = rect(&h, &format!("project.item.{clip}.name")).center();
    drag(&mut h, item, button, Modifiers::NONE);
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::Form));
    assert!(h.state().auto.find("form.field.single").is_some(), "the New Composition from Selection dialog");
    assert_eq!(comps(&h), before + 1, "nothing made yet");
}

/// A file dropped on the window (egui hands it over as a dropped file).
#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Err("not read".into())
    }
}

/// Footage probes that hold file k until `allow` > k (slow video decoding), 5 s at most (an
/// import that blocks the app then fails the test instead of hanging it).
struct Gated {
    started: AtomicUsize,
    allow: Arc<AtomicUsize>,
}

impl effectcraft_engine::Importer for Gated {
    fn probe(&self, path: &str) -> Result<effectcraft_engine::project::Footage, String> {
        let k = self.started.fetch_add(1, Ordering::SeqCst);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.allow.load(Ordering::SeqCst) <= k && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Ok(effectcraft_engine::project::Footage { path: path.into(), width: 64, height: 32, has_video: true, ..Default::default() })
    }
}

/// Step the app until `done` holds (the import runs on another thread).
fn step_until(h: &mut Harness<'_, EffectcraftApp>, done: impl Fn(&Harness<'_, EffectcraftApp>) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !done(h) {
        assert!(std::time::Instant::now() < deadline, "timed out");
        std::thread::sleep(std::time::Duration::from_millis(2));
        h.step();
    }
}

/// #270: files dropped on the window import in the background with an Importing card that names
/// the file being read and how many are left (the app keeps drawing meanwhile); then they are
/// selected and shown in the Project panel, with the count in a toast. The same file dropped
/// again arrives again, selected.
#[test]
fn dropped_files_show_import_progress_then_the_items_they_made() {
    let (mut h, _) = harness();
    let allow = Arc::new(AtomicUsize::new(1));
    h.state_mut().session.importer = Some(Arc::new(Gated { started: AtomicUsize::new(0), allow: allow.clone() }));
    let file = |n: &str| std::env::temp_dir().join(n);
    for n in ["a.png", "b.png", "c.png"] {
        h.input_mut().dropped_files.push(Arc::new(Dropped(file(n))));
    }
    h.run_steps(2);
    let card = |h: &Harness<'_, EffectcraftApp>| h.state().auto.query("import.job.").first().map(|e| e.label.clone());
    step_until(&mut h, |h| card(h).as_deref() == Some("b.png (2 of 3)"));
    let r = rect(&h, &card_id(&h));
    assert!(r.min.x < 40.0 && r.max.y > 960.0, "bottom left, over the panels (where toasts show): {r:?}");
    assert!(h.state().auto.find(&card_id(&h).replace("import.job.", "import.cancel.")).is_some(), "with Cancel");
    let items = |h: &Harness<'_, EffectcraftApp>| h.state().session.project.items.values().filter(|i| i.name.ends_with(".png")).count();
    assert_eq!(items(&h), 0, "nothing added until the import finishes");
    allow.store(usize::MAX, Ordering::SeqCst);
    step_until(&mut h, |h| card(h).is_none() && items(h) == 3);
    h.run_steps(2);
    let st = &h.state().session.state;
    let names: Vec<String> = st.project_selection.iter().map(|i| h.state().session.project.item(*i).unwrap().name.clone()).collect();
    assert_eq!(names, ["a.png", "b.png", "c.png"], "what arrived is selected");
    let last = st.project_selection[2].0;
    assert!(h.state().auto.find(&format!("project.item.{last}")).is_some(), "and in view in the Project panel");
    assert_eq!(h.state().auto.find("toast").map(|e| e.label.as_str()), Some("Imported 3 items"));

    // The same file again: imported again, and that one is selected.
    h.input_mut().dropped_files.push(Arc::new(Dropped(file("a.png"))));
    h.run_steps(2);
    step_until(&mut h, |h| items(h) == 4);
    h.run_steps(2);
    let sel = &h.state().session.state.project_selection;
    assert_eq!(sel.len(), 1);
    assert_eq!(h.state().session.project.item(sel[0]).unwrap().name, "a.png");
    assert_eq!(h.state().auto.find("toast").map(|e| e.label.as_str()), Some("Imported 1 item"));
}

/// The automation id of the Importing card on screen.
fn card_id(h: &Harness<'_, EffectcraftApp>) -> String {
    h.state().auto.query("import.job.").first().map(|e| e.id.clone()).unwrap_or_default()
}

/// Probes any existing file as a 64×32 PNG still.
struct Stills;

impl effectcraft_engine::Importer for Stills {
    fn probe(&self, path: &str) -> Result<effectcraft_engine::project::Footage, String> {
        if !std::path::Path::new(path).is_file() {
            return Err(format!("{path}: no such file"));
        }
        Ok(effectcraft_engine::project::Footage { path: path.into(), width: 64, height: 32, has_video: true, codec: "PNG".into(), ..Default::default() })
    }
}

/// A fresh folder holding empty `frame_0001.png` … `frame_000<n>.png`.
fn frames(tag: &str, n: u32) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ec-ui-seq-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for k in 1..=n {
        std::fs::write(d.join(format!("frame_{k:04}.png")), b"").unwrap();
    }
    d
}

fn click_id(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    // A dialog that just opened settles its size and place in a frame or two.
    h.run_steps(3);
    let p = rect(h, id).center();
    h.event(Event::PointerMoved(p));
    h.step();
    h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

/// (name, sequence files) of every footage item.
fn footage_items(h: &Harness<'_, EffectcraftApp>) -> Vec<(String, usize)> {
    h.state()
        .session
        .project
        .items
        .values()
        .filter_map(|i| match &i.kind {
            effectcraft_engine::project::ItemKind::Footage(f) => Some((i.name.clone(), f.sequence.len())),
            _ => None,
        })
        .collect()
}

/// #297: File ▸ Import ▸ File… on one numbered still asks, as After Effects' "PNG Sequence"
/// checkbox does, whether to import its whole run as one image sequence (and at what frame
/// rate); unticked, the file imports as a still.
#[test]
fn import_dialog_offers_the_image_sequence_option() {
    let (mut h, _) = harness();
    let dir = frames("dialog", 3);
    let pick = dir.join("frame_0002.png").to_string_lossy().to_string();
    h.state_mut().session.importer = Some(Arc::new(Stills));
    h.state_mut().hooks.pick_files = Some(Box::new(move |_: &[&str]| vec![pick.clone()]));
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "file.import", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(effectcraft_ui_egui::Dialog::Form));
    let label = |h: &Harness<'_, EffectcraftApp>, id: &str| h.state().auto.find(id).map(|e| e.label.clone());
    assert_eq!(label(&h, "form.field.sequence").as_deref(), Some("PNG Sequence"));
    assert!(label(&h, "form.field.alphabetical").is_some() && label(&h, "form.field.frameRate").is_some());
    assert!(footage_items(&h).is_empty(), "nothing imported before OK");
    click_id(&mut h, "form.ok");
    step_until(&mut h, |h| !footage_items(h).is_empty());
    assert_eq!(footage_items(&h), [("frame_[0001-0003].png".to_string(), 3)]);

    // Unticked: the picked file alone, as a still.
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "file.import", json!({})).unwrap();
    h.run_steps(2);
    click_id(&mut h, "form.field.sequence");
    click_id(&mut h, "form.ok");
    step_until(&mut h, |h| footage_items(h).len() == 2);
    assert_eq!(footage_items(&h)[1], ("frame_0002.png".to_string(), 0));
    let _ = std::fs::remove_dir_all(&dir);
}

/// #297: every frame of a sequence dropped on the window arrives as one image sequence (it used
/// to make one item per frame), and Interpret Footage shows its Start Frame (missing frames
/// always show colour bars, as in After Effects: there is no Missing Frames choice).
#[test]
fn dropped_frames_import_as_one_sequence_with_sequence_interpretation() {
    let (mut h, _) = harness();
    let dir = frames("drop", 4);
    h.state_mut().session.importer = Some(Arc::new(Stills));
    for k in 1..=4 {
        h.input_mut().dropped_files.push(Arc::new(Dropped(dir.join(format!("frame_{k:04}.png")))));
    }
    h.run_steps(2);
    step_until(&mut h, |h| !footage_items(h).is_empty());
    h.run_steps(2);
    assert_eq!(footage_items(&h), [("frame_[0001-0004].png".to_string(), 4)]);
    assert!(h.state().dialog.is_none(), "drops don't ask");
    // Interpret Footage on it (the import selected it).
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "file.interpretFootage", json!({})).unwrap();
    h.run_steps(2);
    for id in ["form.field.sequenceInfo", "form.field.startFrame", "form.field.frameRate", "form.field.alpha"] {
        assert!(h.state().auto.find(id).is_some(), "{id}");
    }
    assert!(h.state().auto.find("form.field.missingFrames").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
