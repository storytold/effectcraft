//! M13.4: HEVC / AV1 output modules and the WebM codec / Opus options over the command API.

use effectcraft_project::render_queue::{Channels, CodecProfile, OpusApplication, OutputFormat, RateControlMode, VideoCodecOptions, WebmVideoCodec};
use serde_json::json;

use crate::Session;

fn session() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

#[test]
fn hevc_and_av1_output_module_options() {
    let mut s = session();
    let a = s
        .execute(
            "renderQueue.add",
            json!({"format": "hevc", "profile": "main10", "level": "4.1", "rateControl": "quality", "quality": 85, "keyframeInterval": 30}),
        )
        .unwrap();
    assert!(a["outputPath"].as_str().unwrap().ends_with(".mp4"), "{a}");
    let om = &s.project.render_queue[0].output;
    assert_eq!(om.format, OutputFormat::Hevc);
    assert_eq!(om.codec, VideoCodecOptions { profile: CodecProfile::Main10, level: Some(41), rate_control: RateControlMode::Quality, quality: 85 });
    assert_eq!(om.keyframe_interval, 30);
    assert_eq!(a["output"]["codec"]["profile"], "Main10", "{a}");
    let id = a["item"].clone();
    s.execute("renderQueue.setOutputModule", json!({"item": id, "format": "av1", "level": null, "rateControl": "bitrate", "bitrate": 3000})).unwrap();
    let om = &s.project.render_queue[0].output;
    assert_eq!((om.format, om.codec.level, om.codec.rate_control, om.bitrate_kbps), (OutputFormat::Av1, None, RateControlMode::Bitrate, 3000));
    assert!(s.execute("renderQueue.setOutputModule", json!({"item": id, "channels": "rgba"})).is_err(), "AV1 MP4 has no alpha");
    assert!(s.execute("renderQueue.setOutputModule", json!({"item": id, "profile": "high"})).is_err());
    assert!(s.execute("renderQueue.setOutputModule", json!({"item": id, "level": "0.5"})).is_err());
    assert!(s.execute("renderQueue.setOutputModule", json!({"item": id, "rateControl": "cbr2"})).is_err());

    // WebM: VP9 with alpha, then AV1 (alpha dropped) with voice-tuned Opus.
    s.execute("renderQueue.setOutputModule", json!({"item": id, "format": "webm", "channels": "rgba"})).unwrap();
    assert_eq!(s.project.render_queue[0].output.channels, Channels::Rgba);
    s.execute("renderQueue.setOutputModule", json!({"item": id, "webmCodec": "av1", "audioBitrate": 24, "opusApplication": "voice"})).unwrap();
    let om = &s.project.render_queue[0].output;
    assert_eq!((om.webm_codec, om.channels, om.opus_bitrate_kbps, om.opus_application), (WebmVideoCodec::Av1, Channels::Rgb, 24, OpusApplication::Voip));
    assert!(s.execute("renderQueue.setOutputModule", json!({"item": id, "webmCodec": "theora"})).is_err());

    // The formats query lists the new formats and option values.
    let f = s.execute("renderQueue.formats", json!({})).unwrap();
    let ids: Vec<&str> = f["formats"].as_array().unwrap().iter().filter_map(|v| v["id"].as_str()).collect();
    assert!(ids.contains(&"Hevc") && ids.contains(&"Av1"), "{ids:?}");
    assert_eq!(f["codecProfiles"], json!(["Main", "Main 10"]));
    assert!(f["hevcLevels"].as_array().unwrap().contains(&json!("4.1")));
    assert_eq!(f["webmCodecs"], json!(["vp9", "av1"]));
}

