//! Design tokens. Every widget reads colours/sizes from [`Tokens`] so themes apply everywhere.
//!
//! The default theme follows the look of After Effects' dark UI (values estimated from public
//! documentation and product knowledge, tuned by eye; see `plan/aftereffects/README.md` §2). Fonts
//! are Inter + JetBrains Mono (OFL).

use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use egui::FontTweak;
#[cfg(not(target_arch = "wasm32"))]
use egui::emath::GuiRounding as _;
use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeKind {
    #[default]
    Dark,
    Darker,
    Light,
}

impl ThemeKind {
    pub fn from_name(s: &str) -> Option<ThemeKind> {
        match s.to_ascii_lowercase().as_str() {
            "dark" | "default" => Some(ThemeKind::Dark),
            "darker" | "darkest" => Some(ThemeKind::Darker),
            "light" => Some(ThemeKind::Light),
            _ => None,
        }
    }

    /// The settings value (`appearance.theme`, `appearance.darkTheme`, `appearance.lightTheme`).
    pub fn name(self) -> &'static str {
        match self {
            ThemeKind::Dark => "dark",
            ThemeKind::Darker => "darker",
            ThemeKind::Light => "light",
        }
    }

    pub fn is_light(self) -> bool {
        self == ThemeKind::Light
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub kind: ThemeKind,
    pub app_bg: Color32,
    pub header_bg: Color32,
    pub panel_bg: Color32,
    pub tab_text: Color32,
    pub tab_text_active: Color32,
    pub focus: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub icon: Color32,
    pub icon_active: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub field_bg: Color32,
    pub field_border: Color32,
    pub separator: Color32,
    pub row: Color32,
    pub row_alt: Color32,
    pub row_selected: Color32,
    pub hot_text: Color32,
    pub tl_bg: Color32,
    pub tl_ruler_bg: Color32,
    pub tl_ruler_tick: Color32,
    pub tl_ruler_text: Color32,
    pub cti: Color32,
    pub work_area: Color32,
    pub cache_green: Color32,
    /// Timeline cache bar for frames held in the disk cache (not in RAM).
    pub cache_blue: Color32,
    pub keyframe: Color32,
    pub keyframe_selected: Color32,
    pub pasteboard: Color32,
    pub timecode: Color32,
    pub danger: Color32,
    pub warning: Color32,
    /// Effect Controls effect headers, and the header of the effect being worked on (it or one
    /// of its properties selected).
    pub fx_header: Color32,
    pub fx_header_active: Color32,
    /// Timeline: a selected layer's row in the time graph, the boxes behind the A/V and layer
    /// switches, the time navigator and the work area bar.
    pub tl_graph_selected: Color32,
    pub switch_well: Color32,
    pub tl_nav_bar: Color32,
    pub tl_work_area_bar: Color32,
    /// The Timeline's expression field and the colour of enabled expressions (their text and
    /// the names of properties that have one).
    pub expr_bg: Color32,
    pub expr_text: Color32,
    /// Label colours (`Label::ALL` order), from Settings ▸ Labels.
    pub labels: [Color32; 17],
    pub radius: f32,
    pub radius_sm: f32,
    pub gap: f32,
    pub tab_h: f32,
    pub row_h: f32,
    /// Settings ▸ Appearance ▸ Use Gradients: panel tab strips and buttons get a soft vertical
    /// gradient (off: flat fills).
    pub gradients: bool,
}

/// A vertical gradient fill (`top` → `bottom`) over `rect`.
pub fn gradient_rect(painter: &egui::Painter, rect: egui::Rect, top: Color32, bottom: Color32) {
    let mut m = egui::Mesh::default();
    m.colored_vertex(rect.left_top(), top);
    m.colored_vertex(rect.right_top(), top);
    m.colored_vertex(rect.right_bottom(), bottom);
    m.colored_vertex(rect.left_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(m));
}

impl Tokens {
    /// The gradient top colour for a fill (a little lighter), or the fill itself when gradients
    /// are off.
    pub fn grad_top(&self, c: Color32) -> Color32 {
        if self.gradients { c.lerp_to_gamma(Color32::WHITE, 0.06) } else { c }
    }

    /// The tokens a property's value fields use (#1306): with an expression driving the property
    /// (`expression`, [`Property::has_expression`]) its values show in the expression colour, red
    /// as in After Effects, hovered and dragged too; otherwise these tokens. For the values only:
    /// the name, stopwatch and swatches keep the ordinary tokens.
    ///
    /// [`Property::has_expression`]: effectcraft_engine::project::Property::has_expression
    pub fn for_value(self, expression: bool) -> Tokens {
        if expression { Tokens { hot_text: self.expr_text, accent_hover: self.expr_text, ..self } } else { self }
    }
}

