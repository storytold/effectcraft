//! Info panel: colour and position under the pointer, plus the current selection.

use crate::i18n::{tr, tr_args};
use egui::{Align2, Color32, Rect, pos2, vec2};

use crate::theme::Tokens;
use crate::{EffectcraftApp, a11y, widgets};

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let x0 = rect.min.x + 12.0;
    let scroll = widgets::PanelScroll::begin(ui, egui::Id::new("info-scroll"), rect);
    let mut y = rect.min.y + 14.0 - scroll.offset;
    if app.pointer_comp.is_some() {
        // GPU frames: read the shown frame back for sampling.
        app.viewer_pixels();
    }
    let (rgba, xy) = match (app.pointer_comp, &app.viewer_image) {
        (Some([cx, cy]), Some(img)) => {
            let comp = app.session.active_comp().map_or([1.0, 1.0], |c| [c.width as f32, c.height as f32]);
            let region = super::viewer_tools::shown_region(ui.ctx(), comp);
            (super::fx_widgets::frame_pixel(img, region, [cx as f64, cy as f64]), Some((cx, cy)))
        }
        _ => (None, None),
    };
    let row = |p: &egui::Painter, y: f32, k: &str, v: String, col: Color32| {
        p.text(pos2(x0, y), Align2::LEFT_CENTER, k, Tokens::ui(12.0), col);
        p.text(pos2(x0 + 22.0, y), Align2::LEFT_CENTER, v, Tokens::mono(12.0), t.text);
    };
    let unpre = |c: Color32| -> [u8; 4] {
        let a = c.a();
        if a == 0 {
            [0, 0, 0, 0]
        } else {
            let f = |v: u8| ((v as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            [f(c.r()), f(c.g()), f(c.b()), a]
        }
    };
    let px = rgba.map(unpre).unwrap_or([0, 0, 0, 0]);
    // The panel paints its text, which assistive technology cannot see: each line is also a
    // read-out with the same words (a sentence a reader can speak: "Red: 128").
    let full = rect.width() - 24.0;
    let say = |key: &str, x: f32, y: f32, w: f32, text: &str| {
        a11y::readout(ui, egui::Id::new(("info", key)), Rect::from_min_size(pos2(x, y - 8.0), vec2(w.max(24.0), 16.0)), text);
    };
    let half = rect.width() * 0.5 - 16.0;
    say("r", x0, y, half, &tr_args("Red: {}", &[&px[0]]));
    say("g", x0, y + 18.0, half, &tr_args("Green: {}", &[&px[1]]));
    say("b", x0, y + 36.0, half, &tr_args("Blue: {}", &[&px[2]]));
    say("a", x0, y + 54.0, half, &tr_args("Alpha: {}", &[&px[3]]));
    row(&p, y, "R :", format!("{}", px[0]), Color32::from_rgb(0xe0, 0x60, 0x60));
    row(&p, y + 18.0, "G :", format!("{}", px[1]), Color32::from_rgb(0x60, 0xd0, 0x60));
    row(&p, y + 36.0, "B :", format!("{}", px[2]), Color32::from_rgb(0x60, 0x90, 0xf0));
    row(&p, y + 54.0, "A :", format!("{}", px[3]), t.text_dim);
    let cx = rect.min.x + rect.width() * 0.5;
    if let Some((x, yy)) = xy {
        say("x", cx, y, half, &tr_args("X: {}", &[&format!("{x:.0}")]));
        say("y", cx, y + 18.0, half, &tr_args("Y: {}", &[&format!("{yy:.0}")]));
        p.text(pos2(cx, y), Align2::LEFT_CENTER, format!("X : {x:.0}"), Tokens::mono(12.0), t.text);
        p.text(pos2(cx, y + 18.0), Align2::LEFT_CENTER, format!("Y : {yy:.0}"), Tokens::mono(12.0), t.text);
    }
    if let Some(c) = rgba {
        let sw = egui::Rect::from_min_size(pos2(cx, y + 34.0), vec2(36.0, 30.0));
        p.rect_filled(sw, 3.0, Color32::from_rgb(px[0], px[1], px[2]));
        a11y::graphic(ui, egui::Id::new(("info", "swatch")), sw, &tr_args("Color under the pointer: {}", &[&format!("{}, {}, {}", px[0], px[1], px[2])]));
        let _ = c;
    }
    y += 82.0;
    p.line_segment([pos2(rect.min.x + 8.0, y), pos2(rect.max.x - 8.0, y)], egui::Stroke::new(1.0, t.separator));
    y += 14.0;
    let mut bottom = y;
    let comp = app.session.active_comp_arc();
    if let Some(c) = comp {
        let sel: Vec<String> = app.session.state.selected_layers.iter().filter_map(|id| c.layer(*id)).map(|l| l.name.clone()).collect();
        let tc = crate::panels::timecode(&app.session, &c, app.session.time());
        let selection = if sel.is_empty() { tr("No layer selected").to_string() } else { sel.join(", ") };
        p.text(pos2(x0, y), Align2::LEFT_CENTER, &selection, Tokens::semibold(12.0), t.text);
        say("selection", x0, y, full, &tr_args("Selected layers: {}", &[&selection]));
        let time_line = tr_args("Time: {}", &[&tc]);
        p.text(pos2(x0, y + 18.0), Align2::LEFT_CENTER, &time_line, Tokens::ui(12.0), t.text_dim);
        say("time", x0, y + 18.0, full, &time_line);
        // One layer selected: Duration, In and Out like AE's Info panel.
        let mut ly = y + 36.0;
        if let [id] = app.session.state.selected_layers.as_slice()
            && let Some(l) = c.layer(*id)
        {
            let tc = |t| crate::panels::timecode(&app.session, &c, t);
            let lines = [
                // A length, so no start-timecode offset.
                format!("Duration: {}", {
                    let fr = c.frame_rate;
                    let n = fr.frame_at(l.out_point) - fr.frame_at(l.in_point);
                    let s = effectcraft_engine::time::format_timecode_frames(n, fr, fr.supports_drop_frame());
                    if fr.supports_drop_frame() { s } else { s.replace(';', ":") }
                }),
                format!("In: {}, Out: {}", tc(l.in_point), tc(effectcraft_engine::time::Tick(l.out_point.0 - c.frame_duration().0))),
            ];
            for (k, line) in lines.into_iter().enumerate() {
                p.text(pos2(x0, ly), Align2::LEFT_CENTER, &line, Tokens::ui(12.0), t.text_dim);
                say(if k == 0 { "duration" } else { "inout" }, x0, ly, full, &line);
                ly += 18.0;
            }
        }
        // While a preview plays: the frame rate it achieves and whether that is real time.
        if let (Some(fps), Some(rt)) = (app.playback.achieved_fps(), app.playback.real_time()) {
            let note = if rt { "real-time" } else { "not real-time" };
            p.text(pos2(x0, ly), Align2::LEFT_CENTER, format!("{fps:.2} fps ({note})"), Tokens::ui(11.5), if rt { t.text_dim } else { t.warning });
            say("fps", x0, ly, full, &format!("{fps:.2} fps ({note})"));
            app.auto.add("info.previewFps", Rect::from_min_size(pos2(x0, ly - 8.0), vec2(200.0, 16.0)), &format!("{fps:.2} fps ({note})"));
            ly += 18.0;
        }
        // Settings ▸ Composition ▸ Show Rendering Progress in Info Panel.
        if app.session.prefs.composition.show_rendering_progress {
            let ms = app.frames.last_ms.lock().map(|v| *v).unwrap_or(0.0);
            let rq = app.session.render_job.as_ref().map(|_| "  •  Rendering queue…").unwrap_or("");
            let render_line = tr_args("Render: {} ms  •  UI {} fps{}", &[&format!("{ms:.0}"), &format!("{:.0}", app.fps), &rq]);
            p.text(pos2(x0, ly), Align2::LEFT_CENTER, &render_line, Tokens::ui(11.5), t.text_faint);
            say("render", x0, ly, full, &render_line);
        }
        if !app.session.state.selected_keys.is_empty() {
            // One key: its property, time and value (as After Effects' Info panel shows them).
            let sel = &app.session.state.selected_keys;
            let one = match sel.as_slice() {
                [k] => c.layer(k.layer).and_then(|l| {
                    let pr = l.props.find(k.prop)?;
                    let key = pr.keys.iter().find(|x| x.time == k.time)?;
                    let tc = crate::panels::timecode(&app.session, &c, l.comp_time(k.time));
                    let v = match &key.value {
                        effectcraft_engine::keyframe::Value::Scalar(v) => format!("{v:.2}"),
                        effectcraft_engine::keyframe::Value::Vec2(v) => format!("{:.1}, {:.1}", v[0], v[1]),
                        effectcraft_engine::keyframe::Value::Vec3(v) => format!("{:.1}, {:.1}, {:.1}", v[0], v[1], v[2]),
                        _ => String::new(),
                    };
                    Some(format!("{}  {tc}  {v}", pr.name).trim_end().to_string())
                }),
                _ => None,
            };
            let line = one.unwrap_or_else(|| format!("{} keyframes selected", sel.len()));
            p.text(pos2(x0, ly + 18.0), Align2::LEFT_CENTER, &line, Tokens::ui(11.5), t.text_dim);
            say("keys", x0, ly + 18.0, full, &line);
        }
        bottom = ly + 30.0;
    }
    let content = bottom - (rect.min.y - scroll.offset);
    scroll.end(ui, &mut app.auto, "info.scroll", content, &t);
}
