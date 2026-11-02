//! Community and project links (shown in Help ▸, the About dialog, the start screen and the
//! header's website button).

pub const APP_NAME: &str = "Visualab Effects";
pub const APP_SLUG: &str = "visualab-effects";
pub const WEBSITE: &str = "https://www.visualabstudios.com";
pub const APP_PAGE: &str = "https://www.visualabstudios.com";
pub const GITHUB: &str = "https://github.com/wasigonzi/visualab-effects";
pub const ISSUES: &str = "https://github.com/wasigonzi/visualab-effects/issues";

/// Sibling apps: none shipped in this distribution.
pub const SIBLINGS: &[(&str, &str)] = &[];

pub fn github_of(slug: &str) -> String {
    format!("https://github.com/wasigonzi/{slug}")
}
pub fn page_of(_slug: &str) -> String {
    "https://www.visualabstudios.com".to_string()
}
