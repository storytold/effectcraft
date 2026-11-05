//! Render Settings and Output Module templates (Edit ▸ Templates ▸ Render Settings… / Output
//! Module…): the built-in presets, templates saved with the project, and the defaults new
//! Render Queue items, pre-renders and proxies start from.
//!
//! Built-ins always exist; a saved template with a built-in's name overrides it. Templates
//! carry their name in [`RenderSettings::name`] / [`OutputModule::name`], which the Render Queue
//! shows like After Effects does.

use serde::{Deserialize, Serialize};

use crate::render_queue::{
    AlphaMode, Channels, CurrentOrOff, FieldRender, OutputFormat, OutputModule, ProResProfile, ProxyUse, RenderQuality, RenderSettings, SwitchOverride,
};
use effectcraft_time::FrameRate;

/// A default slot (the Defaults section of the Templates dialogs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TemplateSlot {
    /// New Render Queue items rendering a movie.
    Movie,
    /// Single-frame renders (Composition ▸ Save Frame As ▸ File…).
    Still,
    /// Composition ▸ Pre-render….
    PreRender,
    /// File ▸ Create Proxy ▸ Movie….
    MovieProxy,
    /// File ▸ Create Proxy ▸ Still….
    StillProxy,
}

impl TemplateSlot {
    pub const ALL: [TemplateSlot; 5] = [TemplateSlot::Movie, TemplateSlot::Still, TemplateSlot::PreRender, TemplateSlot::MovieProxy, TemplateSlot::StillProxy];
    pub fn label(self) -> &'static str {
        match self {
            TemplateSlot::Movie => "Movie Default",
            TemplateSlot::Still => "Frame Default",
            TemplateSlot::PreRender => "Pre-Render Default",
            TemplateSlot::MovieProxy => "Movie Proxy Default",
            TemplateSlot::StillProxy => "Still Proxy Default",
        }
    }
    pub fn parse(s: &str) -> Option<TemplateSlot> {
        match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "movie" | "moviedefault" => Some(TemplateSlot::Movie),
            "still" | "frame" | "framedefault" | "stilldefault" => Some(TemplateSlot::Still),
            "prerender" | "prerenderdefault" => Some(TemplateSlot::PreRender),
            "movieproxy" | "movieproxydefault" => Some(TemplateSlot::MovieProxy),
            "stillproxy" | "stillproxydefault" => Some(TemplateSlot::StillProxy),
            _ => None,
        }
    }
}

/// Which template list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TemplateKind {
    RenderSettings,
    OutputModule,
}

impl TemplateKind {
    pub fn parse(s: &str) -> Option<TemplateKind> {
        match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "rendersettings" | "settings" | "render" => Some(TemplateKind::RenderSettings),
            "outputmodule" | "output" | "module" => Some(TemplateKind::OutputModule),
            _ => None,
        }
    }
}

/// Defaults per slot (template names).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateDefaults {
    pub render_settings: Vec<(TemplateSlot, String)>,
    pub output_modules: Vec<(TemplateSlot, String)>,
}

impl Default for TemplateDefaults {
    fn default() -> Self {
        use TemplateSlot::*;
        let s = |v: &[(TemplateSlot, &str)]| v.iter().map(|(k, n)| (*k, n.to_string())).collect();
        TemplateDefaults {
            render_settings: s(&[
                (Movie, "Best Settings"),
                (Still, "Current Settings"),
                (PreRender, "Best Settings"),
                (MovieProxy, "Draft Settings"),
                (StillProxy, "Best Settings"),
            ]),
            output_modules: s(&[
                (Movie, "H.264 - Match Render Settings - 15 Mbps"),
                (Still, "PNG Sequence"),
                (PreRender, "Lossless with Alpha"),
                (MovieProxy, "Lossless with Alpha"),
                (StillProxy, "PNG Sequence with Alpha"),
            ]),
        }
    }
}

/// Templates saved with the project, plus the defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderTemplates {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub render_settings: Vec<RenderSettings>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub output_modules: Vec<OutputModule>,
    pub defaults: TemplateDefaults,
}

/// The built-in Render Settings templates.
pub fn builtin_render_settings() -> Vec<RenderSettings> {
    let best = RenderSettings {
        name: "Best Settings".into(),
        frame_blending: SwitchOverride::OnForChecked,
        motion_blur_mode: SwitchOverride::OnForChecked,
        ..RenderSettings::default()
    };
    let draft = RenderSettings {
        name: "Draft Settings".into(),
        quality: RenderQuality::Draft,
        resolution: 0.5,
        proxy_use: ProxyUse::CurrentSettings,
        frame_blending: SwitchOverride::Current,
        motion_blur_mode: SwitchOverride::Current,
        ..RenderSettings::default()
    };
    let dv =
        RenderSettings { name: "DV Settings".into(), field_render: FieldRender::LowerFirst, frame_rate: Some(FrameRate::new(30000, 1001)), ..best.clone() };
    let multi = RenderSettings { name: "Multi-Machine Settings".into(), skip_existing: true, ..best.clone() };
    let current = RenderSettings {
        name: "Current Settings".into(),
        proxy_use: ProxyUse::CurrentSettings,
        guide_layers: CurrentOrOff::Current,
        ..RenderSettings::default()
    };
    vec![best, draft, dv, multi, current]
}