impl Tokens {
    pub fn for_kind(kind: ThemeKind) -> Self {
        let dark = Tokens {
            kind,
            app_bg: Color32::from_rgb(0x12, 0x12, 0x12),
            header_bg: Color32::from_rgb(0x1f, 0x1f, 0x1f),
            panel_bg: Color32::from_rgb(0x23, 0x23, 0x23),
            tab_text: Color32::from_rgb(0x9a, 0x9a, 0x9a),
            tab_text_active: Color32::from_rgb(0xe3, 0xe3, 0xe3),
            focus: Color32::from_rgb(0x2d, 0x8c, 0xeb),
            accent: Color32::from_rgb(0x2d, 0x8c, 0xeb),
            accent_hover: Color32::from_rgb(0x4a, 0xa0, 0xf5),
            text: Color32::from_rgb(0xc8, 0xc8, 0xc8),
            text_dim: Color32::from_rgb(0x9a, 0x9a, 0x9a),
            text_faint: Color32::from_rgb(0x8c, 0x8c, 0x8c),
            icon: Color32::from_rgb(0xb4, 0xb4, 0xb4),
            icon_active: Color32::from_rgb(0xf0, 0xf0, 0xf0),
            hover: Color32::from_rgb(0x30, 0x30, 0x30),
            pressed: Color32::from_rgb(0x45, 0x45, 0x45),
            field_bg: Color32::from_rgb(0x17, 0x17, 0x17),
            field_border: Color32::from_rgb(0x3a, 0x3a, 0x3a),
            separator: Color32::from_rgb(0x34, 0x34, 0x34),
            row: Color32::from_rgb(0x23, 0x23, 0x23),
            row_alt: Color32::from_rgb(0x27, 0x27, 0x27),
            row_selected: Color32::from_rgb(0x3a, 0x3a, 0x3a),
            hot_text: Color32::from_rgb(0x3d, 0x8f, 0xf5),
            tl_bg: Color32::from_rgb(0x1b, 0x1b, 0x1b),
            tl_ruler_bg: Color32::from_rgb(0x23, 0x23, 0x23),
            tl_ruler_tick: Color32::from_rgb(0x70, 0x70, 0x70),
            tl_ruler_text: Color32::from_rgb(0x9a, 0x9a, 0x9a),
            cti: Color32::from_rgb(0x2d, 0x8c, 0xeb),
            work_area: Color32::from_rgb(0x4a, 0x4a, 0x4a),
            cache_green: Color32::from_rgb(0x3c, 0xa6, 0x4c),
            cache_blue: Color32::from_rgb(0x3a, 0x6f, 0xd8),
            keyframe: Color32::from_rgb(0xa8, 0xa8, 0xa8),
            keyframe_selected: Color32::from_rgb(0x3d, 0x8f, 0xf5),
            pasteboard: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            timecode: Color32::from_rgb(0x3d, 0x8f, 0xf5),
            danger: Color32::from_rgb(0xe0, 0x4a, 0x3c),
            warning: Color32::from_rgb(0xe8, 0x9a, 0x2c),
            fx_header: Color32::from_rgb(0x2a, 0x2a, 0x2a),
            fx_header_active: Color32::from_rgb(0x2f, 0x3a, 0x52),
            tl_graph_selected: Color32::from_rgb(0x2a, 0x2a, 0x2a),
            switch_well: Color32::from_rgb(0x19, 0x19, 0x19),
            tl_nav_bar: Color32::from_rgb(0x55, 0x55, 0x55),
            tl_work_area_bar: Color32::from_rgb(0x5c, 0x5c, 0x5c),
            expr_bg: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            expr_text: Color32::from_rgb(0xe8, 0x7c, 0x5c),
            labels: default_labels(),
            radius: 6.0,
            radius_sm: 3.0,
            gap: 4.0,
            tab_h: 30.0,
            row_h: 19.0,
            gradients: true,
        };
        match kind {
            ThemeKind::Dark => dark,
            ThemeKind::Darker => Tokens {
                app_bg: Color32::from_rgb(0x08, 0x08, 0x08),
                header_bg: Color32::from_rgb(0x16, 0x16, 0x16),
                panel_bg: Color32::from_rgb(0x19, 0x19, 0x19),
                row: Color32::from_rgb(0x19, 0x19, 0x19),
                row_alt: Color32::from_rgb(0x1d, 0x1d, 0x1d),
                tl_bg: Color32::from_rgb(0x12, 0x12, 0x12),
                tl_ruler_bg: Color32::from_rgb(0x19, 0x19, 0x19),
                field_bg: Color32::from_rgb(0x0e, 0x0e, 0x0e),
                pasteboard: Color32::from_rgb(0x11, 0x11, 0x11),
                ..dark
            },
            ThemeKind::Light => Tokens {
                app_bg: Color32::from_rgb(0xb4, 0xb4, 0xb4),
                header_bg: Color32::from_rgb(0xd6, 0xd6, 0xd6),
                panel_bg: Color32::from_rgb(0xe6, 0xe6, 0xe6),
                tab_text: Color32::from_rgb(0x5a, 0x5a, 0x5a),
                tab_text_active: Color32::from_rgb(0x14, 0x14, 0x14),
                text: Color32::from_rgb(0x22, 0x22, 0x22),
                text_dim: Color32::from_rgb(0x5a, 0x5a, 0x5a),
                text_faint: Color32::from_rgb(0x68, 0x68, 0x68),
                hot_text: Color32::from_rgb(0x00, 0x5a, 0x9c),
                timecode: Color32::from_rgb(0x00, 0x5a, 0x9c),
                icon: Color32::from_rgb(0x3c, 0x3c, 0x3c),
                icon_active: Color32::from_rgb(0x10, 0x10, 0x10),
                hover: Color32::from_rgb(0xd0, 0xd0, 0xd0),
                pressed: Color32::from_rgb(0xc0, 0xc0, 0xc0),
                field_bg: Color32::from_rgb(0xfa, 0xfa, 0xfa),
                field_border: Color32::from_rgb(0xaa, 0xaa, 0xaa),
                separator: Color32::from_rgb(0xc8, 0xc8, 0xc8),
                row: Color32::from_rgb(0xe6, 0xe6, 0xe6),
                row_alt: Color32::from_rgb(0xde, 0xde, 0xde),
                row_selected: Color32::from_rgb(0xaa, 0xc8, 0xf0),
                tl_bg: Color32::from_rgb(0xd2, 0xd2, 0xd2),
                tl_ruler_bg: Color32::from_rgb(0xe2, 0xe2, 0xe2),
                tl_ruler_text: Color32::from_rgb(0x50, 0x50, 0x50),
                pasteboard: Color32::from_rgb(0xa8, 0xa8, 0xa8),
                // Keyframes have no outline: dark enough to stand out on the light time graph.
                keyframe: Color32::from_rgb(0x6c, 0x6c, 0x6c),
                // Custom-painted surfaces in the Light palette, so dark text stays readable on
                // them (#644).
                fx_header: Color32::from_rgb(0xd8, 0xd8, 0xd8),
                fx_header_active: Color32::from_rgb(0xb0, 0xcf, 0xec),
                tl_graph_selected: Color32::from_rgb(0xc2, 0xc2, 0xc2),
                switch_well: Color32::from_rgb(0xf2, 0xf2, 0xf2),
                tl_nav_bar: Color32::from_rgb(0x9a, 0x9a, 0x9a),
                tl_work_area_bar: Color32::from_rgb(0x94, 0x94, 0x94),
                expr_bg: Color32::from_rgb(0xf2, 0xf2, 0xf2),
                expr_text: Color32::from_rgb(0xa3, 0x34, 0x17),
                ..dark
            },
        }
    }

    pub fn mono(size: f32) -> FontId {
        FontId::new(size, FontFamily::Monospace)
    }
    pub fn ui(size: f32) -> FontId {
        FontId::new(size, FontFamily::Proportional)
    }
    pub fn semibold(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("semibold".into()))
    }
    pub fn medium(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("medium".into()))
    }
    /// sRGB colour of an item/layer label (Settings ▸ Labels).
    pub fn label(&self, l: effectcraft_color::Label) -> Color32 {
        let i = effectcraft_color::Label::ALL.iter().position(|x| *x == l).unwrap_or(0);
        self.labels[i]
    }

    /// Theme tokens for the current settings: theme, UI brightness and label colours.
    /// The tokens for theme `kind` (resolved from the appearance mode) with the rest of the
    /// Appearance settings: brightness, label colours, gradients.
    pub fn from_prefs(p: &effectcraft_engine::prefs::Prefs, kind: ThemeKind) -> Tokens {
        let mut t = Tokens::for_kind(kind).with_brightness(p.appearance.brightness as f32);
        for (i, l) in effectcraft_color::Label::ALL.iter().enumerate() {
            let [r, g, b] = p.label_rgb(*l);
            t.labels[i] = Color32::from_rgb(r, g, b);
        }
        t.gradients = p.appearance.use_gradients;
        t
    }

    /// Lighten (b > 0) or darken (b < 0) the interface greys, like After Effects' brightness
    /// slider. Accent colours, labels and keyframe colours are kept.
    pub fn with_brightness(mut self, b: f32) -> Tokens {
        let b = b.clamp(-1.0, 1.0);
        if b.abs() < 1e-3 {
            return self;
        }
        let adj = |c: &mut Color32| {
            let f = |v: u8| -> u8 {
                let v = v as f32;
                (if b > 0.0 { v + (110.0 - v).max(0.0) * b * 0.8 + 10.0 * b } else { v * (1.0 + b * 0.6) }).round().clamp(0.0, 255.0) as u8
            };
            *c = Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()));
        };
        for c in [
            &mut self.app_bg,
            &mut self.header_bg,
            &mut self.panel_bg,
            &mut self.hover,
            &mut self.pressed,
            &mut self.field_bg,
            &mut self.field_border,
            &mut self.separator,
            &mut self.row,
            &mut self.row_alt,
            &mut self.row_selected,
            &mut self.tl_bg,
            &mut self.tl_ruler_bg,
            &mut self.pasteboard,
            &mut self.work_area,
            &mut self.fx_header,
            &mut self.tl_graph_selected,
            &mut self.switch_well,
            &mut self.expr_bg,
        ] {
            adj(c);
        }
        self
    }
}

/// The default label colours, one RGB colour per [`effectcraft_color::Label`] in
/// `Label::ALL` order.
fn default_labels() -> [Color32; 17] {
    effectcraft_color::Label::ALL.map(|l| {
        let [r, g, b] = l.rgb();
        Color32::from_rgb(r, g, b)
    })
}

static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
static INTER_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
static JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
/// Chrome symbols (`✔` U+2714, `▣` U+25A3, …) that Inter, Inter Medium/SemiBold and
/// JetBrains Mono do not cover. 0.37's default font set no longer includes the old emoji
/// fonts, and named families (`medium`, `semibold`) never received egui's default fallbacks.
static NOTO_SYMBOLS2: &[u8] = include_bytes!("../../../assets/fonts/NotoSansSymbols2-Regular.ttf");

