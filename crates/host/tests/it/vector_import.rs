//! Photoshop and SVG import through the fully wired session (media decoding included): pixels
//! of imported compositions, SVG footage at any scale, and Create Shapes from Vector Layer.

use effectcraft_engine::render::RenderOpts;
use effectcraft_project::ItemId;
use effectcraft_psd::Rect;
use effectcraft_psd::write::*;
use effectcraft_raster::Image;
use serde_json::json;

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("effectcraft-host-vector-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

fn mean_diff(a: &Image, b: &Image) -> f32 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let mut s = 0.0;
    for (p, q) in a.data.iter().zip(&b.data) {
        for c in 0..4 {
            s += (p[c] - q[c]).abs();
        }
    }
    s / (a.data.len() * 4) as f32
}

fn psd_doc() -> WDoc {
    let mut d = WDoc::new(64, 48);
    let mut masked = WLayer::solid("Masked", Rect::new(30, 8, 24, 24), [0.1, 0.9, 0.3, 1.0]);
    masked.mask = Some(WMask {
        rect: Rect::new(30, 8, 24, 24),
        data: (0..576).map(|i| if (i / 24) < 12 { 1.0 } else { 0.25 }).collect(),
        default_color: 0,
        disabled: false,
    });
    d.layers = vec![
        WLayer::solid("Background", Rect::new(0, 0, 64, 48), [0.2, 0.25, 0.3, 1.0]),
        WLayer::solid("Half Red", Rect::new(4, 4, 30, 20), [1.0, 0.0, 0.0, 1.0]).with_opacity(128),
        masked,
        WLayer::group_end(),
        WLayer::solid("In Group", Rect::new(10, 30, 20, 12), [0.9, 0.9, 0.1, 1.0]),
        WLayer::group("Group", true, *b"norm"),
    ];
    d
}

/// Straight-alpha over of the layers as Photoshop composites them (normal blending), as the
/// merged image to compare the imported composition with.
fn reference(bytes: &[u8]) -> Vec<[f32; 4]> {
    let p = effectcraft_psd::Psd::parse(bytes.to_vec()).unwrap();
    let mut acc = vec![[0.0f32; 4]; (p.width * p.height) as usize];
    for l in p.layers.iter().filter(|l| l.has_pixels() && !l.hidden) {
        let px = p.layer_pixels(l.index, true).unwrap();
        let o = l.opacity as f32 / 255.0;
        for (d, s) in acc.iter_mut().zip(&px.data) {
            let a = s[3] * o;
            let oa = a + d[3] * (1.0 - a);
            for c in 0..3 {
                d[c] = if oa > 0.0 { (s[c] * a + d[c] * d[3] * (1.0 - a)) / oa } else { 0.0 };
            }
            d[3] = oa;
        }
    }
    acc
}

#[test]
fn psd_composition_renders_like_the_document() {
    let mut doc = psd_doc();
    let bytes = write(&doc);
    let merged = reference(&bytes);
    doc.composite = Some(merged.clone());
    let bytes = write(&doc);
    let path = tmp("layers.psd");
    std::fs::write(&path, &bytes).unwrap();
    let mut s = effectcraft_host::session();
    // Footage (merged image).
    let r = s.execute_checked("file.import", json!({"paths": [path]})).unwrap();
    let merged_item = r["items"][0].as_u64().unwrap();
    // Composition and Composition – Retain Layer Sizes.
    let mut renders = vec![];
    for kind in ["composition", "compositionLayerSizes"] {
        let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": kind})).unwrap();
        let cid = ItemId(r["comps"][0].as_u64().unwrap());
        renders.push(s.render(cid, effectcraft_time::Tick::ZERO, RenderOpts::default()));
    }
    // The merged footage in a comp of the same size.
    s.execute("comp.new", json!({"name": "Merged", "width": 64, "height": 48, "frameRate": 30, "duration": 1})).unwrap();
    s.execute("layer.addItem", json!({"item": merged_item})).unwrap();
    let mcid = s.active_comp_id().unwrap();
    let merged_img = s.render(mcid, effectcraft_time::Tick::ZERO, RenderOpts::default());
    let expected = Image { width: 64, height: 48, data: merged.iter().map(|p| [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]).collect() };
    assert!(mean_diff(&merged_img, &expected) < 2.0 / 255.0, "merged footage");
    for (i, img) in renders.iter().enumerate() {
        let d = mean_diff(img, &expected);
        assert!(d < 2.0 / 255.0, "import kind {i}: mean diff {d}");
    }
    // A single layer as footage (Choose Layer).
    let r = s.execute_checked("file.import", json!({"paths": [path], "layer": "Masked"})).unwrap();
    let it = s.project.item(ItemId(r["items"][0].as_u64().unwrap())).unwrap();
    assert_eq!(it.name, "Masked/layers.psd");
}

#[test]
fn psd_16bit_cmyk_composition_imports() {
    let mut doc = psd_doc();
    doc.depth = 16;
    doc.mode = effectcraft_psd::ColorMode::Cmyk;
    doc.rle = false;
    let bytes = write(&doc);
    let merged = reference(&bytes);
    let path = tmp("layers16.psd");
    std::fs::write(&path, &bytes).unwrap();
    let mut s = effectcraft_host::session();
    let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": "composition"})).unwrap();
    let cid = ItemId(r["comps"][0].as_u64().unwrap());
    let img = s.render(cid, effectcraft_time::Tick::ZERO, RenderOpts::default());
    let expected = Image { width: 64, height: 48, data: merged.iter().map(|p| [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]).collect() };
    let d = mean_diff(&img, &expected);
    assert!(d < 2.0 / 255.0, "mean diff {d}");
}

const SVG: &[u8] = include_bytes!("../../../svg/tests/fixtures/basic-shapes.svg");
const SVG2: &[u8] = include_bytes!("../../../svg/tests/fixtures/paths-gradients.svg");

#[test]
fn svg_footage_and_shapes_from_vector_layer() {
    for (name, bytes) in [("basic-shapes.svg", SVG), ("paths-gradients.svg", SVG2)] {
        let path = tmp(name);
        std::fs::write(&path, bytes).unwrap();
        let mut s = effectcraft_host::session();
        let r = s.execute_checked("file.import", json!({"paths": [path]})).unwrap();
        assert_eq!(r["errors"], json!([]), "{r}");
        let item = r["items"][0].as_u64().unwrap();
        let doc = effectcraft_svg::parse(bytes).unwrap();
        let (w, h) = doc.pixel_size();
        s.execute("comp.new", json!({"name": "V", "width": w, "height": h, "frameRate": 30, "duration": 1})).unwrap();
        let lid = s.execute_checked("layer.addItem", json!({"item": item})).unwrap()["layer"].as_u64().unwrap();
        let cid = s.active_comp_id().unwrap();
        let t = effectcraft_time::Tick::ZERO;
        let foot = s.render(cid, t, RenderOpts::default());
        let direct = effectcraft_svg::rasterize(&doc, w, h, 1.0);
        assert!(mean_diff(&foot, &direct) < 1.0 / 255.0, "{name}: footage = rasterised SVG");
        s.execute("layer.select", json!({"layers": [lid]})).unwrap();
        let r = s.execute_checked("layer.create", json!({"op": "shapesFromVector"})).unwrap();
        let sid = r["layers"][0].as_u64().unwrap();
        let c = s.active_comp().unwrap();
        assert_eq!(c.layers[0].id.0, sid);
        assert!(!c.layers[1].switches.video);
        let shapes = s.render(cid, t, RenderOpts::default());
        let d = mean_diff(&shapes, &direct);
        assert!(d < 0.02, "{name}: shapes vs SVG mean diff {d}");
        // Scaled up 3×, the shape layer stays sharp and matches the SVG rasterised at 3×.
        s.execute("prop.set", json!({"layer": sid, "path": "transform/scale", "value": [300, 300]})).unwrap();
        s.execute("prop.set", json!({"layer": sid, "path": "transform/position", "value": [0, 0]})).unwrap();
        s.execute("prop.set", json!({"layer": sid, "path": "transform/anchor", "value": [0, 0]})).unwrap();
        let big = s.render(cid, t, RenderOpts::default());
        let direct3 = effectcraft_svg::rasterize(&doc, w, h, 3.0);
        let d3 = mean_diff(&big, &direct3);
        assert!(d3 < 0.03, "{name}: ×3 mean diff {d3}");
    }
}

