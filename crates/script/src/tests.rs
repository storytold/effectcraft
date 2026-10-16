//! End-to-end scripts against a headless session: build with the object model, read back through
//! the engine.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::Value as KV;
use effectcraft_engine::project::{Comp, ItemId, Layer};
use serde_json::{Value, json};

use crate::{Outcome, run_code};

fn session() -> Session {
    let mut s = Session { expr: Some(Arc::new(effectcraft_expr::Expressions)), expr_check: Some(effectcraft_expr::check_syntax), ..Default::default() };
    crate::install(&mut s);
    s
}

fn ok(s: &mut Session, code: &str) -> Outcome {
    let o = run_code(s, code, "test.jsx");
    assert!(o.error.is_none(), "script failed: {:?}\noutput: {:?}", o.error, o.output);
    o
}

fn comp_named<'a>(s: &'a Session, name: &str) -> (ItemId, &'a Comp) {
    let it = s.project.items.values().find(|i| i.name == name && i.as_comp().is_some()).unwrap_or_else(|| panic!("no comp {name}"));
    (it.id, it.as_comp().unwrap())
}

fn layer<'a>(c: &'a Comp, name: &str) -> &'a Layer {
    c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no layer {name}"))
}

/// #1063 (item 8): Source Text retains CR while sourceRectAtTime sees both lines.
#[test]
fn carriage_return_source_text_has_multiline_script_bounds() {
    let mut s = session();
    let result = ok(
        &mut s,
        r#"
        var comp = app.project.items.addComp("Line breaks", 640, 360, 1, 1, 30);
        var crDoc = new TextDocument("Line 1\rLine 2");
        crDoc.font = "Inter";
        crDoc.fontSize = 40;
        var lfDoc = new TextDocument("Line 1\nLine 2");
        lfDoc.font = "Inter";
        lfDoc.fontSize = 40;
        var cr = comp.layers.addText(crDoc);
        var lf = comp.layers.addText(lfDoc);
        var crRect = cr.sourceRectAtTime(0, false);
        var lfRect = lf.sourceRectAtTime(0, false);
        [cr.text.sourceText.value.text, crRect.height, lfRect.height, crRect.width, lfRect.width]
        "#,
    );
    assert_eq!(result.result[0], "Line 1\rLine 2");
    assert!(result.result[1].as_f64().unwrap() > 40.0);
    assert_eq!(result.result[1], result.result[2]);
    assert_eq!(result.result[3], result.result[4]);
}

#[test]
fn builds_a_comp_end_to_end() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        app.beginUndoGroup("Build");
        var comp = app.project.items.addComp("Main", 640, 360, 1, 4, 25);
        var bg = comp.layers.addSolid([0.2, 0.4, 0.6], "BG", 640, 360, 1);
        var txt = comp.layers.addText("Hello");
        txt.name = "Title";
        var pos = txt.property("ADBE Transform Group").property("ADBE Position");
        pos.setValueAtTime(0, [100, 180]);
        pos.setValueAtTime(2, [540, 180]);
        txt.transform.opacity.expression = "50 + 25";
        var blur = bg.Effects.addProperty("ADBE Gaussian Blur 2");
        blur.property("Blurriness").setValue(12);
        var nul = comp.layers.addNull();
        txt.setParentWithJump(nul);
        bg.blendingMode = BlendingMode.MULTIPLY;
        var rq = app.project.renderQueue.items.add(comp);
        rq.outputModule(1).file = new File("/tmp/effectcraft-script-test/out.mp4");
        app.endUndoGroup();
        writeLn("layers: " + comp.numLayers);
        [comp.numLayers, pos.numKeys, pos.keyValue(2)[0], txt.transform.opacity.value, comp.layer(1).name]
        "#,
    );
    assert_eq!(o.output, vec!["layers: 3"]);
    assert_eq!(o.result, json!([3, 2, 540, 75, "Null 1"]));
    let (cid, c) = comp_named(&s, "Main");
    assert_eq!((c.width, c.height), (640, 360));
    assert_eq!(c.duration.seconds(), 4.0);
    // AE adds new layers at the top.
    let names: Vec<&str> = c.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Null 1", "Title", "BG"]);
    let title = layer(c, "Title");
    let p = title.props.prop("transform/position").unwrap();
    assert_eq!(p.keys.len(), 2);
    assert_eq!(p.keys[1].value, KV::Vec3([540.0, 180.0, 0.0]));
    assert_eq!(title.props.prop("transform/opacity").unwrap().expr.as_ref().unwrap().text, "50 + 25");
    assert_eq!(title.parent, Some(layer(c, "Null 1").id));
    let bg = layer(c, "BG");
    assert_eq!(bg.blend_mode, effectcraft_engine::color::BlendMode::Multiply);
    let fx = bg.props.group("effects/#1").unwrap();
    assert_eq!(fx.match_id, "ec.blur.gaussian");
    assert_eq!(bg.props.prop("effects/#1/blurriness").unwrap().value, KV::Scalar(12.0));
    assert_eq!(s.project.render_queue.len(), 1);
    assert_eq!(s.project.render_queue[0].comp, cid);
    // One undo step for the whole group.
    assert_eq!(s.history.undo.len(), 1);
    assert_eq!(s.history.undo[0].0, "Build");
    assert!(s.undo());
    assert!(s.project.items.is_empty());
}

#[test]
fn fill_parameters_use_documented_match_names() {
    // M13.15: `ADBE Fill-0002` is Fill's Color (All Masks is -0007), not the second row.
    let mut s = session();
    let o = ok(
        &mut s,
        r#"var c = app.project.items.addComp("F", 100, 100, 1, 1, 12); var l = c.layers.addSolid([1,1,1], "S", 100, 100, 1);
           var fx = l.property("ADBE Effect Parade").addProperty("ADBE Fill");
           [fx.property("ADBE Fill-0002").name, fx.property("ADBE Fill-0007").name, fx.property(2).matchName].join("|")"#,
    );
    assert_eq!(o.result, json!("Color|All Masks|ADBE Fill-0007"));
}