/// Install fonts (Inter, Inter Medium/SemiBold, JetBrains Mono) and egui visuals. `language` is the
/// resolved interface language from `crate::i18n::language` (`en`, `ja`, `zh-hans` or `zh-hant`),
/// not the stored `general.language` preference: `system` is already resolved there, so a Chinese
/// system draws Chinese with the default setting. The CJK fallback that draws the interface's own
/// script is registered first, so a Chinese interface is drawn by a Chinese face and a Japanese one
/// by a Japanese face. `install` is called again when the language changes (see the `EffectcraftApp`
/// frame loop), so the fonts do not need a restart. Installed faces the language doesn't need are
/// read the first time the interface paints text that needs them ([`crate::fallback_fonts`]).
pub fn install(ctx: &egui::Context, t: &Tokens, language: &str) {
    install_fonts(ctx, language);
    ctx.add_plugin(crate::fallback_fonts::Watch::new(language));
    ctx.with_plugin(|w: &mut crate::fallback_fonts::Watch| w.set_language(language));
    apply_visuals(ctx, t);
}

/// The fonts [`install`] sets for `language`, with the installed fallbacks read so far.
pub(crate) fn install_fonts(ctx: &egui::Context, language: &str) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("inter".into(), Arc::new(FontData::from_static(INTER_REGULAR)));
    fonts.font_data.insert("inter-medium".into(), Arc::new(FontData::from_static(INTER_MEDIUM)));
    fonts.font_data.insert("inter-semibold".into(), Arc::new(FontData::from_static(INTER_SEMIBOLD)));
    fonts.font_data.insert("jbmono".into(), Arc::new(FontData::from_static(JETBRAINS_MONO)));
    fonts.font_data.insert("noto-symbols2".into(), Arc::new(FontData::from_static(NOTO_SYMBOLS2)));
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "jbmono".into());
    fonts.families.insert(FontFamily::Name("semibold".into()), vec!["inter-semibold".into(), "inter".into()]);
    fonts.families.insert(FontFamily::Name("medium".into()), vec!["inter-medium".into(), "inter".into()]);
    // Inter and egui's own fallbacks have no small triangles (the `▸` of "File ▸ Import"), which
    // would draw as missing-glyph boxes: JetBrains Mono fills in the symbols the others lack.
    // Noto Sans Symbols 2 follows it for the check/box glyphs (`✔`, `▣`) that JetBrains Mono
    // lacks; without it, named families such as `medium` fall straight through to tofu.
    for (family, stack) in &mut fonts.families {
        if *family != FontFamily::Monospace {
            stack.push("jbmono".into());
        }
        stack.push("noto-symbols2".into());
    }
    install_script_fallbacks(&mut fonts, language);
    ctx.set_fonts(fonts);
}

/// Scripts the bundled fonts lack beyond CJK, each probed with one of its letters: names typed in
/// them (layers, items, comments) are drawn by an installed face instead of missing-glyph boxes
/// (#394). egui shapes each run of one face, so Arabic letters join and a word reads right to
/// left; it has no bidirectional layout, so the words of a name still follow each other left to
/// right.
#[cfg(not(target_arch = "wasm32"))]
const SCRIPT_PROBES: &[(char, &str)] = &[
    ('م', "arabic-system"),
    ('א', "hebrew-system"),
    ('क', "devanagari-system"),
    ('ক', "bengali-system"),
    ('ਕ', "gurmukhi-system"),
    ('ક', "gujarati-system"),
    ('க', "tamil-system"),
    ('క', "telugu-system"),
    ('ಕ', "kannada-system"),
    ('ക', "malayalam-system"),
    ('ก', "thai-system"),
    ('ა', "georgian-system"),
];

/// The size the fallback baseline correction is derived at. The correction is an em fraction, so the
/// value holds at every interface size; any size in the interface's range gives the same answer.
#[cfg(not(target_arch = "wasm32"))]
const BASELINE_METRICS_PX: f32 = 12.0;

/// A face's ascent and row height at `px`, rounded the way epaint rounds them: ascent, descent and
/// line gap separately to 1/32, then `ascent + descent + line_gap` (epaint's descent is negative,
/// this one is positive).
#[cfg(not(target_arch = "wasm32"))]
fn ascent_and_height(m: &effectcraft_text::fonts::VMetrics) -> (f32, f32) {
    let ascent = m.ascent.round_ui();
    let descent = m.descent.round_ui();
    let line_gap = m.line_gap.round_ui();
    (ascent, ascent + descent + line_gap)
}

/// The `FontTweak` that puts a fallback face's glyphs on the row font's baseline.
///
/// epaint draws a glyph at `A_face + 0.5·(H_row − H_face)` (`galley_from_rows` in epaint's
/// `text_layout.rs`), where `A` and `H` are the ascent and the row height of the face that draws the
/// glyph and of the row's own font, both from `styled_metrics` in epaint's `font.rs`. Shifting the
/// face by `(A_row − A_face) − 0.5·(H_row − H_face)` cancels the offset; as an em fraction one
/// number serves every size, because `FontTweak` applies `size · y_offset_factor`.
///
/// The same expression also carries `valign · (max_row_height − line_height)`, which cancels here:
/// every glyph of a row takes the section font's row height, so it is the same term for the Latin
/// and the fallback glyph. The remaining `snap_to_pixel` difference is likewise shared and cancels.
/// (#1309)
///
/// The UI families do not share a row font — Proportional uses Inter, Monospace JetBrains Mono —
/// and their factors differ by 0.0026 em, which is 0.03 px at 12 pt: inside the 1/32 that is already
/// rounded away above, so one factor per face is enough. The test
/// `fallback_glyphs_sit_on_the_row_baseline` pins the result.
#[cfg(not(target_arch = "wasm32"))]
fn baseline_tweak(row: &effectcraft_text::fonts::Face, face: &effectcraft_text::fonts::Face) -> FontTweak {
    let px = BASELINE_METRICS_PX;
    let (a_row, h_row) = ascent_and_height(&row.metrics(px));
    let (a_face, h_face) = ascent_and_height(&face.metrics(px));
    FontTweak { y_offset_factor: ((a_row - a_face) - 0.5 * (h_row - h_face)) / px, ..Default::default() }
}

/// Register the system fallbacks for scripts the bundled fonts lack in every font family, and
/// return their names in registration order (empty when nothing installed covers a probe, so a
/// machine without those fonts simply keeps the bundled ones). A face that covers several probes
/// (Segoe UI draws Arabic and Hebrew, Nirmala UI every Indic script) is registered once.
///
/// CJK comes first. The text engine has its own script-aware fallback (#84), but which face it
/// picks for Han ideographs depends on the locale environment: a Chinese interface on a
/// Japanese-locale machine would be drawn with the Japanese face. The interface language is the
/// better signal, so the probe for its script comes first — kana for Japanese, a Han character for
/// the Chinese catalogs. Both scripts are registered, so file, layer and template names in the
/// other one still render; then [`SCRIPT_PROBES`], then Hangul, since the Japanese and Chinese faces
/// may lack it (Hiragino Sans on macOS has none). The web build has no system fonts
/// and installs nothing.
///
/// Only the faces the interface language needs (its CJK faces and the catalog's extra faces) are
/// read here; the others are registered once painted text needed them ([`LazyFallback`]).
fn install_script_fallbacks(family_fonts: &mut FontDefinitions, language: &str) -> Vec<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (family_fonts, language);
        Vec::new()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use effectcraft_text::fonts;
        // Han text follows the interface language, not the operating system's locale.
        fonts::set_cjk_locale(cjk_locale(language));
        // the row font the fallbacks' baseline tweak is measured against (#1309)
        let base_face = fonts::face(fonts::resolve("Inter", "Regular").face);
        let han = (han_probe(language), "chinese-system");
        let cjk = if is_chinese(language) { [han, KANA] } else { [KANA, han] };
        let mut names = Vec::new();
        let mut seen = Vec::new();
        // A font covering one Han probe may be a sparse subset. Register additional faces
        // needed by the actual interface catalog instead of assuming whole-script coverage.
        let extra = if is_chinese(language) { crate::i18n::catalog_characters(language) } else { Vec::new() };
        let extra: Vec<_> = extra.into_iter().filter(|ch| *ch >= '\u{2e80}').map(|ch| (ch, "catalog-system")).collect();
        // 256-character blocks where a catalog character found no face: with nothing installed
        // covering them, each lookup would read every installed font file again.
        let mut uncovered_blocks = std::collections::HashSet::new();
        for &(probe, name) in cjk.iter().chain(SCRIPT_PROBES).chain(std::iter::once(&HANGUL)).chain(&extra) {
            let catalog = name == "catalog-system";
            if catalog && (uncovered_blocks.contains(&(probe as u32 >> 8)) || seen.iter().any(|&id| fonts::face(id).has_char(probe))) {
                continue;
            }
            // installed faces the language doesn't need wait for painted text to need them
            if !catalog && !needed(name, language) && !is_read(name) {
                continue;
            }
            let id = interface_face(probe, name, language);
            let face = fonts::face(id);
            if !face.has_char(probe) {
                if catalog {
                    uncovered_blocks.insert(probe as u32 >> 8);
                }
                continue; // nothing installed covers this script
            }
            if seen.contains(&id) {
                continue;
            }
            let Some(data) = fallback_data(&face, &base_face) else { continue };
            seen.push(id);
            let name = if name == "catalog-system" { format!("{name}-{:x}", probe as u32) } else { name.to_string() };
            family_fonts.font_data.insert(name.clone(), Arc::new(data));
            for family in family_fonts.families.values_mut() {
                family.push(name.to_string());
            }
            names.push(name.to_string());
        }
        names
    }
}

