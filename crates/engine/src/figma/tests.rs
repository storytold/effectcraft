//! Figma import: REST / plug-in fixtures → plans, sessions and renders; the REST path against a
//! local fake API server; hostile payloads.

use effectcraft_keyframe::Value as KV;
use effectcraft_keyframe::text_doc::Justify;
use effectcraft_project::{LayerSource, MatteKind};
use serde_json::{Value, json};

use super::fetch;
use super::map::{self, Item, PKind, Target};
use crate::Session;

const CARD: &str = include_str!("fixtures/card_rest.json");
const MASKS: &str = include_str!("fixtures/masks_plugin.json");

fn card() -> Value {
    serde_json::from_str(CARD).unwrap()
}
fn masks() -> Value {
    serde_json::from_str(MASKS).unwrap()
}

fn plan(payload: &Value, target: Target) -> map::Plan {
    let nodes = map::select_nodes(payload, &[]).unwrap();
    map::plan(&nodes, &map::Options { target })
}

fn layer<'a>(c: &'a map::PComp, name: &str) -> &'a map::PLayer {
    c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no layer {name}"))
}

fn group_items(l: &map::PLayer) -> &[Item] {
    let PKind::Shape(items) = &l.kind else { panic!("{} is not a shape", l.name) };
    match items.first() {
        Some(Item::Group { items, .. }) => items,
        _ => panic!("no group"),
    }
}

#[test]
fn parses_figma_links() {
    let r = fetch::parse_url("https://www.figma.com/design/AbCdEfGhIjKlMnOpQrStUv/My-File?node-id=12-345&t=x").unwrap();
    assert_eq!(r.key, "AbCdEfGhIjKlMnOpQrStUv");
    assert_eq!(r.ids, ["12:345"]);
    let r = fetch::parse_url("https://figma.com/file/AbCdEfGhIjKlMnOpQrStUv/x?node-id=1%3A2").unwrap();
    assert_eq!(r.ids, ["1:2"]);
    let r = fetch::parse_url("https://www.figma.com/design/AbCdEfGhIjKlMnOpQrStUv/branch/ZyXwVuTsRqPoNmLkJiHgFe/x").unwrap();
    assert_eq!(r.key, "ZyXwVuTsRqPoNmLkJiHgFe");
    assert!(r.ids.is_empty());
    assert!(fetch::parse_url("https://example.com/design/AbCdEfGhIjKlMnOpQrStUv").is_err());
    assert!(fetch::parse_url("https://www.figma.com/design/short").is_err());
    assert!(fetch::parse_url("https://www.figma.com/design/AbCdEfGhIjKlMnOpQrStUv/x?node-id=1-2%22").is_err());
    assert_eq!(fetch::check_api_base("https://api.figma.com/").unwrap(), "https://api.figma.com");
    assert!(fetch::check_api_base("http://127.0.0.1:9").is_ok());
    assert!(fetch::check_api_base("https://evil.example").is_err());
    assert!(fetch::check_api_base("http://127.0.0.1.evil.example").is_err());
    // Review on #1103: user info, host names and malformed authorities never get the token.
    for bad in [
        "http://127.0.0.1:80@evil.com",
        "http://127.0.0.1@evil.com",
        "http://evil.com#@127.0.0.1",
        "http://evil.com?@127.0.0.1",
        "http://localhost:9",
        "http://127.0.0.1:",
        "http://127.0.0.1:9x",
        "http://[::1",
        "http://::1",
        "http://127.0.0.1%2e",
        "http://0x7f.0.0.1",
    ] {
        assert!(fetch::check_api_base(bad).is_err(), "{bad}");
    }
    for good in ["http://127.0.0.1", "http://127.0.0.1:8080/v1", "http://127.1.2.3:9", "http://[::1]:9", "https://127.0.0.1:9"] {
        assert!(fetch::check_api_base(good).is_ok(), "{good}");
    }
    assert_eq!(fetch::base64_decode("iVBORw0KGgo=").unwrap(), b"\x89PNG\r\n\x1a\n");
}

