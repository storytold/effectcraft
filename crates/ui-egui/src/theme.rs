//! Design tokens. Every widget reads colours/sizes from [`Tokens`] so themes apply everywhere.
//!
//! The default theme follows the look of After Effects' dark UI (values estimated from public
//! documentation and product knowledge, tuned by eye; see `plan/aftereffects/README.md` §2). Fonts
//! are Inter + JetBrains Mono (OFL).

use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeKind {
    #[default]
    Dark,
    Darker,
    Light,
    StudioDark,
    StudioLight,
    Classic,
}

impl ThemeKind {
    pub const ALL: [Self; 6] = [Self::Dark, Self::Darker, Self::Light, Self::StudioDark, Self::StudioLight, Self::Classic];

    pub fn id(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Darker => "darker",
            Self::Light => "light",
            Self::StudioDark => "studioDark",
            Self::StudioLight => "studioLight",
            Self::Classic => "classic",
        }
    }

    pub fn is_light(self) -> bool {
        matches!(self, Self::Light | Self::StudioLight | Self::Classic)
    }

    pub fn is_studio(self) -> bool {
        matches!(self, Self::StudioDark | Self::StudioLight)
    }

    pub fn from_name(s: &str) -> Option<ThemeKind> {
        match s.to_ascii_lowercase().as_str() {
            "dark" | "default" => Some(ThemeKind::Dark),
            "darker" | "darkest" => Some(ThemeKind::Darker),
            "light" => Some(ThemeKind::Light),
            "studiodark" | "studio" => Some(ThemeKind::StudioDark),
            "studiolight" => Some(ThemeKind::StudioLight),
            "classic" => Some(ThemeKind::Classic),
            _ => None,
        }
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
    /// Raised/sunken edges for the Classic style; geometry and interaction remain unchanged.
    pub bevel: bool,
    pub bevel_light: Color32,
    pub bevel_dark: Color32,
    pub fx_header: Color32,
    pub fx_header_active: Color32,
    pub timeline_selected: Color32,
    pub timeline_expression: Color32,
    pub navigator: Color32,
    pub work_area_bar: Color32,
    pub switch_bg: Color32,
    pub selected_name_bg: Color32,
    pub selected_name_text: Color32,
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
    /// Paint a surface without changing its hit box. Classic adds raised/sunken edges.
    pub fn surface(&self, p: &egui::Painter, r: egui::Rect, radius: f32, fill: Color32, raised: bool) {
        p.rect_filled(r, if self.bevel { 0.0 } else { radius }, fill);
        if self.bevel {
            let (hi, lo) = if raised { (self.bevel_light, self.bevel_dark) } else { (self.bevel_dark, self.bevel_light) };
            p.line_segment([r.left_bottom(), r.left_top()], Stroke::new(1.0, hi));
            p.line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, hi));
            p.line_segment([r.right_top(), r.right_bottom()], Stroke::new(1.0, lo));
            p.line_segment([r.right_bottom(), r.left_bottom()], Stroke::new(1.0, lo));
        }
    }

    /// The gradient top colour for a fill (a little lighter), or the fill itself when gradients
    /// are off.
    pub fn grad_top(&self, c: Color32) -> Color32 {
        if self.gradients { c.lerp_to_gamma(Color32::WHITE, 0.06) } else { c }
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
            labels: default_labels(),
            radius: 6.0,
            radius_sm: 3.0,
            gap: 4.0,
            tab_h: 30.0,
            row_h: 19.0,
            gradients: true,
            bevel: false,
            bevel_light: Color32::WHITE,
            bevel_dark: Color32::from_gray(96),
            fx_header: Color32::from_gray(0x2a),
            fx_header_active: Color32::from_rgb(0x2f, 0x3a, 0x52),
            timeline_selected: Color32::from_gray(0x2a),
            timeline_expression: Color32::from_gray(0x1a),
            navigator: Color32::from_gray(0x55),
            work_area_bar: Color32::from_gray(0x5c),
            switch_bg: Color32::from_gray(0x19),
            selected_name_bg: Color32::from_gray(0xa6),
            selected_name_text: Color32::from_gray(0x16),
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
                fx_header: Color32::from_gray(0xd8),
                fx_header_active: Color32::from_rgb(0xb0, 0xcf, 0xec),
                timeline_selected: Color32::from_gray(0xc0),
                timeline_expression: Color32::from_gray(0xe4),
                navigator: Color32::from_gray(0x98),
                work_area_bar: Color32::from_gray(0x90),
                switch_bg: Color32::from_gray(0xc0),
                // Keyframes have no outline: dark enough to stand out on the light time graph.
                keyframe: Color32::from_rgb(0x6c, 0x6c, 0x6c),
                ..dark
            },
            ThemeKind::StudioDark => Tokens {
                app_bg: Color32::from_rgb(14, 16, 24),
                header_bg: Color32::from_rgb(21, 24, 34),
                panel_bg: Color32::from_rgb(29, 32, 44),
                field_bg: Color32::from_rgb(18, 21, 31),
                field_border: Color32::from_rgb(64, 70, 91),
                separator: Color32::from_rgb(48, 54, 73),
                hover: Color32::from_rgb(49, 55, 75),
                pressed: Color32::from_rgb(65, 72, 98),
                text: Color32::from_rgb(228, 232, 245),
                text_dim: Color32::from_rgb(170, 179, 204),
                text_faint: Color32::from_rgb(146, 157, 188),
                icon: Color32::from_rgb(195, 204, 226),
                accent: Color32::from_rgb(112, 90, 214),
                accent_hover: Color32::from_rgb(132, 108, 236),
                focus: Color32::from_rgb(157, 143, 255),
                hot_text: Color32::from_rgb(181, 167, 255),
                timecode: Color32::from_rgb(181, 167, 255),
                row: Color32::from_rgb(29, 32, 44),
                row_alt: Color32::from_rgb(32, 36, 49),
                row_selected: Color32::from_rgb(65, 56, 97),
                tl_bg: Color32::from_rgb(22, 25, 36),
                tl_ruler_bg: Color32::from_rgb(29, 32, 44),
                pasteboard: Color32::from_rgb(14, 16, 24),
                fx_header: Color32::from_rgb(38, 43, 59),
                fx_header_active: Color32::from_rgb(65, 56, 97),
                timeline_selected: Color32::from_rgb(40, 39, 61),
                timeline_expression: Color32::from_rgb(22, 25, 36),
                switch_bg: Color32::from_rgb(18, 21, 31),
                radius: 12.0,
                radius_sm: 7.0,
                ..dark
            },
            ThemeKind::StudioLight => Tokens {
                kind,
                app_bg: Color32::from_rgb(226, 230, 242),
                header_bg: Color32::from_rgb(238, 241, 249),
                panel_bg: Color32::from_rgb(248, 250, 255),
                field_bg: Color32::from_rgb(235, 239, 249),
                field_border: Color32::from_rgb(175, 184, 208),
                separator: Color32::from_rgb(208, 215, 233),
                hover: Color32::from_rgb(224, 227, 245),
                pressed: Color32::from_rgb(204, 207, 233),
                accent: Color32::from_rgb(91, 68, 183),
                accent_hover: Color32::from_rgb(111, 87, 203),
                focus: Color32::from_rgb(91, 68, 183),
                hot_text: Color32::from_rgb(78, 53, 164),
                timecode: Color32::from_rgb(78, 53, 164),
                row: Color32::from_rgb(248, 250, 255),
                row_alt: Color32::from_rgb(237, 241, 251),
                row_selected: Color32::from_rgb(216, 207, 246),
                tl_bg: Color32::from_rgb(230, 235, 247),
                tl_ruler_bg: Color32::from_rgb(238, 241, 249),
                pasteboard: Color32::from_rgb(208, 215, 231),
                fx_header: Color32::from_rgb(228, 233, 247),
                fx_header_active: Color32::from_rgb(216, 207, 246),
                timeline_selected: Color32::from_rgb(214, 211, 236),
                timeline_expression: Color32::from_rgb(237, 241, 251),
                switch_bg: Color32::from_rgb(217, 223, 240),
                radius: 12.0,
                radius_sm: 7.0,
                ..Tokens::for_kind(ThemeKind::Light)
            },
            ThemeKind::Classic => Tokens {
                kind,
                // Neutral legacy-editor chrome: restrained relief and blue editing accents.
                app_bg: Color32::from_gray(143),
                header_bg: Color32::from_gray(185),
                panel_bg: Color32::from_gray(176),
                field_bg: Color32::from_gray(200),
                field_border: Color32::from_gray(112),
                separator: Color32::from_gray(140),
                hover: Color32::from_gray(193),
                pressed: Color32::from_gray(156),
                text: Color32::from_gray(35),
                text_dim: Color32::from_gray(60),
                text_faint: Color32::from_gray(76),
                tab_text: Color32::from_gray(62),
                tab_text_active: Color32::from_gray(25),
                icon: Color32::from_gray(60),
                icon_active: Color32::from_gray(25),
                accent: Color32::from_rgb(32, 101, 137),
                accent_hover: Color32::from_rgb(24, 116, 158),
                focus: Color32::from_rgb(31, 115, 155),
                hot_text: Color32::from_rgb(12, 83, 122),
                timecode: Color32::from_rgb(12, 83, 122),
                row: Color32::from_gray(176),
                row_alt: Color32::from_gray(168),
                row_selected: Color32::from_gray(194),
                tl_bg: Color32::from_gray(164),
                tl_ruler_bg: Color32::from_gray(176),
                tl_ruler_text: Color32::from_gray(64),
                tl_ruler_tick: Color32::from_gray(96),
                pasteboard: Color32::from_gray(144),
                fx_header: Color32::from_gray(172),
                fx_header_active: Color32::from_gray(188),
                timeline_selected: Color32::from_gray(174),
                timeline_expression: Color32::from_gray(186),
                switch_bg: Color32::from_gray(168),
                navigator: Color32::from_gray(153),
                work_area_bar: Color32::from_gray(164),
                selected_name_bg: Color32::from_gray(214),
                selected_name_text: Color32::from_gray(32),
                keyframe: Color32::from_gray(72),
                keyframe_selected: Color32::from_rgb(24, 102, 148),
                bevel_light: Color32::from_gray(210),
                bevel_dark: Color32::from_gray(114),
                radius: 0.0,
                radius_sm: 0.0,
                gradients: false,
                bevel: true,
                ..Tokens::for_kind(ThemeKind::Light)
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
    pub fn from_prefs(p: &effectcraft_engine::prefs::Prefs) -> Tokens {
        let kind = ThemeKind::from_name(&p.appearance.theme).unwrap_or_default();
        let mut t = Tokens::for_kind(kind).with_brightness(p.appearance.brightness as f32);
        for (i, l) in effectcraft_color::Label::ALL.iter().enumerate() {
            let [r, g, b] = p.label_rgb(*l);
            t.labels[i] = Color32::from_rgb(r, g, b);
        }
        t.gradients = p.appearance.use_gradients && !t.bevel;
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
        ] {
            adj(c);
        }
        self
    }
}

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