/// An explicit format wins over the output file's extension, which follows it (#154:
/// `effectcraft-cli render --format hevc --out x.mp4` wrote H.264). Without a format, the extension
/// still picks one.
#[test]
fn an_explicit_format_wins_over_the_output_extension() {
    let mut s = session();
    let cases = [
        ("hevc", "/out/x.mp4", OutputFormat::Hevc, "/out/x.mp4"),
        ("av1", "/out/x.mp4", OutputFormat::Av1, "/out/x.mp4"),
        ("hevc", "/out/x.mov", OutputFormat::Hevc, "/out/x.mp4"),
        ("av1", "/out/x.webm", OutputFormat::Av1, "/out/x.mp4"),
        ("prores", "/out/x.mp4", OutputFormat::ProRes, "/out/x.mov"),
    ];
    for (k, (format, out, want, path)) in cases.into_iter().enumerate() {
        s.execute("renderQueue.add", json!({"format": format, "output": out})).unwrap();
        let om = &s.project.render_queue[k].output;
        assert_eq!((om.format, om.output.as_str()), (want, path), "{format} → {out}");
        // A second output module of the item behaves the same.
        s.execute("render.addOutputModule", json!({"index": k + 1, "format": format, "output": out})).unwrap();
        let extra = &s.project.render_queue[k].extra_outputs[0];
        assert_eq!((extra.format, extra.output.as_str()), (want, path), "addOutputModule {format} → {out}");
    }
    // No format: the extension picks it, as before. A format the extension agrees with keeps
    // the name as given (the CLI passes the extension as the format).
    s.execute("renderQueue.add", json!({"output": "/out/y.mov"})).unwrap();
    assert_eq!(s.project.render_queue.last().unwrap().output.format, OutputFormat::ProRes);
    s.execute("renderQueue.add", json!({"format": "png", "output": "/out/still.png"})).unwrap();
    let om = &s.project.render_queue.last().unwrap().output;
    assert_eq!((om.format, om.output.as_str()), (OutputFormat::PngSequence, "/out/still.png"));
}

/// Output To only changes the format when the format can't write the new extension: HEVC and AV1
/// write `.mp4` too, so they stay (#179). Other extensions still pick a file type.
#[test]
fn output_to_keeps_a_format_that_writes_the_extension() {
    let mut s = session();
    let cases = [
        ("hevc", "/out/x.mp4", OutputFormat::Hevc, "/out/x.mp4"),
        ("av1", "/out/x.mp4", OutputFormat::Av1, "/out/x.mp4"),
        ("h264", "/out/x.mov", OutputFormat::ProRes, "/out/x.mov"),
        ("prores", "/out/x.mp4", OutputFormat::H264, "/out/x.mp4"),
        ("h264", "/out/x.mp4", OutputFormat::H264, "/out/x.mp4"),
    ];
    for (format, out, want, path) in cases {
        let a = s.execute("renderQueue.add", json!({"format": format})).unwrap();
        let r = s.execute("renderQueue.setOutput", json!({"item": a["item"], "path": out})).unwrap();
        let om = &s.project.render_queue.last().unwrap().output;
        assert_eq!((om.format, om.output.as_str()), (want, path), "{format} → {out}");
        if OutputFormat::from_name(format) == Some(want) {
            assert_eq!(r["outputModuleSummary"], a["outputModuleSummary"], "{format} → {out}: the module is unchanged");
        }
    }
    // The module's own default name (HEVC resolves to `Main.mp4`) changes nothing either.
    let a = s.execute("renderQueue.add", json!({"format": "hevc"})).unwrap();
    let r = s.execute("renderQueue.setOutput", json!({"item": a["item"], "path": a["outputPath"]})).unwrap();
    assert_eq!((&r["output"]["format"], &r["outputModuleSummary"]), (&json!("Hevc"), &a["outputModuleSummary"]), "{r}");
}

