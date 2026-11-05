//! `link.open` over the control channel must leave the linked comp on screen, not the Home screen.

use effectcraft_engine::Session;
use effectcraft_interchange::fc_media as fm;
use effectcraft_interchange::fc_project as fp;
use effectcraft_interchange::fc_time as ft;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::control::{self, ControlRequest, Outcome};
use egui_kittest::Harness;
use serde_json::{Value, json};

const LINK_ID: &str = "55555555-5555-4555-8555-555555555555";

fn send(h: &mut Harness<'_, EffectcraftApp>, method: &str, params: Value) -> Value {
    let ctx = h.ctx.clone();
    let (req, _rx) = ControlRequest::new(method, params);
    let now = h.state().now();
    match control::handle(h.state_mut(), &ctx, &req, now) {
        Outcome::Done(v) => v,
        _ => panic!("{method}: no immediate reply"),
    }
}

fn otio() -> String {
    let rate = ft::FrameRate { num: 24, den: 1 };
    let mut doc = fp::Project::new("Link");
    let info = fm::MediaInfo {
        name: String::new(),
        kind: fm::MediaKind::Movie,
        duration: ft::Tick::from_seconds_f64(4.0),
        video: Some(fm::VideoStreamInfo {
            width: 320,
            height: 180,
            frame_rate: rate,
            par: (1, 1),
            codec: "png".into(),
            pixel_format: String::new(),
            color: Default::default(),
            has_alpha: false,
            bitrate: None,
            hdr: None,
        }),
        audio_streams: Vec::new(),
        container: String::new(),
        start_timecode: None,
        file_size: None,
    };
    let clip = fp::MediaClip {
        media: fp::MediaRef::File { path: "/media/shot.mov".into() },
        info,
        interpret: Default::default(),
        mark_in: None,
        mark_out: None,
        markers: vec![],
        offline: false,
        proxy: None,
        identity: None,
    };
    let item = doc.add_item("shot", fp::Label::Iris, fp::ItemKind::Media(clip), None);
    let settings = fp::SequenceSettings { width: 320, height: 180, frame_rate: rate, ..Default::default() };
    let main = doc.new_sequence("Main", settings, 1, 0, None);
    let placed = doc
        .make_track_item(
            item,
            fp::TrackKind::Video,
            ft::Tick::from_seconds_f64(0.0),
            ft::TimeRange::new(ft::Tick::from_seconds_f64(0.0), ft::Tick::from_seconds_f64(2.0)),
            rate,
        )
        .unwrap();
    doc.sequence_mut(main).unwrap().video_tracks[0].items = vec![placed];
    let (bytes, _) = effectcraft_interchange::fc::export(&doc, main, effectcraft_interchange::fc::Format::Otio, &Default::default()).unwrap();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn link_open_dismisses_the_home_screen() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(Session::default()));
    h.run_steps(3);
    h.state_mut().ui.start_screen = true;
    let params = json!({
        "link_id": LINK_ID,
        "payload": {"format": "otio-json", "data": otio()},
        "place": {"seconds": 0.5},
        "result_path": format!("/tmp/ec-ui-link-open-{}/{LINK_ID}.mov", std::process::id()),
        "source": {"app": "filmcraft", "doc_id": "22222222-2222-4222-8222-222222222222", "item": "12"},
    });
    let reply = send(&mut h, "link.open", params);
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(h.state().session.active_comp_id().is_some());
    assert!(!h.state().ui.start_screen, "the Home screen still covers the linked comp");
}

#[test]
fn link_attach_dismisses_the_home_screen() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(Session::default()));
    h.run_steps(3);
    let result_path = format!("/tmp/ec-ui-link-attach-{}/{LINK_ID}.mov", std::process::id());
    let params = json!({
        "link_id": LINK_ID,
        "payload": {"format": "otio-json", "data": otio()},
        "place": {"seconds": 0.5},
        "result_path": result_path,
        "source": {"app": "filmcraft", "doc_id": "22222222-2222-4222-8222-222222222222", "item": "12"},
    });
    let opened = send(&mut h, "link.open", params);
    assert_eq!(opened["ok"], true, "{opened}");
    h.state_mut().ui.start_screen = true;
    let attach = json!({"link_id": LINK_ID, "project": opened["result"]["project"], "comp": opened["result"]["comp"]});
    let reply = send(&mut h, "link.attach", attach);
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(!h.state().ui.start_screen, "the Home screen still covers the reattached comp");
}

/// FilmCraft's `app.activate {open: {link_id}}` brings the window forward and opens the linked comp.
/// Home must not stay over it. A link the session does not bind opens nothing and leaves Home.
#[test]
fn app_activate_with_a_link_dismisses_the_home_screen() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(Session::default()));
    h.run_steps(3);
    let params = json!({
        "link_id": LINK_ID,
        "payload": {"format": "otio-json", "data": otio()},
        "place": {"seconds": 0.5},
        "result_path": format!("/tmp/ec-ui-link-activate-{}/{LINK_ID}.mov", std::process::id()),
        "source": {"app": "filmcraft", "doc_id": "22222222-2222-4222-8222-222222222222", "item": "12"},
    });
    let opened = send(&mut h, "link.open", params);
    assert_eq!(opened["ok"], true, "{opened}");

    h.state_mut().ui.start_screen = true;
    let reply = send(&mut h, "app.activate", json!({"token": null, "open": {"link_id": "99999999-9999-4999-8999-999999999999"}}));
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(h.state().ui.start_screen, "an unknown link opens nothing, so Home stays");

    let reply = send(&mut h, "app.activate", json!({"token": null, "open": {"link_id": LINK_ID}}));
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(!h.state().ui.start_screen, "the Home screen still covers the activated link's comp");
}

#[test]
fn a_failed_link_open_leaves_the_home_screen_alone() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(Session::default()));
    h.run_steps(3);
    h.state_mut().ui.start_screen = true;
    let reply = send(&mut h, "link.open", json!({"link_id": "not-a-uuid"}));
    assert_eq!(reply["ok"], false);
    assert!(h.state().ui.start_screen);
}

/// A linked comp whose footage cannot be read must say so in the viewer. Before the banner the
/// viewer showed a flat comp and the user could not tell why.
#[test]
fn the_viewer_says_when_the_footage_is_missing() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(Session::default()));
    h.run_steps(3);
    let params = json!({
        "link_id": LINK_ID,
        "payload": {"format": "otio-json", "data": otio()},
        "place": {"seconds": 0.5},
        "result_path": format!("/tmp/ec-ui-link-missing-{}/{LINK_ID}.mov", std::process::id()),
        "source": {"app": "filmcraft", "doc_id": "22222222-2222-4222-8222-222222222222", "item": "12"},
    });
    let reply = send(&mut h, "link.open", params);
    assert_eq!(reply["ok"], true, "{reply}");
    h.run_steps(5);
    let banner = h.state().auto.find("viewer.footageMissing").map(|e| e.label.clone());
    let text = banner.expect("the viewer must show a banner for footage that is not on disk");
    assert!(text.contains("shot.mov"), "the banner must name the file: {text}");
}