#[test]
fn svg_footage_continuously_rasterizes() {
    let path = tmp("cr.svg");
    std::fs::write(&path, SVG).unwrap();
    let doc = effectcraft_svg::parse(SVG).unwrap();
    let (w, h) = doc.pixel_size();
    let mut s = effectcraft_host::session();
    let item = s.execute_checked("file.import", json!({"paths": [path]})).unwrap()["items"][0].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "C", "width": w, "height": h, "frameRate": 30, "duration": 1})).unwrap();
    let lid = s.execute_checked("layer.addItem", json!({"item": item})).unwrap()["layer"].as_u64().unwrap();
    for (k, v) in [("transform/anchor", json!([0, 0])), ("transform/position", json!([0, 0])), ("transform/scale", json!([300, 300]))] {
        s.execute("prop.set", json!({"layer": lid, "path": k, "value": v})).unwrap();
    }
    let cid = s.active_comp_id().unwrap();
    let t = effectcraft_time::Tick::ZERO;
    let want = effectcraft_svg::rasterize(&doc, w, h, 3.0);
    let soft = mean_diff(&s.render(cid, t, RenderOpts::default()), &want);
    s.execute("layer.setSwitch", json!({"layers": [lid], "switch": "collapse", "value": true})).unwrap();
    let sharp = mean_diff(&s.render(cid, t, RenderOpts::default()), &want);
    assert!(sharp < 0.004, "continuously rasterised diff {sharp}");
    assert!(sharp < soft, "sharper than the upscaled pixels ({sharp} vs {soft})");
}

/// A two-layer Illustrator-style PDF (optional-content layers "Sky" and "Art"): a gradient sky,
/// a red curved shape with a dark outline, and a clipped green bar.
fn layered_pdf() -> Vec<u8> {
    let content = "/OC /L0 BDC\n/Pattern cs /P0 scn 0 0 120 80 re f\nEMC\n\
/OC /L1 BDC\n1 0 0 rg 0.1 0.1 0.1 RG 3 w 20 20 m 20 60 l 60 70 100 60 v 100 20 l h B\n\
q 30 0 40 80 re W n 0 0.8 0 rg 0 35 120 10 re f Q\nEMC\n";
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 80] /Resources << /Properties << /L0 5 0 R /L1 6 0 R >> /Pattern << /P0 7 0 R >> >> /Contents 4 0 R >>";
    let pattern = "<< /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 0 80] /Function << /FunctionType 2 /Domain [0 1] /C0 [0.9 0.9 1] /C1 [0.2 0.4 0.9] /N 1 >> /Extend [true true] >> >>";
    effectcraft_pdf::write::pdf(
        &[
            (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), None),
            (3, page.into(), None),
            (4, "<< >>".into(), Some(content.as_bytes().to_vec())),
            (5, "<< /Type /OCG /Name (Sky) >>".into(), None),
            (6, "<< /Type /OCG /Name (Art) >>".into(), None),
            (7, pattern.into(), None),
        ],
        1,
    )
}

#[test]
fn pdf_and_ai_footage_layers_and_shapes_from_vector_layer() {
    let bytes = layered_pdf();
    let doc = effectcraft_pdf::parse(&bytes).unwrap();
    let (w, h) = doc.pixel_size();
    assert_eq!((w, h), (120, 80));
    let direct = effectcraft_svg::rasterize(&doc, w, h, 1.0);
    for name in ["art.pdf", "art.ai"] {
        let path = tmp(name);
        std::fs::write(&path, &bytes).unwrap();
        let mut s = effectcraft_host::session();
        let r = s.execute_checked("file.import", json!({"paths": [path]})).unwrap();
        assert_eq!(r["errors"], json!([]), "{r}");
        let item = r["items"][0].as_u64().unwrap();
        let effectcraft_project::ItemKind::Footage(f) = &s.project.item(ItemId(item)).unwrap().kind else { panic!() };
        assert_eq!(f.codec, if name.ends_with(".ai") { "AI" } else { "PDF" });
        assert_eq!((f.width, f.height), (120, 80));
        s.execute("comp.new", json!({"name": "V", "width": w, "height": h, "frameRate": 30, "duration": 1})).unwrap();
        let lid = s.execute_checked("layer.addItem", json!({"item": item})).unwrap()["layer"].as_u64().unwrap();
        let cid = s.active_comp_id().unwrap();
        let t = effectcraft_time::Tick::ZERO;
        let foot = s.render(cid, t, RenderOpts::default());
        assert!(mean_diff(&foot, &direct) < 1.0 / 255.0, "{name}: footage = rasterised page");
        // Continuously Rasterize at 300 %.
        for (k, v) in [("transform/anchor", json!([0, 0])), ("transform/position", json!([0, 0])), ("transform/scale", json!([300, 300]))] {
            s.execute("prop.set", json!({"layer": lid, "path": k, "value": v})).unwrap();
        }
        let want = effectcraft_svg::rasterize(&doc, w, h, 3.0);
        let soft = mean_diff(&s.render(cid, t, RenderOpts::default()), &want);
        s.execute("layer.setSwitch", json!({"layers": [lid], "switch": "collapse", "value": true})).unwrap();
        let sharp = mean_diff(&s.render(cid, t, RenderOpts::default()), &want);
        assert!(sharp < 0.004 && sharp < soft, "{name}: continuously rasterised {sharp} vs {soft}");
        s.execute("prop.set", json!({"layer": lid, "path": "transform/scale", "value": [100, 100]})).unwrap();
        // Create Shapes from Vector Layer.
        s.execute("layer.select", json!({"layers": [lid]})).unwrap();
        let r = s.execute_checked("layer.create", json!({"op": "shapesFromVector"})).unwrap();
        let sid = r["layers"][0].as_u64().unwrap();
        let c = s.active_comp().unwrap();
        assert_eq!(c.layers[0].id.0, sid);
        let shapes = s.render(cid, t, RenderOpts::default());
        // The clipped bar (rows 33–47) keeps its clip through Merge Paths ▸ Intersect.
        let d = mean_diff(&shapes, &direct);
        assert!(d < 0.01, "{name}: shapes vs page mean diff {d}");
        for x in [10, 50, 100] {
            let (a, b) = (shapes.get(x, 40), direct.get(x, 40));
            assert!((0..4).all(|c| (a[c] - b[c]).abs() < 0.02), "{name}: bar clipped at x = {x}: {a:?} vs {b:?}");
        }
        let fill = shapes.get(60, 30);
        assert!(fill[0] > 0.9 && fill[1] < 0.1, "red shape {fill:?}");
    }
    // Import As: Composition — one layer per file layer, top first, together like the page.
    let path = tmp("layers.ai");
    std::fs::write(&path, &bytes).unwrap();
    for import_as in ["composition", "compositionLayerSizes"] {
        let mut s = effectcraft_host::session();
        let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": import_as})).unwrap();
        let cid = ItemId(r["comps"][0].as_u64().unwrap());
        let comp = s.project.comp(cid).unwrap();
        let names: Vec<&str> = comp.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["Art", "Sky"]);
        assert_eq!((comp.width, comp.height), (120, 80));
        let art = comp.layers[0].id.0;
        let img = s.render(cid, effectcraft_time::Tick::ZERO, RenderOpts::default());
        let d = mean_diff(&img, &direct);
        assert!(d < 1.0 / 255.0, "layered comp vs page {d}");
        // Each layer's footage shows only its layer.
        s.execute("layer.setSwitch", json!({"layers": [art], "switch": "video", "value": false})).unwrap();
        let sky = s.render(cid, effectcraft_time::Tick::ZERO, RenderOpts::default());
        assert_eq!(sky.get(5, 5), img.get(5, 5));
        let p = sky.get(60, 30);
        assert!(p[2] > p[0], "only sky under the art {p:?}");
    }
}

