//! Composition ▸ Save Frame As ▸ Photoshop Layers… / ProEXR…, and Copy Frame to Clipboard.

use serde_json::json;

use crate::Session;

fn out_dir(name: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 32×32 comp: a red full-frame solid under a 16×16 blue Multiply solid at 50%.
fn frame() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Frame", "width": 32, "height": 32, "frameRate": 24, "duration": 1})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Red", "color": "#ff0000"})).unwrap();
    let blue = s.execute("layer.newSolid", json!({"name": "Blue.Box", "color": "#0000ff", "width": 16, "height": 16})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setBlendMode", json!({"layers": [blue], "mode": "Multiply"})).unwrap();
    s.execute("prop.set", json!({"layer": blue, "path": "transform/opacity", "value": 50})).unwrap();
    s
}

#[test]
fn photoshop_layers_writes_one_psd_layer_per_comp_layer() {
    let mut s = frame();
    let dir = out_dir("frame-psd");
    let path = dir.join("frame.psd").to_string_lossy().to_string();
    let r = s.execute("comp.saveFrameAsPsd", json!({"path": path})).unwrap();
    assert_eq!(r["layers"], 2);
    let psd = effectcraft_psd::Psd::parse(std::fs::read(&path).unwrap()).unwrap();
    assert_eq!((psd.width, psd.height), (32, 32));
    let names: Vec<&str> = psd.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Red", "Blue.Box"], "bottom first");
    let blue = &psd.layers[1];
    assert_eq!(blue.blend_key(), "mul ");
    assert_eq!(blue.opacity, 128);
    // Cropped to its pixels (16×16, centred) and rendered at full opacity.
    assert_eq!((blue.rect.left, blue.rect.top, blue.rect.width(), blue.rect.height()), (8, 8, 16, 16));
    assert_eq!(psd.layers[0].blend_key(), "norm");
    assert_eq!(psd.layers[0].opacity, 255);
    // The merged image is the comp frame: red × (50% blue multiply) in the middle.
    let m = psd.composite().unwrap();
    let mid = m.data[16 * 32 + 16];
    assert!(mid[0] > 0.4 && mid[0] < 0.6 && mid[2] < 0.05, "{mid:?}");
    let corner = m.data[0];
    assert!(corner[0] > 0.99 && corner[2] < 0.01, "{corner:?}");
    // The menu has it.
    assert!(crate::commands::find("comp.saveFrameAsPsd").is_some());
}

