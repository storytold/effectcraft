//! Lumetri Scopes panel (Window ▸ Lumetri Scopes): waveform (RGB / Luma / YC), vectorscope
//! (YUV / HLS), histogram and parade (RGB / YUV) of the active composition's frame at the current
//! time, with the colour standard (Rec. 601 / 709 / 2020), 8-bit or float scale and clamp. The
//! maths is `effectcraft_raster::scopes`; agents read the same numbers with `scopes.analyze`.

use effectcraft_engine::commands::panels_cmds::scope_frame;
use effectcraft_engine::raster::scopes::{self, ColorStandard, Scope, ScopeKind, ScopeOpts};
use egui::{Align2, Color32, Rect, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};

use super::panel_kit as kit;
use crate::EffectcraftApp;
use crate::theme::Tokens;

/// Panel options (serde in the UI state).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScopesState {
    pub scope: String,
    pub standard: String,
    pub float: bool,
    pub clamp: bool,
}

impl Default for ScopesState {
    fn default() -> Self {
        ScopesState { scope: "waveformRgb".into(), standard: "rec709".into(), float: false, clamp: true }
    }
}

const STANDARDS: [&str; 3] = ["Rec. 601", "Rec. 709", "Rec. 2020"];

#[derive(Clone)]
struct Cached {
    key: (u64, u64, i64, String, u32),
    scope: Scope,
    tex: Option<egui::TextureHandle>,
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.panel_bg);
    let mut st = app.ui.scopes.clone();
    let kind = ScopeKind::from_name(&st.scope).unwrap_or(ScopeKind::WaveformRgb);
    let standard = ColorStandard::from_name(&st.standard).unwrap_or_default();
    // Top bar: scope, standard, 8-bit / float, clamp.
    let mut x = rect.min.x + 8.0;
    let y = rect.min.y + 6.0;
    let labels: Vec<&str> = ScopeKind::ALL.iter().map(|k| k.label()).collect();
    let cur = ScopeKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
    if let Some(i) = kit::dropdown(app, ui, Rect::from_min_size(pos2(x, y), vec2(150.0, 20.0)), "scopes.kind", &labels, cur) {
        st.scope = ScopeKind::ALL[i].name().into();
    }
    x += 158.0;
    let si = ColorStandard::ALL.iter().position(|c| *c == standard).unwrap_or(1);
    if let Some(i) = kit::dropdown(app, ui, Rect::from_min_size(pos2(x, y), vec2(90.0, 20.0)), "scopes.standard", &STANDARDS, si) {
        st.standard = ColorStandard::ALL[i].name().into();
    }
    x += 98.0;
    if let Some(i) = kit::dropdown(app, ui, Rect::from_min_size(pos2(x, y), vec2(70.0, 20.0)), "scopes.scale", &["8 Bit", "Float"], usize::from(st.float)) {
        st.float = i == 1;
    }
    x += 78.0;
    // Narrow panels: Clamp Signal on a second row.
    let (cx, cy, top) = if x + 110.0 > rect.max.x { (rect.min.x + 8.0, y + 26.0, 60.0) } else { (x, y, 34.0) };
    st.clamp = kit::checkbox(app, ui, pos2(cx, cy + 2.0), "scopes.clamp", "Clamp Signal", st.clamp);
    if st != app.ui.scopes {
        app.ui.scopes = st.clone();
    }
    let area = Rect::from_min_max(pos2(rect.min.x + 8.0, rect.min.y + top), pos2(rect.max.x - 8.0, rect.max.y - 8.0));
    if area.width() < 40.0 || area.height() < 40.0 {
        return;
    }
    p.rect_filled(area, 2.0, Color32::from_rgb(0x10, 0x10, 0x10));
    app.auto.add("scopes.plot", area, kind.label());
    let Some(cid) = app.session.active_comp_id() else {
        p.text(area.center(), Align2::CENTER_CENTER, "Open a composition to see its scopes", Tokens::ui(12.0), t.text_faint);
        return;
    };
    // Plot rectangle: square for vectorscopes.
    let plot = if kind.is_vectorscope() {
        let s = area.width().min(area.height()) - 16.0;
        Rect::from_center_size(area.center(), vec2(s, s))
    } else {
        Rect::from_min_max(area.min + vec2(34.0, 8.0), area.max - vec2(8.0, 8.0))
    };
    let size = (plot.width().max(plot.height()) as u32).clamp(64, 512);
    let o = ScopeOpts {
        standard,
        float: st.float,
        clamp: st.clamp,
        width: if kind.is_vectorscope() { size } else { (plot.width() as u32).clamp(64, 768) },
        height: if kind.is_vectorscope() { size } else { (plot.height() as u32).clamp(32, 512) },
    };
    if kind != ScopeKind::Histogram && !crate::frames::presentation_check(ui.ctx(), [o.width as usize, o.height as usize], &mut app.ui.status) {
        return;
    }
    let time = app.session.time();
    let key = (app.session.revision, cid.0, time.0, format!("{}{}{}{}{}x{}", st.scope, st.standard, st.float, st.clamp, o.width, o.height), 0);
    let ctx = ui.ctx().clone();
    let id = egui::Id::new("scopes-cache");
    let cached: Option<Cached> = ctx.data(|d| d.get_temp(id));
    let cached = match cached {
        Some(c) if c.key == key => c,
        _ => {
            let Some(frame) = scope_frame(&app.session, cid, time, 480) else { return };
            let scope = scopes::compute(&frame, kind, &o);
            let tex = (kind != ScopeKind::Histogram).then(|| {
                let mono = matches!(kind, ScopeKind::WaveformLuma | ScopeKind::VectorscopeYuv | ScopeKind::VectorscopeHls);
                let img = egui::ColorImage::from_rgba_unmultiplied([scope.width as usize, scope.height as usize], &tint(&scope, mono));
                ctx.load_texture("lumetri-scope", img, egui::TextureOptions::LINEAR)
            });
            let c = Cached { key, scope, tex };
            ctx.data_mut(|d| d.insert_temp(id, c.clone()));
            c
        }
    };
    graticule(&p, plot, kind, &o, &t);
    if let Some(tex) = &cached.tex {
        if !crate::frames::presentation_check(&ctx, tex.size(), &mut app.ui.status) {
            return;
        }
        p.image(tex.id(), plot, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    } else {
        histogram(&p, plot, &cached.scope);
    }
}