// Kana is unambiguously Japanese; Han ideographs are shared, so the order decides which language's
// face draws them.
#[cfg(not(target_arch = "wasm32"))]
const KANA: (char, &str) = ('あ', "japanese-system");
// Hangul is probed last: with no Korean font installed the lookup can land on a catch-all face
// (Unifont) that also covers Arabic or Indic text but can't shape it, so the script-specific faces
// must be registered ahead of it. A face already seen is skipped.
#[cfg(not(target_arch = "wasm32"))]
const HANGUL: (char, &str) = ('한', "korean-system");

/// The installed face the interface draws `probe` (of fallback `name`) with for `language`. The
/// interface asks for the platform's UI face first; the order a composed frame renders with is
/// `fonts::fallback_for`'s and is not touched here. Another context may install a different
/// language concurrently, so the shared fallback is asked with this context's own locale rather
/// than the global request.
#[cfg(not(target_arch = "wasm32"))]
fn interface_face(probe: char, name: &str, language: &str) -> effectcraft_text::fonts::FaceId {
    use effectcraft_text::fonts;
    let base = fonts::resolve("Inter", "Regular").face;
    interface_fallback_candidates(name, language)
        .iter()
        .find_map(|family| {
            let r = fonts::resolve(family, "Regular");
            (!r.missing && fonts::face(r.face).has_char(probe)).then_some(r.face)
        })
        .unwrap_or_else(|| fonts::fallback_for_locale(probe, base, cjk_locale(language)))
}

/// Whether the interface language needs fallback `name` whatever it paints: a Japanese or Chinese
/// interface draws its own labels with the CJK faces, in the language's order.
#[cfg(not(target_arch = "wasm32"))]
fn needed(name: &str, language: &str) -> bool {
    matches!(name, "japanese-system" | "chinese-system") && !cjk_locale(language).is_empty()
}

/// The installed fallbacks read for painted text so far ([`crate::fallback_fonts`]), by name.
static READ: std::sync::RwLock<Vec<&'static str>> = std::sync::RwLock::new(Vec::new());

fn is_read(name: &str) -> bool {
    READ.read().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&name)
}

/// The code points of each installed fallback's script: the text its face is read for (Han, kana
/// and bopomofo with the CJK symbols, punctuation and full-width forms; Hangul with its enclosed and
/// half-width forms; each other script with its extensions and presentation forms), as inclusive
/// `(first, last)` pairs.
const SCRIPT_RANGES: &[(&str, &[(char, char)])] = &[
    (
        "japanese-system",
        &[
            ('\u{2e80}', '\u{2fdf}'),
            ('\u{2ff0}', '\u{303f}'),
            ('\u{3040}', '\u{30ff}'),
            ('\u{3190}', '\u{319f}'),
            ('\u{31c0}', '\u{33ff}'),
            ('\u{3400}', '\u{4dbf}'),
            ('\u{4e00}', '\u{9fff}'),
            ('\u{f900}', '\u{faff}'),
            ('\u{fe30}', '\u{fe4f}'),
            ('\u{ff00}', '\u{ffef}'),
            ('\u{1b000}', '\u{1b16f}'),
            ('\u{20000}', '\u{3ffff}'),
        ],
    ),
    (
        "chinese-system",
        &[
            ('\u{2e80}', '\u{2fdf}'),
            ('\u{2ff0}', '\u{303f}'),
            ('\u{3100}', '\u{312f}'),
            ('\u{3190}', '\u{33ff}'),
            ('\u{3400}', '\u{4dbf}'),
            ('\u{4e00}', '\u{9fff}'),
            ('\u{f900}', '\u{faff}'),
            ('\u{fe30}', '\u{fe4f}'),
            ('\u{ff00}', '\u{ffef}'),
            ('\u{20000}', '\u{3ffff}'),
        ],
    ),
    (
        "arabic-system",
        &[
            ('\u{600}', '\u{6ff}'),
            ('\u{750}', '\u{77f}'),
            ('\u{870}', '\u{8ff}'),
            ('\u{fb50}', '\u{fdff}'),
            ('\u{fe70}', '\u{fefc}'),
            ('\u{1ee00}', '\u{1eeff}'),
        ],
    ),
    ("hebrew-system", &[('\u{590}', '\u{5ff}'), ('\u{fb1d}', '\u{fb4f}')]),
    ("devanagari-system", &[('\u{900}', '\u{97f}'), ('\u{a8e0}', '\u{a8ff}'), ('\u{1cd0}', '\u{1cff}')]),
    ("bengali-system", &[('\u{980}', '\u{9ff}')]),
    ("gurmukhi-system", &[('\u{a00}', '\u{a7f}')]),
    ("gujarati-system", &[('\u{a80}', '\u{aff}')]),
    ("tamil-system", &[('\u{b80}', '\u{bff}'), ('\u{11fc0}', '\u{11fff}')]),
    ("telugu-system", &[('\u{c00}', '\u{c7f}')]),
    ("kannada-system", &[('\u{c80}', '\u{cff}')]),
    ("malayalam-system", &[('\u{d00}', '\u{d7f}')]),
    ("thai-system", &[('\u{e00}', '\u{e7f}')]),
    ("georgian-system", &[('\u{10a0}', '\u{10ff}'), ('\u{1c90}', '\u{1cbf}'), ('\u{2d00}', '\u{2d2f}')]),
    (
        "korean-system",
        &[
            ('\u{1100}', '\u{11ff}'),
            ('\u{3130}', '\u{318f}'),
            ('\u{3200}', '\u{321e}'),
            ('\u{3260}', '\u{327e}'),
            ('\u{a960}', '\u{a97f}'),
            ('\u{ac00}', '\u{d7ff}'),
            ('\u{ffa0}', '\u{ffdc}'),
        ],
    ),
];

/// An installed fallback [`crate::fallback_fonts::Watch`] reads when painted text needs it.
pub(crate) struct LazyFallback {
    probe: char,
    name: &'static str,
    language: String,
}