#[test]
fn undo_steps_without_a_group() {
    let mut s = session();
    ok(&mut s, r#"var c = app.project.items.addComp("A", 100, 100, 1, 1, 24); c.layers.addNull(); c.layers.addNull();"#);
    assert_eq!(s.history.undo.len(), 3);
    // An unbalanced group is closed when the script ends.
    ok(&mut s, r#"app.beginUndoGroup("Two"); var c = app.project.item(1); c.layers.addNull(); c.layers.addNull();"#);
    assert_eq!(s.history.undo.len(), 4);
    assert_eq!(s.history.undo[3].0, "Two");
    // Nested groups fold into the outermost one.
    ok(
        &mut s,
        r#"app.beginUndoGroup("Outer"); app.beginUndoGroup("Inner"); app.project.item(1).layers.addNull(); app.endUndoGroup(); app.project.item(1).layers.addNull(); app.endUndoGroup();"#,
    );
    assert_eq!(s.history.undo.len(), 5);
    assert_eq!(s.history.undo[4].0, "Outer");
    s.undo();
    assert_eq!(s.project.comp(ItemId(1)).unwrap().layers.len(), 4);
}

#[test]
fn errors_report_line_and_column() {
    let mut s = session();
    let o = run_code(&mut s, "var a = 1;\nvar b = 2;\nfoo.bar();\n", "broken.jsx");
    let e = o.error.expect("error");
    assert_eq!(e.line, Some(3), "{e:?}");
    assert!(e.message.contains("foo"), "{e:?}");
    assert_eq!(e.file, "broken.jsx");
    let o = run_code(&mut s, "var a = 1;\n\nvar = ;\n", "syntax.jsx");
    let e = o.error.expect("syntax error");
    assert_eq!(e.line, Some(3), "{e:?}");
    let o = run_code(&mut s, "writeLn('before');\nthrow new Error('custom failure');\n", "throw.jsx");
    let e = o.error.expect("thrown");
    assert_eq!(e.message, "custom failure");
    assert_eq!(e.line, Some(2), "{e:?}");
    assert_eq!(o.output, vec!["before"]);
    // Errors from the object model point at the user's line.
    let o = run_code(&mut s, "var c = app.project.items.addComp('C', 10, 10, 1, 1, 24);\n\nc.layer(5);\n", "range.jsx");
    let e = o.error.expect("range error");
    assert!(e.message.contains("out of range"), "{e:?}");
    assert_eq!(e.line, Some(3), "{e:?}");
}

/// A script wrapped in a function (the usual `(function () { ... })();` shape) reports the line
/// that failed, not the wrapper's line (#1063).
#[test]
fn errors_inside_a_function_wrapper_report_the_failing_line() {
    let mut s = session();
    let o = run_code(&mut s, "(function () {\n  var a = 1;\n  throw new Error('inside');\n})();\n", "thrown.jsx");
    let e = o.error.expect("thrown");
    assert_eq!(e.message, "inside");
    assert_eq!(e.line, Some(3), "{e:?}");
    let o = run_code(&mut s, "(function () {\n  var a = 1;\n  nope.call();\n})();\n", "reference.jsx");
    let e = o.error.expect("reference error");
    assert_eq!(e.line, Some(3), "{e:?}");
    let o = run_code(&mut s, "(function () {\n  var c = app.project.items.addComp('C', 10, 10, 1, 1, 24);\n  c.layer(5);\n})();\n", "range.jsx");
    let e = o.error.expect("range error");
    assert!(e.message.contains("out of range"), "{e:?}");
    assert_eq!(e.line, Some(3), "{e:?}");
}

/// The first use of an undefined name is the one inside the function that reads it: a mention
/// before it, in a comment, a string or another function, doesn't move the report (#1161).
#[test]
fn an_undefined_name_mentioned_earlier_reports_its_use_in_the_reading_function() {
    let mut s = session();
    let code = "// nope is mentioned here\nvar text = 'nope';\nfunction other() { return nope; }\n(function () {\n  var a = 1;\n  nope.call();\n})();\n";
    let o = run_code(&mut s, code, "earlier.jsx");
    let e = o.error.expect("reference error");
    assert!(e.message.contains("nope"), "{e:?}");
    assert_eq!(e.line, Some(6), "{e:?}");
}

#[test]
fn match_names_and_lookups() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("M", 200, 200, 1, 2, 30);
        var l = c.layers.addSolid([1, 1, 1], "S", 200, 200, 1);
        var tr = l.property("ADBE Transform Group");
        var fx = l.property("ADBE Effect Parade").addProperty("Gaussian Blur");
        var m = l.property("ADBE Mask Parade").addProperty("ADBE Mask Atom");
        var sh = c.layers.addShape();
        var grp = sh.property("ADBE Root Vectors Group").addProperty("ADBE Vector Group");
        var rect = grp.property("ADBE Vectors Group").addProperty("ADBE Vector Shape - Rect");
        var fill = grp.property("Contents").addProperty("ADBE Vector Graphic - Fill");
        fill.property("ADBE Vector Fill Color").setValue([1, 0, 0]);
        rect.property("ADBE Vector Rect Size").setValue([50, 60]);
        [
          l.matchName, tr.matchName, tr.property("ADBE Position").matchName, tr.property("Anchor Point").matchName,
          tr.property(1).name, tr.anchorPoint.matchName, l.transform.position.propertyValueType == PropertyValueType.ThreeD_SPATIAL,
          fx.matchName, fx.property(1).matchName, fx.property("ADBE Gaussian Blur 2-0001").name, m.matchName,
          m.property("ADBE Mask Shape").name, sh.matchName, grp.matchName, rect.matchName, fill.matchName,
          tr.propertyType == PropertyType.NAMED_GROUP, l.Effects.propertyType == PropertyType.INDEXED_GROUP,
          tr.position.parentProperty.matchName, tr.position.propertyDepth, l.property("Nope") === null,
          fx.isEffect, m.isMask, l.Effects.numProperties
        ]
        "#,
    );
    assert_eq!(
        o.result,
        json!([
            "ADBE AV Layer",
            "ADBE Transform Group",
            "ADBE Position",
            "ADBE Anchor Point",
            "Anchor Point",
            "ADBE Anchor Point",
            true,
            "ADBE Gaussian Blur 2",
            "ADBE Gaussian Blur 2-0001",
            "Blurriness",
            "ADBE Mask Atom",
            "Mask Path",
            "ADBE Vector Layer",
            "ADBE Vector Group",
            "ADBE Vector Shape - Rect",
            "ADBE Vector Graphic - Fill",
            true,
            true,
            "ADBE Transform Group",
            2,
            true,
            true,
            true,
            1
        ])
    );
    let (_, c) = comp_named(&s, "M");
    let sh = c.layers.iter().find(|l| l.props.sub("contents").is_some()).unwrap();
    assert_eq!(sh.props.prop("contents/#1/contents/rect/size").unwrap().value, KV::Vec2([50.0, 60.0]));
    assert_eq!(sh.props.prop("contents/#1/contents/fill/color").unwrap().value, KV::Color([1.0, 0.0, 0.0, 1.0]));
}

#[test]
fn keyframes_eases_and_interpolation() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("K", 100, 100, 1, 5, 25);
        var l = c.layers.addNull();
        var r = l.transform.rotation;
        r.setValuesAtTimes([0, 1, 2], [0, 90, 180]);
        r.setInterpolationTypeAtKey(2, KeyframeInterpolationType.HOLD, KeyframeInterpolationType.LINEAR);
        r.setTemporalEaseAtKey(3, [new KeyframeEase(0, 75)]);
        var idx = r.addKey(1.5);
        r.removeKey(1);
        var out = [r.numKeys, idx, r.keyTime(1), r.keyInInterpolationType(1) == KeyframeInterpolationType.HOLD,
          r.keyOutInterpolationType(1) == KeyframeInterpolationType.LINEAR, r.keyInTemporalEase(3)[0].influence,
          r.valueAtTime(1, false), r.isTimeVarying, r.nearestKeyIndex(1.9)];
        l.startTime = 1;
        out.push(r.keyTime(1));
        out
        "#,
    );
    let v = o.result.as_array().unwrap();
    assert_eq!(v[0], json!(3));
    assert_eq!(v[1], json!(3));
    assert_eq!(v[2], json!(1));
    assert_eq!(v[3], json!(true));
    assert_eq!(v[4], json!(true));
    assert!((v[5].as_f64().unwrap() - 75.0).abs() < 1e-6, "{v:?}");
    assert_eq!(v[6], json!(90));
    assert_eq!(v[7], json!(true));
    assert_eq!(v[8], json!(3));
    // Moving the layer moves its keys (layer time): key 1 now sits at 2 s comp time.
    assert_eq!(v[9], json!(2));
}

#[test]
fn spatial_tangents_round_trip() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("T", 100, 100, 1, 2, 25);
        var p = c.layers.addNull().transform.position;
        p.setValuesAtTimes([0, 1], [[0, 0], [100, 0]]);
        p.setSpatialTangentsAtKey(1, [0, 0], [30, 40]);
        [p.keyOutSpatialTangent(1)[0], p.keyOutSpatialTangent(1)[1], p.keyInSpatialTangent(1)[0]]
        "#,
    );
    assert_eq!(o.result, json!([30, 40, 0]));
}

#[test]
fn text_documents() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("T", 400, 200, 1, 2, 30);
        var t = c.layers.addText("abc");
        var st = t.property("ADBE Text Properties").property("ADBE Text Document");
        var doc = st.value;
        doc.text = "Changed";
        doc.fontSize = 40;
        doc.fillColor = [1, 0, 0];
        doc.justification = ParagraphJustification.CENTER_JUSTIFY;
        doc.applyStroke = true;
        doc.strokeWidth = 3;
        st.setValue(doc);
        var d2 = t.text.sourceText.value;
        [d2.text, d2.fontSize, d2.fillColor, d2.justification == ParagraphJustification.CENTER_JUSTIFY, d2.strokeWidth, t instanceof TextLayer, t.name]
        "#,
    );
    assert_eq!(o.result, json!(["Changed", 40, [1, 0, 0], true, 3, true, "abc"]));
    let (_, c) = comp_named(&s, "T");
    let KV::Text(d) = &c.layers[0].props.prop("text/sourceText").unwrap().value else { panic!() };
    assert_eq!(d.text, "Changed");
    assert_eq!(d.style_at(0).size, 40.0);
}

