//! #3202: the Graph Editor follows the theme (egui_kittest, a real render). Its plot used to be a
//! fixed dark grey in every theme, and its bar buttons a fixed dark fill under the theme's text
//! colour, so in Light their labels (dark text) only showed while hovered. Now the plot takes the
//! Timeline's time graph background and every bar button's label is readable on its fill, lit or
//! not, in Dark and Light. `GRAPH_THEME_SNAPSHOT=/abs/dir` also saves the rendered window.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::theme::ThemeKind;
use egui::Color32;
use egui_kittest::Harness;
use serde_json::json;

/// WCAG contrast ratio of two rendered pixels.
fn contrast(a: [u8; 4], b: [u8; 4]) -> f32 {
    effectcraft_ui_egui::theme::contrast_ratio(Color32::from_rgb(a[0], a[1], a[2]), Color32::from_rgb(b[0], b[1], b[2]))
}

fn close(got: [u8; 4], want: Color32) -> bool {
    got[..3].iter().zip(want.to_array()).all(|(a, b)| a.abs_diff(b) <= 3)
}

#[test]
fn the_graph_editor_follows_the_theme() {
    let _one_at_a_time = crate::gpu_test::lock();
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Theme", "width": 640, "height": 360, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#204060", "width": 80, "height": 80})).unwrap();
    // An animated property on the selected layer, so the graph shows a curve and its keys.
    let id = s.active_comp().unwrap().layers[0].id;
    for (t, v) in [(0.5, 0.0), (2.0, 180.0), (3.5, 90.0)] {
        s.execute("prop.addKey", json!({"layer": id.0, "path": "transform/rotation", "time": t, "value": v})).unwrap();
    }
    s.execute("layer.select", json!({"layers": [id.0]})).unwrap();

    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(move |_| EffectcraftApp::new(s));
    h.state_mut().ui.timeline.graph_editor = true;
    h.run_steps(4);
    let ctx = h.ctx.clone();

    let mut plots = vec![];
    let mut failures = vec![];
    for (kind, name) in [(ThemeKind::Dark, "dark"), (ThemeKind::Light, "light")] {
        h.state_mut().set_theme(&ctx, kind);
        h.run_steps(4);
        let tokens = h.state().tokens;
        assert_eq!(tokens.kind, kind);
        let plot = h.state().auto.find("timeline.graph").unwrap_or_else(|| panic!("the Graph Editor is shown ({name})")).rect;
        let buttons: Vec<(&str, [f32; 4])> = ["valueGraph", "speedGraph"]
            .iter()
            .map(|id| (*id, h.state().auto.find(&format!("timeline.graph.{id}")).unwrap_or_else(|| panic!("{id} is on the bar ({name})")).rect))
            .collect();
        let image = h.render().expect("the window renders");
        if let Some(dir) = std::env::var_os("GRAPH_THEME_SNAPSHOT") {
            image.save(std::path::Path::new(&dir).join(format!("graph-theme-{name}.png"))).unwrap();
        }
        let scale = image.width() as f32 / 1600.0;
        let at = |px: f32, py: f32| {
            let (x, y) = (((px * scale) as u32).min(image.width() - 1), ((py * scale) as u32).min(image.height() - 1));
            image.get_pixel(x, y).0
        };

        // The plot: the commonest colour down a column near its right edge (clear of the value
        // labels on the left and of the scroll bar; grid lines and the curve cross it only at a
        // sample or two).
        let x = plot[0] + plot[2] - 20.0;
        let mut column: Vec<[u8; 4]> = (0..24).map(|i| at(x, plot[1] + 8.0 + (plot[3] - 16.0) * i as f32 / 23.0)).collect();
        column.sort_unstable();
        let mut runs: Vec<([u8; 4], usize)> = vec![];
        for px in column {
            match runs.last_mut() {
                Some((c, n)) if *c == px => *n += 1,
                _ => runs.push((px, 1)),
            }
        }
        let bg = runs.iter().max_by_key(|(_, n)| *n).map(|(c, _)| *c).unwrap();
        if !close(bg, tokens.tl_bg) {
            failures.push(format!("{name}: the plot is {bg:?}, the theme's time graph background is {:?}", tokens.tl_bg));
        }
        plots.push(bg);

        // Each button (Value is lit, Speed is not): its fill left of the label, and the label's
        // ink, the pixel across its middle rows that stands out most from the fill. Only the
        // label's span is scanned (7 points of padding each side), not the button's feathered
        // edges, which blend the fill with the bar behind it.
        for (id, r) in buttons {
            let mid = r[1] + r[3] / 2.0;
            let fill = at(r[0] + 2.5, mid);
            let ink = (-3..=3)
                .flat_map(|dy| (3..(r[2] as i32 - 3)).map(move |dx| (r[0] + dx as f32, mid + dy as f32)))
                .map(|(px, py)| at(px, py))
                .max_by(|a, b| contrast(*a, fill).total_cmp(&contrast(*b, fill)))
                .unwrap();
            let c = contrast(ink, fill);
            if c < 3.0 {
                failures.push(format!("{name}: the {id} button's label {ink:?} on its fill {fill:?} is {c:.2}:1"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_ne!(plots[0], plots[1], "the plot looks different in Dark and in Light, so it is not a fixed grey");
}
