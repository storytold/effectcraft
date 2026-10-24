//! `file.import {alpha}`: interpret unlabeled alpha while importing, instead of Settings ▸ Import ▸
//! Interpret Unlabeled Alpha As (whose Ask User default opens Interpret Footage, a modal form in
//! the desktop app), through a synthetic importer and footage source.

use std::sync::Arc;

use effectcraft_project::{AlphaMode, Footage, FootageKind, ItemId, ItemKind};
use effectcraft_raster::Image;
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use crate::Session;
use crate::render::FootageSource;

/// `key.*`: a semi-transparent green premultiplied against black (TGA and ProRes 4444 don't say
/// how their alpha is stored); `opaque.*`: white without partial alpha; `.png` states its alpha.
struct Synthetic;

impl FootageSource for Synthetic {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        let name = std::path::Path::new(&f.path).file_stem().and_then(|n| n.to_str()).unwrap_or("");
        // The straight interpretation of the file's pixels (green·a + black·(1 − a), a = 0.5).
        let p = if name == "key" { [0.0, 0.5, 0.0, 0.5] } else { [1.0, 1.0, 1.0, 1.0] };
        let mut img = Image::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                // Decoders hand out premultiplied pixels.
                img.set(x, y, [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]]);
            }
        }
        Some(Arc::new(img))
    }
}

struct Probe;

impl crate::Importer for Probe {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        let video = path.ends_with(".mov");
        Ok(Footage {
            path: path.into(),
            kind: if video { FootageKind::Video } else { FootageKind::Still },
            width: 4,
            height: 4,
            has_video: true,
            frame_rate: FrameRate::FPS_30,
            duration: Tick::from_seconds_f64(if video { 2.0 } else { 0.0 }),
            codec: if video { "ProRes 4444".into() } else { "TGA".into() },
            ..Default::default()
        })
    }
}

fn session() -> Session {
    Session { footage: Arc::new(Synthetic), importer: Some(Arc::new(Probe)), ..Default::default() }
}

fn footage(s: &Session, id: &Value) -> Footage {
    match &s.project.item(ItemId(id.as_u64().unwrap())).unwrap().kind {
        ItemKind::Footage(f) => f.clone(),
        _ => panic!("not footage"),
    }
}

fn asks(s: &mut Session) -> Vec<Value> {
    s.drain_events()
        .into_iter()
        .filter_map(|e| match e {
            crate::Event::Frontend { command, params } if command == "file.interpretFootage" => Some(params),
            _ => None,
        })
        .collect()
}

/// Without `alpha`, Ask User (the default) still asks, and the reply says which items it asked about.
#[test]
fn import_asks_about_unlabeled_alpha_by_default() {
    let mut s = session();
    assert_eq!(s.prefs.unlabeled_alpha(false), None, "Ask User");
    let r = s.execute("file.import", json!({"paths": ["/m/key.tga", "/m/key.png"]})).unwrap();
    let (tga, png) = (r["items"][0].clone(), r["items"][1].clone());
    assert_eq!(r["unlabeledAlpha"], json!([tga]), "a PNG states its alpha");
    assert_eq!(r["askedAlpha"], json!([tga]));
    assert_eq!(r["alphaInterpretation"], json!([{"item": tga, "alpha": "Straight", "matteColor": [0.0, 0.0, 0.0], "from": "ask"}]));
    assert_eq!(asks(&mut s), [json!({"item": tga, "reason": "unlabeledAlpha"})]);
    assert_eq!(footage(&s, &png).alpha, AlphaMode::Straight);
}