/// Output To ends in the format's extension exactly once (#293). A name from a save dialog
/// filtered to projects (`Comp 1.mov.ecproj`; for a PNG sequence every frame got `.png.ecproj`)
/// loses the `.ecproj` and picks the file type from the extension before it; a comp named
/// `Comp 2.mov` renders to `Comp 2.mov`, not `Comp 2.mov.mov`.
#[test]
fn output_to_ends_in_the_format_extension_once() {
    let mut s = session();
    let cases = [
        ("h264", "/out/Comp 1.mov.ecproj", OutputFormat::ProRes, "/out/Comp 1.mov"),
        ("png", "/out/Comp 1_[#####].png.ecproj", OutputFormat::PngSequence, "/out/Comp 1_[#####].png"),
        ("prores", "/out/Comp 2.mov.mov", OutputFormat::ProRes, "/out/Comp 2.mov"),
        ("prores", "/out/render", OutputFormat::ProRes, "/out/render.mov"),
    ];
    for (format, out, want, path) in cases {
        let a = s.execute("renderQueue.add", json!({"format": format})).unwrap();
        let r = s.execute("renderQueue.setOutput", json!({"item": a["item"], "path": out})).unwrap();
        let om = &s.project.render_queue.last().unwrap().output;
        assert_eq!((om.format, om.output.as_str()), (want, path), "{format} → {out}");
        assert!(r["outputPath"].as_str().unwrap().ends_with(&path[5..]), "{r}");
    }
    s.execute("comp.new", json!({"name": "Comp 2.mov", "width": 64, "height": 36, "frameRate": 30, "duration": 1})).unwrap();
    for (format, name) in [("prores", "Comp 2.mov"), ("h264", "Comp 2.mov.mp4"), ("png", "Comp 2.mov_[#####].png")] {
        let a = s.execute("renderQueue.add", json!({"format": format})).unwrap();
        let path = a["outputPath"].as_str().unwrap();
        assert_eq!(std::path::Path::new(path).file_name().unwrap().to_string_lossy(), name, "{format}: {path}");
    }
}

/// An explicit RGB + Alpha request with the AV1 WebM codec (which has no alpha) is refused, not
/// silently rendered opaque (#166). Without `channels`, AV1 WebM renders RGB as before.
#[test]
fn av1_webm_refuses_requested_alpha() {
    let mut s = session();
    let n = s.project.render_queue.len();
    let e = s.execute("renderQueue.add", json!({"format": "webm", "webmCodec": "av1", "channels": "rgba"})).unwrap_err().to_string();
    assert!(e.contains("AV1 WebM has no alpha channel"), "{e}");
    assert_eq!(s.project.render_queue.len(), n, "nothing queued");
    let a = s.execute("renderQueue.add", json!({"format": "webm", "channels": "rgba"})).unwrap();
    let e = s.execute("renderQueue.setOutputModule", json!({"item": a["item"], "webmCodec": "av1", "channels": "rgba"})).unwrap_err().to_string();
    assert!(e.contains("AV1 WebM has no alpha channel"), "{e}");
    let om = &s.project.render_queue.last().unwrap().output;
    assert_eq!((om.webm_codec, om.channels), (WebmVideoCodec::Vp9, Channels::Rgba), "unchanged");
    s.execute("renderQueue.add", json!({"format": "webm", "webmCodec": "av1"})).unwrap();
    let om = &s.project.render_queue.last().unwrap().output;
    assert_eq!((om.webm_codec, om.channels), (WebmVideoCodec::Av1, Channels::Rgb));
}

/// A `comp` that names no composition says so, rather than "no active composition" (#155).
#[test]
fn an_unknown_comp_is_named_in_the_error() {
    let mut s = session();
    for (cmd, p) in [
        ("renderQueue.add", json!({"comp": "Nope", "output": "/out/x.mp4"})),
        ("layer.newSolid", json!({"comp": "Nope"})),
        ("comp.settings", json!({"comp": 99_999, "width": 10})),
    ] {
        let e = s.execute(cmd, p.clone()).unwrap_err().to_string();
        let name = p["comp"].to_string();
        assert!(e.contains(&format!("no composition {name}")), "{cmd}: {e}");
    }
}