fn om(name: &str, format: OutputFormat) -> OutputModule {
    let mut m = OutputModule::for_format(format);
    m.name = name.into();
    m
}

/// The built-in Output Module templates.
pub fn builtin_output_modules() -> Vec<OutputModule> {
    let mut v = vec![];
    let mut hq = om("High Quality", OutputFormat::ProRes);
    hq.prores_profile = ProResProfile::Hq;
    v.push(hq);
    let mut hqa = om("High Quality with Alpha", OutputFormat::ProRes);
    hqa.prores_profile = ProResProfile::P4444;
    hqa.channels = Channels::Rgba;
    hqa.alpha_mode = AlphaMode::Premultiplied;
    v.push(hqa);
    // Lossless: the highest-fidelity movie codec this build writes (ProRes 4444 XQ).
    let mut ll = om("Lossless", OutputFormat::ProRes);
    ll.prores_profile = ProResProfile::P4444Xq;
    v.push(ll);
    let mut lla = om("Lossless with Alpha", OutputFormat::ProRes);
    lla.prores_profile = ProResProfile::P4444Xq;
    lla.channels = Channels::Rgba;
    v.push(lla);
    for mbps in [5u32, 15, 40] {
        let mut h = om(&format!("H.264 - Match Render Settings - {mbps} Mbps"), OutputFormat::H264);
        h.bitrate_kbps = mbps * 1000;
        v.push(h);
    }
    let mut alpha = om("Alpha Only", OutputFormat::ProRes);
    alpha.channels = Channels::Alpha;
    v.push(alpha);
    v.push(om("AIFF 48kHz", OutputFormat::Aiff));
    v.push(om("WAV 48kHz", OutputFormat::Wav));
    v.push(om("PNG Sequence", OutputFormat::PngSequence));
    let mut pa = om("PNG Sequence with Alpha", OutputFormat::PngSequence);
    pa.channels = Channels::Rgba;
    v.push(pa);
    let mut ta = om("TIFF Sequence with Alpha", OutputFormat::TiffSequence);
    ta.channels = Channels::Rgba;
    v.push(ta);
    v.push(om("JPEG Sequence", OutputFormat::JpegSequence));
    let mut exr = om("OpenEXR Sequence with Alpha", OutputFormat::ExrSequence);
    exr.channels = Channels::Rgba;
    v.push(exr);
    v.push(om("Animated GIF", OutputFormat::Gif));
    v.push(om("WebM (VP9 + Opus)", OutputFormat::WebM));
    let mut wa = om("WebM with Alpha", OutputFormat::WebM);
    wa.channels = Channels::Rgba;
    v.push(wa);
    v
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

impl RenderTemplates {
    pub fn is_default(&self) -> bool {
        *self == RenderTemplates::default()
    }
    /// Every Render Settings template: built-ins (unless overridden), then saved ones.
    pub fn render_settings_list(&self) -> Vec<RenderSettings> {
        let mut v: Vec<RenderSettings> =
            builtin_render_settings().into_iter().filter(|b| !self.render_settings.iter().any(|u| same(&u.name, &b.name))).collect();
        v.extend(self.render_settings.iter().cloned());
        v
    }
    pub fn output_module_list(&self) -> Vec<OutputModule> {
        let mut v: Vec<OutputModule> = builtin_output_modules().into_iter().filter(|b| !self.output_modules.iter().any(|u| same(&u.name, &b.name))).collect();
        v.extend(self.output_modules.iter().cloned());
        v
    }
    pub fn render_settings(&self, name: &str) -> Option<RenderSettings> {
        self.render_settings_list().into_iter().find(|t| same(&t.name, name))
    }
    pub fn output_module(&self, name: &str) -> Option<OutputModule> {
        self.output_module_list().into_iter().find(|t| same(&t.name, name))
    }
    pub fn is_builtin(kind: TemplateKind, name: &str) -> bool {
        match kind {
            TemplateKind::RenderSettings => builtin_render_settings().iter().any(|t| same(&t.name, name)),
            TemplateKind::OutputModule => builtin_output_modules().iter().any(|t| same(&t.name, name)),
        }
    }
    /// Save (or replace) a template under `name`.
    pub fn save_render_settings(&mut self, name: &str, mut s: RenderSettings) {
        s.name = name.trim().to_string();
        self.render_settings.retain(|t| !same(&t.name, name));
        self.render_settings.push(s);
    }
    pub fn save_output_module(&mut self, name: &str, mut m: OutputModule) {
        m.name = name.trim().to_string();
        self.output_modules.retain(|t| !same(&t.name, name));
        self.output_modules.push(m);
    }
    /// Delete a saved template (a built-in it overrode comes back). Built-ins themselves can't
    /// be deleted.
    pub fn delete(&mut self, kind: TemplateKind, name: &str) -> Result<(), String> {
        let before = (self.render_settings.len(), self.output_modules.len());
        match kind {
            TemplateKind::RenderSettings => self.render_settings.retain(|t| !same(&t.name, name)),
            TemplateKind::OutputModule => self.output_modules.retain(|t| !same(&t.name, name)),
        }
        if before != (self.render_settings.len(), self.output_modules.len()) {
            return Ok(());
        }
        if Self::is_builtin(kind, name) { Err(format!("“{name}” is a built-in template")) } else { Err(format!("no template “{name}”")) }
    }
    /// The default template name of a slot.
    pub fn default_name(&self, kind: TemplateKind, slot: TemplateSlot) -> String {
        let list = match kind {
            TemplateKind::RenderSettings => &self.defaults.render_settings,
            TemplateKind::OutputModule => &self.defaults.output_modules,
        };
        list.iter().find(|(s, _)| *s == slot).map(|(_, n)| n.clone()).unwrap_or_else(|| {
            let d = TemplateDefaults::default();
            let l = if kind == TemplateKind::RenderSettings { d.render_settings } else { d.output_modules };
            l.into_iter().find(|(s, _)| *s == slot).map(|(_, n)| n).unwrap_or_default()
        })
    }
    pub fn set_default(&mut self, kind: TemplateKind, slot: TemplateSlot, name: &str) -> Result<(), String> {
        let exists = match kind {
            TemplateKind::RenderSettings => self.render_settings(name).is_some(),
            TemplateKind::OutputModule => self.output_module(name).is_some(),
        };
        if !exists {
            return Err(format!("no template “{name}”"));
        }
        let list = match kind {
            TemplateKind::RenderSettings => &mut self.defaults.render_settings,
            TemplateKind::OutputModule => &mut self.defaults.output_modules,
        };
        list.retain(|(s, _)| *s != slot);
        list.push((slot, name.trim().to_string()));
        Ok(())
    }
    /// The Render Settings a slot starts from (a missing template falls back to the built-in
    /// default).
    pub fn default_render_settings(&self, slot: TemplateSlot) -> RenderSettings {
        self.render_settings(&self.default_name(TemplateKind::RenderSettings, slot)).unwrap_or_else(|| builtin_render_settings().remove(0))
    }
    pub fn default_output_module(&self, slot: TemplateSlot) -> OutputModule {
        self.output_module(&self.default_name(TemplateKind::OutputModule, slot)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_queue::{ColorDepth, Crop};

    #[test]
    fn builtins_defaults_and_round_trip() {
        let mut t = RenderTemplates::default();
        let names: Vec<String> = t.render_settings_list().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["Best Settings", "Draft Settings", "DV Settings", "Multi-Machine Settings", "Current Settings"]);
        assert_eq!(t.default_render_settings(TemplateSlot::Movie).name, "Best Settings");
        assert_eq!(t.default_output_module(TemplateSlot::PreRender).channels, Channels::Rgba);
        let dv = t.render_settings("dv settings").unwrap();
        assert_eq!(dv.field_render, FieldRender::LowerFirst);
        // Save a custom template, make it the movie default, round-trip through JSON.
        let mut s = RenderSettings { resolution: 0.25, ..Default::default() };
        s.color_depth = ColorDepth::Bpc16;
        t.save_render_settings("Quarter 16", s);
        t.set_default(TemplateKind::RenderSettings, TemplateSlot::Movie, "Quarter 16").unwrap();
        let mut m = OutputModule::for_format(OutputFormat::WebM);
        m.crop = Crop { enabled: true, top: 4, left: 2, ..Default::default() };
        t.save_output_module("Web Crop", m);
        let json = serde_json::to_string(&t).unwrap();
        let back: RenderTemplates = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
        assert_eq!(back.default_render_settings(TemplateSlot::Movie).color_depth, ColorDepth::Bpc16);
        assert_eq!(back.output_module("web crop").unwrap().crop.top, 4);
        assert!(t.delete(TemplateKind::RenderSettings, "Best Settings").is_err());
        t.delete(TemplateKind::OutputModule, "Web Crop").unwrap();
        assert!(t.output_module("Web Crop").is_none());
        // Overriding a built-in, then deleting the override, restores it.
        t.save_output_module("High Quality", OutputModule::for_format(OutputFormat::Gif));
        assert_eq!(t.output_module("High Quality").unwrap().format, OutputFormat::Gif);
        t.delete(TemplateKind::OutputModule, "High Quality").unwrap();
        assert_eq!(t.output_module("High Quality").unwrap().format, OutputFormat::ProRes);
    }
}