#[test]
fn psd_smart_object_imports_its_embedded_document() {
    // The embedded document: red left half, green right half (20×10).
    let mut inner = WDoc::new(20, 10);
    inner.layers =
        vec![WLayer::solid("Red", Rect::new(0, 0, 10, 10), [1.0, 0.0, 0.0, 1.0]), WLayer::solid("Green", Rect::new(10, 0, 10, 10), [0.0, 1.0, 0.0, 1.0])];
    let inner_bytes = write(&inner);
    let mut d = WDoc::new(64, 48);
    // Placed at 2× and turned a quarter clockwise: the content's top-left corner at (40, 4).
    let quad = [[40.0, 4.0], [40.0, 44.0], [20.0, 44.0], [20.0, 4.0]];
    d.layers = vec![
        WLayer::solid("Back", Rect::new(0, 0, 64, 48), [0.0, 0.0, 0.0, 1.0]),
        // Placeholder pixels (grey) that the embedded file replaces.
        WLayer::solid("Placed", Rect::new(20, 4, 20, 40), [0.5, 0.5, 0.5, 1.0]).with_block(smart_object_block("so-1", quad, [20.0, 10.0])),
    ];
    d.linked = vec![("so-1".into(), "inner.psd".into(), *b"8BPS", inner_bytes)];
    let path = tmp("smart.psd");
    std::fs::write(&path, write(&d)).unwrap();
    let mut s = effectcraft_host::session();
    let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": "composition"})).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    let cid = ItemId(r["comps"][0].as_u64().unwrap());
    let comp = s.project.comp(cid).unwrap();
    let placed = comp.layers.iter().find(|l| l.name == "Placed").unwrap();
    let effectcraft_project::LayerSource::Footage { item } = placed.source else { panic!("footage layer") };
    let effectcraft_project::ItemKind::Footage(f) = &s.project.item(item).unwrap().kind else { panic!() };
    assert_eq!((f.width, f.height), (20, 10));
    assert_eq!(f.layer.as_ref().unwrap().embedded.as_deref(), Some("so-1"));
    assert!(s.project.item(item).unwrap().name.contains("inner.psd"));
    let img = s.render(cid, effectcraft_time::Tick::ZERO, RenderOpts::default());
    // Rotated a quarter clockwise: the red half is on top (rows 4–24), green below.
    let (red, green) = (img.get(30, 14), img.get(30, 34));
    assert!(red[0] > 0.95 && red[1] < 0.05, "{red:?}");
    assert!(green[1] > 0.95 && green[0] < 0.05, "{green:?}");
    assert!(img.get(10, 24)[0] < 0.01 && img.get(10, 24)[3] > 0.99, "background outside");
}

#[test]
fn eps_footage_imports() {
    let eps =
        b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 40 20\n1 0 0 setrgbcolor 0 0 20 20 rectfill 0 0 1 setrgbcolor newpath 30 10 8 0 360 arc fill\n%%EOF\n";
    let path = tmp("shape.eps");
    std::fs::write(&path, eps).unwrap();
    let mut s = effectcraft_host::session();
    let r = s.execute_checked("file.import", json!({"paths": [path]})).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    let item = r["items"][0].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "E", "width": 40, "height": 20, "frameRate": 30, "duration": 1})).unwrap();
    s.execute_checked("layer.addItem", json!({"item": item})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let img = s.render(cid, effectcraft_time::Tick::ZERO, RenderOpts::default());
    assert!(img.get(10, 10)[0] > 0.99 && img.get(30, 10)[2] > 0.99, "{:?} {:?}", img.get(10, 10), img.get(30, 10));
}

