//! UI fonts and the optional craft-fonts build input (`CRAFT_FONTS_DIR`): with it, Japanese renders
//! with real glyphs in every UI font family (also on the web, which has no system fonts); without
//! it, the UI installs and renders exactly as before.

use effectcraft_ui_egui::theme::{self, ThemeKind, Tokens};

const FAMILIES: [&str; 4] = ["proportional", "monospace", "medium", "semibold"];

fn family(name: &str) -> egui::FontFamily {
    match name {
        "proportional" => egui::FontFamily::Proportional,
        "monospace" => egui::FontFamily::Monospace,
        n => egui::FontFamily::Name(n.into()),
    }
}

fn installed() -> egui::Context {
    let ctx = egui::Context::default();
    theme::install(&ctx, &Tokens::for_kind(ThemeKind::Dark));
    // Fonts are loaded at the start of the first frame.
    let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
    // Nothing renders here; drop the atlas upload instead of applying it.
    out.textures_delta.clear();
    ctx
}

/// Where a glyph sits in the font atlas.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Uv {
    min: [u16; 2],
    max: [u16; 2],
    size: egui::Vec2,
}

/// The atlas rectangle of each character of `text`, and of the replacement glyph.
fn uvs(ctx: &egui::Context, text: &str, fam: &str) -> (Vec<(char, Uv)>, Uv) {
    let font = egui::FontId::new(13.0, family(fam));
    ctx.fonts_mut(|fonts| {
        // A non-character first: always the replacement glyph.
        let mut out: Vec<(char, Uv)> = Vec::new();
        for c in std::iter::once('\u{10ffff}').chain(text.chars()) {
            let g = fonts.layout_no_wrap(c.to_string(), font.clone(), egui::Color32::WHITE);
            let uv = g.rows.first().and_then(|r| r.glyphs.first()).map(|g| Uv { min: g.uv_rect.min, max: g.uv_rect.max, size: g.uv_rect.size });
            out.push((c, uv.unwrap_or(Uv { min: [0, 0], max: [0, 0], size: egui::Vec2::ZERO })));
        }
        let missing = out.remove(0).1;
        (out, missing)
    })
}

#[test]
fn craft_fonts_render_japanese_in_every_ui_family() {
    if effectcraft_text::fonts::craft_fonts_jpan().is_empty() {
        eprintln!("built without craft-fonts (CRAFT_FONTS_DIR unset); skipping");
        return;
    }
    let ctx = installed();
    for fam in FAMILIES {
        let (glyphs, missing) = uvs(&ctx, "日本語の文字", fam);
        for (c, uv) in glyphs {
            assert!(uv.size.x > 0.0 && uv.size.y > 0.0, "empty {c} in {fam}");
            assert_ne!(uv, missing, "replacement glyph (tofu) for {c} in {fam}");
        }
    }
}

#[test]
fn ui_fonts_work_without_craft_fonts() {
    // Runs with or without craft-fonts: the app's own fonts render Latin text in every family.
    let ctx = installed();
    for fam in FAMILIES {
        let (glyphs, missing) = uvs(&ctx, "EffectCraft", fam);
        for (c, uv) in glyphs {
            assert!(uv.size.x > 0.0 && uv.size.y > 0.0, "empty {c} in {fam}");
            assert_ne!(uv, missing, "replacement glyph for {c} in {fam}");
        }
    }
    if effectcraft_text::fonts::CRAFT_FONTS.is_empty() {
        assert!(effectcraft_text::fonts::craft_fonts_jpan().is_empty());
    }
}
