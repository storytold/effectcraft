//! Targa import through the same host and commands used by the desktop and CLI.

use effectcraft_engine::render::RenderOpts;
use effectcraft_project::{FootageKind, ItemId, ItemKind};
use effectcraft_time::{FrameRate, Tick};
use serde_json::json;

#[test]
fn tga_stills_and_numbered_sequences_import_and_render() {
    let dir = std::env::temp_dir().join(format!("effectcraft-tga-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A generated 4×4, 24-bit, top-origin TGA. No binary fixtures are retained.
    let write = |name: &str, bgr: [u8; 3]| {
        let mut bytes = vec![0; 18];
        bytes[2] = 2;
        bytes[12] = 4;
        bytes[14] = 4;
        bytes[16] = 24;
        bytes[17] = 0x20;
        bytes.extend_from_slice(&bgr.repeat(16));
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().to_string()
    };
    let still = write("still.TGA", [0, 0, 255]);
    let first = write("shot_0001.tga", [0, 0, 255]);
    write("shot_0002.tga", [255, 0, 0]);
    for (path, sequence) in [(still, false), (first, true)] {
        let mut session = effectcraft_host::session();
        let imported = session.execute_checked("file.import", json!({"paths": [path], "sequence": sequence})).unwrap();
        assert_eq!(imported["errors"], json!([]), "{imported}");
        assert_eq!(imported["items"].as_array().unwrap().len(), 1);
        let item = ItemId(imported["items"][0].as_u64().unwrap());
        let ItemKind::Footage(footage) = &session.project.item(item).unwrap().kind else { panic!("not footage") };
        assert_eq!(footage.kind, if sequence { FootageKind::Sequence } else { FootageKind::Still });
        assert_eq!(footage.sequence.len(), if sequence { 2 } else { 0 });
        session.execute_checked("comp.new", json!({"name": "TGA", "width": 4, "height": 4, "frameRate": 30, "duration": 1})).unwrap();
        session.execute_checked("layer.addItem", json!({"item": item.0})).unwrap();
        let comp = session.active_comp_id().unwrap();
        let image = session.render(comp, Tick::ZERO, RenderOpts::default());
        assert_eq!(image.get(2, 2), [1.0, 0.0, 0.0, 1.0]);
        if sequence {
            let image = session.render(comp, FrameRate::FPS_30.tick_of(1), RenderOpts::default());
            assert_eq!(image.get(2, 2), [0.0, 0.0, 1.0, 1.0]);
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}