/// Install fonts (Inter, Inter Medium/SemiBold, JetBrains Mono) and egui visuals. `language` is the
/// resolved interface language from [`crate::i18n::language`] (`en`, `ja`, `zh-hans` or `zh-hant`),
/// not the stored `general.language` preference: `system` is already resolved there, so a Chinese
/// system draws Chinese with the default setting. The CJK fallback that draws the interface's own
/// script is registered first, so a Chinese interface is drawn by a Chinese face and a Japanese one
/// by a Japanese face. `install` is called again when the language changes (see the `EffectcraftApp`
/// frame loop), so the fonts do not need a restart.
pub fn install(ctx: &egui::Context, t: &Tokens, language: &str) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("inter".into(), Arc::new(FontData::from_static(INTER_REGULAR)));
    fonts.font_data.insert("inter-medium".into(), Arc::new(FontData::from_static(INTER_MEDIUM)));
    fonts.font_data.insert("inter-semibold".into(), Arc::new(FontData::from_static(INTER_SEMIBOLD)));
    fonts.font_data.insert("jbmono".into(), Arc::new(FontData::from_static(JETBRAINS_MONO)));
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "jbmono".into());
    fonts.families.insert(FontFamily::Name("semibold".into()), vec!["inter-semibold".into(), "inter".into()]);
    fonts.families.insert(FontFamily::Name("medium".into()), vec!["inter-medium".into(), "inter".into()]);
    install_script_fallbacks(&mut fonts, language);
    ctx.set_fonts(fonts);
    apply_visuals(ctx, t);
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
];

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
/// other one still render; then [`SCRIPT_PROBES`]. The web build has no system fonts and installs
/// nothing.
fn install_script_fallbacks(family_fonts: &mut FontDefinitions, language: &str) -> Vec<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (family_fonts, language);
        Vec::new()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use effectcraft_text::fonts;
        // Kana is unambiguously Japanese; Han ideographs are shared, so the order decides which
        // language's face draws them.
        const KANA: (char, &str) = ('あ', "japanese-system");
        const HAN: (char, &str) = ('文', "chinese-system");
        // Han text follows the interface language, not the operating system's locale.
        fonts::set_cjk_locale(cjk_locale(language));
        let base = fonts::resolve("Inter", "Regular").face;
        let cjk = if is_chinese(language) { [HAN, KANA] } else { [KANA, HAN] };
        let mut names = Vec::new();
        let mut seen = Vec::new();
        for &(probe, name) in cjk.iter().chain(SCRIPT_PROBES) {
            let id = fonts::fallback_for(probe, base);
            if seen.contains(&id) {
                continue;
            }
            let face = fonts::face(id);
            if !face.has_char(probe) {
                continue; // nothing installed covers this script
            }
            let Some(font) = face.font() else { continue };
            seen.push(id);
            // Use the already-read, parsed bytes rather than reading a font file twice.
            let mut data = FontData::from_owned(font.data().as_bytes().to_vec());
            data.index = face.info.index;
            family_fonts.font_data.insert(name.to_string(), Arc::new(data));
            for family in family_fonts.families.values_mut() {
                family.push(name.to_string());
            }
            names.push(name.to_string());
        }
        names
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

