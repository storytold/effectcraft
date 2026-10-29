use serde::{Deserialize, Serialize};

use super::Prefs;

impl Prefs {
    /// Remember a project in File ▸ Open Recent.
    pub fn push_recent(&mut self, path: &str) {
        self.recent_projects.retain(|p| p != path);
        self.recent_projects.insert(0, path.to_string());
        self.recent_projects.truncate(self.general.recent_items as usize);
    }

    /// Remember a font in the Character panel's recent fonts (Type ▸ Number of Recent Fonts).
    pub fn push_recent_font(&mut self, family: &str) {
        push_mru(&mut self.recent_fonts, family, 30);
    }

    /// Is effect `id` starred in Effects & Presets?
    pub fn is_effect_favorite(&self, id: &str) -> bool {
        self.effects_favorites.iter().any(|f| f == id)
    }

    /// Star effect `id` in Effects & Presets, or un-star it; true when it is starred now.
    pub fn toggle_effect_favorite(&mut self, id: &str) -> bool {
        if self.is_effect_favorite(id) {
            self.effects_favorites.retain(|f| f != id);
            false
        } else {
            self.effects_favorites.push(id.to_string());
            self.effects_favorites.truncate(MAX_EFFECTS_FAVORITES);
            self.is_effect_favorite(id)
        }
    }

    /// The recent fonts the Character panel shows (at most Number of Recent Fonts to Display).
    pub fn recent_fonts_shown(&self) -> &[String] {
        let n = (self.type_.recent_fonts as usize).min(self.recent_fonts.len());
        &self.recent_fonts[..n]
    }

    /// Remember imported footage for File ▸ Import Recent Footage.
    pub fn push_recent_footage(&mut self, path: &str) {
        push_mru(&mut self.recent_footage, path, self.general.recent_items as usize);
    }

    /// Remember an applied animation preset for Animation ▸ Recent Animation Presets.
    pub fn push_recent_preset(&mut self, path: &str) {
        push_mru(&mut self.recent_presets, path, self.general.recent_items as usize);
    }

    /// Remember a script that ran for File ▸ Scripts ▸ Recent Script Files.
    pub fn push_recent_script(&mut self, script: RecentScript) {
        self.recent_scripts.retain(|x| *x != script);
        self.recent_scripts.insert(0, script);
        self.recent_scripts.truncate(MAX_RECENT_SCRIPTS);
    }
}

/// Most scripts File ▸ Scripts ▸ Recent Script Files lists.
pub const MAX_RECENT_SCRIPTS: usize = 10;

/// A File ▸ Scripts ▸ Recent Script Files entry, stored as the `file.runScript` parameter that
/// ran it: `{"path": …}` for a script file, `{"name": …}` for an installed or bundled script.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecentScript {
    Path(String),
    Name(String),
}

impl RecentScript {
    /// The file's path or the script's name.
    pub fn text(&self) -> &str {
        match self {
            RecentScript::Path(s) | RecentScript::Name(s) => s,
        }
    }
}

/// Most effects Effects & Presets keeps starred (a hand-edited settings file can hold more).
pub(super) const MAX_EFFECTS_FAVORITES: usize = 1000;

fn push_mru(list: &mut Vec<String>, v: &str, max: usize) {
    list.retain(|x| x != v);
    list.insert(0, v.to_string());
    list.truncate(max.max(1));
}
