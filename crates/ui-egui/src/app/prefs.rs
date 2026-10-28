use super::{EffectcraftApp, Tokens, json, theme};

impl EffectcraftApp {
    /// Apply changed settings: theme and brightness, label colours, tool tips, preview caches.
    pub fn apply_prefs(&mut self, ctx: &egui::Context) {
        // Sync with System re-applies when the system appearance changes (the host wakes the UI;
        // nothing polls).
        let system = if self.session.prefs.appearance.appearance_mode == "auto" { self.system_light(ctx) } else { None };
        if self.applied_prefs == Some((self.session.prefs_revision, system)) {
            return;
        }
        self.applied_prefs = Some((self.session.prefs_revision, system));
        let p = &self.session.prefs;
        self.ui.theme = theme::ThemeKind::from_name(p.appearance.resolved_theme(system)).unwrap_or_default();
        self.tokens = Tokens::from_prefs(p, self.ui.theme);
        theme::apply_visuals(ctx, &self.tokens);
        ctx.set_theme(theme::theme_preference(&p.appearance.appearance_mode, self.ui.theme.is_light()));
        let tips = p.general.show_tool_tips;
        ctx.all_styles_mut(|s| s.interaction.tooltip_delay = if tips { 0.5 } else { f32::INFINITY });
        // Settings ▸ Appearance ▸ UI Scale (#284). egui's own Ctrl+= / Ctrl+- / Ctrl+0 zoom stays
        // off: Ctrl+0 is Window ▸ Project, and the scale is a setting.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        ctx.set_zoom_factor(p.appearance.ui_scale as f32 / 100.0);
        self.ui.cache_when_idle = p.previews.cache_frames_when_idle;
        self.ui.viewer.fast_preview = p.previews.fast_previews;
        self.frames.set_budget(p.cache_budgets(self.session.sys_memory).preview);
    }

    /// Change settings from the UI (applied next frame and saved).
    pub fn set_pref(&mut self, key: &str, value: serde_json::Value) -> Result<(), String> {
        self.session.prefs.set(key, value)?;
        self.session.prefs_changed();
        self.session.save_prefs();
        Ok(())
    }

    /// Show theme `k`: the appearance mode becomes its family's (Dark or Light), and `k` that
    /// family's theme.
    pub fn set_theme(&mut self, ctx: &egui::Context, k: theme::ThemeKind) {
        if let Err(e) = self.set_pref("appearance.theme", json!(k.name())) {
            self.ui.status = e;
        }
        self.apply_prefs(ctx);
    }

    /// The system appearance: light (`Some(true)`), dark, or unknown. The host's answer (the Linux
    /// desktop portal) wins over egui's, which Wayland compositors often leave empty.
    pub fn system_light(&self, ctx: &egui::Context) -> Option<bool> {
        self.hooks.system_theme.as_ref().and_then(|read| read(ctx)).or_else(|| ctx.system_theme()).map(|t| t == egui::Theme::Light)
    }