/// Whether `language` is one of the Chinese catalogs.
#[cfg(not(target_arch = "wasm32"))]
fn is_chinese(language: &str) -> bool {
    matches!(language, "zh-hans" | "zh-hant")
}

pub fn apply_visuals(ctx: &egui::Context, t: &Tokens) {
    let mut v = if t.kind.is_light() { Visuals::light() } else { Visuals::dark() };
    v.panel_fill = t.panel_bg;
    v.window_fill = t.panel_bg;
    v.extreme_bg_color = t.field_bg;
    v.faint_bg_color = t.row_alt;
    v.override_text_color = Some(t.text);
    v.selection.bg_fill = t.accent;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = t.accent;
    v.window_stroke = Stroke::new(1.0, t.field_border);
    v.window_corner_radius = egui::CornerRadius::same(if t.bevel { 0 } else { (t.radius + 2.0) as u8 });
    v.menu_corner_radius = egui::CornerRadius::same(t.radius as u8);
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(if t.bevel {
            0
        } else if t.kind.is_light() {
            55
        } else {
            150
        }),
    };
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 36,
        spread: 0,
        color: Color32::from_black_alpha(if t.bevel {
            0
        } else if t.kind.is_light() {
            65
        } else {
            170
        }),
    };
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
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.interact_size.y = 22.0;
        s.spacing.menu_margin = egui::Margin::same(5);
        // Scroll bars always show (egui's float in only over the list), so long menus and lists
        // can be dragged without a mouse wheel or touchpad (#269).
        s.spacing.scroll = egui::style::ScrollStyle::solid();
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
    use effectcraft_text::fonts;

    /// Whether an installed face covers `probe` (else the glyph check is skipped).
    fn installed(probe: char) -> bool {
        let base = fonts::resolve("Inter", "Regular").face;
        fonts::face(fonts::fallback_for(probe, base)).has_char(probe)
    }

    /// Every character of `text` draws a real glyph in every UI font family with the `language`
    /// interface's fonts installed.
    fn assert_drawn(language: &str, text: &str) {
        let ctx = egui::Context::default();
        super::install(&ctx, &super::Tokens::for_kind(super::ThemeKind::Dark), language);
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        // No renderer here: drop the frame's texture uploads (egui asserts on unhandled ones in debug).
        out.textures_delta.clear();
        ctx.fonts_mut(|fonts| {
            for family in [
                egui::FontFamily::Proportional,
                egui::FontFamily::Monospace,
                egui::FontFamily::Name("medium".into()),
                egui::FontFamily::Name("semibold".into()),
            ] {
                let font = egui::FontId::new(13.0, family);
                // epaint 0.36's has_glyph compares face keys, so it reports a false
                // negative when a real glyph shares the replacement face.
                // Check the rendered atlas glyph instead of that face-level predicate.
                let missing = fonts.layout_no_wrap("\u{10ffff}".into(), font.clone(), egui::Color32::WHITE);
                let missing_uv = missing.rows[0].glyphs[0].uv_rect;
                for ch in text.chars() {
                    let rendered = fonts.layout_no_wrap(ch.to_string(), font.clone(), egui::Color32::WHITE);
                    let uv = rendered.rows[0].glyphs[0].uv_rect;
                    assert!(uv.size.x > 0.0 && uv.size.y > 0.0, "empty {ch} in {font:?}");
                    assert_ne!((uv.min, uv.max), (missing_uv.min, missing_uv.max), "replacement glyph for {ch} in {font:?}");
                }
            }
        });
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
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
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
                (data.font.as_ref(), data.index)
            })
            .collect();
        for (i, a) in faces.iter().enumerate() {
            assert!(!faces[i + 1..].contains(a), "{} shares its face with a later fallback: {installed:?}", installed[i]);
        }
        // Leave the operating system's locale behind for whatever runs next.
        install_script_fallbacks(&mut fonts, "system");
    }
}