#[test]
fn linear_gradients_preserve_affine_handles_in_non_square_nodes() {
    let axis = |paint: Value| {
        let node = json!({"id":"r","type":"RECTANGLE","size":{"x":200,"y":100},"fills":[paint]});
        let contents = map::shape_contents(&node).unwrap();
        let Item::Group { items, .. } = &contents[0] else { panic!("missing shape group") };
        items
            .iter()
            .find_map(|item| match item {
                Item::GradientFill { radial, start, end, .. } => Some((*radial, *start, *end)),
                _ => None,
            })
            .unwrap()
    };
    let mut paint = json!({"type":"GRADIENT_LINEAR","gradientHandlePositions":[{"x":0,"y":0},{"x":1,"y":1},{"x":0,"y":1}],
        "gradientStops":[{"position":0,"color":{"r":1,"g":0,"b":0}},{"position":1,"color":{"r":0,"g":0,"b":1}}]});
    assert_eq!(axis(paint.clone()), (false, [0.0, 0.0], [200.0, 0.0]));
    paint["gradientHandlePositions"] = json!([{"x":0.1,"y":0.2},{"x":0.9,"y":0.8},{"x":0.6,"y":1.2}]);
    assert_eq!(axis(paint.clone()), (false, [20.0, 20.0], [70.0, -30.0]));

    // An absent, invalid, coincident or collinear width handle keeps the two-point fallback.
    for third in [Value::Null, json!({"x":"bad","y":1}), json!({"x":0.1,"y":0.2}), json!({"x":0.5,"y":0.5}), json!({"x":1e308,"y":1e308})] {
        paint["gradientHandlePositions"] = json!([{"x":0.1,"y":0.2},{"x":0.9,"y":0.8}]);
        if !third.is_null() {
            paint["gradientHandlePositions"].as_array_mut().unwrap().push(third);
        }
        assert_eq!(axis(paint.clone()), (false, [20.0, 20.0], [180.0, 80.0]));
    }
    paint["type"] = json!("GRADIENT_RADIAL");
    paint["gradientHandlePositions"] = json!([{"x":0.1,"y":0.2},{"x":0.9,"y":0.8},{"x":0.6,"y":1.2}]);
    assert_eq!(axis(paint), (true, [20.0, 20.0], [180.0, 80.0]));
}