    /// The Tools bar's appearance button (`view.theme.toggle`): Sync with System, then Light, then
    /// Dark, then Sync with System again. The light and dark theme choices are kept.
    pub fn cycle_appearance(&mut self, ctx: &egui::Context) {
        let next = match self.session.prefs.appearance.appearance_mode.as_str() {
            "auto" => "light",
            "light" => "dark",
            _ => "auto",
        };
        if let Err(e) = self.set_pref("appearance.appearanceMode", json!(next)) {
            self.ui.status = e;
        }
        self.apply_prefs(ctx);
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;
    use crate::menus;
    use crate::theme::ThemeKind;

    fn app() -> EffectcraftApp {
        EffectcraftApp::new(effectcraft_engine::Session::default())
    }

    /// The Tools bar's appearance button: Sync with System, Light, Dark, then Sync with System
    /// again, showing the saved theme of each family without losing either choice.
    #[test]
    fn appearance_button_cycles_modes_without_losing_theme_choices() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.hooks.system_theme = Some(Box::new(|_| Some(egui::Theme::Dark)));
        app.set_pref("appearance.darkTheme", json!("darker")).unwrap();
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Darker);
        for (mode, shown) in [("auto", ThemeKind::Darker), ("light", ThemeKind::Light), ("dark", ThemeKind::Darker), ("auto", ThemeKind::Darker)] {
            menus::invoke(&mut app, &ctx, "view.theme.toggle", json!({})).unwrap();
            assert_eq!(app.session.prefs.appearance.appearance_mode, mode);
            assert_eq!(app.ui.theme, shown, "{mode}");
            assert_eq!(app.tokens.kind, shown);
            assert_eq!(app.session.prefs.appearance.dark_theme, "darker");
            assert_eq!(app.session.prefs.appearance.light_theme, "light");
        }
    }

    /// Sync with System follows the host's answer (the Linux portal) when it changes, and falls
    /// back to Dark when nothing reports a system appearance.
    #[test]
    fn auto_follows_a_stubbed_system_theme() {
        use std::sync::{Arc, Mutex};
        let ctx = egui::Context::default();
        let mut app = app();
        let system = Arc::new(Mutex::new(Some(egui::Theme::Light)));
        let read = Arc::clone(&system);
        app.hooks.system_theme = Some(Box::new(move |_| *read.lock().unwrap_or_else(std::sync::PoisonError::into_inner)));
        app.set_pref("appearance.appearanceMode", json!("auto")).unwrap();
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Light);
        *system.lock().unwrap() = Some(egui::Theme::Dark);
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Dark, "a system change applies without a settings change");
        *system.lock().unwrap() = None;
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Dark);
        // A fixed mode ignores the system.
        *system.lock().unwrap() = Some(egui::Theme::Light);
        app.set_pref("appearance.appearanceMode", json!("dark")).unwrap();
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Dark);
    }

    /// Sync with System leaves egui's theme preference at System, so egui never gives the native
    /// window an appearance of its own (on macOS that stops system appearance changes reaching
    /// the app, #642); egui's light and dark styles both carry the resolved theme. Fixed modes
    /// pin the window to their family.
    #[test]
    fn auto_mode_leaves_the_native_window_following_the_system() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.hooks.system_theme = Some(Box::new(|_| Some(egui::Theme::Light)));
        app.set_pref("appearance.appearanceMode", json!("auto")).unwrap();
        app.apply_prefs(&ctx);
        assert_eq!(app.ui.theme, ThemeKind::Light);
        assert_eq!(ctx.options(|o| o.theme_preference), egui::ThemePreference::System);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            assert_eq!(ctx.style_of(theme).visuals.panel_fill, app.tokens.panel_bg, "{theme:?}");
        }
        // egui asks the window to follow the system, never to take Light or Dark.
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let cmds = out.viewport_output.values().flat_map(|v| v.commands.iter()).collect::<Vec<_>>();
        assert!(!cmds.iter().any(|c| matches!(c, egui::ViewportCommand::SetTheme(t) if t != &egui::SystemTheme::SystemDefault)), "{cmds:?}");
        for (mode, pref) in [("light", egui::ThemePreference::Light), ("dark", egui::ThemePreference::Dark)] {
            app.set_pref("appearance.appearanceMode", json!(mode)).unwrap();
            app.apply_prefs(&ctx);
            assert_eq!(ctx.options(|o| o.theme_preference), pref, "{mode}");
            assert_eq!(ctx.global_style().visuals.panel_fill, app.tokens.panel_bg, "{mode}");
        }
    }

    /// Picking a theme (`view.theme.*`, `ui.set {theme}`) fixes the mode to its family.
    #[test]
    fn picking_a_theme_fixes_its_mode() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.set_pref("appearance.appearanceMode", json!("auto")).unwrap();
        menus::invoke(&mut app, &ctx, "view.theme.light", json!({})).unwrap();
        assert_eq!(app.session.prefs.appearance.appearance_mode, "light");
        assert_eq!(app.ui.theme, ThemeKind::Light);
        menus::invoke(&mut app, &ctx, "view.theme.darker", json!({})).unwrap();
        assert_eq!(app.session.prefs.appearance.appearance_mode, "dark");
        assert_eq!(app.session.prefs.appearance.dark_theme, "darker");
        assert_eq!(app.ui.theme, ThemeKind::Darker);
    }
}