#[test]
fn layers_items_and_markers() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var f = app.project.items.addFolder("Comps");
        var c = f.items.addComp("Inner", 100, 100, 1, 3, 30);
        var main = app.project.items.addComp("Outer", 200, 200, 1, 6, 30);
        var nested = main.layers.add(c);
        var a = main.layers.addSolid([1, 0, 0], "A", 50, 50, 1, 2);
        var b = main.layers.addSolid([0, 1, 0], "B", 50, 50, 1);
        b.moveAfter(nested);
        a.enabled = false; a.solo = true; a.locked = true; a.shy = true;
        a.threeDLayer = true;
        var cam = main.layers.addCamera("Cam", [100, 100]);
        var lt = main.layers.addLight("Key", [100, 100]);
        lt.lightType = LightType.SPOT;
        var d = a.duplicate();
        d.name = "A copy";
        var mv = new MarkerValue("hello");
        mv.duration = 0.5;
        main.markerProperty.setValueAtTime(1, mv);
        b.marker.setValueAtTime(0.5, new MarkerValue("layer marker"));
        b.inPoint = 1; b.outPoint = 3;
        var pc = main.layers.precompose([b.index], "Pre", true);
        [c.parentFolder.name, nested.source.name, a.outPoint, a.enabled, a.solo, a.locked, a.shy, a.threeDLayer,
         cam instanceof CameraLayer, lt.lightType == LightType.SPOT, main.markerProperty.numKeys, main.markerProperty.keyValue(1).comment,
         main.markerProperty.keyValue(1).duration, pc.name, pc instanceof CompItem, main.layer("A copy").index, app.project.numItems,
         main.layers.byName("Pre") !== null, f.numItems]
        "#,
    );
    assert_eq!(o.result, json!(["Comps", "Inner", 2, false, true, true, true, true, true, true, 1, "hello", 0.5, "Pre", true, 3, 7, true, 1]));
    let (_, c) = comp_named(&s, "Outer");
    assert_eq!(c.markers.len(), 1);
    assert!(
        c.layers
            .iter()
            .any(|l| l.name == "Key"
                && matches!(l.source, effectcraft_engine::project::LayerSource::Light { kind: effectcraft_engine::project::LightKind::Spot }))
    );
}