/// Trace colours: green-tinted mono for luma/vectorscope, per-channel colours otherwise.
fn tint(s: &Scope, mono: bool) -> Vec<u8> {
    let n = s.width as usize * s.height as usize;
    let mut out = vec![0u8; n * 4];
    for i in 0..n {
        let (r, g, b) = (s.planes[0][i], s.planes[1][i], s.planes[2][i]);
        let (r, g, b) = if mono {
            let m = r.max(g).max(b);
            (m * 0.75, m, m * 0.75)
        } else {
            (r, g, b)
        };
        let a = r.max(g).max(b).min(1.0);
        out[i * 4] = (r.min(1.0) * 255.0) as u8;
        out[i * 4 + 1] = (g.min(1.0) * 255.0) as u8;
        out[i * 4 + 2] = (b.min(1.0) * 255.0) as u8;
        out[i * 4 + 3] = (a * 255.0) as u8;
    }
    out
}

fn graticule(p: &egui::Painter, plot: Rect, kind: ScopeKind, o: &ScopeOpts, t: &Tokens) {
    let line = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 28));
    if kind.is_vectorscope() {
        let c = plot.center();
        for r in [0.25f32, 0.5, 0.75, 1.0] {
            p.circle_stroke(c, plot.width() / 2.0 * r, line);
        }
        p.line_segment([pos2(plot.min.x, c.y), pos2(plot.max.x, c.y)], line);
        p.line_segment([pos2(c.x, plot.min.y), pos2(c.x, plot.max.y)], line);
        for (name, [u, v]) in scopes::vector_targets(o.standard, kind == ScopeKind::VectorscopeHls) {
            let q = pos2(plot.min.x + (u + 0.5) * plot.width(), plot.min.y + (0.5 - v) * plot.height());
            p.rect_stroke(Rect::from_center_size(q, vec2(8.0, 8.0)), 0.0, Stroke::new(1.0, Color32::from_rgb(0xc8, 0x6a, 0x6a)), egui::StrokeKind::Middle);
            p.text(q + vec2(7.0, -7.0), Align2::LEFT_BOTTOM, name, Tokens::ui(10.0), t.text_dim);
        }
        return;
    }
    if kind == ScopeKind::Histogram {
        return;
    }
    let (lo, hi) = o.range();
    let marks: &[f32] = if o.float { &[-0.25, 0.0, 0.25, 0.5, 0.75, 1.0, 1.25] } else { &[0.0, 0.25, 0.5, 0.75, 1.0] };
    for v in marks {
        let y = plot.max.y - (v - lo) / (hi - lo) * plot.height();
        p.line_segment([pos2(plot.min.x, y), pos2(plot.max.x, y)], line);
        let label = if o.float { format!("{v:.2}") } else { format!("{}", (v * 255.0).round() as i32) };
        p.text(pos2(plot.min.x - 4.0, y), Align2::RIGHT_CENTER, label, Tokens::ui(9.5), t.text_faint);
    }
    if matches!(kind, ScopeKind::ParadeRgb | ScopeKind::ParadeYuv) {
        for k in 1..3 {
            let x = plot.min.x + plot.width() * k as f32 / 3.0;
            p.line_segment([pos2(x, plot.min.y), pos2(x, plot.max.y)], line);
        }
    }
}

fn histogram(p: &egui::Painter, plot: Rect, s: &Scope) {
    let bars = scopes::histogram_bars(s);
    let max = bars.iter().flat_map(|b| b.iter()).copied().max().unwrap_or(1).max(1) as f32;
    let cols = [
        Color32::from_rgba_unmultiplied(230, 70, 70, 150),
        Color32::from_rgba_unmultiplied(70, 220, 90, 150),
        Color32::from_rgba_unmultiplied(80, 120, 255, 150),
    ];
    let n = bars[0].len().max(1) as f32;
    for (c, b) in bars.iter().enumerate() {
        let pts: Vec<egui::Pos2> =
            b.iter().enumerate().map(|(i, v)| pos2(plot.min.x + i as f32 / n * plot.width(), plot.max.y - (*v as f32 / max).sqrt() * plot.height())).collect();
        for (i, q) in pts.iter().enumerate() {
            p.line_segment([pos2(q.x, plot.max.y), *q], Stroke::new((plot.width() / n).max(1.0), cols[c].gamma_multiply(0.5)));
            if i > 0 {
                p.line_segment([pts[i - 1], *q], Stroke::new(1.0, cols[c]));
            }
        }
    }
}
