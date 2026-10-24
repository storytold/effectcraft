//! A marker's name is drawn in a small outlined box beside its flag, in the theme's own field colours (egui_kittest, a real
//! render): the fill and the outline follow Dark and Light, and the two themes differ. `MARKER_NAMES_SNAPSHOT=/abs/dir`
//! also saves the rendered panel for each theme.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::theme::ThemeKind;
use egui_kittest::Harness;
use serde_json::json;

/// One render at a time: two tests creating a GPU device together crash some Vulkan loaders.
static RENDER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn close(got: [u8; 4], want: egui::Color32) -> bool {
    got[..3].iter().zip(want.to_array()).all(|(a, b)| a.abs_diff(b) <= 3)
}

#[test]
fn the_name_box_follows_the_theme() {
    let _one_at_a_time = RENDER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Song", "width": 640, "height": 360, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Bed", "color": "#204060", "width": 640, "height": 360})).unwrap();
    s.execute("markers.set", json!({"new": true, "time": 1.0, "comment": "Verse"})).unwrap();

    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.run_steps(4);
    let ctx = h.ctx.clone();

    let mut seen = vec![];
    for (kind, name) in [(ThemeKind::Dark, "dark"), (ThemeKind::Light, "light")] {
        h.state_mut().set_theme(&ctx, kind);
        h.run_steps(4);
        let tokens = h.state().tokens;
        assert_eq!(tokens.kind, kind);
        // the flag's hit area is 10 wide, centred on the marker; the box starts 6 points right of the centre
        let hit = h.state().auto.find("timeline.marker.0").unwrap_or_else(|| panic!("the marker is on screen ({name})")).rect;
        let (x, y0) = (hit[0] + hit[2] / 2.0, hit[1] + 1.0);
        let h_flag = (hit[3] - 1.0).min(10.0);
        let image = h.render().expect("the panel renders");
        if let Some(dir) = std::env::var_os("MARKER_NAMES_SNAPSHOT") {
            image.save(std::path::Path::new(&dir).join(format!("marker-name-{name}.png"))).unwrap();
        }
        let scale = image.width() as f32 / 1600.0;
        let at = |px: f32, py: f32| image.get_pixel((px * scale) as u32, (py * scale) as u32).0;
        let mid = y0 + h_flag / 2.0;
        // inside the box, left of the text: the field fill; on its left edge: the field outline
        let fill = at(x + 6.0 + 2.5, mid);
        let edge = at(x + 6.0 + 0.5, mid);
        assert!(close(fill, tokens.field_bg), "{name}: box fill {fill:?}, the theme's field_bg is {:?}", tokens.field_bg);
        assert!(close(edge, tokens.field_border), "{name}: box outline {edge:?}, the theme's field_border is {:?}", tokens.field_border);
        seen.push((fill, edge));
    }
    assert_ne!(seen[0], seen[1], "the box looks different in Dark and in Light, so it is not a fixed grey");
}

/// A duration shorter than the name box ends where the box would sit. The box steps past the end cap, so the duration
/// stays visible: the cap is drawn, the bar shows left of it, and the box starts after it.
#[test]
fn a_short_duration_stays_visible_beside_the_name_box() {
    let _one_at_a_time = RENDER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Song", "width": 640, "height": 360, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Bed", "color": "#204060", "width": 640, "height": 360})).unwrap();
    s.execute("markers.set", json!({"new": true, "time": 1.0, "comment": "Verse", "duration": 0.1})).unwrap();
    // a second marker half a second later gives the timeline's scale (points per second)
    s.execute("markers.set", json!({"new": true, "time": 1.5})).unwrap();

    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.run_steps(5);
    let tokens = h.state().tokens;
    let centre = |id: &str| {
        let r = h.state().auto.find(id).unwrap_or_else(|| panic!("{id} is on screen")).rect;
        (r[0] + r[2] / 2.0, r[1] + 1.0, (r[3] - 1.0).min(10.0))
    };
    let ((xa, y0, flag_h), (xb, _, _)) = (centre("timeline.marker.0"), centre("timeline.marker.1"));
    let per_second = (xb - xa) / 0.5;
    let xe = xa + 0.1 * per_second;
    assert!(xe - xa < 30.0, "the duration is shorter than the name box: {} points", xe - xa);
    let image = h.render().expect("the panel renders");
    if let Some(dir) = std::env::var_os("MARKER_NAMES_SNAPSHOT") {
        image.save(std::path::Path::new(&dir).join("marker-short-duration.png")).unwrap();
    }
    let scale = image.width() as f32 / 1600.0;
    let at = |px: f32, py: f32| image.get_pixel((px * scale) as u32, (py * scale) as u32).0;
    let mid = y0 + flag_h / 2.0;
    // the end cap: a pixel in the marker's colour (new markers are red; the theme's field fill is neutral) within a point
    // of where the duration ends, which the box would otherwise cover
    let cap = (-1..=1).map(|d| at(xe + d as f32, mid)).max_by_key(|p| i32::from(p[0]) - i32::from(p[1])).unwrap();
    assert!(i32::from(cap[0]) - i32::from(cap[1]) > 40, "the end cap is drawn: {cap:?}");
    // the bar shows left of the cap, not the box's fill
    let bar = at(xe - 4.0, mid);
    assert!(i32::from(bar[0]) - i32::from(bar[1]) > 20, "the bar shows left of the end cap, not the box: {bar:?}");
    // the box starts after the cap
    let fill = at(xe + 3.0 + 2.5, mid);
    assert!(close(fill, tokens.field_bg), "the box begins after the end cap: {fill:?}, field_bg {:?}", tokens.field_bg);
}