/// A two-page PDF: page 2 has text in a non-embedded standard font inside a page-wide clip.
fn two_page_pdf() -> Vec<u8> {
    effectcraft_pdf::write::pdf(
        &[
            (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
            (2, "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into(), None),
            (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 80] /Contents 4 0 R >>".into(), None),
            (4, "<< >>".into(), Some(b"1 0 0 rg 0 0 120 80 re f".to_vec())),
            (
                5,
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 60] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >> >> >> /Contents 6 0 R >>".into(),
                None,
            ),
            (6, "<< >>".into(), Some(b"10 5 80 50 re W n 0 0 1 rg 0 0 100 60 re f 1 1 0 rg BT /F1 40 Tf 15 15 Td (Hi) Tj ET".to_vec())),
        ],
        1,
    )
}

#[test]
fn pdf_pages_text_and_clips_to_masks() {
    let bytes = two_page_pdf();
    let path = tmp("pages.pdf");
    std::fs::write(&path, &bytes).unwrap();
    let mut s = effectcraft_host::session();
    // Page 2 as footage (File ▸ Import ▸ Page), named after its page.
    let r = s.execute_checked("file.import", json!({"paths": [path], "page": 2})).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    let item = ItemId(r["items"][0].as_u64().unwrap());
    let it = s.project.item(item).unwrap();
    assert_eq!(it.name, "pages.pdf (Page 2)");
    let effectcraft_project::ItemKind::Footage(f) = &it.kind else { panic!() };
    assert_eq!((f.width, f.height, f.page), (100, 60, 1));
    assert!(s.execute_checked("file.import", json!({"paths": [path], "page": 3})).unwrap()["errors"][0].as_str().unwrap().contains("no page 3"));
    assert!(s.execute("file.import", json!({"paths": [path], "page": 0})).is_err());
    let doc = effectcraft_pdf::parse_page(&bytes, 1).unwrap();
    let direct = effectcraft_svg::rasterize(&doc, 100, 60, 1.0);
    s.execute("comp.new", json!({"name": "P", "width": 100, "height": 60, "frameRate": 30, "duration": 1})).unwrap();
    let lid = s.execute_checked("layer.addItem", json!({"item": item.0})).unwrap()["layer"].as_u64().unwrap();
    let cid = s.active_comp_id().unwrap();
    let t = effectcraft_time::Tick::ZERO;
    let foot = s.render(cid, t, RenderOpts::default());
    assert!(mean_diff(&foot, &direct) < 1.0 / 255.0, "page 2 footage");
    // Text drew (yellow glyphs over the blue page).
    let ink = (0..60).flat_map(|y| (0..100).map(move |x| (x, y))).filter(|&(x, y)| foot.get(x, y)[0] > 0.9).count();
    assert!(ink > 100, "text pixels {ink}");
    // Create Shapes from Vector Layer: the page-wide clip becomes the layer's mask, the text a
    // shape group named after it.
    s.execute("layer.select", json!({"layers": [lid]})).unwrap();
    let sid = s.execute_checked("layer.create", json!({"op": "shapesFromVector"})).unwrap()["layers"][0].as_u64().unwrap();
    let c = s.active_comp().unwrap();
    let l = c.layer(effectcraft_project::LayerId(sid)).unwrap();
    assert_eq!(l.masks().map(|m| m.groups().count()), Some(1));
    fn names(g: &effectcraft_project::PropGroup, out: &mut Vec<String>) {
        for c in g.groups() {
            out.push(c.name.clone());
            names(c, out);
        }
    }
    let mut names_found = vec![];
    names(l.props.sub("contents").unwrap(), &mut names_found);
    let names = names_found;
    assert!(names.iter().any(|n| n == "Text: Hi"), "{names:?}");
    let shapes = s.render(cid, t, RenderOpts::default());
    let d = mean_diff(&shapes, &direct);
    assert!(d < 0.01, "shapes vs page {d}");
    assert_eq!(shapes.get(5, 30)[3], 0.0, "masked outside the clip");
    // Page 1 as a composition.
    let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": "composition", "page": 1})).unwrap();
    let comp = s.project.comp(ItemId(r["comps"][0].as_u64().unwrap())).unwrap();
    assert_eq!((comp.width, comp.height), (120, 80));
}

#[test]
fn psd_smart_objects_with_perspective_and_warp_bake_as_placed() {
    let mut inner = WDoc::new(20, 10);
    inner.layers =
        vec![WLayer::solid("Red", Rect::new(0, 0, 10, 10), [1.0, 0.0, 0.0, 1.0]), WLayer::solid("Green", Rect::new(10, 0, 10, 10), [0.0, 1.0, 0.0, 1.0])];
    let inner_bytes = write(&inner);
    let mut d = WDoc::new(64, 48);
    // A perspective trapezoid, and an affine placement with an Arc warp.
    let trap = [[20.0, 4.0], [44.0, 10.0], [44.0, 38.0], [20.0, 44.0]];
    let warp = warp_descriptor("warpArc", 50.0, 0.0, 0.0, [0.0, 0.0, 20.0, 10.0], None);
    d.layers = vec![
        WLayer::solid("Back", Rect::new(0, 0, 64, 48), [0.0, 0.0, 0.0, 1.0]),
        WLayer::solid("Pinned", Rect::new(20, 4, 24, 40), [0.5, 0.5, 0.5, 1.0]).with_block(smart_object_block_warped("so-1", trap, [20.0, 10.0], None)),
    ];
    d.linked = vec![("so-1".into(), "inner.psd".into(), *b"8BPS", inner_bytes.clone())];
    let mut d2 = WDoc::new(64, 48);
    d2.layers = vec![
        WLayer::solid("Back", Rect::new(0, 0, 64, 48), [0.0, 0.0, 0.0, 1.0]),
        WLayer::solid("Arc", Rect::new(20, 10, 40, 20), [0.5, 0.5, 0.5, 1.0]).with_block(smart_object_block_warped(
            "so-2",
            [[20.0, 10.0], [60.0, 10.0], [60.0, 30.0], [20.0, 30.0]],
            [20.0, 10.0],
            Some(warp),
        )),
    ];
    d2.linked = vec![("so-2".into(), "inner.psd".into(), *b"8BPS", inner_bytes)];
    let parsed = effectcraft_psd::Psd::parse(write(&d2)).unwrap();
    let so = parsed.layers.iter().find_map(|l| l.smart_object.clone()).unwrap();
    assert_eq!(so.warp.as_ref().map(|w| w.style.as_str()), Some("warpArc"));
    let mut s = effectcraft_host::session();
    let render = |s: &mut effectcraft_engine::Session, doc: &WDoc, file: &str| {
        let path = tmp(file);
        std::fs::write(&path, write(doc)).unwrap();
        let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": "composition"})).unwrap();
        assert_eq!(r["errors"], json!([]), "{r}");
        let cid = ItemId(r["comps"][0].as_u64().unwrap());
        let placed = s.project.comp(cid).unwrap().layers.iter().any(|l| {
            let effectcraft_project::LayerSource::Footage { item } = l.source else { return false };
            matches!(&s.project.item(item).unwrap().kind, effectcraft_project::ItemKind::Footage(f) if f.layer.as_ref().is_some_and(|x| x.placed))
        });
        assert!(placed, "{file}: baked as placed");
        s.render(cid, effectcraft_time::Tick::ZERO, RenderOpts::default())
    };
    let img = render(&mut s, &d, "pinned.psd");
    let (red, green) = (img.get(24, 24), img.get(40, 24));
    assert!(red[0] > 0.9 && red[1] < 0.1, "{red:?}");
    assert!(green[1] > 0.9 && green[0] < 0.1, "{green:?}");
    // Above the slanted top edge (y = 4 + 6·(x − 20)/24): background.
    assert!(img.get(42, 6)[0] < 0.05 && img.get(42, 6)[1] < 0.05, "{:?}", img.get(42, 6));
    let img = render(&mut s, &d2, "arc.psd");
    // The arc keeps the top centre and pulls the corners down and out.
    let top = img.get(40, 11);
    assert!(top[0] > 0.5 || top[1] > 0.5, "top centre covered {top:?}");
    assert!(img.get(21, 11)[0] < 0.05 && img.get(21, 11)[1] < 0.05, "corner region empty {:?}", img.get(21, 11));
}

/// A PDF with two placed images (one turned a quarter) between a background and a shape.
fn image_pdf() -> Vec<u8> {
    image_pdf_with("")
}

/// [`image_pdf`] with `boxes` added to the page dictionary.
fn image_pdf_with(boxes: &str) -> Vec<u8> {
    let content = "0.9 0.9 0.9 rg 0 0 120 80 re f\nq 60 0 0 30 10 40 cm /Im Do Q\nq 0 40 -20 0 110 10 cm /Im Do Q\n0 0 1 rg 20 5 30 20 re f\n";
    // 8×8 pixels in quadrants: red, green / white, black.
    let quad = |x: usize, y: usize| -> [u8; 3] { [[[255, 0, 0], [0, 255, 0]], [[255, 255, 255], [0, 0, 0]]][y / 4][x / 4] };
    let px: Vec<u8> = (0..64).flat_map(|i| quad(i % 8, i / 8)).collect();
    effectcraft_pdf::write::pdf(
        &[
            (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), None),
            (3, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 80] {boxes} /Resources << /XObject << /Im 5 0 R >> >> /Contents 4 0 R >>"), None),
            (4, "<< >>".into(), Some(content.as_bytes().to_vec())),
            (5, "<< /Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceRGB /BitsPerComponent 8 >>".into(), Some(px)),
        ],
        1,
    )
}

