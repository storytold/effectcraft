//! `audio.analyze`: levels and loudness of a known sine tone, silences, layers and bad input.

use serde_json::json;

use crate::Session;
use crate::tests_layout::{comp, solid};

/// A file in this test run's temporary folder.
fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ec-audio-analysis-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// A comp with a solid making a `freq` Hz sine at `level` % (Tone with one frequency).
fn tone(freq: f64, level: f64) -> (Session, u64) {
    let mut s = comp();
    let l = solid(&mut s, "Tone", 10, 10, "#000000");
    s.execute("effect.apply", json!({"layers": [l], "effect": "Tone"})).unwrap();
    for (k, v) in [("frequency1", freq), ("frequency2", 0.0), ("frequency3", 0.0), ("frequency4", 0.0), ("frequency5", 0.0), ("level", level)] {
        s.execute("prop.set", json!({"layer": l, "path": format!("effects/#1/{k}"), "value": v})).unwrap();
    }
    (s, l)
}

#[test]
fn audio_of_a_sine_tone() {
    let (mut s, l) = tone(1000.0, 50.0);
    let r = s.execute("audio.analyze", json!({})).unwrap();
    assert_eq!(r["hasAudio"], true, "{r}");
    let near = |k: &str, want: f64, tol: f64| {
        let got = r[k].as_f64().unwrap_or_else(|| panic!("{k}: {r}"));
        assert!((got - want).abs() <= tol, "{k}: {got} vs {want}");
    };
    // Amplitude 0.5 on both channels: peak −6.02 dBFS, RMS −9.03 dBFS; a 1 kHz stereo sine at
    // amplitude A reads 20·log10(A) LUFS (BS.1770: 0 dBFS in one channel = −3.01 LKFS).
    near("peakDbfs", -6.02, 0.02);
    near("rmsDbfs", -9.03, 0.02);
    near("integratedLufs", -6.02, 0.15);
    near("momentaryMaxLufs", -6.02, 0.15);
    near("truePeakDbtp", -6.02, 0.1);
    assert_eq!(r["clippedSamples"], 0);
    assert_eq!(r["silences"], json!([]));
    assert_eq!(r["channels"][1]["channel"], "R");

    // Trim the layer to 1 s: the second second is silent. The absolute gate drops the silent
    // blocks, so loudness is not halved; only the blocks straddling the cut pull it down a little.
    s.execute("layer.timing", json!({"layers": [l], "out": 1.0})).unwrap();
    let wf = tmp("wave.png");
    let r = s.execute("audio.analyze", json!({"waveform": true, "width": 100, "height": 40, "path": wf.to_string_lossy()})).unwrap();
    let sil = &r["silences"][0];
    assert!((sil["start"].as_f64().unwrap() - 1.0).abs() < 0.011 && sil["end"] == 2.0, "{r}");
    let lufs = r["integratedLufs"].as_f64().unwrap();
    assert!((-7.0..-6.0).contains(&lufs), "{r}");
    assert!((r["rmsDbfs"].as_f64().unwrap() + 12.04).abs() < 0.05, "{r}");
    let cols = r["waveform"]["columns"].as_array().unwrap();
    assert_eq!(cols.len(), 100);
    assert!((cols[10][1].as_f64().unwrap() - 0.5).abs() < 0.01 && cols[90] == json!([0.0, 0.0, 0.0, 0.0]), "{}", cols[10]);
    let img = image::open(&wf).unwrap();
    assert_eq!((img.width(), img.height()), (100, 40));

    // A range: only the silent part.
    let r = s.execute("audio.analyze", json!({"start": 1.2, "end": 1.8, "truePeak": false})).unwrap();
    assert_eq!(r["silent"], true);
    assert!(r["peakDbfs"].is_null() && r["integratedLufs"].is_null() && r["truePeakDbtp"].is_null(), "{r}");

    // Clipping: level 150 % → amplitude 1.5.
    let (mut s, _) = tone(1000.0, 150.0);
    let r = s.execute("audio.analyze", json!({"end": 0.5})).unwrap();
    assert!(r["clippedSamples"].as_u64().unwrap() > 1000, "{r}");
}

#[test]
fn audio_of_layers_and_bad_input() {
    let (mut s, l) = tone(440.0, 20.0);
    let quiet = solid(&mut s, "Silent", 10, 10, "#ffffff");
    // One layer alone.
    let r = s.execute("audio.analyze", json!({"layer": l, "end": 0.5})).unwrap();
    assert_eq!(r["hasAudio"], true);
    let r = s.execute("audio.analyze", json!({"layer": quiet})).unwrap();
    assert_eq!(r["hasAudio"], false, "{r}");
    // A comp with no audio.
    let mut s2 = comp();
    solid(&mut s2, "S", 10, 10, "#ffffff");
    let r = s2.execute("audio.analyze", json!({})).unwrap();
    assert_eq!(r["hasAudio"], false);
    assert!(r["why"].as_str().unwrap().contains("no audible"));
    for p in [
        json!({"start": 1.0, "end": 1.0}),
        json!({"start": 1.0, "end": 0.5}),
        json!({"end": 100000.0}),
        json!({"layer": "Nope"}),
        json!({"silenceDb": 10}),
        json!({"minSilence": -1}),
        json!({"waveform": true, "width": 0}),
        json!({"comp": 999}),
    ] {
        assert!(s.execute("audio.analyze", p.clone()).is_err(), "{p}");
    }
}

#[test]
fn loudness_gating_follows_bs1770() {
    use crate::audio_analysis::integrated;
    // Equal blocks: the mean.
    let p = 10f64.powf((-20.0 + 0.691) / 10.0);
    assert!((integrated(&[p; 10]).unwrap() + 20.0).abs() < 1e-9);
    // Blocks under −70 LKFS and more than 10 LU below the rest are gated out.
    let quiet = 10f64.powf((-40.0 + 0.691) / 10.0);
    let mut v = vec![p; 10];
    v.extend([quiet; 10]);
    v.extend([0.0; 5]);
    assert!((integrated(&v).unwrap() + 20.0).abs() < 1e-9);
    assert_eq!(integrated(&[]), None);
    assert_eq!(integrated(&[1e-12; 4]), None);
}