#[test]
fn card_frame_becomes_a_comp_with_native_layers() {
    let p = plan(&card(), Target::NewComp);
    assert_eq!(p.roots, [0]);
    let c = &p.comps[0];
    assert_eq!((c.name.as_str(), c.width, c.height), ("Card", 400, 520));
    let names: Vec<&str> = c.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Card Background", "Header", "Avatar", "Title", "Body", "Divider", "Button", "Button BG", "Label", "Badge", "Sparkle"]);

    // Background: the frame's rounded white rectangle, centred in the comp.
    let bg = layer(c, "Card Background");
    assert_eq!(bg.position, [200.0, 260.0]);
    assert_eq!(bg.anchor, [200.0, 260.0]);
    assert!(matches!(group_items(bg), [Item::Rect { size: [400.0, 520.0], roundness: 24.0, .. }, Item::Fill { .. }]));

    // Header: top corners only → an editable path; all three affine gradient handles count.
    let h = group_items(layer(c, "Header"));
    let [Item::Path(sp), Item::GradientFill { radial: false, start, end, gradient, .. }] = h else { panic!("{h:?}") };
    assert!(sp.closed && sp.vertices.len() >= 6);
    assert_eq!(*start, [0.0, 0.0]);
    assert!((end[0] - 48600.0 / 181.0).abs() < 1e-9);
    assert!((end[1] - 54000.0 / 181.0).abs() < 1e-9);
    assert_eq!(gradient.colors.len(), 2);

    // Image fill → rendered image at the node's box.
    let a = layer(c, "Avatar");
    assert!(matches!(a.kind, PKind::Raster(0)));
    assert_eq!(a.position, [72.0, 180.0]);
    assert_eq!(p.rasters[0].reason, "image fill");

    // Text: font, size, leading, tracking, box.
    let PKind::Text(t) = &layer(c, "Title").kind else { panic!() };
    assert_eq!(t.text, "Figma to EffectCraft");
    let st = t.style_at(0);
    assert_eq!((st.font.as_str(), st.style.as_str(), st.size), ("Inter", "Bold", 28.0));
    assert_eq!(st.leading, Some(34.0));
    assert!((st.tracking - (-0.5 / 28.0 * 1000.0)).abs() < 1e-9);
    assert_eq!(t.box_size.map(|b| b[0]), Some(352.0));

    // Style runs: "editable paths" bold and purple.
    let PKind::Text(b) = &layer(c, "Body").kind else { panic!() };
    let at = b.text.find("editable").unwrap();
    assert_eq!(b.style_at(at).style, "Bold");
    assert!((b.style_at(at).fill[0] - 0x6C as f32 / 255.0).abs() < 1e-3);
    assert_eq!(b.style_at(0).style, "Regular");

    // Line: an open path with a 2 px stroke.
    let d = group_items(layer(c, "Divider"));
    assert!(matches!(d, [Item::Path(sp), Item::Stroke(s)] if !sp.closed && s.width == 2.0));

    // Group → null; its children parented to it.
    let bi = c.layers.iter().position(|l| l.name == "Button").unwrap();
    assert!(matches!(c.layers[bi].kind, PKind::Null));
    assert_eq!(c.layers[bi].position, [200.0, 466.0]);
    assert_eq!(layer(c, "Button BG").parent, Some(bi));
    assert_eq!(layer(c, "Label").parent, Some(bi));
    // Inside stroke on a rounded rect: an inset rectangle in its own group.
    let bb = group_items(layer(c, "Button BG"));
    let [Item::Group { name: sn, items: si }, Item::Group { name: fname, .. }] = bb else { panic!("{bb:?}") };
    assert_eq!((sn.as_str(), fname.as_str()), ("Stroke", "Fill"));
    assert!(matches!(si.as_slice(), [Item::Rect { size: [350.0, 50.0], roundness: 25.0, .. }, Item::Stroke(s)] if (s.opacity - 40.0).abs() < 1e-9));
    let PKind::Text(lbl) = &layer(c, "Label").kind else { panic!() };
    assert_eq!(lbl.justify, Justify::Center);

    // Ellipse: radial gradient, dashed round-capped stroke.
    let e = group_items(layer(c, "Badge"));
    assert!(
        matches!(e, [Item::Ellipse { size: [48.0, 48.0], .. }, Item::Stroke(s), Item::GradientFill { radial: true, .. }] if s.dash == Some((4.0, 4.0)) && s.cap == 1)
    );

    // Vector: its fill geometry as a path, 90 % opaque fill.
    let v = group_items(layer(c, "Sparkle"));
    assert!(matches!(v, [Item::Path(sp), Item::Fill { opacity, .. }] if sp.vertices.len() == 8 && (opacity - 90.0).abs() < 1e-9));

    // The root frame's shadow can't be kept on a comp: said so.
    assert!(p.notes.iter().any(|n| n.contains("Card") && n.contains("effects")), "{:?}", p.notes);
}

#[test]
fn card_into_active_comp_is_a_precomp_with_shadow_and_rounded_clip() {
    let p = plan(&card(), Target::ActiveComp { width: 1920, height: 1080 });
    let top = &p.comps[p.into_active.unwrap()];
    assert_eq!(top.layers.len(), 1);
    let l = &top.layers[0];
    let PKind::Precomp(i) = l.kind else { panic!() };
    assert_eq!((p.comps[i].width, p.comps[i].height), (400, 520));
    assert_eq!(l.position, [960.0, 540.0]);
    assert_eq!(l.anchor, [200.0, 260.0]);
    let s = &l.shadows[0];
    assert!(!s.inner && (s.opacity - 18.0).abs() < 1e-9 && s.distance == 8.0 && s.size == 24.0);
    assert!((s.angle - 90.0).abs() < 1e-9, "light from above: {}", s.angle);
    assert_eq!(l.masks.len(), 1, "rounded corners clip");
}

