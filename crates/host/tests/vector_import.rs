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

const SVG: &[u8] = include_bytes!("../../svg/tests/fixtures/basic-shapes.svg");
const SVG2: &[u8] = include_bytes!("../../svg/tests/fixtures/paths-gradients.svg");

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
    let content = "0.9 0.9 0.9 rg 0 0 120 80 re f\nq 60 0 0 30 10 40 cm /Im Do Q\nq 0 40 -20 0 110 10 cm /Im Do Q\n0 0 1 rg 20 5 30 20 re f\n";
    // 8×8 pixels in quadrants: red, green / white, black.
    let quad = |x: usize, y: usize| -> [u8; 3] { [[[255, 0, 0], [0, 255, 0]], [[255, 255, 255], [0, 0, 0]]][y / 4][x / 4] };
    let px: Vec<u8> = (0..64).flat_map(|i| quad(i % 8, i / 8)).collect();
    effectcraft_pdf::write::pdf(
        &[
            (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), None),
            (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 80] /Resources << /XObject << /Im 5 0 R >> >> /Contents 4 0 R >>".into(), None),
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
    for l in &c.layers[1..3] {
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