#[test]
fn shapes_from_vector_layer_keeps_images_as_footage_layers() {
    let bytes = image_pdf();
    let doc = effectcraft_pdf::parse(&bytes).unwrap();
    let (w, h) = doc.pixel_size();
    let direct = effectcraft_svg::rasterize(&doc, w, h, 1.0);
    let path = tmp("images.pdf");
    std::fs::write(&path, &bytes).unwrap();
    let mut s = effectcraft_host::session();
    let item = s.execute_checked("file.import", json!({"paths": [path]})).unwrap()["items"][0].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "I", "width": w, "height": h, "frameRate": 30, "duration": 1})).unwrap();
    let lid = s.execute_checked("layer.addItem", json!({"item": item})).unwrap()["layer"].as_u64().unwrap();
    let cid = s.active_comp_id().unwrap();
    let t = effectcraft_time::Tick::ZERO;
    let foot = s.render(cid, t, RenderOpts::default());
    assert!(mean_diff(&foot, &direct) < 1.0 / 255.0);
    s.execute("layer.select", json!({"layers": [lid]})).unwrap();
    let r = s.execute_checked("layer.create", json!({"op": "shapesFromVector"})).unwrap();
    assert_eq!(r["skipped"], json!([]), "{r}");
    let sid = r["layers"][0].as_u64().unwrap();
    let c = s.active_comp().unwrap();
    // In the document's paint order: the shape drawn last, the two images (the later one
    // higher), the background drawn first, then the hidden source.
    let names: Vec<&str> = c.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["images.pdf Outlines", "Image", "Image", "images.pdf Outlines 1", "images.pdf"]);
    assert_eq!(c.layers[0].id.0, sid);
    assert_eq!(r["layers"].as_array().unwrap().len(), 2);
    for l in c.layers.iter().skip(1).take(2) {
        assert_eq!(l.parent.map(|p| p.0), Some(sid), "{}", l.name);
        assert!(matches!(l.source, effectcraft_project::LayerSource::Footage { .. }));
    }
    assert!(!c.layers[4].switches.video);
    let shapes = s.render(cid, t, RenderOpts::default());
    let d = mean_diff(&shapes, &direct);
    assert!(d < 0.02, "converted vs page: mean diff {d}");
    // Image pixels in place: quarters of the upright image and of the turned one, and the
    // shape over the background.
    for (x, y) in [(25, 17), (55, 17), (25, 32), (55, 32), (95, 60), (95, 40), (105, 40), (30, 65)] {
        let (a, b) = (shapes.get(x, y), direct.get(x, y));
        assert!((0..3).all(|k| (a[k] - b[k]).abs() < 0.05), "({x}, {y}): {a:?} vs {b:?}");
    }
}

fn retain_pdf(width: u32) -> Vec<u8> {
    effectcraft_pdf::write::pdf(
        &[
            (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), None),
            (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 80] /Resources << /Properties << /L0 5 0 R /L1 6 0 R >> >> /Contents 4 0 R >>".into(), None),
            (
                4,
                "<< >>".into(),
                Some(
                    format!("/OC /L0 BDC q 20 15 {width} 20 re W n 1 0 0 rg 0 0 120 80 re f Q EMC /OC /L1 BDC 0 0 1 RG 4 w 70 40 m 100 40 l S EMC")
                        .into_bytes(),
                ),
            ),
            (5, "<< /Type /OCG /Name (Box) >>".into(), None),
            (6, "<< /Type /OCG /Name (Line) >>".into(), None),
        ],
        1,
    )
}

/// A 200 × 100 pt page: red left half, blue right half.
fn halves_pdf() -> Vec<u8> {
    effectcraft_pdf::write::pdf(
        &[
            (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), None),
            (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".into(), None),
            (4, "<< >>".into(), Some(b"1 0 0 rg 0 0 100 100 re f 0 0 1 rg 100 0 100 100 re f".to_vec())),
        ],
        1,
    )
}

/// #3099: footage from an After Effects project ([`effectcraft_project::Footage::placed_page`])
/// keeps the size After Effects stored for a placed .ai, which can differ from the page as read
/// here. The page fills that size at full and at reduced resolution alike: it was drawn at its own
/// aspect at full resolution and stretched to the stored size below 3/4 (the art jumped when the
/// viewer changed resolution, and the masks, stored against that size, came out too large at
/// full resolution). Two placed items of one file at different stored sizes each get their own
/// pixels, and neither reuses the file's frame at its own size, decoded first.
#[test]
fn placed_vector_footage_fills_its_stored_size_at_every_resolution() {
    use effectcraft_project::ItemKind;
    use effectcraft_time::Tick;

    let path = tmp("stored-size.ai");
    std::fs::write(&path, halves_pdf()).unwrap();
    let mut s = effectcraft_host::session();
    let item = ItemId(s.execute_checked("file.import", json!({"paths": [path]})).unwrap()["items"][0].as_u64().unwrap());
    let red = |p: [f32; 4]| p[3] > 0.99 && p[0] > 0.9 && p[2] < 0.1;
    let blue = |p: [f32; 4]| p[3] > 0.99 && p[2] > 0.9 && p[0] < 0.1;
    // At the file's own size first (its frame goes to the pool's cache).
    s.execute("comp.new", json!({"name": "Own", "width": 200, "height": 100, "frameRate": 25, "duration": 1})).unwrap();
    s.execute_checked("layer.addItem", json!({"item": item.0})).unwrap();
    let own_comp = s.active_comp_id().unwrap();
    let own = s.render(own_comp, Tick::ZERO, RenderOpts::default());
    assert!(red(own.get(50, 90)) && blue(own.get(150, 90)));
    // Two placed items of the file with the sizes an After Effects project stores (other aspects).
    let proj = std::sync::Arc::make_mut(&mut s.project);
    let ItemKind::Footage(f) = &mut proj.item_mut(item).unwrap().kind else { panic!("footage") };
    (f.width, f.height, f.placed_page) = (100, 80, true);
    let tall = effectcraft_project::Footage { width: 60, height: 120, ..f.clone() };
    let tall = proj.add_item("tall", Default::default(), None, ItemKind::Footage(tall));
    for (item, (w, h)) in [(item, (100u32, 80u32)), (tall, (60, 120))] {
        s.execute("comp.new", json!({"name": format!("Stored {w}"), "width": w, "height": h, "frameRate": 25, "duration": 1})).unwrap();
        let lid = s.execute_checked("layer.addItem", json!({"item": item.0})).unwrap()["layer"].as_u64().unwrap();
        let cid = s.active_comp_id().unwrap();
        let (l, r, top, bottom) = (i64::from(w / 4), i64::from(w * 3 / 4), 3, i64::from(h) - 4);
        for collapse in [false, true] {
            s.execute("layer.setSwitch", json!({"layers": [lid], "switch": "collapse", "value": collapse})).unwrap();
            let full = s.render(cid, Tick::ZERO, RenderOpts::default());
            assert_eq!((full.width, full.height), (w, h));
            for y in [top, bottom] {
                let (a, b) = (full.get(l, y), full.get(r, y));
                assert!(red(a) && blue(b), "{w}×{h}, collapse {collapse}, full resolution, row {y}: {a:?} {b:?}");
            }
            let half = s.render(cid, Tick::ZERO, RenderOpts { scale: 0.5, ..RenderOpts::default() });
            assert_eq!((half.width, half.height), (w / 2, h / 2));
            for y in [top / 2, bottom / 2] {
                let (a, b) = (half.get(l / 2, y), half.get(r / 2, y));
                assert!(red(a) && blue(b), "{w}×{h}, collapse {collapse}, half resolution, row {y}: {a:?} {b:?}");
            }
        }
        // Create Shapes from Vector Layer draws the shapes where the footage is drawn.
        s.execute_checked("layer.create", json!({"op": "shapesFromVector", "layers": [lid]})).unwrap();
        s.execute("layer.setSwitch", json!({"layers": [lid], "switch": "video", "value": false})).unwrap();
        let shapes = s.render(cid, Tick::ZERO, RenderOpts::default());
        for y in [top, bottom] {
            let (a, b) = (shapes.get(l, y), shapes.get(r, y));
            assert!(red(a) && blue(b), "{w}×{h}, shapes, row {y}: {a:?} {b:?}");
        }
    }
}