impl LazyFallback {
    pub(crate) fn name(&self) -> &'static str {
        self.name
    }

    /// The code points of the face this fallback would read, without reading it: the face the
    /// interface asks for first (its UI family, else the first of the platform's usual families for
    /// the script). Looked up once per fallback and language.
    fn coverage(&self) -> Arc<Vec<(u32, u32)>> {
        type Cache = Vec<((&'static str, String), Arc<Vec<(u32, u32)>>)>;
        static CACHE: std::sync::Mutex<Cache> = std::sync::Mutex::new(Vec::new());
        let key = (self.name, self.language.clone());
        if let Some((_, ranges)) = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|(k, _)| *k == key) {
            return ranges.clone();
        }
        #[cfg(target_arch = "wasm32")]
        let ranges: Arc<Vec<(u32, u32)>> = Arc::default();
        #[cfg(not(target_arch = "wasm32"))]
        let ranges = {
            use effectcraft_text::fonts;
            let base = fonts::resolve("Inter", "Regular").face;
            let ui = interface_fallback_candidates(self.name, &self.language).iter().map(|family| fonts::resolve(family, "Regular")).find(|r| !r.missing);
            let id = ui.map(|r| r.face).or_else(|| fonts::usual_faces(self.probe, base, cjk_locale(&self.language)).first().copied());
            Arc::new(id.map(|id| fonts::face(id).coverage()).unwrap_or_default())
        };
        CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((key, ranges.clone()));
        ranges
    }

    /// The installed face for the interface language, when one covers the probe.
    fn face(&self) -> Option<Arc<effectcraft_text::fonts::Face>> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (self.probe, &self.language);
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let face = effectcraft_text::fonts::face(interface_face(self.probe, self.name, &self.language));
            face.has_char(self.probe).then_some(face)
        }
    }

    /// The face read for this fallback, as `theme::install_fonts` registers it (the same name,
    /// bytes and baseline tweak), for egui to add while it lays text out
    /// ([`crate::fallback_fonts`]). `None` when the face isn't read or there is none.
    pub(crate) fn insert(&self) -> Option<egui::FontInsert> {
        if !is_read(self.name) {
            return None;
        }
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let base = effectcraft_text::fonts::face(effectcraft_text::fonts::resolve("Inter", "Regular").face);
            let face = self.face()?;
            let data = fallback_data(&face, &base)?;
            Some(egui::FontInsert { name: self.name.to_owned(), data, families: Vec::new() })
        }
    }
}

impl crate::fallback_fonts::Candidate for LazyFallback {
    fn in_script(&self, c: char) -> bool {
        SCRIPT_RANGES.iter().find(|(name, _)| *name == self.name).is_some_and(|(_, ranges)| ranges.iter().any(|&(first, last)| (first..=last).contains(&c)))
    }
    fn is_read(&self) -> bool {
        is_read(self.name)
    }
    fn covers(&self, c: char) -> bool {
        self.is_read() && self.face().is_some_and(|f| f.has_char(c))
    }
    fn might_cover(&self, c: char) -> bool {
        if self.is_read() {
            return self.covers(c);
        }
        let cp = u32::from(c);
        let within = |&(a, b): &(u32, u32)| {
            if b < cp {
                std::cmp::Ordering::Less
            } else if a > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        };
        self.coverage().binary_search_by(within).is_ok()
    }
    fn read(&self) -> bool {
        if self.face().is_none() {
            return false;
        }
        let mut read = READ.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !read.contains(&self.name) {
            read.push(self.name);
        }
        true
    }
}

/// The installed fallbacks for `language` that are read when painted text needs them, in the order
/// [`install_script_fallbacks`] registers them.
pub(crate) fn lazy_fallbacks(language: &str) -> Vec<LazyFallback> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = language;
        Vec::new()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let han = (han_probe(language), "chinese-system");
        let cjk = if is_chinese(language) { [han, KANA] } else { [KANA, han] };
        cjk.iter()
            .chain(SCRIPT_PROBES)
            .chain(std::iter::once(&HANGUL))
            .filter(|(_, name)| !needed(name, language))
            .map(|&(probe, name)| LazyFallback { probe, name, language: language.to_owned() })
            .collect()
    }
}

/// `face` for egui, sharing the bytes the font database keeps for it. A copy of the file
/// (`font.data().as_bytes().to_vec()`) meant two copies of every fallback face for the life of the
/// process: the database's and egui's. `None` when the face's file can't be read.
#[cfg(not(target_arch = "wasm32"))]
fn shared(face: &effectcraft_text::fonts::Face) -> Option<FontData> {
    Some(FontData::from_blob(face.shared_bytes()?, face.info.index))
}

/// The font data a fallback face is registered with: its bytes, shared ([`shared`]), and the
/// tweak that puts its glyphs on the baseline of `row`, the interface's row font (#1309).
#[cfg(not(target_arch = "wasm32"))]
fn fallback_data(face: &effectcraft_text::fonts::Face, row: &effectcraft_text::fonts::Face) -> Option<FontData> {
    Some(shared(face)?.tweak(baseline_tweak(row, face)))
}

/// The faces the interface asks for before it settles on the one a composed frame renders with.
///
/// Windows ships a UI variant of its Japanese and Chinese families - `Meiryo`/`Meiryo UI`, `Yu
/// Gothic`/`Yu Gothic UI`, `MS Gothic`/`MS UI Gothic`, `Microsoft YaHei`/`Microsoft YaHei UI`,
/// `Microsoft JhengHei`/`Microsoft JhengHei UI` - and its own link table for `Segoe UI`
/// (`FontLink\SystemLink`) picks the UI one for what it draws on screen. `PMingLiU` is the one face
/// here that is not a sibling: it is `MingLiU`'s proportional relative, and proportional spacing is
/// what makes the chrome read better in it.
///
/// Only the interface takes this order. `fonts::fallback_for` keeps the order a document renders
/// with, so an existing project's output does not move when the chrome changes.
#[cfg(not(target_arch = "wasm32"))]
fn interface_fallback_candidates(name: &str, language: &str) -> &'static [&'static str] {
    match (name, cjk_locale(language)) {
        // Kana, and the Han ideographs of a Japanese interface: the language asks for the Japanese
        // face, so its UI sibling is the one the platform would draw chrome with.
        ("japanese-system", _) | ("chinese-system", "ja") => &["Meiryo UI", "Yu Gothic UI", "MS UI Gothic"],
        ("chinese-system", "zh-hant") => &["Microsoft JhengHei UI", "PMingLiU"],
        ("chinese-system", "zh-hans") => &["Microsoft YaHei UI"],
        // The other scripts have no UI variant, and their shared order already picks the face made
        // for the script - the one that draws it correctly at all.
        _ => &[],
    }
}

/// The CJK locale the text engine should prefer for Han text: the interface language when it names
/// one, otherwise the empty string, which leaves the operating system's locale in charge.
#[cfg(not(target_arch = "wasm32"))]
fn cjk_locale(language: &str) -> &str {
    match language {
        "ja" | "zh-hans" | "zh-hant" => language,
        _ => "",
    }
}

/// The Han character that picks the Chinese fallback face. A Simplified Chinese interface asks for
/// a hanzi used by its catalog. Some Japanese faces also cover 页 while lacking 选 or 删, so
/// coverage of this probe alone is not enough: the fallback must use the interface's own language
/// policy, even while another context is installing Japanese fonts (#1141).
#[cfg(not(target_arch = "wasm32"))]
fn han_probe(language: &str) -> char {
    if language == "zh-hans" { '页' } else { '文' }
}

/// Whether `language` is one of the Chinese catalogs.
#[cfg(not(target_arch = "wasm32"))]
fn is_chinese(language: &str) -> bool {
    matches!(language, "zh-hans" | "zh-hant")
}

/// egui's theme preference for an appearance mode showing a `light` (or dark) theme. Sync with
/// System leaves it at System: a concrete preference makes egui set the native window's
/// appearance, and on macOS a window with its own appearance no longer hears when the system's
/// changes (#642). A fixed Light or Dark mode pins the window to match.
pub fn theme_preference(appearance_mode: &str, light: bool) -> egui::ThemePreference {
    match appearance_mode {
        "auto" => egui::ThemePreference::System,
        _ if light => egui::ThemePreference::Light,
        _ => egui::ThemePreference::Dark,
    }
}