#[test]
fn masks_groups_and_fallbacks() {
    let p = plan(&masks(), Target::NewComp);
    let c = &p.comps[p.roots[0]];
    let mi = c.layers.iter().position(|l| l.name == "Mask").unwrap();
    assert!(c.layers[mi].is_matte);
    assert_eq!(layer(c, "Stripes").matte, Some((mi, MatteKind::Alpha)));
    assert_eq!(layer(c, "Dot").matte, Some((mi, MatteKind::Alpha)));
    assert_eq!(layer(c, "Dot").blend, effectcraft_color::BlendMode::Multiply);
    // A half-transparent group: a precomp at 50 %.
    let f = layer(c, "Faded");
    assert!(matches!(f.kind, PKind::Precomp(_)) && f.opacity == 50.0);
    // A star without geometry: an image.
    assert!(matches!(layer(c, "Star").kind, PKind::Raster(_)));
    // Layer blur → Gaussian Blur; background blur noted.
    assert_eq!(layer(c, "Blurred").blur, Some(6.0));
    assert!(p.notes.iter().any(|n| n.contains("background blur")));
    // Comp spans the selection.
    assert_eq!((c.width, c.height), (440, 260));
}

#[test]
fn plugin_polystar_and_transforms() {
    // Plug-in style: absoluteTransform, rotation, pointCount without geometry.
    let n = json!({"nodes": [{"id": "9:1", "name": "Poly", "type": "REGULAR_POLYGON", "pointCount": 6, "size": {"x": 100, "y": 100},
        "absoluteTransform": [[0, -1, 50], [1, 0, 10]], "absoluteBoundingBox": {"x": -50, "y": 10, "width": 100, "height": 100},
        "fills": [{"type": "SOLID", "color": {"r": 1, "g": 0, "b": 0, "a": 1}}], "strokes": []}]});
    let p = plan(&n, Target::NewComp);
    let c = &p.comps[0];
    let l = &c.layers[0];
    assert!((l.rotation - 90.0).abs() < 1e-9, "{}", l.rotation);
    assert_eq!(l.position, [50.0, 50.0]);
    assert!(matches!(group_items(l), [Item::Star { star: false, points: 6.0, outer: 50.0, .. }, Item::Fill { .. }]));
}

fn session_with_comp() -> Session {
    let mut s = Session::new();
    s.execute("comp.new", json!({"name": "Main", "width": 1280, "height": 720})).unwrap();
    s
}

