//! A still in a file type EffectCraft does not know imports through a `format` plug-in. Real host,
//! with the raw RGB reader in `tests/fixtures/raw-rgb.wat`. The importer is the one
//! `effectcraft-host` uses: `effectcraft_media::probe`.

mod common;

use std::sync::Arc;

use common::{RAW_RGB, install, raw_rgb_package, rig};
use effectcraft_engine::Importer;
use effectcraft_media::MediaPool;
use effectcraft_project::{Footage, ItemKind};
use effectcraft_time::Tick;
use serde_json::json;

struct MediaImporter;

impl Importer for MediaImporter {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        effectcraft_media::probe(path).map_err(|e| e.to_string())
    }
}

fn rgb_file(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
    let mut f = b"RGB8".to_vec();
    f.extend_from_slice(&w.to_le_bytes());
    f.extend_from_slice(&h.to_le_bytes());
    f.extend_from_slice(rgb);
    f
}

#[test]
fn an_unknown_extension_imports_through_the_plugin_and_renders_its_pixels() {
    let dir = std::env::temp_dir().join(format!("ec-formats-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pic.rgbraw");
    std::fs::write(&file, rgb_file(2, 1, &[255, 0, 0, 0, 0, 255])).unwrap();
    let path = file.to_string_lossy().to_string();

    let (mut session, host, _comp) = rig();
    session.importer = Some(Arc::new(MediaImporter));
    let before = session.execute("file.import", json!({"paths": [&path]})).unwrap();
    assert!(
        before["items"].as_array().unwrap().is_empty() && !before["errors"].as_array().unwrap().is_empty(),
        "EffectCraft alone cannot open .rgbraw: {before}"
    );

    install(&host, &raw_rgb_package());
    assert!(host.surfaces.formats.handles("rgbraw"));
    let r = session.execute("file.import", json!({"paths": [&path]})).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    let id = effectcraft_project::ItemId(r["items"][0].as_u64().unwrap());
    let Some(ItemKind::Footage(footage)) = session.project.item(id).map(|i| i.kind.clone()) else { panic!("not footage") };
    assert_eq!((footage.width, footage.height), (2, 1));

    let img = MediaPool::new().frame_at(&footage, Tick::ZERO).unwrap();
    assert_eq!((img.width, img.height), (2, 1));
    let [r0, g0, b0, a0] = img.data[0];
    let [r1, g1, b1, a1] = img.data[1];
    assert!(r0 > 0.9 && g0 < 0.1 && b0 < 0.1 && a0 > 0.9, "left pixel is red: {:?}", img.data[0]);
    assert!(r1 < 0.1 && g1 < 0.1 && b1 > 0.9 && a1 > 0.9, "right pixel is blue: {:?}", img.data[1]);

    // A file the plug-in rejects reports its reason. Removing the plug-in takes the format away.
    let bad = dir.join("bad.rgbraw");
    std::fs::write(&bad, b"nope").unwrap();
    let err = effectcraft_media::probe(&bad).unwrap_err().to_string();
    assert!(err.contains("12 byte header"), "{err}");
    host.manager.remove(RAW_RGB).unwrap();
    assert!(!host.surfaces.formats.handles("rgbraw"));
    assert!(effectcraft_media::probe(&file).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