pub fn apply_visuals(ctx: &egui::Context, t: &Tokens) {
    let light = t.kind.is_light();
    let mut v = if light { Visuals::light() } else { Visuals::dark() };
    v.panel_fill = t.panel_bg;
    v.window_fill = t.panel_bg;
    v.extreme_bg_color = t.field_bg;
    v.faint_bg_color = t.row_alt;
    v.override_text_color = Some(t.text);
    v.selection.bg_fill = t.accent;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = t.accent;
    v.window_stroke = Stroke::new(1.0, t.field_border);
    v.window_corner_radius = egui::CornerRadius::same(8);
    v.menu_corner_radius = egui::CornerRadius::same(6);
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(150) };
    v.window_shadow = egui::epaint::Shadow { offset: [0, 10], blur: 36, spread: 0, color: Color32::from_black_alpha(170) };
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = egui::CornerRadius::same(t.radius_sm as u8);
    }
    v.widgets.noninteractive.bg_fill = t.panel_bg;
    v.widgets.noninteractive.weak_bg_fill = t.panel_bg;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.separator);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.bg_fill = t.field_bg;
    v.widgets.inactive.weak_bg_fill = t.field_bg;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, t.field_border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, t.field_border);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, t.tab_text_active);
    v.widgets.active.bg_fill = t.pressed;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.fg_stroke = Stroke::new(1.0, t.tab_text_active);
    v.widgets.open.bg_fill = t.hover;
    v.widgets.open.weak_bg_fill = t.hover;
    // egui keeps one style per light/dark theme and picks it by its theme preference (see
    // [`theme_preference`]); both get ours, so whichever it picks, egui's defaults never show.
    ctx.all_styles_mut(|s| {
        s.visuals = v.clone();
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.interact_size.y = 22.0;
        s.spacing.menu_margin = egui::Margin::same(5);
        // Scroll bars always show (egui's float in only over the list), so long menus and lists
        // can be dragged without a mouse wheel or touchpad (#269).
        s.spacing.scroll = egui::style::ScrollStyle::solid();
        // The handle takes the text colour: the fill it would otherwise get is the colour of its
        // track (both are the field background), which left it invisible until hovered.
        s.spacing.scroll.foreground_color = true;
        s.text_styles.insert(TextStyle::Body, FontId::new(12.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Button, FontId::new(12.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Small, FontId::new(11.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Heading, FontId::new(15.0, FontFamily::Name("semibold".into())));
        s.text_styles.insert(TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace));
        s.animation_time = 0.1;
    });
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod script_font_tests {
    use super::{SCRIPT_PROBES, interface_fallback_candidates};
    use effectcraft_text::fonts;

    /// Whether an installed face covers `probe` (else the glyph check is skipped).
    fn installed(probe: char) -> bool {
        let base = fonts::resolve("Inter", "Regular").face;
        fonts::face(fonts::fallback_for(probe, base)).has_char(probe)
    }

    /// Every character of `text` draws a real glyph in every UI font family with the `language`
    /// interface's fonts installed, on the first frame that paints it: installed faces are read
    /// during that frame ([`crate::fallback_fonts`]).
    fn assert_drawn(language: &str, text: &str) {
        let ctx = egui::Context::default();
        super::install(&ctx, &super::Tokens::for_kind(super::ThemeKind::Dark), language);
        let families =
            [egui::FontFamily::Proportional, egui::FontFamily::Monospace, egui::FontFamily::Name("medium".into()), egui::FontFamily::Name("semibold".into())];
        // one label per character and family: each glyph is the first of its own galley
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            for family in &families {
                for ch in text.chars() {
                    ui.label(egui::RichText::new(ch.to_string()).font(egui::FontId::new(13.0, family.clone())));
                }
            }
        });
        // No renderer here: drop the frame's texture uploads (egui asserts on unhandled ones in debug).
        out.textures_delta.clear();
        let galleys: Vec<_> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.clone()),
                _ => None,
            })
            .filter(|g| g.job.text.chars().count() == 1 && text.contains(g.job.text.as_str()))
            .collect();
        assert_eq!(galleys.len(), families.len() * text.chars().count(), "every label is painted");
        for galley in galleys {
            let font = galley.job.sections[0].format.font_id.clone();
            let ch = galley.job.text.clone();
            // epaint 0.36's has_glyph compares face keys, so it reports a false
            // negative when a real glyph shares the replacement face.
            // Check the rendered atlas glyph instead of that face-level predicate.
            let missing_uv = ctx.fonts_mut(|f| f.layout_no_wrap("\u{10ffff}".into(), font.clone(), egui::Color32::WHITE).rows[0].glyphs[0].uv_rect);
            let uv = galley.rows[0].glyphs[0].uv_rect;
            assert!(uv.size.x > 0.0 && uv.size.y > 0.0, "empty {ch} in {font:?}");
            assert_ne!((uv.min, uv.max), (missing_uv.min, missing_uv.max), "replacement glyph for {ch} in {font:?} on the first frame");
        }
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    }

    #[test]
    fn installed_japanese_fallback_is_available_in_all_ui_families() {
        if !installed('あ') {
            eprintln!("no Japanese system font installed; skipping glyph coverage");
            return;
        }
        assert_drawn("ja", "日本語コンポジションレイヤーエフェクト設定");
    }

    /// #394: Arabic, Hebrew, Indic and Thai names are drawn by an installed face in an English
    /// interface: the UI fonts were Inter and the CJK fallbacks only, so a layer named in Arabic
    /// showed as missing-glyph boxes.
    #[test]
    fn installed_script_fallbacks_draw_arabic_hebrew_indic_and_thai_names() {
        // Letters only: a vowel sign or virama on its own is drawn on a dotted circle.
        for text in ["طبقةمرحبا", "שכבה", "परत", "কলম", "ਕਲਮ", "કલમ", "கடல", "కలమ", "ಕಲಮ", "കലമ", "กขค"]
        {
            let Some(probe) = text.chars().next() else { continue };
            if !installed(probe) {
                eprintln!("no system font covers {text}; skipping its glyph coverage");
                continue;
            }
            assert_drawn("en", text);
        }
    }

    #[test]
    fn the_interface_asks_for_the_platforms_ui_faces() {
        // The face a window draws its own chrome with, which is not the one a composed frame
        // renders with: the two orders are deliberately separate.
        let ja = &["Meiryo UI", "Yu Gothic UI", "MS UI Gothic"][..];
        assert_eq!(interface_fallback_candidates("japanese-system", "ja"), ja);
        assert_eq!(interface_fallback_candidates("chinese-system", "ja"), ja);
        assert_eq!(interface_fallback_candidates("chinese-system", "zh-hans"), &["Microsoft YaHei UI"][..]);
        assert_eq!(interface_fallback_candidates("chinese-system", "zh-hant"), &["Microsoft JhengHei UI", "PMingLiU"][..]);
        // Every other script the install probes has no UI variant, so it keeps the shared order.
        for &(_, name) in SCRIPT_PROBES.iter() {
            assert!(interface_fallback_candidates(name, "en").is_empty(), "{name} asked for a UI face");
        }
    }

    /// The Georgian interface draws its Mkhedruli text with an installed Georgian face (Sylfaen,
    /// SF Georgian, Noto Sans Georgian…): Inter has no Georgian, so the labels showed as boxes.
    #[test]
    fn installed_georgian_fallback_draws_the_georgian_interface() {
        if !installed('ა') {
            eprintln!("no Georgian system font installed; skipping glyph coverage");
            return;
        }
        assert_drawn("ka", "კომპოზიციაფენასაკვანძოკადრიეფექტიპარამეტრები");
    }

    /// The pass that first lays out text in a script whose face isn't read yet draws it already:
    /// egui asks the fallback provider for the face while it lays the text out, so a context that
    /// treats a missing glyph as a bug sees none. The face then takes its place in the fallback
    /// order, and a Han character after it is still drawn by the Japanese face's place ahead of it.
    #[test]
    fn a_face_read_during_layout_takes_its_place_in_the_fallback_order() {
        if !installed('\u{d55c}') || !installed('\u{4e2d}') {
            eprintln!("no Korean or Han system font installed; skipping");
            return;
        }
        let ctx = egui::Context::default();
        ctx.set_missing_glyph_policy(egui::MissingGlyphPolicy::Panic);
        super::install(&ctx, &super::Tokens::for_kind(super::ThemeKind::Dark), "en");
        let frame = |text: &'static str| {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.label(text);
            })
            .textures_delta
            .clear();
        };
        frame("\u{d55c}\u{ad6d}");
        frame("\u{d55c}\u{ad6d}");
        ctx.fonts_mut(|f| {
            assert!(f.discovered_fonts().is_empty(), "the face egui added while laying out is installed in its place");
            assert!(f.definitions().font_data.contains_key("korean-system"), "{:?}", f.definitions().font_data.keys());
        });
        frame("\u{4e2d}\u{6587}");
        frame("\u{4e2d}\u{6587}");
        // what this context installed, not the process-wide read flags: another test may have read
        // the Japanese face first
        ctx.fonts_mut(|f| {
            let installed = f.definitions().font_data.contains_key("japanese-system");
            assert!(installed, "Han in an English interface is drawn by the Japanese face, as before: {:?}", f.definitions().font_data.keys());
        });
    }

    /// The faces read are process-wide. A context whose fonts were installed before another one read
    /// a face installs it once its text needs the face, even when a face later in the order (here
    /// the Korean one, which has Han too) already draws that text; and a context created after the
    /// read installs it at once.
    #[test]
    fn a_face_read_by_another_context_is_installed_in_this_one() {
        use crate::fallback_fonts::Candidate;
        if !installed('\u{d55c}') || !installed('\u{4e2d}') {
            eprintln!("no Korean or Han system font installed; skipping");
            return;
        }
        let tokens = super::Tokens::for_kind(super::ThemeKind::Dark);
        let earlier = egui::Context::default();
        super::install(&earlier, &tokens, "en");
        let pass = |ctx: &egui::Context, text: &'static str| {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.label(text);
            })
            .textures_delta
            .clear();
        };
        pass(&earlier, "\u{d55c}\u{ad6d}");
        pass(&earlier, "\u{d55c}\u{ad6d}");
        let japanese = super::lazy_fallbacks("en").into_iter().find(|f| f.name() == "japanese-system");
        assert!(japanese.is_some_and(|f| f.read()), "the Japanese face is read (as another context's text would)");
        pass(&earlier, "\u{4e2d}\u{6587}");
        pass(&earlier, "\u{4e2d}\u{6587}");
        earlier.fonts_mut(|f| assert!(f.definitions().font_data.contains_key("japanese-system"), "{:?}", f.definitions().font_data.keys()));
        let later = egui::Context::default();
        super::install(&later, &tokens, "en");
        pass(&later, "Composition");
        later.fonts_mut(|f| assert!(f.definitions().font_data.contains_key("japanese-system"), "{:?}", f.definitions().font_data.keys()));
    }

    #[test]
    fn simplified_chinese_interface_draws_simplified_hanzi_after_another_language() {
        let ctx = egui::Context::default();
        let tokens = super::Tokens::for_kind(super::ThemeKind::Dark);
        super::install(&ctx, &tokens, "en");
        super::install(&ctx, &tokens, "zh-hans");
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let font = egui::FontId::new(13.0, egui::FontFamily::Proportional);
        let missing: String = ctx.fonts_mut(|fonts| {
            let replacement = fonts.layout_no_wrap("\u{10ffff}".into(), font.clone(), egui::Color32::WHITE).rows[0].glyphs[0].uv_rect;
            // Representative hanzi of the zh-hans catalog (选择, 删除, 图层, 设置, 导入, 页面).
            "选删层设导页"
                .chars()
                .filter(|ch| {
                    let uv = fonts.layout_no_wrap(ch.to_string(), font.clone(), egui::Color32::WHITE).rows[0].glyphs[0].uv_rect;
                    (uv.min, uv.max) == (replacement.min, replacement.max)
                })
                .collect()
        });
        // Asked after the layout: the coverage lookup itself loads a face with these hanzi.
        let drawable: String = missing.chars().filter(|&ch| installed(ch)).collect();
        assert!(drawable.is_empty(), "an installed face covers {drawable}, but the Simplified Chinese interface draws boxes");
    }

    /// egui shapes each run of one face: Arabic letters take their joined forms, and an Arabic
    /// word reads right to left (its first letter drawn rightmost).
    #[test]
    fn arabic_words_are_joined_and_read_right_to_left() {
        if !installed('م') {
            eprintln!("no Arabic system font installed; skipping");
            return;
        }
        let ctx = egui::Context::default();
        super::install(&ctx, &super::Tokens::for_kind(super::ThemeKind::Dark), "en");
        // the frame that first paints Arabic reads the installed Arabic face
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("مرحبا");
        });
        out.textures_delta.clear();
        ctx.fonts_mut(|fonts| {
            let font = egui::FontId::new(13.0, egui::FontFamily::Proportional);
            let missing = fonts.layout_no_wrap("\u{10ffff}".into(), font.clone(), egui::Color32::WHITE).rows[0].glyphs[0].uv_rect;
            let alone = fonts.layout_no_wrap("م".into(), font.clone(), egui::Color32::WHITE);
            let word = fonts.layout_no_wrap("مرحبا".into(), font, egui::Color32::WHITE);
            let glyphs = &word.rows[0].glyphs;
            let meem = glyphs.iter().find(|g| g.chr == 'م').expect("the meem is laid out");
            let alif = glyphs.iter().find(|g| g.chr == 'ا').expect("the alif is laid out");
            assert!(meem.pos.x > alif.pos.x, "the first letter is drawn rightmost: {} vs {}", meem.pos.x, alif.pos.x);
            let isolated = alone.rows[0].glyphs[0].uv_rect;
            for uv in [meem.uv_rect, isolated] {
                assert_ne!((uv.min, uv.max), (missing.min, missing.max), "a real glyph, not the replacement");
            }
            assert_ne!((meem.uv_rect.min, meem.uv_rect.max), (isolated.min, isolated.max), "the meem takes its initial form");
        });
    }

    /// A fallback face's glyphs sit on the row font's baseline, in every UI family and at every
    /// interface size. (#1309)
    ///
    /// `A` stands in for the row's baseline: epaint exposes none, but a capital without a descender
    /// has its ink bottom on the baseline, so its ink bottom *is* the row's baseline. A glyph with a
    /// descender (`g`) or a round one with an overshoot (`O`) would quietly stop holding, so keep `A`
    /// as the Latin half of this pair.
    ///
    /// Both glyphs of `Aあ` end on the baseline, so their ink bottoms agree up to the difference
    /// between the two outlines themselves - the kana reaches a little below the Latin letter in the
    /// Japanese faces. Without the correction the kana sits 3 pt high at 12 pt (measured), so the
    /// tolerance below separates "the correction is applied" from "it is missing"; it is not a claim
    /// about the outlines. Two pixels per point, because epaint truncates a glyph's box height to
    /// whole pixels, which would otherwise put up to a point of noise on the measurement.
    #[test]
    fn fallback_glyphs_sit_on_the_row_baseline() {
        if !installed('\u{3042}') {
            eprintln!("no Japanese system font installed; skipping the baseline check");
            return;
        }
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(2.0);
        super::install(&ctx, &super::Tokens::for_kind(super::ThemeKind::Dark), "ja");
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let bottom = |g: &egui::epaint::text::Glyph| g.pos.y + g.uv_rect.offset.y + g.uv_rect.size.y;
        ctx.fonts_mut(|fonts| {
            for family in [
                egui::FontFamily::Proportional,
                egui::FontFamily::Monospace,
                egui::FontFamily::Name("medium".into()),
                egui::FontFamily::Name("semibold".into()),
            ] {
                for size in [12.0_f32, 15.0, 18.0] {
                    let font = egui::FontId::new(size, family.clone());
                    let galley = fonts.layout_no_wrap("A\u{3042}".into(), font, egui::Color32::WHITE);
                    let row = &galley.rows[0].glyphs;
                    assert_eq!(row.len(), 2, "A\u{3042} should shape to two glyphs");
                    let (latin, kana) = (bottom(&row[0]), bottom(&row[1]));
                    let drift = kana - latin;
                    // Without the correction the kana sits 3-5 pt *above* the Latin ink, so the sign is
                    // what tells the two apart. With it, the kana reaches a little *below* the Latin
                    // one, by however far the face's kana overshoot past the baseline - measured at
                    // 1.94 pt for Noto Sans CJK JP against Yu Gothic's 0.4 - plus up to half a point of
                    // whole-point rounding in `pos.y` for each glyph.
                    assert!(
                        drift > -0.5,
                        "the kana sits {drift:.2} pt above the Latin baseline at {size} pt in {family:?} (latin {latin}, kana {kana}); the fallback tweak is missing or wrong"
                    );
                    assert!(
                        drift < 2.5,
                        "the kana sits {drift:.2} pt below the Latin baseline at {size} pt in {family:?} (latin {latin}, kana {kana}); the fallback tweak is too large"
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The last `n` font names of `family`, in order.
    fn tail(family: &[String], n: usize) -> Vec<&str> {
        family.iter().rev().take(n).rev().map(String::as_str).collect()
    }

    #[test]
    fn cjk_fallbacks_follow_the_interface_language() {
        // A Chinese interface asks for the Han probe first, so Han text is drawn by a Chinese face
        // even when the operating system's locale is not Chinese.
        assert_eq!(cjk_locale("zh-hans"), "zh-hans");
        assert_eq!(cjk_locale("zh-hant"), "zh-hant");
        assert_eq!(cjk_locale("ja"), "ja");
        assert_eq!(cjk_locale("en"), "");
        assert_eq!(cjk_locale("system"), "");
        let mut chinese = FontDefinitions::default();
        let installed = install_script_fallbacks(&mut chinese, "zh-hans");
        let cjk = |names: &[String]| names.iter().filter(|n| matches!(n.as_str(), "chinese-system" | "japanese-system")).cloned().collect::<Vec<_>>();
        if cjk(&installed).is_empty() {
            eprintln!("no CJK font is installed: only the language plumbing can be checked here");
        } else {
            assert_eq!(installed.first().map(String::as_str), Some("chinese-system"), "the Han probe comes first");
            assert!(chinese.font_data.contains_key("chinese-system"));
        }
        let expected: Vec<&str> = installed.iter().map(String::as_str).collect();
        for (family, stack) in &chinese.families {
            assert_eq!(tail(stack, installed.len()), expected, "{family:?}");
        }
        // A Japanese interface keeps the kana probe first, as it was before the language existed.
        let mut japanese = FontDefinitions::default();
        let installed = install_script_fallbacks(&mut japanese, "ja");
        if let Some(first) = cjk(&installed).first() {
            assert_eq!(installed.first(), Some(first));
            assert_eq!(first, "japanese-system", "the kana probe comes first");
        }
        // And an English interface leaves the operating system's locale in charge.
        let mut english = FontDefinitions::default();
        install_script_fallbacks(&mut english, "en");
    }

    #[test]
    fn every_family_reaches_the_symbol_fallback() {
        let ctx = egui::Context::default();
        install(&ctx, &Tokens::for_kind(Default::default()), "en");
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        for family in [FontFamily::Proportional, FontFamily::Monospace, FontFamily::Name("medium".into()), FontFamily::Name("semibold".into())] {
            let id = FontId::new(12.0, family);
            // Not has_glyph: it compares face keys, so Inter's `•` reads as missing when no face in
            // the stack has egui's `◻` and Inter's `?` becomes the replacement, as happens once
            // another test has loaded a system face without `◻` (the fallback tries loaded faces first).
            let uv = |s: String| ctx.fonts_mut(|f| f.layout_no_wrap(s, id.clone(), egui::Color32::WHITE).rows[0].glyphs[0].uv_rect);
            let missing = uv("\u{10ffff}".into());
            // Every chrome symbol the panels, menus, shortcut labels and buttons draw.
            for c in "✔▣✓⚠▶◀▸●○…×›•·−←→↑↓⌘⇧⌥⌃".chars() {
                let drawn = uv(c.to_string());
                assert_ne!((drawn.min, drawn.max), (missing.min, missing.max), "{c} has no glyph in {id:?}");
            }
        }
    }

    #[test]
    fn hangul_gets_a_face_when_one_is_installed() {
        use crate::fallback_fonts::Candidate;
        use effectcraft_text::fonts;
        // as the interface does the first time it paints Hangul
        let _ = lazy_fallbacks("en").iter().find(|f| f.name() == "korean-system").map(|f| f.read());
        let mut fonts_def = FontDefinitions::default();
        install_script_fallbacks(&mut fonts_def, "en");
        let base = fonts::resolve("Inter", "Regular").face;
        let id = fonts::fallback_for('한', base);
        let face = fonts::face(id);
        if !face.has_char('한') {
            eprintln!("no Hangul font is installed: nothing to check here");
            return;
        }
        let Some(font) = face.font() else { return };
        let bytes = font.data().as_bytes();
        let covered = fonts_def.font_data.values().any(|d| d.bytes() == bytes && d.index == face.info.index);
        assert!(covered, "a face covering Hangul must be registered with egui");
    }

    /// egui shares the font database's bytes for every installed fallback: installing the fonts
    /// again (another language, another window) hands egui the same bytes, never a copy of the file.
    #[test]
    fn fallback_faces_are_shared_not_copied() {
        let mut first = FontDefinitions::default();
        let installed = install_script_fallbacks(&mut first, "ja");
        let mut again = FontDefinitions::default();
        install_script_fallbacks(&mut again, "ja");
        if installed.is_empty() {
            eprintln!("SKIPPED: no Japanese or Chinese system font installed");
        }
        for name in &installed {
            let (Some(a), Some(b)) = (first.font_data.get(name), again.font_data.get(name)) else { panic!("{name} is missing") };
            assert_eq!(a.bytes().as_ptr(), b.bytes().as_ptr(), "{name}: egui shares the same bytes each install, never a copy");
        }
        install_script_fallbacks(&mut first, "system");
    }

    /// The faces read when painted text needs them come in the order the fonts register them, and
    /// a Japanese or Chinese interface reads its own CJK faces with the fonts instead.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn lazy_fallbacks_follow_the_registration_order() {
        let names = |language: &str| lazy_fallbacks(language).iter().map(LazyFallback::name).collect::<Vec<_>>();
        let en = names("en");
        assert_eq!(en.first(), Some(&"japanese-system"), "{en:?}");
        assert_eq!(en.get(1), Some(&"chinese-system"), "{en:?}");
        assert_eq!(en.last(), Some(&"korean-system"), "{en:?}");
        for language in ["ja", "zh-hans", "zh-hant"] {
            let list = names(language);
            assert!(!list.contains(&"japanese-system") && !list.contains(&"chinese-system"), "{language}: {list:?}");
            assert_eq!(list.first(), Some(&"arabic-system"), "{language}: {list:?}");
        }
    }

    #[test]
    fn a_fallback_face_is_registered_at_most_once() {
        let mut fonts = FontDefinitions::default();
        let installed = install_script_fallbacks(&mut fonts, "zh-hant");
        let mut unique = installed.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), installed.len(), "no name may be registered twice: {installed:?}");
        let faces: Vec<_> = installed
            .iter()
            .map(|name| {
                let data = fonts.font_data.get(name).unwrap_or_else(|| panic!("{name} is missing its font data"));
                (data.bytes(), data.index)
            })
            .collect();
        for (i, a) in faces.iter().enumerate() {
            assert!(!faces[i + 1..].contains(a), "{} shares its face with a later fallback: {installed:?}", installed[i]);
        }
        // Leave the operating system's locale behind for whatever runs next.
        install_script_fallbacks(&mut fonts, "system");
    }
}