#[test]
fn import_payload_into_session_is_one_undo_step() {
    let mut s = Session::new();
    let undo_before = s.history.undo.len();
    let r = s.execute_checked("file.importFigma", json!({"payload": card(), "frameRate": 25, "duration": 5})).unwrap();
    assert_eq!(s.history.undo.len(), undo_before + 1, "one undo step");
    assert_eq!(s.history.undo.last().unwrap().0, "Import from Figma");
    let cid = effectcraft_project::ItemId(r["comp"].as_u64().unwrap());
    assert_eq!(s.active_comp_id(), Some(cid));
    let comp = s.project.comp(cid).unwrap();
    assert_eq!((comp.width, comp.height), (400, 520));
    // Top layer first in the Timeline; the image could not come in (no image in the payload).
    let names: Vec<&str> = comp.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Sparkle", "Badge", "Label", "Button BG", "Button", "Divider", "Body", "Title", "Header", "Card Background"]);
    assert!(r["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("Avatar")));
    assert_eq!(r["native"], 10);
    // Parenting: the label sits in the button null's space.
    let btn = comp.layers.iter().find(|l| l.name == "Button").unwrap();
    let lbl = comp.layers.iter().find(|l| l.name == "Label").unwrap();
    assert_eq!(lbl.parent, Some(btn.id));
    assert!(matches!(btn.source, LayerSource::Null));
    let pos = lbl.props.prop("transform/position").unwrap().value.clone();
    assert_eq!(pos, KV::Vec3([50.0, 50.0, 0.0]), "label centre = button centre");
    assert!(lbl.comment.contains("1:10"));
    // Edit like any shape: the header's gradient colours are a property.
    let hdr = comp.layers.iter().find(|l| l.name == "Header").unwrap();
    assert!(hdr.props.match_path_of(hdr.props.sub("contents").unwrap().groups().next().unwrap().uid).is_some());
    // Render: white card, purple button, gradient header.
    let img = s.render(cid, s.time(), Default::default());
    let px = |x: u32, y: u32| img.data[(y * img.width + x) as usize];
    let white = px(380, 420);
    assert!(white.iter().all(|c| *c > 0.97), "{white:?}");
    let button = px(60, 466);
    assert!((button[0] - 0x6C as f32 / 255.0).abs() < 0.06 && (button[2] - 0xE7 as f32 / 255.0).abs() < 0.06, "{button:?}");
    let left = px(10, 120);
    let right = px(390, 170);
    assert!(left[2] > left[1] && right[1] > right[2], "gradient runs purple → green: {left:?} {right:?}");
    // Rounded corners: outside the card's top-left radius is transparent.
    assert!(px(1, 1)[3] < 0.05);
    // Undo removes everything.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.comp(cid).is_none());
}

#[test]
fn import_into_active_comp_and_dry_run() {
    let mut s = session_with_comp();
    let cid = s.active_comp_id().unwrap();
    let dry = s.execute_checked("file.importFigma", json!({"payload": masks(), "dryRun": true})).unwrap();
    assert_eq!(dry["comps"][0]["name"], "Masked (Figma)");
    assert!(s.project.comp(cid).unwrap().layers.is_empty(), "dry run changes nothing");
    let r = s.execute_checked("file.importFigma", json!({"payload": masks()})).unwrap();
    assert_eq!(r["comp"], cid.0);
    let comp = s.project.comp(cid).unwrap();
    let mask = comp.layers.iter().find(|l| l.name == "Mask").unwrap();
    assert!(!mask.switches.video, "matte layers are hidden");
    let stripes = comp.layers.iter().find(|l| l.name == "Stripes").unwrap();
    assert_eq!(stripes.track_matte.map(|t| t.layer), Some(mask.id));
    let faded = comp.layers.iter().find(|l| l.name == "Faded").unwrap();
    assert!(matches!(faded.source, LayerSource::Comp { .. }));
    let blurred = comp.layers.iter().find(|l| l.name == "Blurred").unwrap();
    assert_eq!(blurred.effects().unwrap().groups().count(), 1);
    assert_eq!(s.state.selected_layers.len(), r["layers"].as_array().unwrap().len());
    // The matte cuts the red square to a circle.
    let img = s.render(cid, s.time(), Default::default());
    let px = |x: u32, y: u32| img.data[(y * img.width + x) as usize];
    // Selection 440×260 centred in 1280×720: origin (420, 230).
    assert!(px(420 + 100, 230 + 30)[0] > 0.9, "inside the circle");
    assert!(px(420 + 3, 230 + 3)[3] < 0.05, "outside the circle");
}

#[test]
fn token_is_never_journaled_and_status_reports_source() {
    let mut s = Session::new();
    let _ = s.execute_checked("file.importFigma", json!({"payload": masks(), "token": "figd_secret"}));
    assert!(!s.journal.iter().any(|(id, p)| id == "file.importFigma" || p.to_string().contains("figd_secret")));
    let st = s.execute_checked("figma.status", json!({})).unwrap();
    assert!(st["token"].is_string());
    // No settings folder: storing explains instead of failing silently.
    assert!(s.execute_checked("figma.setToken", json!({"token": "x"})).is_err());
}

#[test]
fn secrets_are_written_readable_only_by_the_user() {
    let dir = std::env::temp_dir().join(format!("ec-figma-private-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("figma-token");
    super::write_private(&path, b"figd_one").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "figd_one");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        // An existing file with wider permissions is narrowed.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    super::write_private(&path, b"two").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_payloads_are_errors_not_crashes() {
    let mut s = session_with_comp();
    for bad in [json!(null), json!("x"), json!({"nodes": "x"}), json!({"nodes": []}), json!({"nodes": {"1:2": null}}), json!({})] {
        assert!(s.execute_checked("file.importFigma", json!({"payload": bad})).is_err(), "{bad}");
    }
    // Garbage values, NaN-ish numbers, absurd sizes, bad paths, deep nesting.
    let mut deep = json!({"id": "0:0", "type": "RECTANGLE", "name": "leaf", "size": {"x": 10, "y": 10}});
    for i in 0..300 {
        deep = json!({"id": format!("d:{i}"), "type": if i % 2 == 0 { "GROUP" } else { "FRAME" }, "name": "g", "children": [deep]});
    }
    let weird = json!({"nodes": [
        deep,
        {"type": "VECTOR", "fillGeometry": [{"path": "M 1e308 1e308 L nan Z q"}], "size": {"x": -5, "y": 1e300}},
        {"type": "TEXT", "characters": "😀ab", "characterStyleOverrides": [1, 1, 1, 2, 9, 9, 9], "styleOverrideTable": {"1": {"fontSize": -3}, "2": "x"}, "style": {"fontSize": 1e9, "lineHeightPx": "a"}},
        {"type": "ELLIPSE", "strokes": [{"type": "GRADIENT_LINEAR", "gradientStops": []}], "strokeWeight": 1e12, "strokeDashes": [-1, "x"], "absoluteTransform": [[1], []]},
        {"type": "WIDGET", "name": "w"},
        {"type": "RECTANGLE", "isMask": true},
        {"type": "FRAME", "size": {"x": 1e9, "y": 1e9}, "children": [{"type": "STAR", "pointCount": 1e9}]}
    ]});
    let r = s.execute_checked("file.importFigma", json!({"payload": weird}));
    assert!(r.is_ok(), "{r:?}");
    assert!(r.unwrap()["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("levels deep")));
}

/// A minimal HTTP/1.1 server for the REST path: answers from `routes` (path prefix → (content
/// type, body)) and records each request head.
fn fake_api(routes: Vec<(String, &'static str, Vec<u8>)>) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{BufRead, BufReader, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let log = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let log2 = log.clone();
    std::thread::spawn(move || {
        for stream in l.incoming().flatten() {
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                head.push_str(&line);
            }
            let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
            log2.lock().unwrap().push(head.clone());
            let (status, ct, body) = match routes.iter().find(|(p, _, _)| path.starts_with(p.as_str())) {
                Some((_, ct, b)) => ("200 OK", *ct, b.clone()),
                None => ("404 Not Found", "application/json", br#"{"status":404,"err":"Not found"}"#.to_vec()),
            };
            let mut w = stream;
            let _ = write!(w, "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = w.write_all(&body);
        }
    });
    (base, log)
}

#[test]
fn rest_path_against_a_fake_api() {
    if std::process::Command::new("curl").arg("--version").output().is_err() {
        eprintln!("curl not available: skipped");
        return;
    }
    let png = crate::commands::comp_more::encode_png(&[255u8, 0, 0, 255].repeat(192 * 192), 192, 192).unwrap();
    let key = "AbCdEfGhIjKlMnOpQrStUv";
    // Image links point at a second server (as Figma's point at its CDN).
    let (img_base, img_log) = fake_api(vec![("/img/avatar.png".into(), "image/png", png)]);
    let images = json!({"err": null, "images": {"1:4": format!("{img_base}/img/avatar.png")}}).to_string().into_bytes();
    let (base, log2) = fake_api(vec![
        (format!("/v1/files/{key}/nodes"), "application/json", CARD.as_bytes().to_vec()),
        (format!("/v1/images/{key}"), "application/json", images),
    ]);
    let mut s = Session::new();
    let r = s
        .execute_checked(
            "file.importFigma",
            json!({"url": format!("https://www.figma.com/design/{key}/Fixtures?node-id=1-2"), "token": "figd_test-token", "apiBase": base}),
        )
        .unwrap();
    let cid = effectcraft_project::ItemId(r["comp"].as_u64().unwrap());
    assert_eq!(s.project.comp(cid).unwrap().width, 400);
    let heads = log2.lock().unwrap().clone();
    assert!(heads[0].starts_with(&format!("GET /v1/files/{key}/nodes?ids=1%3A2&geometry=paths")), "{heads:?}");
    assert!(heads.iter().all(|h| h.contains("X-Figma-Token: figd_test-token")), "token sent to the API");
    assert!(heads.iter().any(|h| h.contains("/v1/images/") && h.contains("scale=2") && h.contains("use_absolute_bounds=true")));
    let img_heads = img_log.lock().unwrap().clone();
    assert_eq!(img_heads.len(), 1);
    assert!(!img_heads[0].contains("X-Figma-Token"), "image links get no token");
    // The image was downloaded (this bare session has no media importer, so it stops there).
    let notes: Vec<&str> = r["notes"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(notes.iter().any(|n| n.contains("Avatar") && n.contains("not available")), "{notes:?}");
    // Bad token → Figma's message.
    let (bad_base, _) = fake_api(vec![]);
    let e = s.execute_checked("file.importFigma", json!({"fileKey": key, "nodeIds": ["1:2"], "token": "t", "apiBase": bad_base})).unwrap_err().to_string();
    assert!(e.contains("404"), "{e}");
    // Never to another host.
    let e = s
        .execute_checked("file.importFigma", json!({"fileKey": key, "nodeIds": ["1:2"], "token": "t", "apiBase": "https://example.com"}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("apiBase"), "{e}");
}

#[test]
fn figcraft_frame_preserves_overflow_and_root_appearance_without_changing_figma_frames() {
    let node = json!({"figcraft":true,"id":"frame","type":"FRAME","name":"Overflow","size":{"x":100,"y":80},
        "absoluteTransform":[[1,0,200],[0,1,300]],"absoluteRenderBounds":{"x":180,"y":290,"width":160,"height":110},
        "opacity":0.5,"clipsContent":false,
        "effects":[{"type":"DROP_SHADOW","radius":8,"offset":{"x":2,"y":4},"color":{"r":0,"g":0,"b":0,"a":1}}],
        "children":[{"id":"child","type":"RECTANGLE","name":"Child","size":{"x":120,"y":80},
            "absoluteTransform":[[1,0,190],[0,1,300]],"absoluteRenderBounds":{"x":180,"y":290,"width":160,"height":110},
            "fills":[{"type":"SOLID","color":{"r":1,"g":0,"b":0}}]}]});
    let p = plan(&node, Target::NewComp);
    let root = &p.comps[p.roots[0]];
    assert_eq!((root.width, root.height), (160, 110));
    let wrapper = &root.layers[0];
    assert_eq!(wrapper.opacity, 50.0);
    assert_eq!(wrapper.shadows.len(), 1);
    let PKind::Precomp(inner) = wrapper.kind else { panic!("missing editable frame") };
    assert_eq!((p.comps[inner].width, p.comps[inner].height), (160, 110));
    assert_eq!(wrapper.position, [70.0, 50.0]);
    assert_eq!(wrapper.anchor, [70.0, 50.0]);

    let mut clipped = node.clone();
    clipped["clipsContent"] = json!(true);
    clipped["cornerRadius"] = json!(12);
    let p = plan(&clipped, Target::NewComp);
    let wrapper = &p.comps[p.roots[0]].layers[0];
    let PKind::Precomp(inner) = wrapper.kind else { panic!("missing clipping frame") };
    assert_eq!((p.comps[inner].width, p.comps[inner].height), (100, 80));
    assert!(!wrapper.masks.is_empty());

    let mut figma = node;
    figma.as_object_mut().unwrap().remove("figcraft");
    let p = plan(&figma, Target::NewComp);
    assert_eq!((p.comps[p.roots[0]].width, p.comps[p.roots[0]].height), (100, 80));
}

#[test]
fn figcraft_text_uses_exported_baseline_and_line_advance() {
    let node = json!({"id":"text","type":"TEXT","characters":"First\nSecond","size":{"x":250,"y":100},
        "figcraftTextBaseline":27.25,"style":{"fontFamily":"Inter","fontSize":28,"lineHeightPx":34}});
    let (doc, _) = map::text_doc(&node);
    let layout = effectcraft_text::layout_doc(&doc);
    assert_eq!(layout.line_boxes.len(), 2);
    assert!((layout.line_boxes[0][1] - 27.25).abs() < 1e-5);
    assert!((layout.line_boxes[1][1] - 61.25).abs() < 1e-5);
    assert_eq!(doc.text, "First\nSecond");
}

#[test]
fn imported_gradients_keep_nonlinear_kind_and_third_handle() {
    for (ty, kind) in [("GRADIENT_RADIAL", 1), ("GRADIENT_ANGULAR", 2), ("GRADIENT_DIAMOND", 3)] {
        let node = json!({"id":"paint","type":"RECTANGLE","size":{"x":200,"y":100},
            "fills":[{"type":ty,"gradientHandlePositions":[{"x":0.5,"y":0.5},{"x":1,"y":0.5},{"x":0.75,"y":1}],
                "gradientStops":[{"position":0,"color":{"r":1,"g":0,"b":0}},{"position":1,"color":{"r":0,"g":0,"b":1}}]}]});
        let p = plan(&node, Target::NewComp);
        let PKind::Shape(items) = &p.comps[0].layers[0].kind else { panic!("not editable") };
        let Some(Item::GradientFill { kind: k, width, .. }) = items.iter().find(|i| matches!(i, Item::GradientFill { .. })) else { panic!("missing paint") };
        assert_eq!(*k, kind);
        assert_eq!(*width, Some([150.0, 100.0]));
    }
}

#[test]
fn multiple_shadow_styles_keep_independent_parameters_and_native_properties() {
    let node = json!({"id":"glow","type":"RECTANGLE","name":"Glow","size":{"x":20,"y":20},
        "fills":[{"type":"SOLID","color":{"r":1,"g":1,"b":1}}],
        "effects":[
            {"type":"DROP_SHADOW","radius":10,"spread":5,"color":{"r":1,"g":0,"b":0}},
            {"type":"DROP_SHADOW","radius":40,"blendMode":"SCREEN","showBehind":true,"color":{"r":0,"g":1,"b":0}},
            {"type":"DROP_SHADOW","visible":false,"radius":200},
            {"type":"INNER_SHADOW","radius":4,"color":{"r":0,"g":0,"b":1}}]});
    let p = plan(&node, Target::NewComp);
    let shadows = &p.comps[0].layers[0].shadows;
    assert_eq!(shadows.len(), 3);
    assert_eq!(shadows[0].size, 15.0);
    assert!((shadows[0].spread - 100.0 / 3.0).abs() < 1e-6);
    assert_eq!(shadows[1].size, 40.0);
    assert_eq!(shadows[1].blend, effectcraft_color::BlendMode::Screen);
    assert!(shadows[1].show_behind);
    let mut s = Session::new();
    s.execute_checked("file.importFigma", json!({"payload":node})).unwrap();
    let layer = &s.active_comp().unwrap().layers[0];
    let shadows: Vec<_> = layer.props.sub(effectcraft_project::styles::GROUP).unwrap().groups().filter(|g| g.match_id == "dropShadow").collect();
    assert_eq!(shadows.len(), 2);
    assert_ne!(shadows[0].uid, shadows[1].uid);
    assert_eq!(shadows[0].get("size").unwrap().value, KV::Scalar(15.0));
    assert_eq!(shadows[1].get("size").unwrap().value, KV::Scalar(40.0));
    assert_eq!(shadows[1].get("knocksOut").unwrap().value, KV::Bool(false));
}

#[test]
fn figcraft_only_collapses_unclipped_opaque_effect_free_pass_through_frames() {
    let node = json!({"figcraft":true,"id":"frame","type":"FRAME","name":"Frame","size":{"x":100,"y":80},
        "blendMode":"PASS_THROUGH","opacity":1,"clipsContent":false,"children":[]});
    let p = plan(&node, Target::NewComp);
    assert!(p.comps[p.roots[0]].layers[0].collapse);
    for (key, value) in
        [("blendMode", json!("NORMAL")), ("opacity", json!(0.5)), ("clipsContent", json!(true)), ("effects", json!([{"type":"DROP_SHADOW","radius":5}]))]
    {
        let mut changed = node.clone();
        changed[key] = value;
        let p = plan(&changed, Target::NewComp);
        assert!(!p.comps[p.roots[0]].layers[0].collapse, "{key}");
    }
}