#[test]
fn proexr_writes_layer_prefixed_channels_and_the_composite() {
    use exr::prelude::*;
    let mut s = frame();
    let dir = out_dir("frame-exr");
    let path = dir.join("frame.exr").to_string_lossy().to_string();
    let r = s.execute("comp.saveFrameAsExr", json!({"path": path})).unwrap();
    let names: Vec<&str> = r["channels"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
    assert_eq!(names.len(), 12);
    assert!(names.contains(&"R") && names.contains(&"A") && names.contains(&"Red.R") && names.contains(&"Blue_Box.B"), "{names:?}");
    let img = read().no_deep_data().largest_resolution_level().all_channels().all_layers().all_attributes().from_file(&path).unwrap();
    let layer = &img.layer_data[0];
    assert_eq!((layer.size.0, layer.size.1), (32, 32));
    let ch = |n: &str| layer.channel_data.list.iter().find(|c| c.name.to_string() == n).unwrap_or_else(|| panic!("no channel {n}"));
    let at = |n: &str, x: usize, y: usize| match &ch(n).sample_data {
        FlatSamples::F32(v) => v[y * 32 + x],
        o => panic!("{o:?}"),
    };
    // The blue layer alone: blue inside its box, transparent outside.
    assert!((at("Blue_Box.B", 16, 16) - 1.0).abs() < 1e-3);
    assert_eq!(at("Blue_Box.A", 0, 0), 0.0);
    assert!((at("Red.R", 0, 0) - 1.0).abs() < 1e-3);
    // Composite: opaque everywhere.
    assert!((at("A", 16, 16) - 1.0).abs() < 1e-3);
    assert!(at("R", 16, 16) < 0.5, "multiplied by 50% blue (linear)");
}

#[test]
fn proexr_names_a_non_ascii_layer_with_underscores_instead_of_panicking() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Frame", "width": 8, "height": 8, "frameRate": 24, "duration": 1})).unwrap();
    s.execute("layer.newSolid", json!({"name": "你好", "color": "#ff0000"})).unwrap();
    let dir = out_dir("frame-exr-non-ascii");
    let path = dir.join("frame.exr").to_string_lossy().to_string();
    let r = s.execute("comp.saveFrameAsExr", json!({"path": path})).unwrap();
    let names: Vec<&str> = r["channels"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
    assert!(names.contains(&"__.R") && names.contains(&"__.A"), "{names:?}");
    assert!(std::path::Path::new(&path).exists());
}

/// Save a 32×32 comp holding one opaque solid of `color` (an sRGB-encoded triple) as ProEXR and
/// return the centre pixel of the composite and of the solid's own layer: `[R, G, B]` each.
fn proexr_centre(color: [f64; 3], bit_depth: u64) -> ([f32; 3], [f32; 3]) {
    use exr::prelude::*;
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Bright", "width": 32, "height": 32, "frameRate": 24, "duration": 1})).unwrap();
    s.execute("file.projectSettings", json!({"bitDepth": bit_depth})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Bright", "color": color})).unwrap();
    let dir = out_dir(&format!("frame-exr-over-{}", color[0]));
    let path = dir.join(format!("frame{bit_depth}.exr")).to_string_lossy().to_string();
    s.execute("comp.saveFrameAsExr", json!({"path": path})).unwrap();
    let img = read().no_deep_data().largest_resolution_level().all_channels().all_layers().all_attributes().from_file(&path).unwrap();
    let layer = &img.layer_data[0];
    let at = |n: &str| {
        let c = layer.channel_data.list.iter().find(|c| c.name.to_string() == n).unwrap_or_else(|| panic!("no channel {n}"));
        match &c.sample_data {
            FlatSamples::F32(v) => v[16 * 32 + 16],
            o => panic!("{o:?}"),
        }
    };
    ([at("R"), at("G"), at("B")], [at("Bright.R"), at("Bright.G"), at("Bright.B")])
}

#[test]
fn proexr_keeps_values_above_one_in_32_bpc() {
    // Issue #339: over-range colour reaches the EXR in scene-linear light (sRGB decode, uncapped).
    let src = [1.5, 1.25, 0.75];
    let (comp, layer) = proexr_centre(src, 32);
    for i in 0..3 {
        let want = effectcraft_color::srgb_to_linear(src[i] as f32);
        for got in [comp[i], layer[i]] {
            assert!((got - want).abs() < 1e-3 * want.max(1.0), "channel {i}: got {got}, want {want} ({comp:?} / {layer:?})");
        }
    }
    assert!(comp[0] > 1.5 && comp[1] > 1.25, "{comp:?}");
}

#[test]
fn proexr_values_at_or_below_one_are_unchanged() {
    let (comp, layer) = proexr_centre([1.0, 0.4, 0.0], 32);
    for got in [comp, layer] {
        assert!((got[0] - 1.0).abs() < 1e-4 && (got[1] - 0.1329).abs() < 1e-3 && got[2].abs() < 1e-6, "{got:?}");
    }
}

/// Composition ▸ Copy Frame to Clipboard (After Effects 26.3): the frame, as Save Frame As
/// renders it, goes to the frontend as an image for the system clipboard.
#[test]
fn copy_frame_hands_the_rendered_frame_to_the_frontend() {
    let mut s = frame();
    s.drain_events();
    let r = s.execute("comp.copyFrame", json!({})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(32), Some(32)));
    let events = s.drain_events();
    let Some((width, height, rgba)) = events.iter().find_map(|e| match e {
        crate::Event::CopyImage { width, height, rgba } => Some((*width, *height, rgba)),
        _ => None,
    }) else {
        panic!("no CopyImage event in {events:?}")
    };
    assert_eq!((width, height, rgba.len()), (32, 32, 32 * 32 * 4));
    // Red in the corner; red × (50% blue multiply) in the middle; opaque everywhere.
    assert_eq!(&rgba[..4], &[255, 0, 0, 255]);
    let mid = &rgba[(16 * 32 + 16) * 4..][..4];
    assert!(mid[0] > 100 && mid[0] < 160 && mid[2] < 13 && mid[3] == 255, "{mid:?}");
    assert!(rgba.chunks(4).all(|p| p[3] == 255));
    // The pixels are not serialized into the event stream.
    let json = serde_json::to_value(events.iter().find(|e| matches!(e, crate::Event::CopyImage { .. })).unwrap()).unwrap();
    assert!(json["CopyImage"].get("rgba").is_none(), "{json}");
    // Scaled, and refused without a composition.
    assert_eq!(s.execute("comp.copyFrame", json!({"scale": 0.5})).unwrap()["width"], 16);
    assert!(Session::default().execute("comp.copyFrame", json!({})).is_err());
    assert!(crate::menus::entries().iter().any(|(path, e)| e.command == "comp.copyFrame" && path.first().map(String::as_str) == Some("Composition")));
}

#[test]
fn save_frame_as_png_preserves_alpha_channel() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "AlphaComp", "width": 32, "height": 32, "frameRate": 24, "duration": 1})).unwrap();
    let red = s.execute("layer.newSolid", json!({"name": "Red", "color": "#ff0000", "width": 16, "height": 16})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": red, "path": "transform/position", "value": [8, 8, 0]})).unwrap();
    let dir = out_dir("frame-png-alpha");
    let path = dir.join("alpha.png").to_string_lossy().to_string();

    s.execute("comp.saveFrameAs", json!({"path": path})).unwrap();
    let img = image::load_from_memory(&std::fs::read(&path).unwrap()).unwrap().to_rgba8();
    assert_eq!((img.width(), img.height()), (32, 32));
    let solid_px = img.get_pixel(8, 8);
    assert_eq!(solid_px[3], 255, "solid region must be opaque: {solid_px:?}");
    assert_eq!(solid_px[0], 255);
    let empty_px = img.get_pixel(24, 24);
    assert_eq!(empty_px[3], 0, "outside solid region must have alpha 0: {empty_px:?}");

    let flat_path = dir.join("flat.png").to_string_lossy().to_string();
    s.execute_checked("comp.saveFrameAs", json!({"path": flat_path, "transparent": false})).unwrap();
    let flat_img = image::load_from_memory(&std::fs::read(&flat_path).unwrap()).unwrap().to_rgba8();
    let flat_empty_px = flat_img.get_pixel(24, 24);
    assert_eq!(flat_empty_px[3], 255, "transparent: false must composite over background: {flat_empty_px:?}");
}

