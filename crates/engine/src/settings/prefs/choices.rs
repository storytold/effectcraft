/// Settings ▸ General ▸ Language: (label, `general.language` value).
pub const LANGUAGES: &[(&str, &str)] = &[
    ("Match System", "system"),
    ("English", "en"),
    // Alphabetical by the language's own name after English; Cyrillic, Georgian, then CJK names
    // follow the Latin ones.
    ("Čeština", "cs"),
    ("Deutsch", "de"),
    ("Español", "es"),
    ("Français", "fr"),
    ("Português (Brasil)", "pt-br"),
    ("Русский", "ru"),
    ("Українська", "uk"),
    ("ქართული", "ka"),
    ("日本語", "ja"),
    ("简体中文", "zh-hans"),
    ("繁體中文", "zh-hant"),
];

/// Settings ▸ Startup & Repair ▸ Window Graphics: (label, `startup.windowGraphics` value).
pub const WINDOW_GRAPHICS: &[(&str, &str)] = &[("Automatic", "auto"), ("OpenGL (compatibility)", "gl")];

/// Settings ▸ Appearance ▸ Appearance Mode: (label, `appearance.appearanceMode` value). Auto
/// follows the operating system's light or dark appearance.
pub const APPEARANCE_MODES: &[(&str, &str)] = &[("Sync with System", "auto"), ("Dark", "dark"), ("Light", "light")];

/// The dark family of themes: (label, `appearance.darkTheme` value).
pub const DARK_THEMES: &[(&str, &str)] = &[("Dark", "dark"), ("Darker", "darker")];

/// The light family of themes: (label, `appearance.lightTheme` value).
pub const LIGHT_THEMES: &[(&str, &str)] = &[("Light", "light")];

/// Every theme (the legacy single-theme `appearance.theme`).
pub const THEMES: &[(&str, &str)] = &[("Dark", "dark"), ("Darker", "darker"), ("Light", "light")];

/// The settings whose value must be one of their choices.
pub(super) fn choices(key: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match key {
        "general.language" => Some(LANGUAGES),
        "startup.windowGraphics" => Some(WINDOW_GRAPHICS),
        "appearance.appearanceMode" => Some(APPEARANCE_MODES),
        "appearance.darkTheme" => Some(DARK_THEMES),
        "appearance.lightTheme" => Some(LIGHT_THEMES),
        "appearance.theme" => Some(THEMES),
        _ => None,
    }
}

pub(super) fn is_choice(choices: &[(&str, &str)], v: &str) -> bool {
    choices.iter().any(|(_, c)| *c == v)
}

/// The appearance mode and the per-mode setting a theme belongs to (`darker` → (`dark`,
/// `darkTheme`)); `None` for an unknown theme.
pub fn theme_family(theme: &str) -> Option<(&'static str, &'static str)> {
    if is_choice(DARK_THEMES, theme) {
        Some(("dark", "darkTheme"))
    } else if is_choice(LIGHT_THEMES, theme) {
        Some(("light", "lightTheme"))
    } else {
        None
    }
}
