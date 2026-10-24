//! End-to-end integration test for opening and rendering an After Effects project (.aep).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

fn make_synthetic_aep() -> Vec<u8> {
    let mut buf = Vec::new();
    let mut comp_item_payload = Vec::new();

    let name_bytes = b"Main Comp\0";
    comp_item_payload.extend_from_slice(b"Utf8");
    comp_item_payload.extend_from_slice(&(name_bytes.len() as u32).to_be_bytes());
    comp_item_payload.extend_from_slice(name_bytes);
    if name_bytes.len() % 2 != 0 {
        comp_item_payload.push(0);
    }

    let mut cdta_bytes = vec![0u8; 180];
    cdta_bytes[10] = 0x78;
    cdta_bytes[11] = 0x00;
    cdta_bytes[140] = 0x07;
    cdta_bytes[141] = 0x80; // 1920
    cdta_bytes[142] = 0x04;
    cdta_bytes[143] = 0x38; // 1080
    cdta_bytes[156] = 0x00;
    cdta_bytes[157] = 0x1e; // 30 fps

    comp_item_payload.extend_from_slice(b"cdta");
    comp_item_payload.extend_from_slice(&(cdta_bytes.len() as u32).to_be_bytes());
    comp_item_payload.extend_from_slice(&cdta_bytes);

    let mut layr_payload = Vec::new();
    let lay_name = b"Layer 1\0";
    layr_payload.extend_from_slice(b"Utf8");
    layr_payload.extend_from_slice(&(lay_name.len() as u32).to_be_bytes());
    layr_payload.extend_from_slice(lay_name);
    if lay_name.len() % 2 != 0 {
        layr_payload.push(0);
    }

    let mut ldta_bytes = vec![0u8; 96];
    ldta_bytes[16] = 0x00;
    ldta_bytes[17] = 0x00;
    ldta_bytes[18] = 0x78;
    ldta_bytes[19] = 0x00;
    ldta_bytes[28] = 0x00;
    ldta_bytes[29] = 0x04;
    ldta_bytes[30] = 0xb0;
    ldta_bytes[31] = 0x00;

    layr_payload.extend_from_slice(b"ldta");
    layr_payload.extend_from_slice(&(ldta_bytes.len() as u32).to_be_bytes());
    layr_payload.extend_from_slice(&ldta_bytes);

    comp_item_payload.extend_from_slice(b"LIST");
    let layr_len = (layr_payload.len() + 4) as u32;
    comp_item_payload.extend_from_slice(&layr_len.to_be_bytes());
    comp_item_payload.extend_from_slice(b"Layr");
    comp_item_payload.extend_from_slice(&layr_payload);

    let mut fold_payload = Vec::new();
    fold_payload.extend_from_slice(b"LIST");
    let item_len = (comp_item_payload.len() + 4) as u32;
    fold_payload.extend_from_slice(&item_len.to_be_bytes());
    fold_payload.extend_from_slice(b"Item");
    fold_payload.extend_from_slice(&comp_item_payload);

    let mut egg_payload = Vec::new();
    egg_payload.extend_from_slice(b"LIST");
    let fold_len = (fold_payload.len() + 4) as u32;
    egg_payload.extend_from_slice(&fold_len.to_be_bytes());
    egg_payload.extend_from_slice(b"Fold");
    egg_payload.extend_from_slice(&fold_payload);

    buf.extend_from_slice(b"RIFX");
    let container_size = (egg_payload.len() + 4) as u32;
    buf.extend_from_slice(&container_size.to_be_bytes());
    buf.extend_from_slice(b"Egg!");
    buf.extend_from_slice(&egg_payload);

    buf
}

#[test]
fn opens_and_displays_after_effects_project_in_ui() {
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join("test_ui_fixture.aep");
    let bytes = make_synthetic_aep();
    std::fs::write(&tmp_path, bytes).expect("writes synthetic AEP fixture");
    let path_str = tmp_path.to_string_lossy().to_string();

    let mut s = Session::default();
    let res = s.execute("file.open", json!({"path": path_str})).expect("file.open succeeds on AEP");
    assert_eq!(res["type"], "aep");
    assert_eq!(res["comps"], 1);

    let active_id = s.active_comp_id().expect("has active comp");
    let comp = s.project.comp(active_id).expect("comp exists");
    assert_eq!(comp.width, 1920);
    assert_eq!(comp.height, 1080);
    assert_eq!(comp.layers.len(), 1);

    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(4);

    assert!(h.state().session.project.items.len() >= 1);
    assert!(h.state().session.path.is_some());

    let _ = std::fs::remove_file(tmp_path);
}
