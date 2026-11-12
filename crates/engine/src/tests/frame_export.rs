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

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// A 32×32 comp in a managed project (sRGB working space, linearized): a full-frame over-range
/// solid under a 16×16 in-range Multiply solid at 50%.
fn hdr_frame(bit_depth: u64, hdr: &str) -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Frame", "width": 32, "height": 32, "frameRate": 24, "duration": 1})).unwrap();
    s.execute("file.projectSettings", json!({"bitDepth": bit_depth, "workingSpace": "srgb", "linearize": true, "hdr": hdr})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Bright", "color": [4.0, 2.0, 0.5]})).unwrap();
    let box_ = s.execute("layer.newSolid", json!({"name": "Box", "color": [0.85, 0.9, 0.2], "width": 16, "height": 16})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setBlendMode", json!({"layers": [box_], "mode": "Multiply"})).unwrap();
    s.execute("prop.set", json!({"layer": box_, "path": "transform/opacity", "value": 50})).unwrap();
    s
}

/// Photoshop Layers keeps the HDR step (it is a display-referred output): its bytes for each bit
/// depth × HDR mode.
#[test]
fn photoshop_layers_bytes_are_unchanged() {
    let dir = out_dir("frame-psd-golden");
    let mut got = vec![];
    for depth in [8, 16, 32] {
        for hdr in ["clip", "compand", "toneMap"] {
            let mut s = hdr_frame(depth, hdr);
            let path = dir.join(format!("{depth}-{hdr}.psd")).to_string_lossy().to_string();
            s.execute("comp.saveFrameAsPsd", json!({"path": path})).unwrap();
            got.push(fnv(&std::fs::read(&path).unwrap()));
        }
    }
    // The hashes before ProEXR skipped the HDR step.
    let want: [u64; 9] = [
        0x3b921d4d9fc3119f,
        0x1886d026edf36a7f,
        0x6b564aa7fb50139f,
        0xabd585992079fc9f,
        0x88ca38726eaa557f,
        0xa3636606d7a4927f,
        0x7385ed211e9815ff,
        0x7385ed211e9815ff,
        0xfb59fa3f3bdfac3f,
    ];
    assert_eq!(got, want);
}

/// Every sample of a ProEXR file, channels in name order.
fn proexr_samples(path: &str) -> Vec<(String, Vec<f32>)> {
    use exr::prelude::*;
    let img = read().no_deep_data().largest_resolution_level().all_channels().all_layers().all_attributes().from_file(path).unwrap();
    let mut out: Vec<(String, Vec<f32>)> = img.layer_data[0]
        .channel_data
        .list
        .iter()
        .map(|c| match &c.sample_data {
            FlatSamples::F32(v) => (c.name.to_string(), v.clone()),
            o => panic!("{o:?}"),
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// ProEXR decodes with the output space's own curve. A Rec. 709 output has the sRGB working
/// space's primaries but another curve, so the composite and the isolated layer must hold the
/// solid's light (its sRGB working value decoded), not the sRGB decode of its Rec. 709 encoding.
#[test]
fn proexr_decodes_a_non_srgb_output_curve() {
    let dir = out_dir("frame-exr-rec709");
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Frame", "width": 32, "height": 32, "frameRate": 24, "duration": 1})).unwrap();
    s.execute("file.projectSettings", json!({"bitDepth": 32, "workingSpace": "srgb", "linearize": true, "outputSpace": "rec709"})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Warm", "color": [0.8, 0.5, 0.2]})).unwrap();
    let path = dir.join("rec709.exr").to_string_lossy().to_string();
    s.execute("comp.saveFrameAsExr", json!({"path": path})).unwrap();
    let got = proexr_samples(&path);
    let at = |n: &str| got.iter().find(|c| c.0 == n).unwrap_or_else(|| panic!("no {n}")).1[0];
    let lin = effectcraft_color::srgb_to_linear;
    for (n, want) in [("R", lin(0.8)), ("G", lin(0.5)), ("B", lin(0.2)), ("Warm.R", lin(0.8)), ("Warm.B", lin(0.2))] {
        assert!((at(n) - want).abs() <= 1e-3 * want, "{n}: {} vs {want}", at(n));
    }
}

/// ProEXR samples in Clip mode (sRGB output) for each bit depth.
#[test]
fn proexr_clip_samples_are_unchanged() {
    let dir = out_dir("frame-exr-clip-golden");
    let mut got = vec![];
    for depth in [8, 16, 32] {
        let mut s = hdr_frame(depth, "clip");
        let path = dir.join(format!("{depth}.exr")).to_string_lossy().to_string();
        s.execute("comp.saveFrameAsExr", json!({"path": path})).unwrap();
        let bytes: Vec<u8> =
            proexr_samples(&path).iter().flat_map(|(n, v)| n.bytes().chain(v.iter().flat_map(|x| x.to_bits().to_le_bytes())).collect::<Vec<u8>>()).collect();
        got.push(fnv(&bytes));
    }
    // The hashes before ProEXR skipped the HDR step.
    let want: [u64; 3] = [0x4b9ecc87e6db7d03, 0xe06a35d302e71503, 0x694ca552f3efc903];
    assert_eq!(got, want);
}

/// ProEXR ignores compand and tone mapping: the composite and every isolated layer hold the Clip
/// mode's samples, and over-range values stay.
#[test]
fn proexr_layers_are_not_tone_mapped() {
    let dir = out_dir("frame-exr-hdr");
    let samples = |depth: u64, hdr: &str| {
        let mut s = hdr_frame(depth, hdr);
        let path = dir.join(format!("{depth}-{hdr}.exr")).to_string_lossy().to_string();
        s.execute("comp.saveFrameAsExr", json!({"path": path})).unwrap();
        proexr_samples(&path)
    };
    for depth in [8, 16, 32] {
        let clip = samples(depth, "clip");
        assert!(clip.iter().any(|(n, _)| n == "Bright.R") && clip.iter().any(|(n, _)| n == "Box.R"), "{:?}", clip.iter().map(|c| &c.0).collect::<Vec<_>>());
        for hdr in ["compand", "toneMap"] {
            assert!(samples(depth, hdr) == clip, "{depth} bpc {hdr}");
        }
    }
    let tone = samples(32, "toneMap");
    let at = |n: &str| tone.iter().find(|c| c.0 == n).unwrap().1[0];
    // The solid's colour is sRGB-encoded in the working space (4.0 is about 25.3 in linear light).
    let lin = effectcraft_color::srgb_to_linear;
    for (n, want) in [("Bright.R", lin(4.0)), ("Bright.G", lin(2.0)), ("Bright.B", lin(0.5)), ("R", lin(4.0))] {
        assert!((at(n) - want).abs() <= 1e-4 * want, "{n}: {} vs {want}", at(n));
    }
}

/// The `chromaticities` attribute of the ProEXR file `path`: red, green, blue, white (CIE xy).
fn proexr_chromaticities(s: &mut Session, name: &str) -> Option<[[f32; 2]; 4]> {
    let path = out_dir(name).join("frame.exr").to_string_lossy().to_string();
    s.execute("comp.saveFrameAsExr", json!({"path": path})).unwrap();
    let meta = exr::meta::MetaData::read_from_buffered(std::io::BufReader::new(std::fs::File::open(&path).unwrap()), false).unwrap();
    meta.headers[0].shared_attributes.chromaticities.map(|c| [c.red, c.green, c.blue, c.white].map(|v| [v.0, v.1]))
}

/// ProEXR names the primaries and white of the output space (default: Rec. 709 / sRGB).
#[test]
fn proexr_writes_the_output_spaces_chromaticities() {
    let d65 = [0.3127, 0.3290];
    let mut s = frame();
    assert_eq!(proexr_chromaticities(&mut s, "frame-exr-chroma-default"), Some([[0.64, 0.33], [0.30, 0.60], [0.15, 0.06], d65]));
    s.execute("file.projectSettings", json!({"workingSpace": "srgb", "outputSpace": "p3"})).unwrap();
    assert_eq!(proexr_chromaticities(&mut s, "frame-exr-chroma-p3"), Some([[0.680, 0.320], [0.265, 0.690], [0.150, 0.060], d65]));
    s.execute("file.projectSettings", json!({"outputSpace": "rec2020"})).unwrap();
    assert_eq!(proexr_chromaticities(&mut s, "frame-exr-chroma-2020"), Some([[0.708, 0.292], [0.170, 0.797], [0.131, 0.046], d65]));
    s.execute("file.projectSettings", json!({"outputSpace": "rec2100pq"})).unwrap();
    assert_eq!(proexr_chromaticities(&mut s, "frame-exr-chroma-pq"), Some([[0.708, 0.292], [0.170, 0.797], [0.131, 0.046], d65]));
    // Unmanaged: the output space is ignored, the Working Gamma's curve (Rec. 709 primaries) decodes.
    s.execute("file.projectSettings", json!({"workingSpace": "none", "outputSpace": "p3"})).unwrap();
    assert_eq!(proexr_chromaticities(&mut s, "frame-exr-chroma-unmanaged"), Some([[0.64, 0.33], [0.30, 0.60], [0.15, 0.06], d65]));
}
