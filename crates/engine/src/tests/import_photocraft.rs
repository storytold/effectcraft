//! PhotoCraft bundles written from synthetic manifest data.

use effectcraft_color::BlendMode;
use effectcraft_project::{ItemId, ItemKind, LayerSource};
use serde_json::{Value, json};

use crate::Session;

struct Import;

impl crate::Importer for Import {
    fn probe(&self, path: &str) -> Result<effectcraft_project::Footage, String> {
        effectcraft_media::probe(path).map_err(|e| e.to_string())
    }
}

fn surface(color: &str) -> Value {
    json!({"format":{"mode":"Rgb","sample":"U8","alpha":true},"default":color,"tiles":[]})
}

fn layer(name: &str, visible: bool, opacity: f32, blend: &str, content: Value) -> Value {
    json!({"name":name,"visible":visible,"opacity":opacity,"blend":blend,"content":content})
}

fn bundle(layers: Value) -> Vec<u8> {
    bundle_sized(layers, (8, 6))
}

fn bundle_sized(layers: Value, size: (u32, u32)) -> Vec<u8> {
    let manifest = json!({"format_version":1,"generator":"synthetic PhotoCraft fixture",
        "document":{"size":{"width":size.0,"height":size.1},"resolution_dpi":72.0,"mode":"Rgb","depth":"U8","layers":layers},
        "thumbnail":null,"composite":"composite/preview.png"});
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(4, 3, image::Rgba([20, 30, 40, 255]))).write_to(&mut png, image::ImageFormat::Png).unwrap();
    effectcraft_lottie::zip::store(&[("manifest.json".into(), serde_json::to_vec(&manifest).unwrap()), ("composite/preview.png".into(), png.into_inner())])
}

fn fixture(name: &str, layers: Value) -> String {
    fixture_bytes(name, bundle(layers))
}