/// `alpha: straight|premultiplied|ignore` interprets stills and movies with unlabeled alpha and
/// never asks; files that state their alpha keep it.
#[test]
fn import_alpha_param_interprets_unlabeled_alpha_without_asking() {
    for (param, want) in [("straight", AlphaMode::Straight), ("premultiplied", AlphaMode::Premultiplied), ("ignore", AlphaMode::Ignore)] {
        let mut s = session();
        let r = s.execute("file.import", json!({"paths": ["/m/key.tga", "/m/key.mov", "/m/key.png"], "alpha": param})).unwrap();
        let ids = r["items"].as_array().unwrap().clone();
        assert_eq!(r["unlabeledAlpha"], json!([ids[0], ids[1]]), "{param}");
        assert_eq!(r["askedAlpha"], json!([]), "{param}");
        assert!(asks(&mut s).is_empty(), "{param}: no Interpret Footage form");
        for (i, row) in r["alphaInterpretation"].as_array().unwrap().iter().enumerate() {
            assert_eq!((&row["item"], &row["from"]), (&ids[i], &json!("alpha")), "{param}");
            assert_eq!(row["alpha"], json!(format!("{want:?}")), "{param}");
        }
        assert_eq!(footage(&s, &ids[0]).alpha, want, "{param}: still");
        assert_eq!(footage(&s, &ids[1]).alpha, want, "{param}: movie");
        assert_eq!(footage(&s, &ids[2]).alpha, AlphaMode::Straight, "{param}: the PNG keeps its own alpha");
    }
}

/// `alpha: guess` is Interpret Footage ▸ Guess from the first frame (with its matte colour); a
/// frame without partial alpha falls back to Settings ▸ Import's Guess. Still one undo step.
#[test]
fn import_alpha_guess_reads_the_first_frame() {
    let mut s = session();
    let undo = s.history.undo.len();
    let r = s.execute("file.import", json!({"paths": ["/m/key.tga", "/m/opaque.tga", "/m/opaque.mov"], "alpha": "guess"})).unwrap();
    let ids = r["items"].as_array().unwrap().clone();
    assert!(asks(&mut s).is_empty());
    let key = footage(&s, &ids[0]);
    assert_eq!((key.alpha, key.premul_color), (AlphaMode::Premultiplied, [0.0; 3]), "premultiplied against black");
    assert_eq!(footage(&s, &ids[1]).alpha, AlphaMode::Straight, "a still, as Settings ▸ Import ▸ Guess");
    assert_eq!(footage(&s, &ids[2]).alpha, AlphaMode::Premultiplied, "a movie, as Settings ▸ Import ▸ Guess");
    assert_eq!(r["alphaInterpretation"][0]["alpha"], json!("Premultiplied"));
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    assert!(s.undo());
    assert!(ids.iter().all(|i| s.project.item(ItemId(i.as_u64().unwrap())).is_none()), "undo removes the import");
}

/// `alpha` wins over Settings ▸ Import; without it the setting applies and the reply says so.
#[test]
fn import_alpha_param_overrides_settings() {
    let mut s = session();
    s.prefs.import.unlabeled_alpha = "straight".into();
    let r = s.execute("file.import", json!({"paths": ["/m/key.mov"]})).unwrap();
    assert_eq!(r["alphaInterpretation"][0]["from"], json!("settings"));
    assert_eq!(footage(&s, &r["items"][0]).alpha, AlphaMode::Straight);
    let r = s.execute("file.import", json!({"paths": ["/m/key.mov"], "alpha": "premultiplied"})).unwrap();
    assert_eq!(r["alphaInterpretation"][0]["from"], json!("alpha"));
    assert_eq!(footage(&s, &r["items"][0]).alpha, AlphaMode::Premultiplied);
    assert!(asks(&mut s).is_empty());
}

/// Background imports apply `alpha` too (the guess runs when the job's result is added).
#[test]
fn background_import_applies_alpha() {
    let mut s = session();
    s.execute("file.import", json!({"paths": ["/m/key.tga"], "alpha": "guess", "background": true})).unwrap();
    s.execute("jobs.wait", json!({})).unwrap();
    assert!(asks(&mut s).is_empty());
    let f = s.project.items.values().find_map(|it| match &it.kind {
        ItemKind::Footage(f) if f.path == "/m/key.tga" => Some(f.clone()),
        _ => None,
    });
    assert_eq!(f.map(|f| f.alpha), Some(AlphaMode::Premultiplied));
}

/// A bad `alpha` is an error naming the choices, and nothing is imported.
#[test]
fn import_rejects_an_unknown_alpha() {
    let mut s = session();
    let before = s.project.items.len();
    for bad in [json!("auto"), json!(1), json!(true)] {
        let e = s.execute("file.import", json!({"paths": ["/m/key.tga"], "alpha": bad})).unwrap_err().to_string();
        assert!(e.contains("straight|premultiplied|ignore|guess"), "{e}");
    }
    assert_eq!(s.project.items.len(), before);
    assert!(asks(&mut s).is_empty());
}