/// #3099 review: Create Shapes from Vector Layer on a placed page keeps the page's frame. The
/// footage's raster shows only the ArtBox (241 × 121) of `art_box_ai()`, whose grey media box
/// reaches past it on every side; in a composition larger than the footage, the outlines show
/// the same colours inside the frame and nothing (the background) outside it. Without the clip
/// they showed the whole page: the grey background around the art.
#[test]
fn shapes_from_a_placed_page_stay_inside_its_frame() {
    use effectcraft_project::ItemKind;
    use effectcraft_time::Tick;

    let path = tmp("placed-frame.ai");
    std::fs::write(&path, art_box_ai()).unwrap();
    let mut s = effectcraft_host::session();
    let item = ItemId(s.execute_checked("file.import", json!({"paths": [path]})).unwrap()["items"][0].as_u64().unwrap());
    // As an After Effects project stores it: the ArtBox's size, placed.
    let ItemKind::Footage(f) = &mut std::sync::Arc::make_mut(&mut s.project).item_mut(item).unwrap().kind else { panic!("footage") };
    (f.width, f.height, f.placed_page) = (241, 121, true);
    s.execute("comp.new", json!({"name": "Big", "width": 400, "height": 300, "frameRate": 25, "duration": 1})).unwrap();
    let lid = s.execute_checked("layer.addItem", json!({"item": item.0})).unwrap()["layer"].as_u64().unwrap();
    let cid = s.active_comp_id().unwrap();
    // The layer is centred: layer pixel (x, y) is comp (x + 79.5, y + 89.5).
    let at = |img: &Image, x: f64, y: f64| img.get((x + 79.5).floor() as i64, (y + 89.5).floor() as i64);
    let red = |p: [f32; 4]| p[3] > 0.99 && p[0] > 0.9 && p[1] < 0.1 && p[2] < 0.1;
    let blue = |p: [f32; 4]| p[3] > 0.99 && p[2] > 0.9 && p[0] < 0.1 && p[1] < 0.1;
    let empty = |p: [f32; 4]| p[0] < 0.02 && p[1] < 0.02 && p[2] < 0.02;
    let inside = [([20.0, 20.0], true), ([110.0, 100.0], true), ([130.0, 20.0], false), ([230.0, 110.0], false)];
    // Outside the frame, where the page's grey media box would show: left, right, above, below.
    let outside = [[-20.0, 60.0], [-55.0, 100.0], [260.0, 60.0], [300.0, 10.0], [120.0, -20.0], [120.0, 140.0]];
    let check = |img: &Image, what: &str| {
        for (p, want_red) in inside {
            let px = at(img, p[0], p[1]);
            assert!(if want_red { red(px) } else { blue(px) }, "{what}: inside the frame at {p:?}: {px:?}");
        }
        for p in outside {
            let px = at(img, p[0], p[1]);
            assert!(empty(px), "{what}: outside the frame at {p:?}: {px:?}");
        }
    };
    check(&s.render(cid, Tick::ZERO, RenderOpts::default()), "footage");
    s.execute_checked("layer.create", json!({"op": "shapesFromVector", "layers": [lid]})).unwrap();
    let comp = s.project.comp(cid).unwrap();
    assert!(!comp.layers.iter().find(|l| l.id.0 == lid).unwrap().switches.video, "the source is switched off");
    check(&s.render(cid, Tick::ZERO, RenderOpts::default()), "shapes");
}

/// #3099 review: the images of a placed page become footage layers clipped to the frame too.
/// `image_pdf_with` an ArtBox [20 15 100 70]: the upright image reaches past the frame's left
/// edge, the turned one past its right and bottom edges, the grey page around it. After the
/// conversion the layers inside the frame match the footage, and outside it nothing shows.
#[test]
fn images_from_a_placed_page_stay_inside_its_frame() {
    use effectcraft_project::{ItemKind, MatteKind, TrackMatte};
    use effectcraft_time::Tick;

    let path = tmp("placed-images.pdf");
    std::fs::write(&path, image_pdf_with("/ArtBox [20 15 100 70]")).unwrap();
    let mut s = effectcraft_host::session();
    let item = ItemId(s.execute_checked("file.import", json!({"paths": [path]})).unwrap()["items"][0].as_u64().unwrap());
    let ItemKind::Footage(f) = &mut std::sync::Arc::make_mut(&mut s.project).item_mut(item).unwrap().kind else { panic!("footage") };
    (f.width, f.height, f.placed_page) = (80, 55, true);
    s.execute("comp.new", json!({"name": "Big", "width": 200, "height": 150, "frameRate": 25, "duration": 1})).unwrap();
    let lid = s.execute_checked("layer.addItem", json!({"item": item.0})).unwrap()["layer"].as_u64().unwrap();
    let cid = s.active_comp_id().unwrap();
    // Centred: layer pixel (x, y) is comp (x + 60, y + 47.5).
    let at = |img: &Image, x: f64, y: f64| img.get((x + 60.0).floor() as i64, (y + 47.5).floor() as i64);
    let empty = |p: [f32; 4]| p[0] < 0.02 && p[1] < 0.02 && p[2] < 0.02;
    // Inside: both images and the grey page; outside: the images past the frame, the grey page.
    let inside = [[10.0, 8.0], [40.0, 8.0], [10.0, 22.0], [40.0, 22.0], [75.0, 25.0], [75.0, 50.0], [5.0, 40.0], [60.0, 10.0]];
    let outside = [[-5.0, 15.0], [85.0, 40.0], [75.0, 58.0], [-15.0, 50.0], [40.0, -5.0]];
    let footage = s.render(cid, Tick::ZERO, RenderOpts::default());
    for p in outside {
        assert!(empty(at(&footage, p[0], p[1])), "footage outside the frame at {p:?}: {:?}", at(&footage, p[0], p[1]));
    }
    let r = s.execute_checked("layer.create", json!({"op": "shapesFromVector", "layers": [lid]})).unwrap();
    assert_eq!(r["skipped"], json!([]), "{r}");
    // One hidden frame layer is the Alpha track matte of every layer made from the page.
    let comp = s.project.comp(cid).unwrap();
    let frame = comp.layers.iter().find(|l| l.name.ends_with(" Frame")).expect("the frame matte layer");
    assert!(!frame.switches.video);
    let made: Vec<_> = comp.layers.iter().filter(|l| l.id != frame.id && l.id.0 != lid).collect();
    assert_eq!(made.iter().filter(|l| l.name == "Image").count(), 2);
    for l in &made {
        assert_eq!(l.track_matte, Some(TrackMatte { layer: frame.id, kind: MatteKind::Alpha }), "{}", l.name);
    }
    let shapes = s.render(cid, Tick::ZERO, RenderOpts::default());
    for p in inside {
        let (a, b) = (at(&shapes, p[0], p[1]), at(&footage, p[0], p[1]));
        assert!((0..4).all(|k| (a[k] - b[k]).abs() < 0.05), "inside the frame at {p:?}: {a:?} vs footage {b:?}");
    }
    for p in outside {
        assert!(empty(at(&shapes, p[0], p[1])), "shapes outside the frame at {p:?}: {:?}", at(&shapes, p[0], p[1]));
    }
}