fn fixture_bytes(name: &str, bytes: Vec<u8>) -> String {
    let dir = std::env::temp_dir().join(format!("effectcraft-pcraft-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn photocraft_composite_imports_as_footage() {
    let path = fixture("composite.pcraft", json!([]));
    let mut s = Session { importer: Some(std::sync::Arc::new(Import)), ..Default::default() };
    let r = s.execute("file.import", json!({"paths":[path]})).unwrap();
    assert!(r["errors"].as_array().unwrap().is_empty(), "{r}");
    let id = ItemId(r["items"][0].as_u64().unwrap());
    let ItemKind::Footage(f) = &s.project.item(id).unwrap().kind else { panic!("not footage") };
    assert_eq!((f.path.as_str(), f.width, f.height, f.codec.as_str()), (path.as_str(), 4, 3, "PhotoCraft"));
    assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().is_some_and(|w| w.contains("4 × 3") && w.contains("8 × 6"))), "{r}");
    let image = effectcraft_media::MediaPool::new().frame_at(f, effectcraft_time::Tick::ZERO).unwrap();
    assert_eq!((image.width, image.height), (4, 3));
    assert!((image.data[0][0] - 20.0 / 255.0).abs() < 1e-6);
    assert_eq!(crate::media_browser::kind_of("shot_001.pcraft"), Some("image"));
    assert!(crate::sequence::sequences(s.services.as_ref(), &["/shot_001.pcraft".into(), "/shot_002.pcraft".into()], false).is_empty());
    assert!(effectcraft_media::is_importable(std::path::Path::new("shot.PCRAFT")));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn photocraft_composition_keeps_layer_names_visibility_opacity_and_blend() {
    let layers = json!([
        layer("Bottom", true, 1.0, "Normal", json!({"kind":"raster","surface":surface("ff0000ff")})),
        layer("Hidden", false, 0.25, "Multiply", json!({"kind":"text","cache":surface("00ff0080")})),
        layer(
            "Group",
            true,
            1.0,
            "Normal",
            json!({"kind":"group","children":[
                layer("Nested", true, 0.5, "Screen", json!({"kind":"shape","cache":surface("0000ffff")}))
            ]})
        )
    ]);
    let path = fixture("layered.pcraft", layers);
    let mut s = Session::default();
    let before = s.project.items.len();
    let r = s.execute("file.import", json!({"paths":[path],"importAs":"composition"})).unwrap();
    assert_eq!(r["comps"].as_array().unwrap().len(), 1, "{r}");
    assert!(r["errors"].as_array().unwrap().iter().any(|e| e.as_str().is_some_and(|e| e.contains("Some masks, effects and profiles are unsupported"))), "{r}");
    let cid = ItemId(r["comps"][0].as_u64().unwrap());
    let c = s.project.comp(cid).unwrap();
    assert_eq!((c.width, c.height), (8, 6));
    let bottom = c.layers.iter().find(|l| l.name == "Bottom").unwrap();
    assert!(matches!(bottom.source, LayerSource::Footage { .. }));
    let hidden = c.layers.iter().find(|l| l.name == "Hidden").unwrap();
    assert!(!hidden.switches.video);
    assert_eq!(hidden.blend_mode, BlendMode::Multiply);
    assert_eq!(hidden.props.prop("transform/opacity").unwrap().value.as_f64(), 25.0);
    let group = c.layers.iter().find(|l| l.name == "Group").unwrap();
    let LayerSource::Comp { item } = group.source else { panic!("group is not a precomposition") };
    let nested = &s.project.comp(item).unwrap().layers[0];
    assert_eq!(nested.name, "Nested");
    assert_eq!(nested.blend_mode, BlendMode::Screen);
    assert_eq!(nested.props.prop("transform/opacity").unwrap().value.as_f64(), 50.0);
    let LayerSource::Footage { item } = nested.source else { panic!("nested layer is not footage") };
    let ItemKind::Footage(f) = &s.project.item(item).unwrap().kind else { panic!("not footage") };
    assert_eq!(f.layer.as_ref().unwrap().index, 3);
    let image = effectcraft_media::MediaPool::new().frame_at(f, effectcraft_time::Tick::ZERO).unwrap();
    assert_eq!((image.width, image.height), (8, 6));
    assert_eq!(image.data[0], [0.0, 0.0, 1.0, 1.0]);
    let saved = s.project.to_json();
    let limits = craft_doc::Limits::default();
    assert_eq!(craft_doc::info(saved.as_bytes(), &limits).unwrap().kind, craft_doc::Kind::EffectCraft);
    let written = craft_doc::outline(saved.as_bytes(), &limits).unwrap();
    assert!(written.pages.iter().any(|p| p.layers.iter().any(|l| l.name == "Hidden")));
    assert!(craft_doc::renditions(saved.as_bytes(), &limits).unwrap().is_empty());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.items.len(), before);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn photocraft_masks_and_sample_depths_decode_through_the_media_pool() {
    use effectcraft_project::{AlphaMode, Footage, SourceLayer};
    for (sample, color, expected) in [
        ("U8", "ff0000ff", 1.0),
        ("U16", "ffff00000000ffff", 1.0),
        ("F32", "0000803e00000000000000000000803f", 0.5370987),
        ("F32", "0000c07f00000000000000000000803f", 0.0),
    ] {
        let content = json!({"kind":"raster","surface":{"format":{"mode":"Rgb","sample":sample,"alpha":true},"default":color,"tiles":[]}});
        let mut raster = layer("Masked", true, 1.0, "Normal", content);
        raster["mask"] = json!({"enabled":true,"density":0.5,"feather":if sample == "U16" { 0.75 } else { 0.0 },
            "surface":{"format":{"mode":"Grayscale","sample":"U8","alpha":false},"default":"80","tiles":[]}});
        let path = fixture(&format!("masked-{sample}.pcraft"), json!([raster]));
        let mut f = Footage {
            path: path.clone(),
            width: 8,
            height: 6,
            has_video: true,
            alpha: AlphaMode::Straight,
            layer: Some(SourceLayer { index: 0, name: "Masked".into(), layer_size: false, embedded: None, placed: false }),
            ..Default::default()
        };
        let pool = effectcraft_media::MediaPool::new();
        let image = pool.frame_at(&f, effectcraft_time::Tick::ZERO).unwrap();
        let coverage = 1.0 - 0.5 * (1.0 - 128.0 / 255.0);
        assert!((image.data[0][0] - expected * coverage).abs() < 1e-5, "{sample}: {:?}", image.data[0]);
        assert!((image.data[0][3] - coverage).abs() < 1e-6);
        f.alpha = AlphaMode::Premultiplied;
        let image = pool.frame_at(&f, effectcraft_time::Tick::ZERO).unwrap();
        assert!((image.data[0][0] - expected * coverage).abs() < 1e-5, "masked premultiplied {sample}: {:?}", image.data[0]);
        assert!((image.data[0][3] - coverage).abs() < 1e-6);
        f.alpha = AlphaMode::Ignore;
        let image = pool.frame_at(&f, effectcraft_time::Tick::ZERO).unwrap();
        assert!((image.data[0][0] - expected).abs() < 1e-5);
        assert_eq!(image.data[0][3], 1.0, "Ignore Alpha includes the source mask");
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn photocraft_overlapping_layers_render_in_source_stack_order() {
    let path = fixture(
        "stack-order.pcraft",
        json!([
            layer("Bottom", true, 1.0, "Normal", json!({"kind":"raster","surface":surface("ff0000ff")})),
            layer(
                "Group",
                true,
                1.0,
                "Normal",
                json!({"kind":"group","children":[
                    layer("Group bottom", true, 1.0, "Normal", json!({"kind":"raster","surface":surface("00ff00ff")})),
                    layer("Group top", true, 1.0, "Normal", json!({"kind":"raster","surface":surface("0000ffff")}))
                ]})
            )
        ]),
    );
    let mut s = Session { footage: std::sync::Arc::new(effectcraft_media::MediaPool::new()), ..Default::default() };
    let result = s.execute("file.import", json!({"paths":[path],"importAs":"composition"})).unwrap();
    let comp = ItemId(result["comps"][0].as_u64().unwrap());
    let image = s.render(comp, effectcraft_time::Tick::ZERO, Default::default());
    assert_eq!(image.data[0], [0.0, 0.0, 1.0, 1.0], "the source's top layer must cover both lower layers");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn photocraft_bad_metadata_and_missing_layer_pixels_report_errors() {
    use effectcraft_project::{AlphaMode, Footage, SourceLayer};
    for (name, bytes) in [
        ("empty-side.pcraft", bundle_sized(json!([]), (0, 6))),
        ("large-canvas.pcraft", bundle_sized(json!([]), (30_000, 30_000))),
        ("unsupported-fill.pcraft", bundle(json!([layer("Fill", true, 1.0, "Normal", json!({"kind":"fill"}))]))),
        ("truncated.pcraft", b"PK\x03\x04broken".to_vec()),
    ] {
        let path = fixture_bytes(name, bytes);
        let mut s = Session::default();
        let before = s.project.items.len();
        let result = s.execute("file.import", json!({"paths":[path],"importAs":"composition"})).unwrap();
        assert!(result["comps"].as_array().unwrap().is_empty(), "{name}: {result}");
        assert!(!result["errors"].as_array().unwrap().is_empty(), "{name}: {result}");
        assert_eq!(s.project.items.len(), before);
        std::fs::remove_file(path).unwrap();
    }
    let path = fixture("missing-cache.pcraft", json!([layer("Text", true, 1.0, "Normal", json!({"kind":"text"}))]));
    let mut f = Footage {
        path: path.clone(),
        width: 8,
        height: 6,
        has_video: true,
        alpha: AlphaMode::Straight,
        layer: Some(SourceLayer { index: 0, name: "Text".into(), layer_size: false, embedded: None, placed: false }),
        ..Default::default()
    };
    let pool = effectcraft_media::MediaPool::new();
    assert!(pool.frame_at(&f, effectcraft_time::Tick::ZERO).unwrap_err().to_string().contains("no saved pixels"));
    f.layer.as_mut().unwrap().index = 99;
    assert!(pool.frame_at(&f, effectcraft_time::Tick::ZERO).unwrap_err().to_string().contains("no PhotoCraft layer"));
    std::fs::remove_file(path).unwrap();
}

#[test]
#[cfg(unix)] // Sparse length checks exercise the guard without writing half a GiB.
fn photocraft_large_files_are_rejected_before_reading() {
    for name in ["large-sparse.pcraft", "renamed-sparse.zip"] {
        let path = fixture_bytes(name, b"PK\x03\x04".to_vec());
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len((512 << 20) + 1).unwrap();
        assert!(effectcraft_media::probe(&path).unwrap_err().to_string().contains("512 MiB input limit"));
        let mut s = Session::default();
        let before = s.project.items.len();
        let r = s.execute("file.import", json!({"paths":[path],"importAs":"composition"})).unwrap();
        assert_eq!(s.project.items.len(), before);
        assert!(r["errors"].as_array().unwrap().iter().any(|e| e.as_str().is_some_and(|e| e.contains("512 MiB input limit"))), "{r}");
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn photocraft_layer_buffers_are_bounded_before_decoding_pixels() {
    use effectcraft_project::{AlphaMode, Footage, SourceLayer};
    // An invalid source sample proves the budget is checked before the pixel reader.
    let mut raster = layer("Large masked layer", true, 1.0, "Normal", json!({"kind":"raster","surface":surface("invalid-pixel-sample")}));
    raster["mask"] = json!({"enabled":true,"density":1.0,"feather":4.0,
        "surface":{"format":{"mode":"Grayscale","sample":"U8","alpha":false},"default":"ff","tiles":[]}});
    let path = fixture_bytes("mask-budget.pcraft", bundle_sized(json!([raster]), (4097, 3072)));
    let f = Footage {
        path: path.clone(),
        width: 4097,
        height: 3072,
        has_video: true,
        alpha: AlphaMode::Straight,
        layer: Some(SourceLayer { index: 0, name: "Large masked layer".into(), layer_size: false, embedded: None, placed: false }),
        ..Default::default()
    };
    let error = effectcraft_media::MediaPool::new().frame_at(&f, effectcraft_time::Tick::ZERO).unwrap_err().to_string();
    assert!(error.contains("1 GiB working-buffer budget") && error.contains("Footage"), "{error}");
    std::fs::remove_file(path).unwrap();
}
