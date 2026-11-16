//! `drawString`: ScriptUI text painted from the engine's [`UiText`] layout, the one
//! `measureString` measured (#3782). The glyphs are rasterised by the engine at the screen's pixel
//! density and shown as textures, so the faces, fallbacks, advances and rows are the measured
//! ones; egui's own text layout is not involved.

use std::sync::Arc;

use effectcraft_engine::scriptui::{MAX_COVERAGE_SIDE, UiFont, UiText};
use effectcraft_text::fonts;
use egui::cache::FramePublisher;
use egui::{Color32, Painter, Pos2, Rect, pos2, vec2};

/// A laid-out `drawString` text: its string, face, size (as `f64` bits) and the font database
/// [generation](fonts::generation) it was laid out in, so a new language or font lays it out again.
type LayoutKey = (String, UiFont, u64, u64);
/// A rasterised tile of one: the layout, the pixels per point (as bits) and the tile.
type RasterKey = (LayoutKey, u32, Tile);

/// A window of a text's pixels: its corner in physical pixels from the text's top-left corner, and
/// its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Tile {
    min: [i64; 2],
    size: [u32; 2],
}

/// `[left, top, right, bottom]` in physical pixels from the text's top-left corner.
type Bounds = [i64; 4];

/// The tiles, at most `side` pixels square, covering the part of `ink` that is `visible`. They sit
/// on a grid anchored at the ink's corner, so moving the text or the clip keeps the tiles that
/// stay whole.
fn tiles(ink: Bounds, visible: Bounds, side: u32) -> impl Iterator<Item = Tile> {
    let side = i64::from(side.max(1));
    let (x0, y0, x1, y1) = (ink[0].max(visible[0]), ink[1].max(visible[1]), ink[2].min(visible[2]), ink[3].min(visible[3]));
    // Grid cells `lo..hi` covers, and one cell's span within it.
    let cells = move |lo: i64, hi: i64, anchor: i64| {
        if hi > lo { lo.saturating_sub(anchor) / side..hi.saturating_sub(anchor).saturating_sub(1) / side + 1 } else { 0..0 }
    };
    let span = move |cell: i64, lo: i64, hi: i64, anchor: i64| {
        let start = anchor.saturating_add(cell.saturating_mul(side)).max(lo);
        (start, u32::try_from(anchor.saturating_add(cell.saturating_add(1).saturating_mul(side)).min(hi).saturating_sub(start)).unwrap_or(0))
    };
    let cols = cells(x0, x1, ink[0]);
    cells(y0, y1, ink[1]).flat_map(move |row| {
        let (top, h) = span(row, y0, y1, ink[1]);
        cols.clone().map(move |col| {
            let (left, w) = span(col, x0, x1, ink[0]);
            Tile { min: [left, top], size: [w, h] }
        })
    })
}

/// Paint `text` with its top-left corner at `at` (snapped to the pixel grid), within `p`'s clip
/// rect. Text that cannot be laid out ([`UiText::layout`]'s error) paints nothing; scripts get the
/// same error from `drawString` and `measureString`.
pub(super) fn paint_text(p: &Painter, at: Pos2, text: &str, font: UiFont, size: f64, color: Color32) {
    if !(at.x.is_finite() && at.y.is_finite()) {
        return;
    }
    let ctx = p.ctx();
    let key = layout_key(text, font, size);
    let Some(lay) = layout(ctx, &key) else { return };
    let ppp = ctx.pixels_per_point();
    let ppp = if ppp.is_finite() && ppp > 0.0 { ppp } else { 1.0 };
    let s = f64::from(ppp);
    let origin = [(f64::from(at.x) * s).round() as i64, (f64::from(at.y) * s).round() as i64];
    // Ink can stray outside the measured box (italic overhang, marks, tall fallbacks): rasterise
    // an em around it, but only what the clip rect shows on screen.
    let pad = (lay.font_size() * s).ceil() as i64;
    let [w, h] = lay.size();
    let ink = [-pad, -pad, ((w * s).ceil() as i64).saturating_add(pad), ((h * s).ceil() as i64).saturating_add(pad)];
    let shown = p.clip_rect().intersect(ctx.viewport_rect());
    let px = |v: f32, o: i64, round: fn(f64) -> f64| (round(f64::from(v) * s) as i64).saturating_sub(o);
    let visible = [
        px(shown.min.x, origin[0], f64::floor),
        px(shown.min.y, origin[1], f64::floor),
        px(shown.max.x, origin[0], f64::ceil),
        px(shown.max.y, origin[1], f64::ceil),
    ];
    // Tiles fit every texture egui allows: its smallest maximum side is 1024.
    for tile in tiles(ink, visible, MAX_COVERAGE_SIDE) {
        let tex = texture(ctx, &lay, (key.clone(), ppp.to_bits(), tile), s);
        // The texture's own size: what was rasterised is shown pixel for pixel, never stretched.
        let [tw, th] = tex.size();
        let min = pos2(origin[0].saturating_add(tile.min[0]) as f32, origin[1].saturating_add(tile.min[1]) as f32);
        let rect = Rect::from_min_size(min, vec2(tw as f32, th as f32)) / ppp;
        p.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
    }
}