#[test]
fn security_gate_for_files() {
    let dir = std::env::temp_dir().join(format!("ec-script-gate-{}", std::process::id()));
    let proj = dir.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("data.txt"), "inside").unwrap();
    std::fs::write(dir.join("secret.txt"), "outside").unwrap();
    let mut s = session();
    s.path = Some(proj.join("p.ecproj").to_string_lossy().to_string());
    let q = |p: &std::path::Path| serde_json::to_string(&p.to_string_lossy()).unwrap();
    // Reading inside the project folder works.
    let o = ok(&mut s, &format!("var f = new File({}); f.open('r'); var t = f.read(); f.close(); t", q(&proj.join("data.txt"))));
    assert_eq!(o.result, json!("inside"));
    // Outside it (also via `..`) is refused.
    for path in [dir.join("secret.txt"), proj.join("../secret.txt")] {
        let o = run_code(&mut s, &format!("var f = new File({}); f.open('r');", q(&path)), "gate.jsx");
        let e = o.error.expect("blocked");
        assert!(e.message.contains("Allow Scripts to Write Files"), "{e:?}");
    }
    // Writing is refused anywhere, the network too.
    let o = run_code(&mut s, &format!("var f = new File({}); f.open('w'); f.write('x'); f.close();", q(&proj.join("out.txt"))), "gate.jsx");
    assert!(o.error.is_some());
    assert!(!proj.join("out.txt").exists());
    assert!(run_code(&mut s, "new Socket()", "net.jsx").error.is_some());
    assert!(run_code(&mut s, "system.callSystem('ls')", "sys.jsx").error.is_some());
    // With the preference on, scripts can write and read anywhere.
    s.prefs.scripting.allow_scripts_write_files = true;
    ok(&mut s, &format!("var f = new File({}); f.open('w'); f.writeln('written'); f.close();", q(&dir.join("w.txt"))));
    assert_eq!(std::fs::read_to_string(dir.join("w.txt")).unwrap(), "written\n");
    let o = ok(&mut s, &format!("var f = new File({}); f.open('r'); f.readln()", q(&dir.join("secret.txt"))));
    assert_eq!(o.result, json!("outside"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// #816: CompItem.saveFrameToPng writes the frame as a PNG, behind the file-write preference.
#[test]
fn save_frame_to_png_writes_behind_the_file_gate() {
    let dir = std::env::temp_dir().join(format!("ec-save-frame-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("frame.png");
    let mut s = session();
    let code = format!(
        "var c = app.project.items.addComp('F', 40, 30, 1, 2, 25); c.layers.addSolid([1, 0, 0], 'S', 40, 30, 1); c.saveFrameToPng(1, new File({}))",
        serde_json::to_string(&png.to_string_lossy()).unwrap()
    );
    let o = run_code(&mut s, &code, "frame.jsx");
    assert!(o.error.expect("refused").message.contains("Allow Scripts to Write Files"));
    assert!(!png.exists() || std::fs::metadata(&png).unwrap().len() == 0);
    s.prefs.scripting.allow_scripts_write_files = true;
    ok(&mut s, &code);
    let bytes = std::fs::read(&png).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    // IHDR width and height.
    assert_eq!((u32::from_be_bytes(bytes[16..20].try_into().unwrap()), u32::from_be_bytes(bytes[20..24].try_into().unwrap())), (40, 30));
    let _ = std::fs::remove_dir_all(&dir);
}

/// #1246: with Warn User When Executing Files on, a script that asserts `confirmed` itself still
/// gets the question; only the user's answer opens the file.
#[test]
fn a_script_cannot_confirm_executing_a_file_for_itself() {
    let dir = std::env::temp_dir().join(format!("ec-script-exec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("run.txt");
    std::fs::write(&f, "x").unwrap();
    let q = serde_json::to_string(&f.to_string_lossy()).unwrap();
    let mut s = session();
    // The user allows file access; the warning stays on (the default).
    s.execute("prefs.set", json!({"key": "scripting.allowScriptsWriteFiles", "value": true})).unwrap();
    s.drain_events();
    let opened = |s: &mut Session| s.drain_events().iter().any(|e| matches!(e, effectcraft_engine::Event::OpenUrl(_)));
    for code in [
        format!("JSON.stringify(app.run('file.executeFile', {{path: {q}, confirmed: true}}))"),
        format!("JSON.stringify(app.run('engine.batch', {{steps: [{{command: 'file.executeFile', params: {{path: {q}, confirmed: true}}}}]}}))"),
        format!("new File({q}).execute()"),
    ] {
        let o = ok(&mut s, &code);
        assert!(!o.result.to_string().contains("opened") || o.result == json!(false), "{code}: {:?}", o.result);
        assert!(!opened(&mut s), "{code} opened the file without asking");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// #590: a script can't give itself file and network access (or turn off the warning before
/// executing files) by changing Settings ▸ Scripting & Expressions; the user still can.
#[test]
fn scripts_cant_change_what_scripts_may_do() {
    let mut s = session();
    let attempts = [
        "app.run('prefs.set', {key: 'scripting.allowScriptsWriteFiles', value: true})",
        "app.run('prefs.set', {values: {'scripting.warnExecutingFiles': false}})",
        "app.run('prefs.set', {key: 'scripting', value: {allowScriptsWriteFiles: true}})",
        "app.run('engine.batch', {steps: [{command: 'prefs.set', params: {key: 'scripting.enableJsDebugger', value: true}}]})",
    ];
    for code in attempts {
        let o = run_code(&mut s, code, "grant.jsx");
        let e = o.error.unwrap_or_else(|| panic!("`{code}` was allowed"));
        assert!(e.message.contains("Scripts can't change what scripts are allowed to do"), "{code}: {e:?}");
        let sc = &s.prefs.scripting;
        assert!(!sc.allow_scripts_write_files && sc.warn_executing_files && !sc.enable_js_debugger, "{code}");
    }
    assert!(run_code(&mut s, "new Socket()", "net.jsx").error.is_some());
    // Other settings, and the expression editor's, are still the script's to change.
    ok(&mut s, "app.run('prefs.set', {values: {'general.undoLevels': 40, 'scripting.editorFontSize': 16}})");
    assert_eq!((s.prefs.general.undo_levels, s.prefs.scripting.editor_font_size), (40, 16));
    // The user grants access; a script's Reset Settings doesn't take it back or change it.
    s.execute("prefs.set", json!({"key": "scripting.allowScriptsWriteFiles", "value": true})).unwrap();
    ok(&mut s, "app.run('prefs.reset', {})");
    assert!(s.prefs.scripting.allow_scripts_write_files);
    assert_eq!(s.prefs.general.undo_levels, effectcraft_engine::prefs::Prefs::default().general.undo_levels);
    // A command script file is a script too.
    let dir = std::env::temp_dir().join(format!("ec-script-grant-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("grant.json");
    std::fs::write(&f, r#"[{"command": "prefs.set", "params": {"key": "scripting.allowScriptsWriteFiles", "value": false}}]"#).unwrap();
    let e = s.execute("file.runScript", json!({"path": f.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("Scripts can't change"), "{e}");
    assert!(s.prefs.scripting.allow_scripts_write_files);
    // The user (UI, CLI, control channel) can take it back.
    s.execute("prefs.set", json!({"key": "scripting.allowScriptsWriteFiles", "value": false})).unwrap();
    assert!(!s.prefs.scripting.allow_scripts_write_files);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn engine_entry_points() {
    let mut s = session();
    // script.run reports errors in the result, not as a command failure.
    let r = s.execute("script.run", json!({"code": "writeLn('hi'); 1 + 2"})).unwrap();
    assert_eq!(r["ok"], json!(true));
    assert_eq!(r["result"], json!(3));
    assert_eq!(r["output"], json!("hi"));
    let r = s.execute("script.run", json!({"code": "\nnope()"})).unwrap();
    assert_eq!(r["ok"], json!(false));
    assert_eq!(r["error"]["line"], json!(2));
    // Scripts can't run scripts.
    let r = s.execute("script.run", json!({"code": "app.run('script.run', {code: '1'})"})).unwrap();
    assert!(r["error"]["message"].as_str().unwrap().contains("scripts can't run scripts"), "{r}");
    // Compiled .jsxbin scripts (by name or by their header) are unsupported, not a SyntaxError
    // at 1:1 (#176).
    for p in [json!({"code": "@JSXBIN@ES@2.0@MyBbyBn0AB", "name": "compiled.bin"}), json!({"code": "1", "name": "/x/compiled.JSXBIN"})] {
        let e = s.execute("script.run", p.clone()).unwrap_err().to_string();
        assert!(e.contains(".jsxbin scripts are not supported: run the .jsx source"), "{p}: {e}");
    }
    // File ▸ Scripts ▸ Run Script File… runs .jsx as JavaScript.
    let dir = std::env::temp_dir().join(format!("ec-script-file-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("make.jsx");
    std::fs::write(&f, "app.project.items.addComp('From File', 320, 240, 1, 2, 24);").unwrap();
    s.execute("file.runScript", json!({"path": f.to_string_lossy()})).unwrap();
    assert!(s.project.items.values().any(|i| i.name == "From File"));
    std::fs::write(&f, "\n\nbroken(;").unwrap();
    let e = s.execute("file.runScript", json!({"path": f.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("make.jsx (line 3)"), "{e}");
    // JSON command scripts still work.
    let j = dir.join("steps.json");
    std::fs::write(&j, r#"[{"command": "comp.new", "params": {"name": "From Steps"}}]"#).unwrap();
    s.execute("file.runScript", json!({"path": j.to_string_lossy()})).unwrap();
    assert!(s.project.items.values().any(|i| i.name == "From Steps"));
    // Menu commands by name.
    let o = ok(&mut s, "app.findMenuCommandId('New Project') > 0 && app.findMenuCommandId('No Such Thing') === 0");
    assert_eq!(o.result, json!(true));
    let o = ok(&mut s, "app.executeCommand(app.findMenuCommandId('New Project')); app.project.numItems");
    assert_eq!(o.result, json!(0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn console_keeps_variables() {
    let mut s = session();
    let req = |code: &'static str| effectcraft_engine::ScriptRequest { code, name: "console", console: true };
    let r = crate::run(&mut s, &req("var counter = 41;"));
    assert!(r.error.is_none(), "{r:?}");
    let r = crate::run(&mut s, &req("counter + 1"));
    assert_eq!(r.result, json!(42));
    let _ = Value::Null;
}

#[test]
fn schedule_task_runs_after_the_script() {
    let mut s = session();
    let o = ok(
        &mut s,
        "app.scheduleTask('writeLn(\"later\")', 100, false); var id = app.scheduleTask('writeLn(\"never\")', 10, false); app.cancelTask(id); writeLn('now');",
    );
    assert_eq!(o.output, vec!["now", "later"]);
}

#[test]
fn layer_switches_timing_and_comp_settings() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("S", 320, 240, 1, 5, 30);
        var matte = c.layers.addText("M");
        var fill = c.layers.addSolid([1, 1, 1], "Fill", 320, 240, 1);
        fill.moveToEnd();
        fill.setTrackMatte(matte, TrackMatteType.LUMA);
        var t1 = fill.trackMatteType == TrackMatteType.LUMA && fill.hasTrackMatte && fill.trackMatteLayer.name == "M";
        fill.stretch = 200;
        fill.label = 3;
        c.width = 400;
        c.duration = 8;
        c.bgColor = [0.1, 0.2, 0.3];
        c.time = 2;
        c.workAreaStart = 1;
        c.workAreaDuration = 3;
        var extra = c.layers.addNull();
        extra.remove();
        fill.selected = true;
        var sel = c.selectedLayers.length;
        fill.transform.opacity.expression = "thisLayer.nope(";
        var err = fill.transform.opacity.expressionError;
        fill.transform.opacity.expressionEnabled = false;
        var copy = c.duplicate();
        var f = app.project.items.addFolder("Bin");
        copy.parentFolder = f;
        [t1, fill.stretch, fill.label, c.width, c.duration, c.bgColor[2], c.time, c.workAreaStart, c.workAreaDuration,
         c.numLayers, sel, err.length > 0, fill.transform.opacity.expressionEnabled, copy.name, f.numItems, copy.parentFolder.name]
        "#,
    );
    let v = o.result;
    assert_eq!(v[0], json!(true), "{v}");
    assert_eq!(v[1], json!(200));
    assert_eq!(v[2], json!(3));
    assert_eq!(v[3], json!(400));
    assert_eq!(v[4], json!(8));
    assert!((v[5].as_f64().unwrap() - 0.3).abs() < 1e-6);
    assert_eq!(v[6], json!(2));
    assert_eq!(v[7], json!(1));
    assert_eq!(v[8], json!(3));
    assert_eq!(v[9], json!(2));
    assert_eq!(v[10], json!(1));
    assert_eq!(v[11], json!(true));
    assert_eq!(v[12], json!(false));
    assert_eq!(v[13], json!("S 2"));
    assert_eq!(v[14], json!(1));
    assert_eq!(v[15], json!("Bin"));
    // Removing the copy through the object model.
    let o = ok(&mut s, "var n = app.project.numItems; app.project.item(1).remove(); n - app.project.numItems");
    assert_eq!(o.result, json!(1));
}

#[test]
fn motion_graphics_template_hooks_add_mirrors_and_controllers() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var comp = app.project.items.addComp("Card", 64, 64, 1, 2, 30);
        var bg = comp.layers.addSolid([1, 1, 1], "BG", 64, 64, 1);
        var op = bg.transform.opacity;
        var r = [op.canAddToMotionGraphicsTemplate(comp), op.addToMotionGraphicsTemplateAs(comp, "Fade")];
        // Again: a mirror of the same property (a controller of its own).
        r.push(op.addToMotionGraphicsTemplate(comp));
        r.push(comp.motionGraphicsTemplateControllerCount);
        r.push(comp.getMotionGraphicsTemplateControllerName(1), comp.getMotionGraphicsTemplateControllerName(2));
        comp.setMotionGraphicsControllerName(2, "Fade Again");
        r.push(comp.getMotionGraphicsTemplateControllerName(2));
        r
        "#,
    );
    assert_eq!(o.result, json!([true, true, true, 2, "Fade", "Fade", "Fade Again"]));
    let (_, c) = comp_named(&s, "Card");
    let eg = c.essential.as_ref().unwrap();
    assert!(matches!(eg.controls[1].kind, effectcraft_engine::project::essential::EgKind::Mirror { of } if of == eg.controls[0].id));
}

#[test]
fn comp_preserves_nested_frame_rate_and_resolution() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var comp = app.project.items.addComp("Nested", 320, 180, 1, 2, 12);
        var before = [comp.preserveNestedFrameRate, comp.preserveNestedResolution];
        comp.preserveNestedFrameRate = true;
        comp.preserveNestedResolution = true;
        before.concat([comp.preserveNestedFrameRate, comp.preserveNestedResolution]).join(",");
        "#,
    );
    assert_eq!(o.result, json!("false,false,true,true"));
    let (_, c) = comp_named(&s, "Nested");
    assert!(c.preserve_frame_rate && c.preserve_resolution);
}

#[test]
fn text_document_baseline_flags_reflect_pending_edits_and_applied_value() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var comp = app.project.items.addComp("Baseline", 64, 64, 1, 1, 24);
        var src = comp.layers.addText("AB").text.sourceText;
        var doc = src.value;
        doc.superscript = true;
        var pendingSuper = [doc.superscript, doc.subscript];
        doc.text = doc.superscript ? "super" : "normal";
        src.setValue(doc);
        var appliedSuper = [src.value.text, src.value.superscript, src.value.subscript];
        doc = src.value;
        doc.subscript = true;
        var pendingSub = [doc.superscript, doc.subscript];
        src.setValue(doc);
        var appliedSub = [src.value.superscript, src.value.subscript];
        doc = src.value;
        doc.subscript = false;
        var cleared = [doc.superscript, doc.subscript];
        src.setValue(doc);
        var appliedClear = [src.value.superscript, src.value.subscript];
        doc = src.value;
        doc.superscript = true;
        doc.superscript = false;
        var repeatClear = [doc.superscript, doc.subscript];
        [pendingSuper, appliedSuper, pendingSub, appliedSub, cleared, appliedClear, repeatClear];
        "#,
    );
    assert_eq!(o.result, json!([[true, false], ["super", true, false], [false, true], [false, true], [false, false], [false, false], [false, false]]));
}

#[test]
fn setting_item_selected_false_preserves_other_project_items() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var target = app.project.items.addFolder("Target");
        var keep = app.project.items.addFolder("Keep");
        app.run("project.select", {items: []});
        keep.selected = true;
        target.selected = true;
        var states = [[target.selected, keep.selected]];
        target.selected = false;
        states.push([target.selected, keep.selected]);
        target.selected = false; // idempotent: never remove Keep
        states.push([target.selected, keep.selected]);
        target.selected = true;
        states.push([target.selected, keep.selected]);
        app.project.selection.map(function (item) { return item.name; }).join(",")
          + "|" + JSON.stringify(states);
        "#,
    );
    assert_eq!(o.result, json!("Keep,Target|[[true,true],[false,true],[false,true],[true,true]]"));
}

#[test]
fn add_solid_keeps_the_requested_pixel_aspect() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("PA", 64, 32, 1, 1, 10);
        var square = c.layers.addSolid([1, 0, 0], "Square", 16, 16, 1);
        var wide = c.layers.addSolid([0, 1, 0], "Wide", 16, 16, 2);
        var inherited = c.layers.addSolid([0, 0, 1], "Inherited", 16, 16);
        [c.pixelAspect, square.source.pixelAspect, wide.source.pixelAspect, inherited.source.pixelAspect,
         wide.source.width, wide.source.height]
        "#,
    );
    assert_eq!(o.result, json!([1, 1, 2, 1, 16, 16]));
    let bad = run_code(&mut s, r#"app.project.items.addComp("Q", 8, 8, 1, 1, 10).layers.addSolid([1, 1, 1], "Bad", 8, 8, 0)"#, "bad.jsx");
    assert!(bad.error.is_some_and(|e| e.message.contains("pixelAspect")));
}

#[test]
fn property_selected_false_preserves_other_property_and_key_selections() {
    let mut s = session();
    let out = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("Selection", 64, 64, 1, 1, 10);
        c.openInViewer();
        var layer = c.layers.addSolid([1, 0, 0], "Layer", 8, 8, 1);
        var opacity = layer.transform.opacity;
        var rotation = layer.transform.rotation;
        rotation.setValueAtTime(0, 0);
        rotation.setValueAtTime(0.5, 45);
        app.run("edit.deselectAll", {});
        rotation.selected = true;
        opacity.selected = true;
        var states = [[opacity.selected, rotation.selected]];
        opacity.selected = false;
        states.push([opacity.selected, rotation.selected]);
        opacity.selected = false;
        states.push([opacity.selected, rotation.selected]);
        opacity.selected = true;
        states.push([opacity.selected, rotation.selected]);
        [states, rotation.selectedKeys.length, c.selectedProperties.length];
        "#,
    );
    assert_eq!(out.result, json!([[[true, true], [false, true], [false, true], [true, true]], 2, 2]));
}

#[test]
fn group_selected_false_deselects_masks_and_effects() {
    let mut s = session();
    let out = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("Groups", 64, 64, 1, 1, 10);
        c.openInViewer();
        var l = c.layers.addSolid([1, 0, 0], "Layer", 64, 64, 1);
        var m = l.property("ADBE Mask Parade").addProperty("ADBE Mask Atom");
        var fx = l.property("ADBE Effect Parade").addProperty("Gaussian Blur");
        app.run("edit.deselectAll", {});
        m.selected = true;
        fx.selected = true;
        var states = [[m.selected, fx.selected]];
        m.selected = false;
        states.push([m.selected, fx.selected]);
        fx.selected = false;
        states.push([m.selected, fx.selected]);
        states;
        "#,
    );
    assert_eq!(out.result, json!([[true, true], [false, true], [false, false]]));
}

/// A layer selected by script in a comp that is not open stays selected for the next script:
/// every comp keeps its own layer selection (#1001).
#[test]
fn layer_selection_in_a_closed_comp_survives_to_the_next_script() {
    let mut s = session();
    let out = ok(
        &mut s,
        r#"
        var a = app.project.items.addComp("Active", 400, 300, 1, 2, 25);
        a.openInViewer();
        var c = app.project.items.addComp("Other", 400, 300, 1, 2, 25);
        c.layers.addText("A");
        c.layers.addText("B");
        c.layer("B").selected = true;
        c.selectedLayers.length + " " + c.layer("B").selected + " " + a.selectedLayers.length
        "#,
    );
    assert_eq!(out.result, json!("1 true 0"));
    // A second script call, the way a panel talks to the app over the control channel.
    let out = ok(
        &mut s,
        r#"
        var c = null;
        for (var k = 1; k <= app.project.numItems; k++) { if (app.project.item(k).name === "Other") c = app.project.item(k); }
        c.selectedLayers.length + " " + c.layer("B").selected
        "#,
    );
    assert_eq!(out.result, json!("1 true"));
}

/// Probes every file as a 64x32 still (the media layer's single-file probe).
struct Stills;
impl effectcraft_engine::Importer for Stills {
    fn probe(&self, path: &str) -> Result<effectcraft_engine::project::Footage, String> {
        use effectcraft_engine::project::{Footage, FootageKind};
        Ok(Footage { path: path.into(), kind: FootageKind::Still, width: 64, height: 32, has_video: true, codec: "PNG".into(), ..Default::default() })
    }
}

/// `importFile(new ImportOptions(file))` imports just the picked file unless `sequence` is set (#475).
#[test]
fn import_options_sequence_picks_one_still_or_the_run() {
    use effectcraft_engine::project::{FootageKind, ItemKind};
    let dir = std::env::temp_dir().join(format!("ec-script-seq-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["shot_0001.png", "shot_0002.png", "shot_0003.png"] {
        std::fs::write(dir.join(n), b"").unwrap();
    }
    let pick = dir.join("shot_0002.png").to_string_lossy().replace('\\', "/");
    let mut s = session();
    s.importer = Some(Arc::new(Stills));
    let kind_of = |s: &Session, name: &str| {
        let it = s.project.items.values().find(|i| i.name == name).unwrap_or_else(|| panic!("no item {name}"));
        let ItemKind::Footage(f) = &it.kind else { panic!("footage") };
        f.kind
    };
    ok(&mut s, &format!("var io = new ImportOptions(new File('{pick}')); app.project.importFile(io);"));
    assert_eq!(kind_of(&s, "shot_0002.png"), FootageKind::Still);
    ok(&mut s, &format!("var io = new ImportOptions(new File('{pick}')); io.sequence = true; app.project.importFile(io);"));
    assert_eq!(kind_of(&s, "shot_[0001-0003].png"), FootageKind::Sequence);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #813: FootageItem proxies, replaceWithSequence and mainSource.conformFrameRate.
#[test]
fn footage_proxies_sequences_and_conform_rate() {
    use effectcraft_engine::project::ItemKind;
    let dir = std::env::temp_dir().join(format!("ec-script-proxy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["shot_0001.png", "shot_0002.png", "take_0001.png", "take_0002.png", "proxy.png"] {
        std::fs::write(dir.join(n), b"").unwrap();
    }
    let p = |n: &str| dir.join(n).to_string_lossy().replace('\\', "/");
    let mut s = session();
    s.importer = Some(Arc::new(Stills));
    let o = ok(
        &mut s,
        &format!(
            "var io = new ImportOptions(new File('{}')); io.sequence = true; var item = app.project.importFile(io);\
             var same = item.mainSource === item.mainSource;\
             item.mainSource.conformFrameRate = 12; var c12 = item.mainSource.conformFrameRate, fr = item.frameRate;\
             item.mainSource.conformFrameRate = 0; var c0 = item.mainSource.conformFrameRate;\
             var before = [item.useProxy, item.proxySource];\
             item.setProxy(new File('{}')); var set = [item.useProxy, item.proxySource.file.name];\
             item.useProxy = false; var off = item.useProxy;\
             item.setProxyToNone(); var none = item.proxySource;\
             item.replaceWithSequence(new File('{}'), false);\
             [same, c12, fr, c0, before[0], before[1], set[0], set[1], off, none]",
            p("shot_0001.png"),
            p("proxy.png"),
            p("take_0001.png")
        ),
    );
    assert_eq!(o.result, json!([true, 12, 12, 0, false, null, true, "proxy.png", false, null]));
    let paths: Vec<String> =
        s.project.items.values().filter_map(|i| if let ItemKind::Footage(f) = &i.kind { Some(f.path.replace('\\', "/")) } else { None }).collect();
    assert!(paths.iter().any(|f| f.contains("take_")), "replaced with the take sequence: {paths:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A flat green footage source, so `sampleImage` on a footage layer has known pixels.
struct Flat;
impl effectcraft_engine::render::FootageSource for Flat {
    fn frame(
        &self,
        _item: effectcraft_engine::project::ItemId,
        f: &effectcraft_engine::project::Footage,
        _t: effectcraft_engine::time::Tick,
    ) -> Option<Arc<effectcraft_engine::render::Image>> {
        Some(Arc::new(effectcraft_engine::render::Image::filled(f.width.max(1), f.height.max(1), [0.0, 1.0, 0.0, 1.0])))
    }
}

/// `sampleImage` on a footage layer reads the footage's pixels from a script, not transparent
/// black: the expression evaluator needs the session's footage source (#1000).
#[test]
fn sample_image_reads_footage_pixels_in_scripts() {
    let mut s = session();
    s.footage = Arc::new(Flat);
    s.importer = Some(Arc::new(Stills));
    let out = ok(
        &mut s,
        r#"
        var comp = app.project.items.addComp("Scene", 640, 360, 1, 1, 25);
        comp.openInViewer();
        var img = comp.layers.add(app.project.importFile(new ImportOptions(new File("/tmp/ec-flat.png"))));
        img.name = "Img";
        var cc = comp.layers.addNull().property("ADBE Effect Parade").addProperty("ADBE Color Control").property(1);
        cc.expression = 'thisComp.layer("Img").sampleImage([32, 16], [0.5, 0.5], true, time);';
        var v = cc.valueAtTime(0, false);
        [v[0], v[1], v[2], v[3], cc.expressionError];
        "#,
    );
    assert_eq!(out.result, json!([0, 1, 0, 1, ""]), "green footage sampled, not transparent black");
}

#[test]
fn binary_file_io_preserves_all_byte_values() {
    let dir = std::env::temp_dir().join(format!("ec-script-binary-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("sig.bin");
    let dst = dir.join("written.bin");
    let copied = dir.join("copied.bin");
    let original = [0x89u8, 0x50, 0x4e, 0x47, 0x00, 0x7f, 0x80, 0xff, 0x0a];
    std::fs::write(&src, original).unwrap();
    let mut s = session();
    s.path = Some(dir.join("project.ecproj").to_string_lossy().to_string());
    s.prefs.scripting.allow_scripts_write_files = true;
    let q = |p: &std::path::Path| serde_json::to_string(&p.to_string_lossy()).unwrap();

    let read = ok(
        &mut s,
        &format!(
            "var f = new File({}); f.encoding = 'BINARY'; f.open('r'); var data = f.read(); f.close(); [data.length, data.charCodeAt(0), data.charCodeAt(6), data.charCodeAt(7)]",
            q(&src)
        ),
    );
    assert_eq!(read.result, json!([9, 0x89, 0x80, 0xff]));

    ok(
        &mut s,
        &format!(
            "var f = new File({}); f.encoding = 'BINARY'; f.open('w'); f.write(String.fromCharCode(0x89, 0x50, 0x4e, 0x47, 0, 127, 128, 255, 10)); f.close();",
            q(&dst)
        ),
    );
    assert_eq!(std::fs::read(&dst).unwrap(), original);

    let copy = ok(&mut s, &format!("new File({}).copy({})", q(&src), q(&copied)));
    assert_eq!(copy.result, json!(true));
    assert_eq!(std::fs::read(&copied).unwrap(), original);

    ok(&mut s, &format!("var f = new File({}); f.encoding = 'BINARY'; f.open('a'); f.write(String.fromCharCode(0xfe)); f.close();", q(&dst)));
    assert_eq!(std::fs::read(&dst).unwrap(), [original.as_slice(), &[0xfe]].concat());

    let invalid = run_code(
        &mut s,
        &format!("var f = new File({}); f.encoding = 'BINARY'; f.open('w'); f.write(String.fromCharCode(0x100)); f.close();", q(&dst)),
        "invalid-binary.jsx",
    );
    assert!(invalid.error.is_some(), "code points above 255 must not be truncated");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn script_undo_uses_stable_after_effects_command_id() {
    let mut s = session();
    let lookup = ok(&mut s, "app.findMenuCommandId('Undo')");
    assert_eq!(lookup.result, json!(16));
    // ID 16 used to index an unrelated command as the registry grew. It must
    // undo the preceding layer creation rather than dispatching by position.
    let r = ok(&mut s, "var c = app.project.items.addComp('Undo ID', 100, 100, 1, 1, 24); c.layers.addNull(); app.executeCommand(16); c.numLayers");
    assert_eq!(r.result, json!(0));
    assert_eq!(s.project.comp(ItemId(1)).unwrap().layers.len(), 0);
    // Lookup of other command names still uses the existing registry.
    assert!(ok(&mut s, "app.findMenuCommandId('New Project')").result.as_u64().unwrap_or(0) > 0);
}

#[test]
fn long_script_undo_group_counts_as_one_level() {
    let mut s = session();
    s.prefs.general.undo_levels = 3;
    ok(&mut s, "var c = app.project.items.addComp('Long Undo', 100, 100, 1, 1, 24); c.layers.addNull();");
    assert_eq!(s.history.undo.len(), 2);
    // More edits than the history cap must still be one undoable group.
    ok(&mut s, "var c = app.project.item(1); app.beginUndoGroup('Forty layers'); for (var i = 0; i < 40; i++) c.layers.addNull(); app.endUndoGroup();");
    assert_eq!(s.project.comp(ItemId(1)).unwrap().layers.len(), 41);
    assert_eq!(s.history.undo.len(), 3);
    assert_eq!(s.history.undo[2].0, "Forty layers");
    s.undo();
    assert_eq!(s.project.comp(ItemId(1)).unwrap().layers.len(), 1);
    // Entries preceding the group remain available instead of being trimmed by
    // the forty intermediate operations.
    s.undo();
    assert_eq!(s.project.comp(ItemId(1)).unwrap().layers.len(), 0);
}

#[test]
fn script_remove_keys_batches_a_range_as_one_edit() {
    let mut s = session();
    let o = ok(
        &mut s,
        "var c = app.project.items.addComp('Batch Keys', 100, 100, 1, 10, 25); var p = c.layers.addNull().transform.rotation; for (var i = 0; i < 5; i++) p.setValueAtTime(i, i * 10); p.removeKeys(2, 4); [p.numKeys, p.keyTime(1), p.keyTime(2)]",
    );
    assert_eq!(o.result, json!([2, 0, 4]));
    // The entire deletion is one undo step; keys outside the range survive.
    s.undo();
    let restored = ok(&mut s, "app.project.item(1).layer(1).transform.rotation.numKeys");
    assert_eq!(restored.result, json!(5));
    let invalid = run_code(&mut s, "app.project.item(1).layer(1).transform.rotation.removeKeys(0, 2)", "remove-keys-range.jsx");
    assert!(invalid.error.is_some());
}

/// #814: one key is looked up without serialising the whole list (removeKey on a property with
/// thousands of keys was linear in the key count), and the key calls behave as before.
#[test]
fn script_key_lookup_fetches_only_that_key() {
    let mut s = session();
    let o = ok(
        &mut s,
        "var c = app.project.items.addComp('Keys', 100, 100, 1, 100, 25); var p = c.layers.addNull().transform.rotation;\
         var ts = [], vs = []; for (var i = 0; i < 2000; i++) { ts.push(i / 25); vs.push(i); } p.setValuesAtTimes(ts, vs);\
         var q = { comp: p.__layer.__comp, layer: p.__layer.__id, uid: p.__uid };\
         q.index = 2; var one = __get('keys', q); q.index = 2000; var none = __get('keys', q);\
         p.removeKey(p.numKeys); p.removeKey(1);\
         var err = ''; try { p.keyTime(1999); } catch (e) { err = e.message; }\
         [one.count, one.key.time, none.key, p.numKeys, p.keyTime(1), p.keyValue(1998), err]",
    );
    assert_eq!(o.result, json!([2000, 0.08, null, 1998, 0.04, 1998, "keyIndex 1999 is out of range 1..1998"]));
}

/// #815: a stroke's Dashes use After Effects' match names, and addProperty with them returns
/// Dash 1 / Gap 1 / Offset or adds the Dash 2 / Gap 2 pair.
#[test]
fn script_stroke_dashes_use_after_effects_match_names() {
    let mut s = session();
    let o = ok(
        &mut s,
        "function names(g) { var a = []; for (var i = 1; i <= g.numProperties; i++) a.push(g.property(i).matchName); return a.join(','); }\
         var comp = app.project.items.addComp('Names', 100, 100, 1, 10, 25);\
         var contents = comp.layers.addShape().property('ADBE Root Vectors Group'); contents.addProperty('ADBE Vector Shape - Rect');\
         var d = contents.addProperty('ADBE Vector Graphic - Stroke').property('ADBE Vector Stroke Dashes');\
         var before = names(d);\
         var dash1 = d.addProperty('ADBE Vector Stroke Dash 1'); dash1.setValue(12);\
         var gap2 = d.addProperty('ADBE Vector Stroke Gap 2');\
         var err = ''; try { d.addProperty('ADBE Vector Stroke Dash 9'); } catch (e) { err = e.message; }\
         [before, dash1.matchName, d.property('ADBE Vector Stroke Dash 1').value, d.property('ADBE Vector Stroke Offset').matchName, gap2.matchName, names(d), err]",
    );
    assert_eq!(
        o.result,
        json!([
            "ADBE Vector Stroke Dash 1,ADBE Vector Stroke Gap 1,ADBE Vector Stroke Offset",
            "ADBE Vector Stroke Dash 1",
            12,
            "ADBE Vector Stroke Offset",
            "ADBE Vector Stroke Gap 2",
            "ADBE Vector Stroke Dash 1,ADBE Vector Stroke Gap 1,ADBE Vector Stroke Dash 2,ADBE Vector Stroke Gap 2,ADBE Vector Stroke Offset",
            "Can not add \"ADBE Vector Stroke Dash 9\" to Dashes"
        ])
    );
}

#[test]
fn script_layer_comment_and_unnamed_marker_params_round_trip() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("Notes", 100, 100, 1, 5, 25);
        var l = c.layers.addSolid([1, 0, 0], "Solid", 10, 10, 1);
        l.comment = "Saved layer note";
        var markers = l.property("ADBE Marker");
        var plain = new MarkerValue("Plain");
        plain.setParameters({ lml: "shot", index: "1" });
        markers.setValueAtTime(1, plain);
        var named = new MarkerValue("Named");
        named.cuePointName = "cue";
        named.setParameters({ lml: "shot", index: "2" });
        markers.setValueAtTime(2, named);
        [l.comment, markers.keyValue(1).getParameters().index,
         markers.keyValue(1).cuePointName, markers.keyValue(2).getParameters().index]
        "#,
    );
    assert_eq!(o.result, json!(["Saved layer note", "1", "", "2"]));
}

#[test]
fn script_av_layer_transform_shortcuts_return_live_properties() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("Shortcuts", 100, 100, 1, 5, 25);
        var l = c.layers.addSolid([1, 0, 0], "S", 10, 10, 1);
        l.position.setValue([30, 40]);
        l.rotation.setValueAtTime(1, 45);
        [l.position.value[0], l.transform.position.value[1], l.rotation.numKeys,
         l.opacity === undefined, l.scale === undefined, l.anchorPoint === undefined]
        "#,
    );
    assert_eq!(o.result, json!([30, 40, 1, false, false, false]));
    s.undo();
    let restored = ok(&mut s, "app.project.item(1).layer(1).rotation.numKeys");
    assert_eq!(restored.result, json!(0));
}

#[test]
fn extendscript_substr_keeps_start_and_length_semantics() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"[
          "abcdef".substr(0, 2),
          "abcdef".substr(2, 3),
          "abcdef".substr(-2),
          "abcdef".substr(-20, 2),
          "abcdef".substr(2, 0),
          "abcdef".substr(2, -3),
          "abcdef".substr(2.9, 2.9),
          "abcdef".substr(3),
          "abcdef".substr(30, 2),
          "abcdef".substr(NaN, 2),
          "abcdef".substr(2, NaN)
        ]"#,
    );
    assert_eq!(o.result, json!(["ab", "cde", "ef", "ab", "", "", "cd", "def", "", "ab", ""]));
}

#[test]
fn import_options_preserve_layered_psd_compositions() {
    use effectcraft_psd::Rect;
    use effectcraft_psd::write::{WDoc, WLayer, write};

    let dir = std::env::temp_dir().join(format!("ec-script-import-as-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("layers.psd");
    let mut doc = WDoc::new(12, 12);
    doc.layers.push(WLayer::solid("Background", Rect::new(0, 0, 12, 12), [1.0, 1.0, 1.0, 1.0]));
    doc.layers.push(WLayer::solid("Accent", Rect::new(2, 2, 4, 4), [0.0, 0.4, 1.0, 1.0]));
    std::fs::write(&source, write(&doc)).unwrap();
    let file = serde_json::to_string(&source.to_string_lossy()).unwrap();
    let mut s = session();
    let o = ok(
        &mut s,
        &format!(
            "var io = new ImportOptions(new File({file})); io.importAs = ImportAsType.COMPOSITION_CROPPED_LAYERS; \
             [io.canImportAs(io.importAs), app.project.importFile(io).typeName]"
        ),
    );
    assert_eq!(o.result, json!([true, "Composition"]));
    let imported = s.project.comps().next().unwrap().1;
    assert_eq!(imported.layers.len(), 2);
    let bad = ok(&mut s, &format!("new ImportOptions(new File({file})).canImportAs(ImportAsType.PROJECT)"));
    assert_eq!(bad.result, json!(false));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn script_item_and_layer_identity_survives_repeated_access() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("Identity", 120, 80, 1, 3, 25);
        var parent = c.layers.addNull(), child = c.layers.addNull();
        child.parent = parent;
        [
          parent.containingComp === c,
          c.layer(1) === c.layer(1),
          child.parent === parent,
          app.project.item(1) === app.project.item(1),
          app.project.item(1) === c,
          app.project.itemByID(c.id) === c
        ]
        "#,
    );
    assert_eq!(o.result, json!([true, true, true, true, true, true]));
    // Reusing item and layer IDs in a new project must not alias old wrappers.
    let o = ok(
        &mut s,
        r#"
        var old = app.project.item(1);
        app.newProject();
        var next = app.project.items.addComp("New", 120, 80, 1, 3, 25);
        [old !== next, app.project.item(1) === next]
        "#,
    );
    assert_eq!(o.result, json!([true, true]));
}

#[test]
fn script_text_path_options_expose_after_effects_match_names() {
    let mut s = session();
    let r = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("Text Path", 160, 90, 1, 2, 24);
        var t = c.layers.addText("On a path");
        var options = t.property("ADBE Text Properties").property("ADBE Text Path Options");
        var names = [];
        for (var i = 1; i <= options.numProperties; i++)
          names.push(options.property(i).matchName);
        [names, options.property("ADBE Text Path").matchName,
         options.property("ADBE Text Perpendicular To Path").matchName]
        "#,
    );
    assert_eq!(
        r.result,
        json!([
            [
                "ADBE Text Path",
                "ADBE Text Reverse Path",
                "ADBE Text Perpendicular To Path",
                "ADBE Text Force Align Path",
                "ADBE Text First Margin",
                "ADBE Text Last Margin"
            ],
            "ADBE Text Path",
            "ADBE Text Perpendicular To Path"
        ])
    );
}

/// A scripted text animator has only the selectors the script adds, and the Range Selector and
/// its Start/End/Offset resolve by After Effects match name (#837).
#[test]
fn scripted_text_range_selector_resolves_and_is_the_only_selector() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("Sel", 960, 300, 1, 3, 30);
        var t = c.layers.addText("MOVARA");
        var a = t.property("ADBE Text Properties").property("ADBE Text Animators").addProperty("ADBE Text Animator");
        a.property("ADBE Text Animator Properties").addProperty("ADBE Text Opacity").setValue(0);
        var sels = a.property("ADBE Text Selectors");
        var sel = sels.addProperty("ADBE Text Selector");
        sel.property("ADBE Text Percent Start").setValue(50);
        var end = sel.property("ADBE Text Percent End").value;
        [sels.numProperties, sel.matchName, sel.property("start").value, end, sel.property("ADBE Text Percent Offset").matchName]
        "#,
    );
    assert_eq!(o.result, json!([1, "ADBE Text Selector", 50, 100, "ADBE Text Percent Offset"]));
}

/// `setInterpolationTypeAtKey` and `setTemporalEaseAtKey` (`keys.interpolation` / `keys.velocity`)
/// act on effect parameters too, not just transform properties (#994).
#[test]
fn effect_parameter_interpolation_and_ease() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("E", 100, 100, 1, 5, 25);
        var l = c.layers.addNull();
        var slider = l.property("ADBE Effect Parade").addProperty("ADBE Slider Control").property(1);
        slider.setValuesAtTimes([0, 1, 2], [0, 50, 100]);
        slider.setInterpolationTypeAtKey(1, KeyframeInterpolationType.BEZIER, KeyframeInterpolationType.BEZIER);
        slider.setTemporalEaseAtKey(2, [new KeyframeEase(0, 16.67)], [new KeyframeEase(0, 33.33)]);
        var point = l.property("ADBE Effect Parade").addProperty("ADBE Point3D Control").property(1);
        point.setValuesAtTimes([0, 1], [[0, 0, 0], [50, 50, 50]]);
        point.setInterpolationTypeAtKey(1, KeyframeInterpolationType.BEZIER, KeyframeInterpolationType.BEZIER);
        [slider.keyInInterpolationType(1) == KeyframeInterpolationType.BEZIER,
         slider.keyOutInterpolationType(1) == KeyframeInterpolationType.BEZIER,
         slider.keyInTemporalEase(2)[0].influence,
         slider.keyOutTemporalEase(2)[0].influence,
         point.keyInInterpolationType(1) == KeyframeInterpolationType.BEZIER];
        "#,
    );
    assert_eq!(o.result, json!([true, true, 16.67, 33.33, true]));
}

/// #1531: the per-key methods act on a comp that isn't the active one too. Activating the keys'
/// comp brought back its own (empty) key selection over the keys named, so the interpolation and
/// the ease were silently dropped once another comp with a layer had been made first.
#[test]
fn key_interpolation_and_ease_apply_outside_the_active_comp() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var a = app.project.items.addComp("A", 400, 400, 1, 10, 20); a.layers.addNull();
        var c = app.project.items.addComp("C", 400, 400, 1, 10, 20);
        var n = c.layers.addNull(); n.threeDLayer = true;
        var p = n.property("ADBE Transform Group").property("ADBE Rotate Y");
        p.setValueAtTime(1, 0); p.setValueAtTime(2, -178);
        for (var j = 1; j <= 2; j++) {
          p.setInterpolationTypeAtKey(j, KeyframeInterpolationType.BEZIER, KeyframeInterpolationType.BEZIER);
          p.setTemporalEaseAtKey(j, [new KeyframeEase(0, 60)], [new KeyframeEase(0, 60)]);
        }
        [p.keyOutInterpolationType(1) == KeyframeInterpolationType.BEZIER,
         p.keyInInterpolationType(2) == KeyframeInterpolationType.BEZIER,
         p.keyOutTemporalEase(1)[0].influence,
         Math.round(p.valueAtTime(1.25, false) * 100) / 100]
        "#,
    );
    let v = o.result;
    assert_eq!((&v[0], &v[1], &v[2]), (&json!(true), &json!(true), &json!(60)), "{v}");
    // Eased, not the linear -44.5.
    assert!(v[3].as_f64().is_some_and(|x| (x - -44.5).abs() > 1.0), "{v}");
}

/// #808: sourcePointToComp and compPointToSource follow the layer's transform.
#[test]
fn source_point_conversions_follow_the_layer_transform() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var comp = app.project.items.addComp("Repro", 1920, 1080, 1, 10, 25);
        var layer = comp.layers.addSolid([1, 0, 0], "S", 200, 100, 1);
        layer.transform.position.setValue([400, 300]);
        layer.transform.scale.setValue([50, 50]);
        var a = layer.sourcePointToComp([0, 0]);
        var b = layer.compPointToSource([400, 300]);
        [a[0], a[1], b[0], b[1]]
        "#,
    );
    let v = o.result;
    let near = |x: &Value, want: f64| x.as_f64().is_some_and(|g| (g - want).abs() < 1e-6);
    assert!(near(&v[0], 350.0) && near(&v[1], 275.0), "{v}");
    assert!(near(&v[2], 100.0) && near(&v[3], 50.0), "{v}");
}

/// #1063: a script asks a light's options for After Effects' "ADBE Casts Shadows".
#[test]
fn light_options_accept_the_after_effects_casts_shadows_name() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var comp = app.project.items.addComp("Repro", 1920, 1080, 1, 10, 25);
        var light = comp.layers.addLight("Key", [100, 100]);
        var c = light.property("ADBE Light Options Group").property("ADBE Casts Shadows");
        c == null ? "null" : c.name
        "#,
    );
    assert_ne!(o.result, json!("null"), "{:?}", o.result);
}