/// A PDF with an ArtBox, imported here (not from an After Effects project), and a project saved
/// with it before #3099: shown as a PDF viewer frames it (the crop box, 410 × 210) at its own
/// size, exactly as before; the project file has no new field.
#[test]
fn a_pdf_with_an_art_box_imported_here_keeps_the_viewer_framing() {
    use effectcraft_project::{ItemKind, Project};
    use effectcraft_time::Tick;

    let bytes = art_box_ai();
    let path = tmp("viewer-art-box.pdf");
    std::fs::write(&path, &bytes).unwrap();
    let mut s = effectcraft_host::session();
    let item = ItemId(s.execute_checked("file.import", json!({"paths": [path]})).unwrap()["items"][0].as_u64().unwrap());
    let ItemKind::Footage(f) = &s.project.item(item).unwrap().kind else { panic!("footage") };
    assert_eq!((f.width, f.height, f.placed_page), (410, 210, false));
    s.execute("comp.new", json!({"name": "V", "width": 410, "height": 210, "frameRate": 25, "duration": 1})).unwrap();
    s.execute_checked("layer.addItem", json!({"item": item.0})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let direct = effectcraft_svg::rasterize(&effectcraft_pdf::parse(&bytes).unwrap(), 410, 210, 1.0);
    assert!(mean_diff(&s.render(cid, Tick::ZERO, RenderOpts::default()), &direct) < 1.0 / 255.0);
    let saved = s.project.to_json();
    assert!(!saved.contains("placed_page"), "the saved project is in the earlier format");
    s.project = std::sync::Arc::new(Project::from_json(&saved).unwrap());
    assert!(mean_diff(&s.render(cid, Tick::ZERO, RenderOpts::default()), &direct) < 1.0 / 255.0, "reopened");
}

/// `i3099-a-boxes.ai` of `effectcraft-aep`'s `ai-mask-3099.aep` (its boxes as in
/// `tests/fixtures/tools/ai-mask-3099.py`): MediaBox 420 × 220, CropBox, BleedBox and TrimBox
/// inside it, ArtBox [60 50 300.6 170.4]. Here grey over the media box, the art box red on its
/// left half and blue on its right half.
fn art_box_ai() -> Vec<u8> {
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 420 220] /CropBox [5 5 415 215] /BleedBox [10 10 410 210] /TrimBox [20 20 400 200] /ArtBox [60 50 300.6 170.4] /Contents 4 0 R >>";
    let content = b"0.5 g 0 0 420 220 re f 1 0 0 rg 60 50 120.3 120.4 re f 0 0 1 rg 180.3 50 120.3 120.4 re f";
    effectcraft_pdf::write::pdf(
        &[
            (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), None),
            (3, page.into(), None),
            (4, "<< >>".into(), Some(content.to_vec())),
        ],
        1,
    )
}

/// `i3099-d-trim.ai` of `ai-mask-3099.aep`: MediaBox 418 × 218, TrimBox, ArtBox [40 30 250 150]
/// (no CropBox). Grey over the media box, the art box red on its left half, blue on its right.
fn trim_ai() -> Vec<u8> {
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 418 218] /TrimBox [9 9 409 209] /BleedBox [0 0 418 218] /ArtBox [40 30 250 150] /Contents 4 0 R >>";
    let content = b"0.5 g 0 0 418 218 re f 1 0 0 rg 40 30 105 120 re f 0 0 1 rg 145 30 105 120 re f";
    effectcraft_pdf::write::pdf(
        &[
            (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), None),
            (3, page.into(), None),
            (4, "<< >>".into(), Some(content.to_vec())),
        ],
        1,
    )
}

/// `i3099-c-rot.ai` of `ai-mask-3099.aep`: MediaBox [100 50 400 150] with `/Rotate 90`, grey,
/// with a blue 20 × 10 marker 4 pt in from the media box's lower-left corner.
fn rotated_ai() -> Vec<u8> {
    effectcraft_pdf::write::pdf(
        &[
            (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), None),
            (3, "<< /Type /Page /Parent 2 0 R /MediaBox [100 50 400 150] /Rotate 90 /Contents 4 0 R >>".into(), None),
            (4, "<< >>".into(), Some(b"0.5 g 100 50 300 100 re f 0 0 1 rg 104 54 20 10 re f".to_vec())),
        ],
        1,
    )
}