fn layout_key(text: &str, font: UiFont, size: f64) -> LayoutKey {
    (text.to_owned(), font, size.to_bits(), fonts::generation())
}

/// The layout for `key`, kept while it is drawn every frame (`None` when it is an error).
fn layout(ctx: &egui::Context, key: &LayoutKey) -> Option<Arc<UiText>> {
    type Cache = FramePublisher<LayoutKey, Option<Arc<UiText>>>;
    let cached = ctx.memory_mut(|m| m.caches.cache::<Cache>().get(key).cloned());
    let lay = cached.unwrap_or_else(|| UiText::layout(&key.0, key.1, f64::from_bits(key.2), None).ok().map(Arc::new));
    ctx.memory_mut(|m| m.caches.cache::<Cache>().set(key.clone(), lay.clone()));
    lay
}

/// The texture of one rasterised tile, kept while it is drawn every frame. White, with the glyph
/// coverage as alpha: the painter tints it with the script's colour.
fn texture(ctx: &egui::Context, lay: &UiText, key: RasterKey, scale: f64) -> egui::TextureHandle {
    type Cache = FramePublisher<RasterKey, egui::TextureHandle>;
    if let Some(tex) = ctx.memory_mut(|m| m.caches.cache::<Cache>().get(&key).cloned()) {
        ctx.memory_mut(|m| m.caches.cache::<Cache>().set(key, tex.clone()));
        return tex;
    }
    let tile = key.2;
    let cov = lay.coverage(scale, tile.min, tile.size[0], tile.size[1]);
    let pixels = cov.data.iter().map(|c| Color32::from_white_alpha((c.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
    let image = egui::ColorImage::new([cov.width as usize, cov.height as usize], pixels);
    let tex = ctx.load_texture("scriptui-text", image, egui::TextureOptions::NEAREST);
    ctx.memory_mut(|m| m.caches.cache::<Cache>().set(key, tex.clone()));
    tex
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use effectcraft_engine::scriptui::{DrawOp, MAX_COVERAGE_SIDE, MAX_UI_TEXT_UNITS, UiFont, UiText};
    use egui::{Rect, pos2, vec2};

    use super::{Bounds, Tile, tiles};
    use crate::theme::Tokens;

    /// One painted tile of a `drawString`: its rect in points, texture, tint, and the texture's
    /// size and alpha, row-major.
    struct Painted {
        rect: Rect,
        tex: egui::TextureId,
        color: egui::Color32,
        size: [usize; 2],
        alpha: Vec<u8>,
    }

    /// Where the tests' control sits in the window, in points.
    const CONTROL: egui::Pos2 = pos2(3.0, 5.0);

    /// A window at `ppp` pixels per point, `size` points large, kept across frames with every
    /// texture it was sent.
    struct Screen {
        ctx: egui::Context,
        size: egui::Vec2,
        images: HashMap<egui::TextureId, egui::ColorImage>,
    }

    impl Screen {
        fn new(ppp: f32, size: egui::Vec2) -> Self {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(ppp);
            let s = Self { ctx, size, images: HashMap::new() };
            s.ctx.run_ui(s.input(), |_| {}).textures_delta.clear();
            assert_eq!(s.ctx.pixels_per_point(), ppp);
            s
        }

        fn input(&self) -> egui::RawInput {
            egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), self.size)), ..Default::default() }
        }

        /// Paint `ops` in a control of `size` points the way a ScriptUI window does: the painted
        /// tiles, and how many textures this frame rasterised.
        fn paint(&mut self, ops: &[DrawOp], size: egui::Vec2) -> (Vec<Painted>, usize) {
            let t = Tokens::for_kind(crate::theme::ThemeKind::Dark);
            let mut out = self.ctx.run_ui(self.input(), |ui| {
                super::super::paint_draw(ui, Rect::from_min_size(CONTROL, size), ops, &t, &effectcraft_engine::FsServices);
            });
            let new: Vec<(egui::TextureId, egui::ColorImage)> = out
                .textures_delta
                .set
                .iter()
                .filter_map(|(id, d)| {
                    let egui::ImageData::Color(c) = &d.last()?.image;
                    Some((*id, (**c).clone()))
                })
                .collect();
            out.textures_delta.clear();
            let uploaded = new.len();
            self.images.extend(new);
            let painted = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => {
                        let img = self.images.get(&m.texture_id)?;
                        let color = m.vertices.first()?.color;
                        Some(Painted { rect: m.calc_bounds(), tex: m.texture_id, color, size: img.size, alpha: img.pixels.iter().map(|p| p.a()).collect() })
                    }
                    _ => None,
                })
                .collect();
            (painted, uploaded)
        }
    }

    fn paint(ops: &[DrawOp], ppp: f32, size: egui::Vec2) -> Vec<Painted> {
        Screen::new(ppp, vec2(1000.0, 1000.0)).paint(ops, size).0
    }

    fn op(text: &str, font: UiFont, size: f64) -> DrawOp {
        DrawOp::Text { text: text.into(), color: None, x: 0.0, y: 0.0, size, style: String::new(), font }
    }

    /// The text's top-left corner in points: the control's corner plus `(x, y)`, snapped to the
    /// pixel grid.
    fn text_origin(ppp: f32, x: f32, y: f32) -> egui::Pos2 {
        ((CONTROL.to_vec2() + vec2(x, y)) * ppp).round().to_pos2() / ppp
    }

    /// Whether `p` shows its texture pixel for pixel, and the texture holds the layout's coverage of
    /// that window of the text at `origin`.
    fn shows_the_layout(p: &Painted, lay: &UiText, ppp: f32, origin: egui::Pos2) -> Result<(), String> {
        let shown = p.rect.size() * ppp;
        if (shown.x - p.size[0] as f32).abs() > 1e-2 || (shown.y - p.size[1] as f32).abs() > 1e-2 {
            return Err(format!("a {:?} texture is stretched over {shown:?} pixels", p.size));
        }
        let px = |a: f32, b: f32| ((f64::from(a) - f64::from(b)) * f64::from(ppp)).round() as i64;
        let win = [px(p.rect.min.x, origin.x), px(p.rect.min.y, origin.y)];
        let cov = lay.coverage(f64::from(ppp), win, p.size[0] as u32, p.size[1] as u32);
        let want: Vec<u8> = cov.data.iter().map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
        if want != p.alpha {
            return Err(format!("the pixels at {win:?} are not the measured layout's"));
        }
        Ok(())
    }

    /// The ink's extent in points from the text's top-left corner, `[left, top, right, bottom]`,
    /// counting only pixels whose centre passes `keep`.
    fn ink(p: &Painted, ppp: f32, keep: impl Fn(egui::Pos2) -> bool) -> Option<[f32; 4]> {
        let [w, h] = p.size;
        let origin = text_origin(ppp, 0.0, 0.0);
        let at = |px: usize, py: usize| (p.rect.min + vec2(px as f32, py as f32) / ppp) - origin;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..h {
            for x in 0..w {
                let centre = at(x, y) + vec2(0.5, 0.5) / ppp;
                if p.alpha[y * w + x] > 0 && keep(centre.to_pos2()) {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                }
            }
        }
        (x0 != usize::MAX).then(|| {
            let (a, b) = (at(x0, y0), at(x1, y1));
            [a.x, a.y, b.x, b.y]
        })
    }

    /// What `drawString` paints is the text `measureString` measured (#3782): the painted pixels
    /// are the measured layout's glyphs, whose ink fills the measured box from its left to its right
    /// edge, for wide-Unicode text (CJK, symbols, emoji, missing glyphs, right-to-left and mixed
    /// scripts, combining marks) at fractional sizes and pixel densities.
    #[test]
    fn painted_text_is_the_measured_text() {
        let texts = [
            "AVATAR Toyota 0123",
            "\u{6f22}\u{5b57}\u{304b}\u{306a}",
            "\u{2714}\u{25a3}\u{25b8}",
            "\u{1f600}x",
            "\u{378}\u{e000}",
            "\u{645}\u{631}\u{62d}\u{628}\u{627} abc",
            "\u{5e9}\u{5dc}\u{5d5}\u{5dd}",
            "\u{915}\u{93f}\u{924}",
            "e\u{301}e\u{301} \u{416}\u{3a9}",
            "ab \u{6f22}\u{5b57} \u{645}\u{631} \u{2714} cd",
        ];
        let mut bad = vec![];
        for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
            for font in [UiFont::Regular, UiFont::Bold, UiFont::Italic, UiFont::Mono] {
                for size in [9.0, 12.5, 20.25] {
                    let ops: Vec<DrawOp> = texts.iter().map(|t| op(t, font, size)).collect();
                    let painted = paint(&ops, ppp, vec2(400.0, 60.0));
                    assert_eq!(painted.len(), texts.len(), "{font:?} {size} @{ppp}");
                    for (text, p) in texts.iter().zip(&painted) {
                        let what = format!("{font:?} {size} @{ppp} {text:?}");
                        let lay = UiText::layout(text, font, size, None).unwrap();
                        if let Err(e) = shows_the_layout(p, &lay, ppp, text_origin(ppp, 0.0, 0.0)) {
                            bad.push(format!("{what}: {e}"));
                            continue;
                        }
                        let Some([l, top, r, bottom]) = ink(p, ppp, |_| true) else {
                            bad.push(format!("{what}: paints nothing"));
                            continue;
                        };
                        // Side bearings and antialiasing: ink starts and ends within a fifth of an
                        // em (and a pixel) of the measured edges, and stays in the rows.
                        let [w, h] = lay.size().map(|v| v as f32);
                        let slack = (size * 0.2) as f32 + 1.0 / ppp;
                        if l < -slack || l > slack || (r - w).abs() > slack || top < -slack || bottom > h + slack {
                            bad.push(format!("{what}: ink [{l:.2}, {top:.2}, {r:.2}, {bottom:.2}] in measured [{w:.2}, {h:.2}]"));
                        }
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// Explicit line breaks paint the measured rows: ink in every row with text, none in blank and
    /// trailing rows, each row's ink ending where its text's measured width does.
    #[test]
    fn painted_rows_are_the_measured_rows() {
        let text = "alpha  beta\n\n\u{6f22}\u{5b57}\u{6f22}\u{5b57} gamma   \nepsilonzetaetatheta\n";
        let lay = UiText::layout(text, UiFont::Regular, 12.5, None).unwrap();
        let rows = lay.lines();
        assert_eq!(rows.len(), 5, "{rows:?}");
        for ppp in [1.0_f32, 1.5] {
            let painted = paint(&[op(text, UiFont::Regular, 12.5)], ppp, vec2(400.0, 120.0));
            for (i, row) in rows.iter().enumerate() {
                let line = &text[row.range.clone()];
                let in_row = |c: egui::Pos2| f64::from(c.y) >= row.top && f64::from(c.y) < row.top + row.height;
                match ink(&painted[0], ppp, in_row) {
                    Some([_, _, r, _]) if !line.trim().is_empty() => {
                        let alone = UiText::layout(line.trim_end(), UiFont::Regular, 12.5, None).unwrap().size()[0] as f32;
                        assert!((r - alone).abs() <= 2.5 + 1.0 / ppp, "row {i} {line:?} @{ppp}: ink ends at {r}, its text at {alone}");
                    }
                    None if line.trim().is_empty() => {}
                    other => panic!("row {i} {line:?} @{ppp}: ink {other:?}"),
                }
            }
        }
    }

    /// The tiles cover the visible ink exactly once, none larger than the side, on a grid anchored
    /// at the ink's corner.
    #[test]
    fn tiles_cover_the_visible_ink_once() {
        let cases: [(Bounds, Bounds, u32); 6] = [
            ([-3, -3, 20, 9], [-100, -100, 100, 100], 7),
            ([-3, -3, 20, 9], [2, 1, 15, 4], 7),
            ([-3, -3, 20, 9], [20, 0, 40, 4], 7),
            ([-5, -5, 40, 30], [-1, 6, 33, 29], 1),
            ([-2000, -2000, 61_000, 3000], [37_000, -40, 47_125, 9], 1024),
            ([-2000, -2000, 61_000, 3000], [i64::MIN, i64::MIN, i64::MAX, i64::MAX], 512),
        ];
        for (ink, visible, side) in cases {
            let got: Vec<Tile> = tiles(ink, visible, side).collect();
            let [x0, y0, x1, y1] = [ink[0].max(visible[0]), ink[1].max(visible[1]), ink[2].min(visible[2]), ink[3].min(visible[3])];
            let area = (x1 - x0).max(0) * (y1 - y0).max(0);
            assert_eq!(got.iter().map(|t| i64::from(t.size[0]) * i64::from(t.size[1])).sum::<i64>(), area, "{ink:?} {visible:?}");
            for (i, t) in got.iter().enumerate() {
                let [l, top] = t.min;
                let [r, bottom] = [l + i64::from(t.size[0]), top + i64::from(t.size[1])];
                assert!(t.size[0] >= 1 && t.size[1] >= 1 && t.size[0] <= side && t.size[1] <= side, "{t:?}");
                assert!(l >= x0 && top >= y0 && r <= x1 && bottom <= y1, "{t:?} outside {visible:?}");
                assert!(((l - ink[0]) % i64::from(side) == 0 || l == x0) && ((top - ink[1]) % i64::from(side) == 0 || top == y0), "{t:?} off the grid");
                for u in &got[i + 1..] {
                    let apart = u.min[0] >= r || u.min[1] >= bottom || u.min[0] + i64::from(u.size[0]) <= l || u.min[1] + i64::from(u.size[1]) <= top;
                    assert!(apart, "{t:?} overlaps {u:?}");
                }
            }
        }
    }

    /// Text wider than one texture is painted in tiles, each shown pixel for pixel where its
    /// window of the layout belongs, at a fractional pixel density.
    #[test]
    fn wide_text_is_tiled_not_stretched() {
        let (ppp, side) = (1.25_f32, MAX_COVERAGE_SIDE as usize);
        let mut screen = Screen::new(ppp, vec2(8200.0, 100.0));
        let text = "W".repeat(12);
        let lay = UiText::layout(&text, UiFont::Regular, 1000.0, None).unwrap();
        assert!(lay.size()[0] > 8100.0);
        // A strip across the middle of the glyphs, wider than any texture.
        let draw = DrawOp::Text { text, color: None, x: 0.0, y: -500.0, size: 1000.0, style: String::new(), font: UiFont::Regular };
        let (mut painted, _) = screen.paint(&[draw], vec2(8100.0, 4.0));
        painted.sort_by(|a, b| a.rect.min.x.total_cmp(&b.rect.min.x));
        let origin = text_origin(ppp, 0.0, -500.0);
        for p in &painted {
            shows_the_layout(p, &lay, ppp, origin).unwrap();
            assert!(p.size[0] <= side && p.size[1] <= side, "{:?}", p.size);
        }
        assert!(painted.len() >= 8100 * 5 / 4 / side, "{} tiles", painted.len());
        for pair in painted.windows(2) {
            assert!((pair[0].rect.max.x - pair[1].rect.min.x).abs() < 1e-3, "tiles {:?} and {:?} do not meet", pair[0].rect, pair[1].rect);
        }
        let (first, last) = (painted[0].rect, painted[painted.len() - 1].rect);
        let control = Rect::from_min_size(CONTROL, vec2(8100.0, 4.0));
        assert!(
            (first.min.x - (control.min.x * ppp).floor() / ppp).abs() < 1e-3 && (last.max.x - (control.max.x * ppp).ceil() / ppp).abs() < 1e-3,
            "{first:?}..{last:?}"
        );
        assert!(painted.iter().any(|p| p.alpha.iter().any(|&a| a > 0)), "the strip crosses the glyphs");
    }

    /// Text drawn again unchanged is not rasterised again, even in another colour or moved by
    /// whole pixels; clipping it rasterises the newly shown window, and a language change (a new
    /// font generation) lays it out again.
    #[test]
    fn unchanged_text_reuses_its_layout_and_textures() {
        use effectcraft_text::fonts;
        type Layouts = egui::cache::FramePublisher<super::LayoutKey, Option<std::sync::Arc<UiText>>>;
        let ppp = 1.5;
        let text = "Reuse \u{6f22}\u{5b57} \u{645}\u{631}";
        let lay = UiText::layout(text, UiFont::Regular, 14.0, None).unwrap();
        // Away from the control's edges, so its ink and an em of slack are all shown.
        let draw = |x: f64, color: [f32; 4]| DrawOp::Text {
            text: text.into(),
            color: Some(color),
            x,
            y: 20.0,
            size: 14.0,
            style: String::new(),
            font: UiFont::Regular,
        };
        let (red, blue) = ([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]);
        let big = vec2(300.0, 80.0);
        // Other tests may move the font generation meanwhile; only a run without a move counts.
        let mut screen = None;
        for _ in 0..10 {
            let mut s = Screen::new(ppp, vec2(400.0, 200.0));
            let generation = fonts::generation();
            let key = super::layout_key(text, UiFont::Regular, 14.0);
            let kept = |s: &Screen| s.ctx.memory_mut(|m| m.caches.cache::<Layouts>().get(&key).cloned().flatten());
            let (first, made) = s.paint(&[draw(20.0, red)], big);
            let lay1 = kept(&s);
            let (recoloured, recoloured_made) = s.paint(&[draw(20.0, blue)], big);
            let lay2 = kept(&s);
            let (moved, moved_made) = s.paint(&[draw(28.0, blue)], big);
            if fonts::generation() != generation {
                continue;
            }
            assert!(made >= 1 && first.len() == 1, "{made} {}", first.len());
            assert!(lay1.zip(lay2).is_some_and(|(a, b)| std::sync::Arc::ptr_eq(&a, &b)), "the second frame laid the text out again");
            assert_eq!((recoloured_made, recoloured[0].tex, recoloured[0].rect), (0, first[0].tex, first[0].rect), "a new colour rasterised again");
            assert_eq!(recoloured[0].color, egui::Color32::BLUE);
            assert_eq!((moved_made, moved[0].tex), (0, first[0].tex), "moving by whole pixels rasterised again");
            assert!((moved[0].rect.min.x - first[0].rect.min.x - 8.0).abs() < 1e-3);
            shows_the_layout(&moved[0], &lay, ppp, text_origin(ppp, 28.0, 20.0)).unwrap();
            screen = Some((s, first[0].tex, first[0].rect));
            break;
        }
        let Some((mut s, tex, rect)) = screen else { panic!("the font generation never held still") };
        // A narrower control clips the text: the window it shows is rasterised.
        let (clipped, clipped_made) = s.paint(&[draw(20.0, blue)], vec2(30.0, 80.0));
        assert!(clipped_made >= 1 && clipped[0].tex != tex && clipped[0].rect.width() < rect.width());
        shows_the_layout(&clipped[0], &lay, ppp, text_origin(ppp, 20.0, 20.0)).unwrap();
        // A language change can change fallback faces: the text is laid out and rasterised again.
        crate::theme::install(&s.ctx, &Tokens::for_kind(crate::theme::ThemeKind::Dark), "en");
        let (relaid, relaid_made) = s.paint(&[draw(20.0, blue)], vec2(30.0, 80.0));
        assert!(relaid_made >= 1 && relaid[0].tex != clipped[0].tex);
    }

    /// Long text in a small control rasterises only what the control shows, however large the
    /// font or far the text is scrolled; text past the limit or at a position that is not a
    /// number paints nothing.
    #[test]
    fn painted_text_is_bounded() {
        let long = "W".repeat(MAX_UI_TEXT_UNITS);
        let area = |painted: &[Painted]| painted.iter().map(|p| p.size[0] * p.size[1]).sum::<usize>();
        let painted = paint(&[op(&long, UiFont::Regular, 1000.0)], 2.0, vec2(50.0, 40.0));
        assert!(!painted.is_empty() && area(&painted) <= (2 * 51) * (2 * 41), "{:?}", painted.iter().map(|p| p.size).collect::<Vec<_>>());
        assert!(paint(&[op(&format!("{long}W"), UiFont::Regular, 12.0)], 1.0, vec2(50.0, 40.0)).is_empty());
        // Tens of millions of pixels to the left: the part in the control is the layout's.
        let lay = UiText::layout(&long, UiFont::Regular, 1000.0, None).unwrap();
        let x = -(lay.size()[0] / 2.0).round();
        let far = DrawOp::Text { text: long, color: None, x, y: -400.0, size: 1000.0, style: String::new(), font: UiFont::Regular };
        let painted = paint(&[far], 1.0, vec2(50.0, 40.0));
        assert!(!painted.is_empty() && area(&painted) <= 51 * 41 && painted.iter().any(|p| p.alpha.iter().any(|&a| a > 0)));
        for p in &painted {
            shows_the_layout(p, &lay, 1.0, text_origin(1.0, x as f32, -400.0)).unwrap();
        }
        for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e30, -1e30] {
            let at = DrawOp::Text { text: "abc".into(), color: None, x, y: 0.0, size: 12.0, style: String::new(), font: UiFont::Regular };
            assert!(paint(&[at], 1.0, vec2(50.0, 40.0)).is_empty(), "{x}");
        }
    }
}
