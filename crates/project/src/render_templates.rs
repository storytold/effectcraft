//! Render Settings and Output Module templates (Edit ▸ Templates ▸ Render Settings… / Output
//! Module…): the built-in presets, templates saved with the project, and the defaults new
//! Render Queue items, pre-renders and proxies start from.
//!
//! Built-ins always exist; a saved template with a built-in's name overrides it. Templates
//! carry their name in [`RenderSettings::name`] / [`OutputModule::name`], which the Render Queue
//! shows like After Effects does.

use std::sync::LazyLock;

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
        static RENDER_NAMES: LazyLock<Vec<String>> = LazyLock::new(|| builtin_render_settings().into_iter().map(|t| t.name).collect());
        static OUTPUT_NAMES: LazyLock<Vec<String>> = LazyLock::new(|| builtin_output_modules().into_iter().map(|t| t.name).collect());
        let names = match kind {
            TemplateKind::RenderSettings => &*RENDER_NAMES,
            TemplateKind::OutputModule => &*OUTPUT_NAMES,
        };
        names.iter().any(|builtin| same(builtin, name))
    }
    fn has_template(&self, kind: TemplateKind, name: &str) -> bool {
        let custom = match kind {
            TemplateKind::RenderSettings => self.render_settings.iter().any(|t| same(&t.name, name)),
            TemplateKind::OutputModule => self.output_modules.iter().any(|t| same(&t.name, name)),
        };
        custom || Self::is_builtin(kind, name)
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
            if !Self::is_builtin(kind, name) {
                let defaults = match kind {
                    TemplateKind::RenderSettings => &mut self.defaults.render_settings,
                    TemplateKind::OutputModule => &mut self.defaults.output_modules,
                };
                defaults.retain(|(_, template)| !same(template, name));
            }
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
        list.iter().find(|(s, _)| *s == slot).filter(|(_, name)| self.has_template(kind, name)).map(|(_, name)| name.clone()).unwrap_or_else(|| {
            static FACTORY_DEFAULTS: LazyLock<TemplateDefaults> = LazyLock::new(TemplateDefaults::default);
            let defaults = match kind {
                TemplateKind::RenderSettings => &FACTORY_DEFAULTS.render_settings,
                TemplateKind::OutputModule => &FACTORY_DEFAULTS.output_modules,
            };
            defaults.iter().find(|(s, _)| *s == slot).map(|(_, name)| name.clone()).unwrap_or_default()
        })
    }
    pub fn set_default(&mut self, kind: TemplateKind, slot: TemplateSlot, name: &str) -> Result<(), String> {
        if !self.has_template(kind, name) {
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
        t.set_default(TemplateKind::OutputModule, TemplateSlot::Movie, "High Quality").unwrap();
        assert_eq!(t.default_output_module(TemplateSlot::Movie).format, OutputFormat::Gif);
        t.delete(TemplateKind::OutputModule, "High Quality").unwrap();
        assert_eq!(t.default_output_module(TemplateSlot::Movie).format, OutputFormat::ProRes);
    }

    #[test]
    fn deleting_a_custom_template_restores_each_slot_default() {
        let factory = RenderTemplates::default();
        let mut t = factory.clone();
        let slots = [TemplateSlot::PreRender, TemplateSlot::Still];
        t.save_output_module("Temporary", OutputModule::for_format(OutputFormat::Gif));
        for slot in slots {
            t.set_default(TemplateKind::OutputModule, slot, "Temporary").unwrap();
            assert_eq!(t.default_output_module(slot).format, OutputFormat::Gif);
        }
        t.save_render_settings("Temporary", RenderSettings::default());
        t.set_default(TemplateKind::RenderSettings, TemplateSlot::Still, "Temporary").unwrap();
        t.delete(TemplateKind::OutputModule, " temporary ").unwrap();
        for slot in slots {
            assert_eq!(t.default_output_module(slot), factory.default_output_module(slot));
        }
        assert_eq!(t.default_name(TemplateKind::RenderSettings, TemplateSlot::Still), "Temporary");
        t.save_output_module("Temporary", OutputModule::for_format(OutputFormat::Gif));
        for slot in slots {
            assert_eq!(t.default_output_module(slot), factory.default_output_module(slot));
        }
    }

    #[test]
    fn missing_saved_defaults_use_the_factory_slot_values() {
        let factory = RenderTemplates::default();
        let mut t = factory.clone();
        let slot = TemplateSlot::Still;
        for defaults in [&mut t.defaults.render_settings, &mut t.defaults.output_modules] {
            defaults.retain(|(entry, _)| *entry != slot);
            defaults.push((slot, "Gone".into()));
        }
        assert_eq!(t.default_render_settings(slot), factory.default_render_settings(slot));
        assert_eq!(t.default_output_module(slot), factory.default_output_module(slot));
    }

    /// An Output Module template keeps its OpenEXR compression and bit depth through a save and
    /// load (#2911); the built-in EXR template stays ZIP, 32-bit float.
    #[test]
    fn exr_options_survive_a_template_round_trip() {
        use crate::render_queue::{ExrBitDepth, ExrCompression};
        let mut t = RenderTemplates::default();
        let builtin = t.output_module("OpenEXR Sequence with Alpha").unwrap();
        assert_eq!((builtin.exr_compression, builtin.exr_bit_depth), (ExrCompression::Zip, ExrBitDepth::Float));
        let m = OutputModule { exr_compression: ExrCompression::Piz, exr_bit_depth: ExrBitDepth::Half, ..OutputModule::for_format(OutputFormat::ExrSequence) };
        t.save_output_module("EXR Half PIZ", m);
        let back: RenderTemplates = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        let m = back.output_module("exr half piz").unwrap();
        assert_eq!((m.format, m.exr_compression, m.exr_bit_depth), (OutputFormat::ExrSequence, ExrCompression::Piz, ExrBitDepth::Half));
    }
}