/// #3099 end to end, against After Effects 2026's own read-back of the committed project
/// `ai-mask-3099.aep`: a placed .ai (ArtBox inside a larger crop box) parented to a Null parented
/// to a Null, with two rectangular masks. After Effects sizes the footage 241 × 121 (the ArtBox)
/// and puts the layer's corners at the comp points below (`toComp`). The art fills the layer
/// between the masks at full and at half resolution; it was drawn from the crop box (410 × 210)
/// scaled to the layer's width, so the masks cut the wrong part of it and half resolution
/// squeezed it, and the Null left at its default anchor moved it. `artC` (`/Rotate 90`) is drawn
/// unturned as After Effects draws it, `artD` (Continuously Rasterize) as `art`.
#[test]
fn aep_ai_layer_with_masks_under_two_nulls_renders_like_after_effects() {
    use effectcraft_engine::render::EvalCtx;
    use effectcraft_project::ItemKind;
    use effectcraft_time::Tick;

    let dir = std::path::PathBuf::from(tmp("ai-mask-3099"));
    std::fs::create_dir_all(&dir).unwrap();
    let aep = dir.join("ai-mask-3099.aep");
    // The fixture belongs to `effectcraft-aep` (its sidecar and generator are there); this test
    // renders it through the wired session, which that crate cannot.
    std::fs::write(&aep, include_bytes!("../../../aep/tests/fixtures/ai-mask-3099.aep")).unwrap();
    let mut s = effectcraft_host::session();
    s.execute_checked("file.importAep", json!({"path": aep.to_string_lossy(), "allowLosses": true})).unwrap();
    // The project's footage paths are neutral (`C:\fixture\x\...`): the files are linked here.
    for (name, bytes, size) in
        [("i3099-a-boxes.ai", art_box_ai(), (241, 121)), ("i3099-c-rot.ai", rotated_ai(), (300, 100)), ("i3099-d-trim.ai", trim_ai(), (210, 120))]
    {
        let file = dir.join(name);
        std::fs::write(&file, bytes).unwrap();
        let item = s
            .project
            .items
            .values()
            .find(|i| matches!(&i.kind, ItemKind::Footage(f) if f.path.ends_with(name)))
            .map(|i| i.id)
            .unwrap_or_else(|| panic!("footage {name}"));
        let ItemKind::Footage(f) = &mut std::sync::Arc::make_mut(&mut s.project).item_mut(item).unwrap().kind else { panic!("footage") };
        assert_eq!((f.width, f.height, f.placed_page), (size.0, size.1, true), "{name}: After Effects' size of the placed .ai");
        (f.path, f.missing) = (file.to_string_lossy().into_owned(), false);
    }
    let cid = s.project.items.values().find(|i| i.name == "ai-mask").map(|i| i.id).expect("comp ai-mask");

    // After Effects' toComp of the art layer's points (layer pixels → comp pixels).
    let ae = [
        ([0.0, 0.0], [634.193413913741, 195.085717365354]),
        ([241.0, 0.0], [913.539162876539, 269.936185209003]),
        ([0.0, 121.0], [577.822625890412, 405.464362331113]),
        ([40.0, 30.0], [666.58162514008, 259.669026149884]),
        ([350.0, 160.0], [965.342375965623, 581.976354279664]),
    ];
    let comp = s.project.comp(cid).unwrap();
    let art = comp.layer_by_name("art").unwrap();
    let (m, _) = EvalCtx::new(&s.project, cid, comp, Tick::ZERO).layer_to_comp(art);
    let to_comp = |[u, v]: [f64; 2]| [m.0[0][0] * u + m.0[0][1] * v + m.0[0][2], m.0[1][0] * u + m.0[1][1] * v + m.0[1][2]];
    for (p, want) in ae {
        let got = to_comp(p);
        assert!((got[0] - want[0]).abs() < 1e-3 && (got[1] - want[1]).abs() < 1e-3, "{p:?}: {got:?} vs After Effects {want:?}");
    }

    // Inside the masks (M1: x 40..200, y 30..90; M2: x 220..350, y 100..160, cut by the layer).
    let red = |p: [f32; 4]| p[0] > 0.9 && p[1] < 0.1 && p[2] < 0.1;
    let blue = |p: [f32; 4]| p[2] > 0.9 && p[0] < 0.1 && p[1] < 0.1;
    let full = s.render(cid, Tick::ZERO, RenderOpts::default());
    let half = s.render(cid, Tick::ZERO, RenderOpts { scale: 0.5, ..RenderOpts::default() });
    for (p, want_red) in [([60.0, 45.0], true), ([100.0, 75.0], true), ([150.0, 40.0], false), ([190.0, 80.0], false), ([232.0, 112.0], false)] {
        let [x, y] = to_comp(p);
        let at = |img: &Image, k: f64| img.get((x * k).round() as i64, (y * k).round() as i64);
        for (img, k, what) in [(&full, 1.0, "full"), (&half, 0.5, "half")] {
            let px = at(img, k);
            let ok = if want_red { red(px) } else { blue(px) };
            assert!(ok, "{what} resolution, layer point {p:?}: {px:?} (want {})", if want_red { "red" } else { "blue" });
        }
    }
    // Outside the masks the art is cut away: the black background.
    for p in [[100.0, 110.0], [20.0, 60.0]] {
        let [x, y] = to_comp(p);
        let px = full.get(x.round() as i64, y.round() as i64);
        assert!(px[0] < 0.05 && px[1] < 0.05 && px[2] < 0.05, "layer point {p:?}: {px:?}");
    }
    // artC (/Rotate 90, position (1500, 800), anchor (150, 50)): After Effects draws the page
    // unturned, 300 × 100, the marker at its lower left (comp 1354..1374, 836..846).
    for (img, k, what) in [(&full, 1.0, "full"), (&half, 0.5, "half")] {
        let at = |x: f64, y: f64| img.get((x * k).round() as i64, (y * k).round() as i64);
        let marker = at(1364.0, 841.0);
        assert!(blue(marker), "{what} resolution, artC marker: {marker:?}");
        for (x, y) in [(1450.0, 780.0), (1630.0, 790.0), (1364.0, 770.0)] {
            let grey = at(x, y);
            assert!(grey[3] > 0.99 && (grey[0] - grey[2]).abs() < 0.02 && grey[0] > 0.1 && grey[0] < 0.9, "{what} resolution, artC at ({x}, {y}): {grey:?}");
        }
    }
    for (x, y) in [(1340.0, 800.0), (1500.0, 870.0)] {
        let px = full.get(x as i64, y as i64);
        assert!(px[0] < 0.05 && px[2] < 0.05, "outside artC ({x}, {y}): {px:?}");
    }
    // artD (ArtBox 210 × 120, Continuously Rasterize, the same masks, also under N2): placed by
    // After Effects' toComp of its corners.
    let (tl, tr, bl) = ([883.191424310055, 728.610562914909], [1126.6047325349, 793.832962280744], [827.286510567911, 937.250541393348]);
    let d_to_comp = |[u, v]: [f64; 2]| [0, 1].map(|i| tl[i] + u / 210.0 * (tr[i] - tl[i]) + v / 120.0 * (bl[i] - tl[i]));
    for (p, want_red) in [([60.0, 45.0], true), ([90.0, 75.0], true), ([150.0, 40.0], false), ([190.0, 80.0], false)] {
        let [x, y] = d_to_comp(p);
        for (img, k, what) in [(&full, 1.0, "full"), (&half, 0.5, "half")] {
            let px = img.get((x * k).round() as i64, (y * k).round() as i64);
            let ok = if want_red { red(px) } else { blue(px) };
            assert!(ok, "artD, {what} resolution, layer point {p:?}: {px:?} (want {})", if want_red { "red" } else { "blue" });
        }
    }
}

#[test]
fn ai_composition_retains_layer_sizes_and_positions() {
    use effectcraft_project::{ItemKind, LayerSource, Project};
    use effectcraft_time::Tick;

    for name in ["retain.ai", "retain.pdf"] {
        let path = tmp(name);
        let bytes = retain_pdf(30);
        std::fs::write(&path, &bytes).unwrap();
        let mut s = effectcraft_host::session();
        let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": "compositionLayerSizes"})).unwrap();
        assert_eq!(r["errors"], json!([]), "{r}");
        let cid = ItemId(r["comps"][0].as_u64().unwrap());
        let comp = s.project.comp(cid).unwrap();
        assert_eq!((comp.width, comp.height), (120, 80));
        assert_eq!(comp.layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Line", "Box"]);
        let mut ids = vec![];
        for (name, size, anchor, position) in [("Box", (30, 20), [15.0, 10.0], [35.0, 55.0]), ("Line", (30, 4), [15.0, 2.0], [85.0, 40.0])] {
            let l = comp.layer_by_name(name).unwrap();
            let LayerSource::Footage { item } = l.source else { panic!("footage") };
            let ItemKind::Footage(f) = &s.project.item(item).unwrap().kind else { panic!("footage") };
            assert_eq!((f.width, f.height), size, "{name} uses its artwork bounds");
            assert!(f.layer.as_ref().unwrap().layer_size);
            assert_eq!(l.props.prop("transform/anchor").unwrap().value.as_vec2(), anchor);
            assert_eq!(l.props.prop("transform/position").unwrap().value.as_vec2(), position);
            let frame = s.footage.frame(item, f, Tick::ZERO).unwrap();
            assert_eq!((frame.width, frame.height), size);
            let sharp = s.footage.vector_frame(item, f, 2.0).unwrap();
            assert_eq!((sharp.width, sharp.height), (size.0 * 2, size.1 * 2));
            assert!(sharp.get(1, 1)[3] > 0.99);
            ids.push(item);
        }
        let direct = effectcraft_svg::rasterize(&effectcraft_pdf::parse(&bytes).unwrap(), 120, 80, 1.0);
        assert!(mean_diff(&s.render(cid, Tick::ZERO, RenderOpts::default()), &direct) < 0.001);
        let saved = s.project.to_json();
        s.project = std::sync::Arc::new(Project::from_json(&saved).unwrap());
        assert!(mean_diff(&s.render(cid, Tick::ZERO, RenderOpts::default()), &direct) < 0.001, "reopened project");
        let box_id = s.active_comp().unwrap().layer_by_name("Box").unwrap().id.0;
        s.execute_checked("layer.create", json!({"op": "shapesFromVector", "layers": [box_id]})).unwrap();
        assert!(mean_diff(&s.render(cid, Tick::ZERO, RenderOpts::default()), &direct) < 0.001, "shape conversion preserves placement");
        s.execute_checked("edit.undo", json!({})).unwrap();
        std::fs::write(&path, retain_pdf(40)).unwrap();
        s.execute_checked("file.reloadFootage", json!({"items": ids})).unwrap();
        let ItemKind::Footage(f) = &s.project.item(ids[0]).unwrap().kind else { panic!("footage") };
        assert_eq!((f.width, f.height), (40, 20), "reload refreshes the layer's own dimensions");
        let frame = s.footage.frame(ids[0], f, Tick::ZERO).unwrap();
        assert_eq!((frame.width, frame.height), (40, 20));
        assert!(frame.get(39, 10)[0] > 0.99, "reload invalidates cached pixels");
    }
}