/// A Google Earth Studio camera export: transform and camera shortcuts on the camera layer, keys
/// set through them, and Convert Expression to Keyframes run on the selected property (#1077).
#[test]
fn earth_studio_style_script_keys_the_camera() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var comp = app.project.items.addComp("Orbit", 1920, 1080, 1, 1, 10);
        comp.openInViewer();
        var cam = comp.layers.addCamera("Camera", ["960", "540"]);
        cam.locked = true;
        var pos = cam.position, rx = cam.xRotation, ry = cam.yRotation, rz = cam.zRotation, zoom = cam.zoom;
        cam.autoOrient = AutoOrientType.NO_AUTO_ORIENT;
        var ints = comp.layers.addNull();
        ints.name = "ints";
        ints.threeDLayer = true;
        ints.position.setValuesAtTimes([0, 0.5], [[100, 200, 300], [110, 220, 330]]);
        rx.setValuesAtTimes([0, 0.5], [181, 182]);
        ry.setValuesAtTimes([0, 0.5], [39, 38]);
        rz.setValuesAtTimes([0, 0.5], [87, 86]);
        pos.expression = 'thisComp.layer("ints").transform.position';
        var id = app.findMenuCommandId("Convert Expression to Keyframes");
        if (id && pos.canSetExpression && pos.expressionEnabled) {
          pos.selected = true;
          app.executeCommand(id);
          pos.selected = false;
          ints.remove();
        }
        zoom.setValueAtTime(0, 1500);
        zoom.setValueAtTime(0.5, 1600);
        [pos.numKeys, rx.numKeys, ry.numKeys, rz.numKeys, zoom.numKeys, comp.numLayers,
         cam.pointOfInterest.matchName, cam.orientation.matchName, cam.depthOfField.matchName,
         cam.focusDistance.matchName, cam.aperture.matchName, cam.blurLevel.matchName]
        "#,
    );
    assert_eq!(
        o.result,
        json!([
            10,
            2,
            2,
            2,
            2,
            1,
            "ADBE Anchor Point",
            "ADBE Orientation",
            "ADBE Camera Depth of Field",
            "ADBE Camera Focus Distance",
            "ADBE Camera Aperture",
            "ADBE Camera Blur Level"
        ])
    );
    let (_, c) = comp_named(&s, "Orbit");
    let cam = layer(c, "Camera");
    let p = cam.props.prop("transform/position").unwrap();
    assert_eq!(p.keys[5].value, KV::Vec3([110.0, 220.0, 330.0]));
    assert!(!p.expr.as_ref().unwrap().enabled);
    assert_eq!(cam.props.prop("transform/rotationX").unwrap().keys[1].value, KV::Scalar(182.0));
    assert_eq!(cam.props.prop("cameraOptions/zoom").unwrap().keys[1].value, KV::Scalar(1600.0));
}