#[test]
fn save_frame_as_queued_respects_frame_default_template() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Comp", "width": 32, "height": 32, "frameRate": 24, "duration": 1})).unwrap();

    // Default Still template is "PNG Sequence" (Channels::Rgb)
    let r = s.execute_checked("comp.saveFrameAs", json!({"queue": true})).unwrap();
    assert_eq!(r["queued"], true);
    let it = s.project.render_queue.last().unwrap();
    assert_eq!(it.output.name, "PNG Sequence");
    assert_eq!(it.output.channels, effectcraft_project::render_queue::Channels::Rgb);

    // Test undo and redo of queuing a frame
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.render_queue.is_empty());
    s.execute("edit.redo", json!({})).unwrap();
    let it = s.project.render_queue.last().unwrap();
    assert_eq!(it.output.name, "PNG Sequence");

    // Explicit transparent: false forces Channels::Rgb
    s.execute_checked("comp.saveFrameAs", json!({"queue": true, "transparent": false})).unwrap();
    let it = s.project.render_queue.last().unwrap();
    assert_eq!(it.output.channels, effectcraft_project::render_queue::Channels::Rgb);

    // When Frame Default is set to an RGBA template, it is respected
    s.execute_checked("renderQueue.setTemplateDefault", json!({"kind": "outputModule", "slot": "still", "name": "PNG Sequence with Alpha"})).unwrap();
    s.execute_checked("comp.saveFrameAs", json!({"queue": true})).unwrap();
    let it = s.project.render_queue.last().unwrap();
    assert_eq!(it.output.name, "PNG Sequence with Alpha");
    assert_eq!(it.output.channels, effectcraft_project::render_queue::Channels::Rgba);

    // Passing transparent: false forces Channels::Rgb even with an RGBA template
    s.execute_checked("comp.saveFrameAs", json!({"queue": true, "transparent": false})).unwrap();
    let it = s.project.render_queue.last().unwrap();
    assert_eq!(it.output.channels, effectcraft_project::render_queue::Channels::Rgb);

    // With JPEG Sequence as Frame Default (non-alpha format), queue succeeds without error
    s.execute_checked("renderQueue.setTemplateDefault", json!({"kind": "outputModule", "slot": "still", "name": "JPEG Sequence"})).unwrap();
    assert!(s.execute_checked("comp.saveFrameAs", json!({"queue": true})).is_ok());
}
