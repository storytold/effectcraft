//! A modal dialog takes every click and key, so control-channel input aimed at the window behind
//! it used to do nothing without a word (an agent's `ui_click` "succeeded" and nothing happened).
//! `ui.inspect` reports the open dialog as `modal`, pointer input outside it fails naming the
//! dialog and its buttons, and input replies say when a modal is open after the input. Here the
//! dialog is Interpret Footage, which Ask User opens after importing footage with unlabeled alpha.

use std::sync::mpsc::{Receiver, Sender};

use effectcraft_engine::Session;
use effectcraft_engine::project::{Footage, FootageKind};
use effectcraft_ui_egui::control::ControlRequest;
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui_kittest::Harness;
use serde_json::{Value, json};

/// TGA stills with alpha: their files don't say how it is stored.
struct Tga;

impl effectcraft_engine::Importer for Tga {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        Ok(Footage { path: path.into(), kind: FootageKind::Still, width: 64, height: 64, has_video: true, codec: "TGA".into(), ..Default::default() })
    }
}

fn harness() -> (Harness<'static, EffectcraftApp>, Sender<ControlRequest>) {
    let mut s = Session { importer: Some(std::sync::Arc::new(Tga)), ..Default::default() };
    s.prefs.disk.disk_cache_enabled = false;
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s).with_control(rx));
    h.run_steps(3);
    (h, tx)
}

fn call(h: &mut Harness<'static, EffectcraftApp>, tx: &Sender<ControlRequest>, method: &str, params: Value) -> Value {
    let (req, reply): (ControlRequest, Receiver<Value>) = ControlRequest::new(method, params);
    tx.send(req).unwrap();
    for _ in 0..40 {
        // What eframe's raw_input_hook does: the queued synthetic input, a share per frame.
        let mut raw = egui::RawInput::default();
        h.state_mut().feed_synthetic(&mut raw);
        h.input_mut().events.extend(raw.events);
        h.step();
        if let Ok(v) = reply.try_recv() {
            h.run_steps(2);
            return v;
        }
    }
    panic!("no reply to {method}");
}

/// The centre of an element drawn in the last frame.
fn centre(h: &Harness<'static, EffectcraftApp>, id: &str) -> [f32; 2] {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    [e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5]
}

#[test]
fn input_outside_a_modal_dialog_says_the_dialog_is_open() {
    let (mut h, tx) = harness();
    assert_eq!(call(&mut h, &tx, "ui.inspect", json!({}))["result"]["modal"], Value::Null, "no dialog yet");
    // Ask User: importing a TGA with alpha opens Interpret Footage.
    let r = call(&mut h, &tx, "engine.execute", json!({"command": "file.import", "params": {"paths": ["/m/key.tga"]}}));
    assert_eq!(r["ok"], true, "{r}");
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Form));
    let modal = call(&mut h, &tx, "ui.inspect", json!({}))["result"]["modal"].clone();
    assert_eq!(modal["dialog"], "Form", "{modal}");
    assert_eq!(modal["title"], "Interpret Footage", "{modal}");
    let elements: Vec<&str> = modal["elements"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(elements.contains(&"form.ok") && elements.contains(&"form.cancel"), "its buttons: {elements:?}");
    let rect: Vec<f32> = modal["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
    let inside = |p: [f32; 2]| p[0] >= rect[0] && p[0] <= rect[0] + rect[2] && p[1] >= rect[1] && p[1] <= rect[1] + rect[3];

    // A click, a drag and a scroll on the window behind it fail and say what to do.
    let behind = [8.0, 8.0];
    assert!(!inside(behind));
    for (method, params) in [
        ("ui.click", json!({"x": behind[0], "y": behind[1]})),
        ("ui.drag", json!({"from": {"x": behind[0], "y": behind[1]}, "to": {"x": 200, "y": 200}})),
        ("ui.scroll", json!({"x": behind[0], "y": behind[1], "dx": 0, "dy": 40})),
    ] {
        let r = call(&mut h, &tx, method, params);
        assert_eq!(r["ok"], false, "{method}: {r}");
        let e = r["error"].as_str().unwrap();
        assert!(e.contains("a modal dialog is open (\"Interpret Footage\")") && e.contains("form.cancel"), "{method}: {e}");
    }
    assert_eq!(h.state().dialog, Some(Dialog::Form), "still open");

    // Input inside it works; the reply says the dialog is still open.
    let field = elements.iter().find(|id| id.starts_with("form.field.")).expect("a field");
    let r = call(&mut h, &tx, "ui.click", json!({"id": field}));
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["result"]["modal"]["title"], "Interpret Footage", "{r}");
    let r = call(&mut h, &tx, "ui.key", json!({"key": "Tab"}));
    assert_eq!(r["result"]["modal"]["title"], "Interpret Footage", "keys go to the dialog too: {r}");

    // Cancel closes it: replies and ui.inspect report no modal, and the window takes input again.
    let r = call(&mut h, &tx, "ui.click", json!({"id": "form.cancel"}));
    assert_eq!(r, json!({"ok": true, "result": null}));
    assert_eq!(h.state().dialog, None);
    assert_eq!(call(&mut h, &tx, "ui.inspect", json!({}))["result"]["modal"], Value::Null);
    let p = centre(&h, "panel.tab.Timeline");
    assert_eq!(call(&mut h, &tx, "ui.click", json!({"x": p[0], "y": p[1]})), json!({"ok": true, "result": null}));
}

/// With `alpha`, the import decides and no dialog opens.
#[test]
fn importing_with_alpha_opens_no_dialog() {
    let (mut h, tx) = harness();
    let r = call(&mut h, &tx, "engine.execute", json!({"command": "file.import", "params": {"paths": ["/m/key.tga"], "alpha": "straight"}}));
    assert_eq!(r["ok"], true, "{r}");
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    assert_eq!(call(&mut h, &tx, "ui.inspect", json!({}))["result"]["modal"], Value::Null);
}