/// Pro Levels2's Alpha controls sit one group down (under Alpha), so a script finds them by their
/// name on the effect as well as through the group (#1063).
#[test]
fn scripted_levels_alpha_controls_resolve_from_the_effect() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("Levels", 320, 180, 1, 3, 30);
        var l = c.layers.addSolid([1, 0, 0], "S", 320, 180, 1);
        var fx = l.property("ADBE Effect Parade").addProperty("ADBE Pro Levels2");
        fx.property("Alpha Input White").setValue(0.5);
        [fx.property("Alpha").property("Alpha Input White").value, fx.property("Alpha Input White").value]
        "#,
    );
    assert_eq!(o.result, json!([0.5, 0.5]));
}

/// After Effects resolves the Transform properties and Source Text from the layer itself, without
/// naming their group: `layer.property("Position")` (#1348).
#[test]
fn layer_property_resolves_transform_and_source_text_shortcuts() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("x", 100, 100, 1, 5, 30);
        var t = c.layers.addText("hi");
        var s = c.layers.addSolid([1, 0, 0], "s", 100, 100, 1);
        var listed = [t.property("Source Text"), s.property("Position"), s.property("Opacity"), s.property("Scale")].join(", ");
        s.property("Position").setValue([30, 40]);
        s.property("Opacity").setValue(25);
        t.property("Source Text").setValue("bye");
        s.threeDLayer = true;
        [listed, s.property("Position").value[0], s.property("Opacity").value, t.property("Source Text").value.text,
         s.property("ADBE Position").matchName, s("Anchor Point").matchName, s.property("Rotation").matchName,
         s.property("X Rotation").matchName, s.property("Orientation").matchName]
        "#,
    );
    assert_eq!(
        o.result,
        json!([
            "[object Property], [object Property], [object Property], [object Property]",
            30,
            25,
            "bye",
            "ADBE Position",
            "ADBE Anchor Point",
            "ADBE Rotate Z",
            "ADBE Rotate X",
            "ADBE Orientation"
        ])
    );
    let (_, c) = comp_named(&s, "x");
    assert_eq!(layer(c, "s").props.prop("transform/opacity").unwrap().value, KV::Scalar(25.0));
}
