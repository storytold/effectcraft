//! Premiere Pro interop through timeline interchange, with real media: documents written in code
//! with FilmCraft's writers are imported (`file.importTimeline`) and rendered; compositions are
//! exported (`file.exportTimeline`, with a ProRes 4444 pre-render) and re-imported, and both
//! render the same pixels.

use effectcraft_engine::render::RenderOpts;
use effectcraft_interchange::{fc, fc_media, fc_project as fp, fc_time};
use effectcraft_project::{ItemId, LayerSource};
use effectcraft_raster::Image;
use effectcraft_time::Tick;
use serde_json::json;

const W: u32 = 64;
const H: u32 = 36;

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("effectcraft-host-timeline-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

fn png(name: &str, rgb: [u8; 3]) -> String {
    let path = tmp(name);
    image::RgbImage::from_pixel(W, H, image::Rgb(rgb)).save(&path).unwrap();
    path
}

fn px(img: &Image, x: u32, y: u32) -> [f32; 4] {
    img.data[(y * img.width + x) as usize]
}

fn close(a: [f32; 4], b: [f32; 3], tol: f32) -> bool {
    (0..3).all(|c| (a[c] - b[c]).abs() <= tol)
}

fn rate() -> fc_time::FrameRate {
    fc_time::FrameRate { num: 25, den: 1 }
}

fn secs(s: f64) -> fc_time::Tick {
    fc_time::Tick::from_seconds_f64(s)
}

fn still(p: &mut fp::Project, path: &str) -> fp::ItemId {
    let info = fc_media::MediaInfo {
        name: String::new(),
        kind: fc_media::MediaKind::Still,
        duration: secs(10.0),
        video: Some(fc_media::VideoStreamInfo {
            width: W,
            height: H,
            frame_rate: rate(),
            par: (1, 1),
            codec: "png".into(),
            pixel_format: String::new(),
            color: Default::default(),
            has_alpha: false,
            bitrate: None,
        }),
        audio: None,
        container: String::new(),
        start_timecode: None,
        file_size: None,
    };
    let clip = fp::MediaClip {
        media: fp::MediaRef::File { path: path.into() },
        info,
        interpret: Default::default(),
        mark_in: None,
        mark_out: None,
        markers: vec![],
        offline: false,
        proxy: None,
        identity: None,
    };
    let name = path.rsplit('/').next().unwrap().to_string();
    p.add_item(&name, fp::Label::Iris, fp::ItemKind::Media(clip), None)
}

/// V1: red for 2 s; V2: blue at 50 % opacity and 50 % scale; a cross dissolve from red to green
/// at the 2 s cut on V1.
fn document(fmt: fc::Format) -> Vec<u8> {
    let (red, green, blue) = (png("red.png", [255, 0, 0]), png("green.png", [0, 255, 0]), png("blue.png", [0, 0, 255]));
    let mut p = fp::Project::new("Interop");
    let (r, g, b) = (still(&mut p, &red), still(&mut p, &green), still(&mut p, &blue));
    let settings = fp::SequenceSettings { width: W, height: H, frame_rate: rate(), ..Default::default() };
    let seq = p.new_sequence("Interop", settings, 2, 0, None);
    let range = |s, d| fc_time::TimeRange::new(secs(s), secs(d));
    let cr = p.make_track_item(r, fp::TrackKind::Video, secs(0.0), range(0.0, 2.0), rate()).unwrap();
    let cg = p.make_track_item(g, fp::TrackKind::Video, secs(2.0), range(0.0, 2.0), rate()).unwrap();
    let mut cb = p.make_track_item(b, fp::TrackKind::Video, secs(0.0), range(0.0, 1.6), rate()).unwrap();
    cb.effect_mut("motion").unwrap().params.insert("scale".into(), fp::Param::new(fp::ParamValue::Float(50.0)));
    cb.effect_mut("opacity").unwrap().params.insert("opacity".into(), fp::Param::new(fp::ParamValue::Float(50.0)));
    let tr = fp::Transition {
        id: fp::TransitionId(p.alloc_id()),
        effect: fp::find_effect("cross_dissolve").unwrap().instance(),
        start: secs(1.6),
        duration: secs(0.8),
        from: Some(cr.id),
        to: Some(cg.id),
        align: fp::TransitionAlign::CenterAtCut,
        reverse: false,
    };
    let s = p.sequence_mut(seq).unwrap();
    s.video_tracks[0].items = vec![cr, cg];
    s.video_tracks[0].transitions = vec![tr];
    s.video_tracks[1].items = vec![cb];
    fc::export(&p, seq, fmt, &Default::default()).unwrap().0
}

fn render(s: &effectcraft_engine::Session, cid: ItemId, secs: f64) -> Image {
    s.render(cid, Tick::from_seconds_f64(secs), RenderOpts::default())
}

#[test]
fn imported_timelines_render_like_premiere() {
    for (fmt, motion) in [(fc::Format::Fcp7Xml, true), (fc::Format::Otio, false)] {
        let path = tmp(&format!("interop.{}", fmt.extension()));
        std::fs::write(&path, document(fmt)).unwrap();
        let mut s = effectcraft_host::session();
        let r = s.execute("file.importTimeline", json!({"path": path})).unwrap();
        assert!(r["missing"].as_array().unwrap().is_empty(), "{r}");
        let cid = ItemId(r["comps"][0].as_u64().unwrap());
        let img = render(&s, cid, 1.0);
        // Corner: red (V1). Centre: blue at 50 % over red (FCP7 XML carries Motion/Opacity).
        assert!(close(px(&img, 1, 1), [1.0, 0.0, 0.0], 0.02), "{fmt:?} corner {:?}", px(&img, 1, 1));
        if motion {
            assert!(close(px(&img, W / 2, H / 2), [0.5, 0.0, 0.5], 0.06), "{fmt:?} centre {:?}", px(&img, W / 2, H / 2));
        }
        // Mid-dissolve: half red, half green.
        let mid = render(&s, cid, 2.0);
        assert!(close(px(&mid, 1, 1), [0.5, 0.5, 0.0], 0.08), "{fmt:?} dissolve {:?}", px(&mid, 1, 1));
        // After: green.
        assert!(close(px(&render(&s, cid, 3.0), 1, 1), [0.0, 1.0, 0.0], 0.02));
    }
}

#[test]
fn export_prerenders_and_reimports_the_same_pixels() {
    let red = png("x_red.png", [255, 0, 0]);
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"name": "Spot", "width": W, "height": H, "frameRate": 25, "duration": 2})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let item = s.execute("file.import", json!({"paths": [red]})).unwrap()["items"][0].as_u64().unwrap();
    let lr = s.execute("layer.addItem", json!({"item": item})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": lr, "path": "transform/opacity", "value": 60})).unwrap();
    // A small solid: a rendered-only layer (Premiere's colour mattes fill the frame).
    s.execute("layer.newSolid", json!({"color": "#00ffff", "width": 16, "height": 12})).unwrap();
    let out = tmp("Spot.xml");
    let r = s.execute("file.exportTimeline", json!({"path": out})).unwrap();
    assert_eq!(r["format"], "xml");
    let pre = r["prerendered"].as_array().unwrap();
    assert_eq!(pre.len(), 1, "{r}");
    assert!(pre[0].as_str().unwrap().ends_with(".mov") && std::path::Path::new(pre[0].as_str().unwrap()).exists());
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("<xmeml") && text.contains(".mov"));

    let r2 = s.execute("file.importTimeline", json!({"path": out})).unwrap();
    assert!(r2["missing"].as_array().unwrap().is_empty(), "{r2}");
    let back = ItemId(r2["comps"][0].as_u64().unwrap());
    let c = s.project.comp(back).unwrap();
    assert_eq!(c.layers.len(), 2);
    assert!(c.layers.iter().all(|l| matches!(l.source, LayerSource::Footage { .. })));
    for t in [0.0, 1.0] {
        let (a, b) = (render(&s, cid, t), render(&s, back, t));
        for (x, y) in [(1, 1), (W / 2, H / 2), (W - 2, H - 2), (W / 2 - 9, H / 2)] {
            let (pa, pb) = (px(&a, x, y), px(&b, x, y));
            assert!(close(pb, [pa[0], pa[1], pa[2]], 0.03), "t={t} ({x},{y}) {pa:?} vs {pb:?}");
        }
    }
    // Native .prproj is refused with a pointer to the XML route.
    let e = s.execute("file.importTimeline", json!({"path": "/x/a.prproj"})).unwrap_err().to_string();
    assert!(e.contains("Final Cut Pro XML"), "{e}");
}
