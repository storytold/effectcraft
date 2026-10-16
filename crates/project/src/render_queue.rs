//! The Render Queue: compositions queued for export, each with Render Settings (what to render:
//! quality, resolution, time span, frame rate) and an Output Module (how to write it: format,
//! channels, codec options, output path). Saved with the project, like After Effects.
//!
//! Output paths are templates: `[compName].[fileExtension]` (the default), with the tokens
//! `[compName]`, `[projectName]`, `[fileExtension]`, `[width]`, `[height]`, `[frameRate]`,
//! `[startFrame]`, `[endFrame]`, `[outputModuleName]` and, for image sequences, a run of `#`
//! (`[#####]`) that becomes the zero-padded frame number. A relative result is resolved against the
//! project's folder (or the working directory for unsaved projects).

use effectcraft_time::{FrameRate, Tick};
use serde::{Deserialize, Serialize};

use crate::{Comp, ItemId};

/// The default output path template.
pub const DEFAULT_TEMPLATE: &str = "[compName].[fileExtension]";
/// The default template for image sequences.
pub const DEFAULT_SEQUENCE_TEMPLATE: &str = "[compName]_[#####].[fileExtension]";

/// Render Settings ▸ Quality.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderQuality {
    #[default]
    Best,
    Draft,
}

/// Render Settings ▸ Time Span.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TimeSpan {
    /// The composition's work area.
    #[default]
    WorkArea,
    /// The whole composition.
    LengthOfComp,
    /// An explicit span `[start, end)` in comp time.
    Custom { start: Tick, end: Tick },
}

/// Render Settings ▸ Proxy Use: which proxies a render uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProxyUse {
    /// Each item's Use Proxy switch (what the viewer shows).
    #[default]
    CurrentSettings,
    /// Every item that has a proxy uses it.
    UseAll,
    /// Only compositions' proxies.
    UseCompOnly,
    /// No proxies (full-resolution sources).
    UseNone,
}

impl ProxyUse {
    pub const ALL: [ProxyUse; 4] = [ProxyUse::CurrentSettings, ProxyUse::UseAll, ProxyUse::UseCompOnly, ProxyUse::UseNone];
    pub fn label(self) -> &'static str {
        match self {
            ProxyUse::CurrentSettings => "Current Settings",
            ProxyUse::UseAll => "Use All Proxies",
            ProxyUse::UseCompOnly => "Use Comp Proxies Only",
            ProxyUse::UseNone => "Use No Proxies",
        }
    }
    pub fn parse(s: &str) -> Option<ProxyUse> {
        match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "current" | "currentsettings" => Some(ProxyUse::CurrentSettings),
            "all" | "useall" | "useallproxies" => Some(ProxyUse::UseAll),
            "comp" | "componly" | "usecompproxiesonly" | "usecomponly" => Some(ProxyUse::UseCompOnly),
            "none" | "usenone" | "usenoproxies" => Some(ProxyUse::UseNone),
            _ => None,
        }
    }
    /// Whether an item's proxy is used: `comp` = the item is a composition, `enabled` = its Use
    /// Proxy switch.
    pub fn uses(self, comp: bool, enabled: bool) -> bool {
        match self {
            ProxyUse::CurrentSettings => enabled,
            ProxyUse::UseAll => true,
            ProxyUse::UseCompOnly => comp,
            ProxyUse::UseNone => false,
        }
    }
}

/// Render Settings ▸ Field Render: interlaced output, each frame woven from two fields rendered
/// half a frame apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldRender {
    #[default]
    Off,
    /// The upper field (lines 0, 2, 4…) holds the earlier time.
    UpperFirst,
    LowerFirst,
}

impl FieldRender {
    pub const ALL: [FieldRender; 3] = [FieldRender::Off, FieldRender::UpperFirst, FieldRender::LowerFirst];
    pub fn label(self) -> &'static str {
        match self {
            FieldRender::Off => "Off",
            FieldRender::UpperFirst => "Upper Field First",
            FieldRender::LowerFirst => "Lower Field First",
        }
    }
    pub fn parse(s: &str) -> Option<FieldRender> {
        match norm(s).as_str() {
            "off" | "none" | "progressive" => Some(FieldRender::Off),
            "upper" | "upperfirst" | "upperfieldfirst" => Some(FieldRender::UpperFirst),
            "lower" | "lowerfirst" | "lowerfieldfirst" => Some(FieldRender::LowerFirst),
            _ => None,
        }
    }
}

fn norm(s: &str) -> String {
    s.to_ascii_lowercase().replace([' ', '_', '-', '.', ':', '&'], "")
}

/// Render Settings ▸ 3:2 Pulldown: with field rendering, four film frames (at 4/5 of the output
/// rate) spread over five interlaced frames in 2:3 cadence. The phase names which of the five
/// frames are Whole (both fields from one film frame) or Split.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pulldown {
    #[default]
    Off,
    Wssww,
    Sswww,
    Swwws,
    Wwwss,
    Wwssw,
}

/// Film frame (within a group of four) of each of the ten fields of the 2:3 cadence.
const CADENCE: [i64; 10] = [0, 0, 1, 1, 1, 2, 2, 3, 3, 3];

impl Pulldown {
    pub const ALL: [Pulldown; 6] = [Pulldown::Off, Pulldown::Wssww, Pulldown::Sswww, Pulldown::Swwws, Pulldown::Wwwss, Pulldown::Wwssw];
    pub fn label(self) -> &'static str {
        match self {
            Pulldown::Off => "Off",
            Pulldown::Wssww => "WSSWW",
            Pulldown::Sswww => "SSWWW",
            Pulldown::Swwws => "SWWWS",
            Pulldown::Wwwss => "WWWSS",
            Pulldown::Wwssw => "WWSSW",
        }
    }
    pub fn parse(s: &str) -> Option<Pulldown> {
        let n = norm(s);
        if matches!(n.as_str(), "off" | "none") {
            return Some(Pulldown::Off);
        }
        Pulldown::ALL.into_iter().find(|p| p.label().eq_ignore_ascii_case(&n))
    }
    /// Offset (in frames) into the WWSSW base cadence.
    fn rotation(self) -> Option<i64> {
        Some(match self {
            Pulldown::Off => return None,
            Pulldown::Wwssw => 0,
            Pulldown::Wssww => 1,
            Pulldown::Sswww => 2,
            Pulldown::Swwws => 3,
            Pulldown::Wwwss => 4,
        })
    }
    /// The film frames (counted from the first film frame of the render) of the first and
    /// second field of output frame `k`.
    pub fn fields(self, k: u64) -> Option<(i64, i64)> {
        let r = self.rotation()?;
        let film = |f: i64| f.div_euclid(10) * 4 + CADENCE[f.rem_euclid(10) as usize];
        let f0 = 2 * (k as i64 + r);
        let base = film(2 * r);
        Some((film(f0) - base, film(f0 + 1) - base))
    }
    /// The W/S pattern of five consecutive output frames.
    pub fn pattern(self) -> String {
        (0..5).filter_map(|k| self.fields(k).map(|(a, b)| if a == b { 'W' } else { 'S' })).collect()
    }
}

/// A layer switch override (Render Settings ▸ Frame Blending / Motion Blur).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwitchOverride {
    /// The layer switches and the composition's enable switch.
    #[default]
    Current,
    /// Every layer whose switch is on, whatever the composition's enable switch.
    OnForChecked,
    OffForAll,
}

impl SwitchOverride {
    pub const ALL: [SwitchOverride; 3] = [SwitchOverride::Current, SwitchOverride::OnForChecked, SwitchOverride::OffForAll];
    pub fn label(self) -> &'static str {
        match self {
            SwitchOverride::Current => "Current Settings",
            SwitchOverride::OnForChecked => "On for Checked Layers",
            SwitchOverride::OffForAll => "Off for All Layers",
        }
    }
    pub fn parse(s: &str) -> Option<SwitchOverride> {
        match norm(s).as_str() {
            "current" | "currentsettings" => Some(SwitchOverride::Current),
            "on" | "onforchecked" | "onforcheckedlayers" | "true" => Some(SwitchOverride::OnForChecked),
            "off" | "offforall" | "offforalllayers" | "false" => Some(SwitchOverride::OffForAll),
            _ => None,
        }
    }
}

/// Render Settings ▸ Effects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectsMode {
    #[default]
    Current,
    AllOn,
    AllOff,
}

impl EffectsMode {
    pub const ALL: [EffectsMode; 3] = [EffectsMode::Current, EffectsMode::AllOn, EffectsMode::AllOff];
    pub fn label(self) -> &'static str {
        match self {
            EffectsMode::Current => "Current Settings",
            EffectsMode::AllOn => "All On",
            EffectsMode::AllOff => "All Off",
        }
    }
    pub fn parse(s: &str) -> Option<EffectsMode> {
        match norm(s).as_str() {
            "current" | "currentsettings" => Some(EffectsMode::Current),
            "allon" | "on" => Some(EffectsMode::AllOn),
            "alloff" | "off" => Some(EffectsMode::AllOff),
            _ => None,
        }
    }
}

/// Render Settings ▸ Solo Switches / Guide Layers: the current switches, or all off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CurrentOrOff {
    #[default]
    Current,
    AllOff,
}

impl CurrentOrOff {
    pub fn label(self) -> &'static str {
        match self {
            CurrentOrOff::Current => "Current Settings",
            CurrentOrOff::AllOff => "All Off",
        }
    }
    pub fn parse(s: &str) -> Option<CurrentOrOff> {
        match norm(s).as_str() {
            "current" | "currentsettings" | "on" => Some(CurrentOrOff::Current),
            "alloff" | "off" => Some(CurrentOrOff::AllOff),
            _ => None,
        }
    }
}

/// Render Settings ▸ Color Depth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorDepth {
    /// The project's bit depth.
    #[default]
    Current,
    Bpc8,
    Bpc16,
    Bpc32,
}

impl ColorDepth {
    pub const ALL: [ColorDepth; 4] = [ColorDepth::Current, ColorDepth::Bpc8, ColorDepth::Bpc16, ColorDepth::Bpc32];
    pub fn label(self) -> &'static str {
        match self {
            ColorDepth::Current => "Current Settings",
            ColorDepth::Bpc8 => "8 bits per channel",
            ColorDepth::Bpc16 => "16 bits per channel",
            ColorDepth::Bpc32 => "32 bits per channel",
        }
    }
    pub fn parse(s: &str) -> Option<ColorDepth> {
        match norm(s).as_str() {
            "current" | "currentsettings" => Some(ColorDepth::Current),
            "8" | "8bpc" | "8bitsperchannel" => Some(ColorDepth::Bpc8),
            "16" | "16bpc" | "16bitsperchannel" => Some(ColorDepth::Bpc16),
            "32" | "32bpc" | "32bitsperchannel" | "float" => Some(ColorDepth::Bpc32),
            _ => None,
        }
    }
    /// The depth renders use in a project of depth `project`.
    pub fn resolve(self, project: crate::BitDepth) -> crate::BitDepth {
        match self {
            ColorDepth::Current => project,
            ColorDepth::Bpc8 => crate::BitDepth::Bpc8,
            ColorDepth::Bpc16 => crate::BitDepth::Bpc16,
            ColorDepth::Bpc32 => crate::BitDepth::Bpc32,
        }
    }
}

/// How one output frame is sampled in time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameSample {
    Progressive(Tick),
    /// Two fields woven together: `first` goes to the dominant field.
    Fields {
        first: Tick,
        second: Tick,
        upper_first: bool,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    /// The template these settings came from ("" or "Custom" once edited).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub quality: RenderQuality,
    /// Output scale (1 = Full, 0.5 = Half, 1/3 = Third, 0.25 = Quarter).
    pub resolution: f64,
    pub time_span: TimeSpan,
    /// `None` = use the comp's frame rate (Time Sampling ▸ Use comp's frame rate).
    pub frame_rate: Option<FrameRate>,
    /// Motion blur allowed at all (false = Off for All Layers; see `motion_blur_mode`).
    pub motion_blur: bool,
    /// Motion Blur: Current Settings / On for Checked Layers / Off for All Layers.
    pub motion_blur_mode: SwitchOverride,
    /// Frame Blending: Current Settings / On for Checked Layers / Off for All Layers.
    pub frame_blending: SwitchOverride,
    /// Field Render.
    pub field_render: FieldRender,
    /// 3:2 Pulldown phase (with field rendering).
    pub pulldown: Pulldown,
    /// Effects: Current Settings / All On / All Off.
    pub effects: EffectsMode,
    /// Solo Switches: Current Settings / All Off.
    pub solo: CurrentOrOff,
    /// Guide Layers: Current Settings / All Off.
    pub guide_layers: CurrentOrOff,
    /// Color Depth.
    pub color_depth: ColorDepth,
    /// Image sequences: skip frames whose file already exists.
    pub skip_existing: bool,
    /// Proxy Use (Best Settings: Use No Proxies).
    pub proxy_use: ProxyUse,
    /// Use Storage Overflow: when the output volume is full, continue in the project's overflow
    /// folders.
    #[serde(default = "default_true")]
    pub storage_overflow: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings {
            name: String::new(),
            quality: RenderQuality::Best,
            resolution: 1.0,
            time_span: TimeSpan::WorkArea,
            frame_rate: None,
            motion_blur: true,
            motion_blur_mode: SwitchOverride::Current,
            frame_blending: SwitchOverride::Current,
            field_render: FieldRender::Off,
            pulldown: Pulldown::Off,
            effects: EffectsMode::Current,
            solo: CurrentOrOff::Current,
            guide_layers: CurrentOrOff::AllOff,
            color_depth: ColorDepth::Current,
            skip_existing: false,
            proxy_use: ProxyUse::UseNone,
            storage_overflow: true,
        }
    }
}

impl RenderSettings {
    /// Effective Motion Blur override.
    pub fn motion_blur_override(&self) -> SwitchOverride {
        if self.motion_blur { self.motion_blur_mode } else { SwitchOverride::OffForAll }
    }
    /// How output frame `i` is sampled (field rendering and 3:2 pulldown).
    pub fn sample(&self, comp: &Comp, i: u64) -> FrameSample {
        let upper_first = match self.field_render {
            FieldRender::Off => return FrameSample::Progressive(self.frame_time(comp, i)),
            FieldRender::UpperFirst => true,
            FieldRender::LowerFirst => false,
        };
        let r = self.rate(comp);
        if let Some((a, b)) = self.pulldown.fields(i) {
            // Film frames at 4/5 of the output rate, from the first one starting in the span.
            let film = FrameRate::new(r.num * 4, r.den * 5);
            let f0 = Self::ceil_frame(film, self.span(comp).0);
            return FrameSample::Fields { first: film.tick_of(f0 + a), second: film.tick_of(f0 + b), upper_first };
        }
        let f = self.first_frame(comp) + i as i64;
        let t = r.tick_of(f);
        let half = Tick((r.tick_of(f + 1).0 - t.0) / 2);
        FrameSample::Fields { first: t, second: Tick(t.0 + half.0), upper_first }
    }
    /// The comp-time span `[start, end)` to render.
    pub fn span(&self, comp: &Comp) -> (Tick, Tick) {
        let (a, b) = match self.time_span {
            TimeSpan::WorkArea => comp.work_area,
            TimeSpan::LengthOfComp => (Tick::ZERO, comp.duration),
            TimeSpan::Custom { start, end } => (start, end),
        };
        let a = a.clamp(Tick::ZERO, comp.duration);
        let b = b.clamp(a, comp.duration);
        (a, b)
    }
    /// The output frame rate: the Frame Rate override, unless the comp preserves its frame rate
    /// (Composition Settings ▸ Preserve frame rate when nested or in render queue).
    pub fn rate(&self, comp: &Comp) -> FrameRate {
        if comp.preserve_frame_rate { comp.frame_rate } else { self.frame_rate.unwrap_or(comp.frame_rate) }
    }
    /// First output-rate frame starting at or after `t`.
    fn ceil_frame(r: FrameRate, t: Tick) -> i64 {
        let f = r.frame_at(t);
        if r.tick_of(f) < t { f + 1 } else { f }
    }
    /// Output-rate frame number of output frame 0 (the first frame starting inside the span).
    pub fn first_frame(&self, comp: &Comp) -> i64 {
        Self::ceil_frame(self.rate(comp), self.span(comp).0)
    }
    /// Number of output frames: those starting in `[start, end)` (at least 1).
    pub fn frame_count(&self, comp: &Comp) -> u64 {
        let (_, b) = self.span(comp);
        let n = Self::ceil_frame(self.rate(comp), b) - self.first_frame(comp);
        n.max(1) as u64
    }
    /// Comp time of output frame `i`.
    pub fn frame_time(&self, comp: &Comp, i: u64) -> Tick {
        self.rate(comp).tick_of(self.first_frame(comp) + i as i64)
    }
    pub fn output_size(&self, comp: &Comp) -> (u32, u32) {
        let s = self.resolution.clamp(0.01, 4.0);
        (((comp.width as f64 * s).round() as u32).max(1), ((comp.height as f64 * s).round() as u32).max(1))
    }
    pub fn resolution_label(&self) -> &'static str {
        match self.resolution {
            r if (r - 1.0).abs() < 1e-6 => "Full",
            r if (r - 0.5).abs() < 1e-6 => "Half",
            r if (r - 1.0 / 3.0).abs() < 1e-3 => "Third",
            r if (r - 0.25).abs() < 1e-6 => "Quarter",
            _ => "Custom",
        }
    }
    /// One-line summary shown next to "Render Settings:" (the template name, like AE).
    pub fn summary(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        let q = if self.quality == RenderQuality::Best { "Best Settings" } else { "Draft Settings" };
        format!("{q} · {}", self.resolution_label())
    }
    /// Every setting as `(label, value)` lines (render logs, the settings dialog).
    pub fn describe(&self, comp: Option<&Comp>) -> Vec<(&'static str, String)> {
        let span = match self.time_span {
            TimeSpan::WorkArea => "Work Area Only".to_string(),
            TimeSpan::LengthOfComp => "Length of Comp".to_string(),
            TimeSpan::Custom { start, end } => format!("Custom ({:.3}s – {:.3}s)", start.seconds(), end.seconds()),
        };
        let rate = match (self.frame_rate, comp) {
            (Some(r), _) => format!("{:.3} fps", r.as_f64()),
            (None, Some(c)) => format!("Use comp's frame rate ({:.3})", c.frame_rate.as_f64()),
            (None, None) => "Use comp's frame rate".into(),
        };
        vec![
            ("Template", if self.name.is_empty() { "Custom".into() } else { self.name.clone() }),
            ("Quality", format!("{:?}", self.quality)),
            ("Resolution", format!("{} ({:.3})", self.resolution_label(), self.resolution)),
            ("Proxy Use", self.proxy_use.label().into()),
            ("Effects", self.effects.label().into()),
            ("Solo Switches", self.solo.label().into()),
            ("Guide Layers", self.guide_layers.label().into()),
            ("Color Depth", self.color_depth.label().into()),
            ("Frame Blending", self.frame_blending.label().into()),
            ("Field Render", self.field_render.label().into()),
            ("3:2 Pulldown", self.pulldown.label().into()),
            ("Motion Blur", self.motion_blur_override().label().into()),
            ("Time Span", span),
            ("Frame Rate", rate),
            ("Skip Existing Files", if self.skip_existing { "On" } else { "Off" }.into()),
            ("Use Storage Overflow", if self.storage_overflow { "On" } else { "Off" }.into()),
        ]
    }
}

/// Output Module ▸ Format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OutputFormat {
    /// H.264 video (+ AAC audio) in MP4.
    #[default]
    H264,
    /// Apple ProRes in QuickTime (+ PCM audio). 4444 when channels include alpha.
    ProRes,
    PngSequence,
    JpegSequence,
    TiffSequence,
    /// OpenEXR sequence (32-bit float, linear light).
    ExrSequence,
    /// Animated GIF.
    Gif,
    /// WebM: VP9 video (alpha with RGB + Alpha) + Opus audio.
    WebM,
    /// Audio only: WAV (16-bit PCM).
    Wav,
    /// Audio only: AIFF (16-bit PCM).
    Aiff,
    /// HEVC (H.265) video (+ AAC audio) in MP4 (`hvc1`).
    Hevc,
    /// AV1 video (+ AAC audio) in MP4 (`av01`). WebM with AV1: [`OutputFormat::WebM`] with
    /// [`WebmVideoCodec::Av1`].
    Av1,
    /// Uncompressed 8-bit RGB / ARGB (`raw `) in QuickTime (+ PCM audio), for handing to an
    /// external encoder.
    Uncompressed,
}

impl OutputFormat {
    pub const ALL: [OutputFormat; 13] = [
        OutputFormat::H264,
        OutputFormat::Hevc,
        OutputFormat::Av1,
        OutputFormat::ProRes,
        OutputFormat::Uncompressed,
        OutputFormat::WebM,
        OutputFormat::PngSequence,
        OutputFormat::JpegSequence,
        OutputFormat::TiffSequence,
        OutputFormat::ExrSequence,
        OutputFormat::Gif,
        OutputFormat::Wav,
        OutputFormat::Aiff,
    ];

    pub fn label(self) -> &'static str {
        match self {
            OutputFormat::H264 => "H.264",
            OutputFormat::ProRes => "QuickTime (ProRes)",
            OutputFormat::PngSequence => "PNG Sequence",
            OutputFormat::JpegSequence => "JPEG Sequence",
            OutputFormat::TiffSequence => "TIFF Sequence",
            OutputFormat::ExrSequence => "OpenEXR Sequence",
            OutputFormat::Gif => "Animated GIF",
            OutputFormat::WebM => "WebM (VP9 + Opus)",
            OutputFormat::Wav => "WAV",
            OutputFormat::Aiff => "AIFF",
            OutputFormat::Hevc => "HEVC (H.265)",
            OutputFormat::Av1 => "AV1",
            OutputFormat::Uncompressed => "QuickTime (Uncompressed)",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::H264 => "mp4",
            OutputFormat::ProRes | OutputFormat::Uncompressed => "mov",
            OutputFormat::PngSequence => "png",
            OutputFormat::JpegSequence => "jpg",
            OutputFormat::TiffSequence => "tif",
            OutputFormat::ExrSequence => "exr",
            OutputFormat::Gif => "gif",
            OutputFormat::WebM => "webm",
            OutputFormat::Wav => "wav",
            OutputFormat::Aiff => "aif",
            OutputFormat::Hevc | OutputFormat::Av1 => "mp4",
        }
    }
    pub fn is_sequence(self) -> bool {
        matches!(self, OutputFormat::PngSequence | OutputFormat::JpegSequence | OutputFormat::TiffSequence | OutputFormat::ExrSequence)
    }
    pub fn is_movie(self) -> bool {
        matches!(self, OutputFormat::H264 | OutputFormat::ProRes | OutputFormat::Uncompressed | OutputFormat::WebM | OutputFormat::Hevc | OutputFormat::Av1)
    }
    /// Formats whose video codec options live in [`OutputModule::codec`] (HEVC, AV1).
    pub fn has_codec_options(self) -> bool {
        matches!(self, OutputFormat::Hevc | OutputFormat::Av1)
    }
    /// Audio-only outputs (no video is rendered).
    pub fn is_audio_only(self) -> bool {
        matches!(self, OutputFormat::Wav | OutputFormat::Aiff)
    }
    pub fn supports_alpha(self) -> bool {
        !matches!(self, OutputFormat::H264 | OutputFormat::Hevc | OutputFormat::Av1 | OutputFormat::JpegSequence | OutputFormat::Wav | OutputFormat::Aiff)
    }
    pub fn supports_audio(self) -> bool {
        self.is_movie() || self.is_audio_only()
    }
    /// The written frame size for a rendered size: H.264 and HEVC need even dimensions (rounded down,
    /// at least 2).
    pub fn coded_size(self, w: u32, h: u32) -> (u32, u32) {
        let even = |v: u32| if v < 2 { 2 } else { v & !1 };
        if matches!(self, OutputFormat::H264 | OutputFormat::Hevc) { (even(w), even(h)) } else { (w, h) }
    }
    /// Parse a format name: `h264`, `mp4`, `prores`, `mov`, `uncompressed`, `png`, `jpeg`/`jpg`,
    /// `tiff`/`tif`, `exr`, `gif`, or a label.
    pub fn from_name(s: &str) -> Option<OutputFormat> {
        let n = s.trim().to_ascii_lowercase().replace([' ', '.', '-', '_'], "");
        Some(match n.as_str() {
            "h264" | "mp4" | "avc" => OutputFormat::H264,
            "hevc" | "h265" | "hevch265" | "hvc1" | "x265" => OutputFormat::Hevc,
            "av1" | "av01" | "mp4av1" => OutputFormat::Av1,
            "prores" | "mov" | "quicktime" | "quicktimeprores" => OutputFormat::ProRes,
            "uncompressed" | "quicktimeuncompressed" | "rawvideo" => OutputFormat::Uncompressed,
            "png" | "pngsequence" => OutputFormat::PngSequence,
            "jpg" | "jpeg" | "jpegsequence" => OutputFormat::JpegSequence,
            "tif" | "tiff" | "tiffsequence" => OutputFormat::TiffSequence,
            "exr" | "openexr" | "openexrsequence" => OutputFormat::ExrSequence,
            "gif" | "animatedgif" => OutputFormat::Gif,
            "webm" | "vp9" | "webmvp9opus" => OutputFormat::WebM,
            "wav" | "wave" => OutputFormat::Wav,
            "aif" | "aiff" | "aifc" => OutputFormat::Aiff,
            _ => return None,
        })
    }
    /// Guess from a file name's extension.
    pub fn from_path(path: &str) -> Option<OutputFormat> {
        let ext = std::path::Path::new(path).extension()?.to_string_lossy().to_ascii_lowercase();
        OutputFormat::from_name(&ext)
    }
    /// An output `path` without its extensions, for this format to add its own: the project
    /// extensions a save dialog filtered to projects appended (`Comp 1.png.ecproj`, #293), one
    /// output format's extension (the one being replaced), and any more of this format's
    /// (`Comp 2.mov.mov`, from a format change or a comp named `Comp 2.mov`). Another extension
    /// stays part of the name (`logo.png.mov`).
    pub fn output_stem(self, path: &str) -> &str {
        let stem = without_project_extension(path);
        let mut stem = strip_extension(stem, |e| OutputFormat::from_name(e).is_some()).unwrap_or(stem);
        while let Some(s) = strip_extension(stem, |e| e.eq_ignore_ascii_case(self.extension())) {
            stem = s;
        }
        stem
    }
    /// `path` ending in this format's extension exactly once (see [`OutputFormat::output_stem`]);
    /// a path that already does is kept as it is. Templates still to expand `[fileExtension]` are
    /// kept too.
    pub fn with_extension(self, path: &str) -> String {
        if path.contains("[fileExtension]") {
            return path.to_string();
        }
        let fixed = format!("{}.{}", self.output_stem(path), self.extension());
        if fixed.eq_ignore_ascii_case(path) { path.to_string() } else { fixed }
    }
}

/// Project file extensions: never a render's output.
const PROJECT_EXTENSIONS: [&str; 2] = ["ecproj", "ecprojx"];

/// An output `path` without the project extensions a save dialog filtered to projects appended
/// to it (`Comp 1.mov.ecproj` → `Comp 1.mov`, #293).
pub fn without_project_extension(path: &str) -> &str {
    let mut p = path;
    while let Some(s) = strip_extension(p, |e| PROJECT_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x))) {
        p = s;
    }
    p
}

/// `path` without its last extension when `strip` accepts that extension.
fn strip_extension(path: &str, strip: impl Fn(&str) -> bool) -> Option<&str> {
    let ext = std::path::Path::new(path).extension()?.to_str()?;
    if !strip(ext) {
        return None;
    }
    path.strip_suffix(ext)?.strip_suffix('.')
}

/// Output Module ▸ Channels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channels {
    /// Opaque, composited over the comp background colour.
    #[default]
    Rgb,
    /// With alpha (straight or premultiplied: [`AlphaMode`]).
    Rgba,
    /// The alpha channel alone, as an opaque greyscale matte.
    Alpha,
}

impl Channels {
    pub fn label(self) -> &'static str {
        match self {
            Channels::Rgb => "RGB",
            Channels::Rgba => "RGB + Alpha",
            Channels::Alpha => "Alpha",
        }
    }
}

/// Output Module ▸ Color: how RGB is stored with alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlphaMode {
    /// Unmatted colour.
    #[default]
    Straight,
    /// Colour multiplied by alpha (matted with black).
    Premultiplied,
}

/// Output Module ▸ Audio Output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioOutput {
    /// On when the comp has audible layers.
    #[default]
    Auto,
    On,
    Off,
}

/// Output Module ▸ Audio Output ▸ sample format (PCM outputs: WAV, AIFF, QuickTime).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioFormat {
    #[default]
    S16,
    S24,
    /// 32-bit float (WAV, QuickTime; AIFF writes 24-bit).
    F32,
}

impl AudioFormat {
    pub fn label(self) -> &'static str {
        match self {
            AudioFormat::S16 => "16 Bit",
            AudioFormat::S24 => "24 Bit",
            AudioFormat::F32 => "32 Bit Float",
        }
    }
    pub fn parse(s: &str) -> Option<AudioFormat> {
        match norm(s).as_str() {
            "16" | "16bit" | "s16" => Some(AudioFormat::S16),
            "24" | "24bit" | "s24" => Some(AudioFormat::S24),
            "32" | "32bit" | "32bitfloat" | "f32" | "float" => Some(AudioFormat::F32),
            _ => None,
        }
    }
}

/// Output Module ▸ Crop (pixels of the rendered frame; negative values add transparent pixels).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crop {
    pub enabled: bool,
    /// Use Region of Interest: crop to `roi` (comp pixels, captured from the viewer's region of
    /// interest when the option is turned on) instead of the edge values.
    pub use_roi: bool,
    pub roi: Option<[f64; 4]>,
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
}

impl Crop {
    /// Edges `(top, left, bottom, right)` in pixels of a `w`×`h` frame rendered at `scale`.
    pub fn edges(&self, w: u32, h: u32, scale: f64) -> (i32, i32, i32, i32) {
        if !self.enabled {
            return (0, 0, 0, 0);
        }
        if self.use_roi
            && let Some([x, y, rw, rh]) = self.roi
        {
            // The quad arrives unvalidated (control channel or a hand-edited project):
            // clamp like the sibling edge fields (plus/minus 20 000) and subtract
            // without overflow.
            let conv = |v: f64| v.round().clamp(-20_000.0, 20_000.0) as i32;
            let l = conv(x * scale);
            let t = conv(y * scale);
            let r = (w as i32).saturating_sub(conv((x + rw) * scale));
            let b = (h as i32).saturating_sub(conv((y + rh) * scale));
            return (t, l, b, r);
        }
        (self.top, self.left, self.bottom, self.right)
    }
}

/// Output Module ▸ Resize quality.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResizeQuality {
    /// Bilinear.
    Low,
    /// Bicubic.
    #[default]
    High,
}

/// Output Module ▸ Resize.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Resize {
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
    /// Lock Aspect Ratio: setting one dimension derives the other from the rendered frame.
    pub lock_aspect: bool,
    pub quality: ResizeQuality,
}

impl Default for Resize {
    fn default() -> Self {
        Resize { enabled: false, width: 1920, height: 1080, lock_aspect: true, quality: ResizeQuality::High }
    }
}

/// Output Module ▸ Resize ▸ Resize to: presets.
pub const RESIZE_PRESETS: [(&str, u32, u32); 10] = [
    ("HDTV 1080", 1920, 1080),
    ("HDTV 720", 1280, 720),
    ("UHD 4K", 3840, 2160),
    ("DCI 2K", 2048, 1080),
    ("DCI 4K", 4096, 2160),
    ("NTSC DV", 720, 480),
    ("PAL D1/DV", 720, 576),
    ("Square 1080", 1080, 1080),
    ("Vertical 1080×1920", 1080, 1920),
    ("Web 640×360", 640, 360),
];

/// Output Module ▸ Post-Render Action.
pub fn post_render_parse(s: &str) -> Option<PostRenderAction> {
    match norm(s).as_str() {
        "none" => Some(PostRenderAction::None),
        "import" => Some(PostRenderAction::Import),
        "importreplace" | "importandreplace" | "importandreplaceusage" | "importreplaceusage" | "replace" => Some(PostRenderAction::ImportAndReplace),
        "setproxy" | "proxy" => Some(PostRenderAction::SetProxy),
        _ => None,
    }
}

/// ProRes flavour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProResProfile {
    Proxy,
    Lt,
    Standard,
    #[default]
    Hq,
    P4444,
    P4444Xq,
}

impl ProResProfile {
    pub const ALL: [ProResProfile; 6] =
        [ProResProfile::Proxy, ProResProfile::Lt, ProResProfile::Standard, ProResProfile::Hq, ProResProfile::P4444, ProResProfile::P4444Xq];
    pub fn label(self) -> &'static str {
        match self {
            ProResProfile::Proxy => "Apple ProRes 422 Proxy",
            ProResProfile::Lt => "Apple ProRes 422 LT",
            ProResProfile::Standard => "Apple ProRes 422",
            ProResProfile::Hq => "Apple ProRes 422 HQ",
            ProResProfile::P4444 => "Apple ProRes 4444",
            ProResProfile::P4444Xq => "Apple ProRes 4444 XQ",
        }
    }
    pub fn is_4444(self) -> bool {
        matches!(self, ProResProfile::P4444 | ProResProfile::P4444Xq)
    }
    pub fn from_name(s: &str) -> Option<ProResProfile> {
        let n = s.to_ascii_lowercase().replace([' ', '-', '_'], "");
        ProResProfile::ALL.into_iter().find(|p| {
            let l = p.label().to_ascii_lowercase().replace(' ', "");
            l == n || l.trim_start_matches("appleprores") == n.trim_start_matches("appleprores") || format!("{p:?}").to_ascii_lowercase() == n
        })
    }
}

/// HEVC / AV1 profile: 8-bit (HEVC Main, AV1 Main 8-bit) or 10-bit (HEVC Main 10, AV1 Main 10-bit).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodecProfile {
    #[default]
    Main,
    Main10,
}

impl CodecProfile {
    pub const ALL: [CodecProfile; 2] = [CodecProfile::Main, CodecProfile::Main10];
    pub fn label(self) -> &'static str {
        match self {
            CodecProfile::Main => "Main",
            CodecProfile::Main10 => "Main 10",
        }
    }
    pub fn bit_depth(self) -> u8 {
        match self {
            CodecProfile::Main => 8,
            CodecProfile::Main10 => 10,
        }
    }
    pub fn from_name(s: &str) -> Option<CodecProfile> {
        match s.to_ascii_lowercase().replace([' ', '-', '_'], "").as_str() {
            "main" | "main8" | "8" | "8bit" => Some(CodecProfile::Main),
            "main10" | "10" | "10bit" => Some(CodecProfile::Main10),
            _ => None,
        }
    }
}

/// HEVC / AV1 rate control: a target bitrate ([`OutputModule::bitrate_kbps`]) or constant
/// quality ([`VideoCodecOptions::quality`], CRF-like).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RateControlMode {
    #[default]
    Bitrate,
    Quality,
}

impl RateControlMode {
    pub fn label(self) -> &'static str {
        match self {
            RateControlMode::Bitrate => "Target Bitrate",
            RateControlMode::Quality => "Constant Quality",
        }
    }
    pub fn from_name(s: &str) -> Option<RateControlMode> {
        match s.to_ascii_lowercase().replace([' ', '-', '_'], "").as_str() {
            "bitrate" | "vbr" | "abr" | "targetbitrate" => Some(RateControlMode::Bitrate),
            "quality" | "crf" | "cq" | "constantquality" | "qp" => Some(RateControlMode::Quality),
            _ => None,
        }
    }
}

/// Video codec options of the HEVC and AV1 formats (and AV1 in WebM).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoCodecOptions {
    pub profile: CodecProfile,
    /// Level × 10 (`41` = level 4.1); `None` picks the lowest level that fits the frame size and rate.
    pub level: Option<u8>,
    pub rate_control: RateControlMode,
    /// Constant-quality setting 1–100 (100 ≈ visually lossless), used with [`RateControlMode::Quality`].
    pub quality: u8,
}

impl Default for VideoCodecOptions {
    fn default() -> Self {
        VideoCodecOptions { profile: CodecProfile::Main, level: None, rate_control: RateControlMode::Bitrate, quality: 70 }
    }
}

impl VideoCodecOptions {
    /// HEVC levels (× 10) offered in the UI (Main tier).
    pub const HEVC_LEVELS: [u8; 13] = [10, 20, 21, 30, 31, 40, 41, 50, 51, 52, 60, 61, 62];
    /// AV1 levels (× 10) offered in the UI.
    pub const AV1_LEVELS: [u8; 14] = [20, 21, 30, 31, 40, 41, 50, 51, 52, 53, 60, 61, 62, 63];
    /// HEVC QP (0–51) for the constant-quality setting.
    pub fn hevc_qp(&self) -> u8 {
        (4.0 + (100 - self.quality.clamp(1, 100)) as f64 * 0.45).round() as u8
    }
    /// AV1 `base_q_idx` (1–255) for the constant-quality setting.
    pub fn av1_qindex(&self) -> u8 {
        (4.0 + (100 - self.quality.clamp(1, 100)) as f64 * 2.4).round() as u8
    }
    /// `general_level_idc` (30 × level).
    pub fn hevc_level_idc(&self) -> Option<u8> {
        self.level.map(|l| l.saturating_mul(3))
    }
    /// AV1 `seq_level_idx`: (major − 2) × 4 + minor.
    pub fn av1_level_idx(&self) -> Option<u8> {
        self.level.map(|l| ((l / 10).clamp(2, 7) - 2) * 4 + (l % 10).min(3))
    }
    pub fn level_label(&self) -> String {
        match self.level {
            None => "Auto".into(),
            Some(l) => format!("{}.{}", l / 10, l % 10),
        }
    }
    /// Parse `"auto"`, `"4.1"`, `"41"` (level × 10). `None` when unparsable; `Some(None)` = auto.
    pub fn parse_level(s: &str) -> Option<Option<u8>> {
        let s = s.trim().to_ascii_lowercase();
        if s == "auto" || s.is_empty() {
            return Some(None);
        }
        let v: f64 = s.trim_start_matches('l').parse().ok()?;
        let l = if v >= 10.0 { v.round() } else { (v * 10.0).round() };
        (10.0..=73.0).contains(&l).then_some(Some(l as u8))
    }
}

/// The video codec of a WebM output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebmVideoCodec {
    #[default]
    Vp9,
    /// AV1 (`V_AV1`) with the [`OutputModule::codec`] options; no alpha.
    Av1,
}

impl WebmVideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            WebmVideoCodec::Vp9 => "VP9",
            WebmVideoCodec::Av1 => "AV1",
        }
    }
}

/// Opus coding application: general audio (music and mixed content) or voice (speech-tuned
/// SILK / hybrid coding at low bitrates).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpusApplication {
    #[default]
    Audio,
    Voip,
}

impl OpusApplication {
    pub fn label(self) -> &'static str {
        match self {
            OpusApplication::Audio => "Audio",
            OpusApplication::Voip => "Voice",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputModule {
    /// The template this module came from ("" or "Custom" once edited).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub format: OutputFormat,
    pub channels: Channels,
    /// Color: straight or premultiplied RGB with alpha.
    pub alpha_mode: AlphaMode,
    /// Output path or template (see the module docs).
    pub output: String,
    /// JPEG / WebM quality 1–100.
    pub quality: u8,
    /// H.264 target bitrate (and WebM's when `webm_bitrate`; HEVC / AV1 with [`RateControlMode::Bitrate`]).
    pub bitrate_kbps: u32,
    /// WebM: rate control by `bitrate_kbps` instead of `quality`.
    pub webm_bitrate: bool,
    /// Movies: frames between key frames (0 = automatic, two seconds).
    pub keyframe_interval: u32,
    pub prores_profile: ProResProfile,
    pub crop: Crop,
    pub resize: Resize,
    pub audio: AudioOutput,
    pub audio_sample_rate: u32,
    /// 1 (mono) or 2 (stereo).
    pub audio_channels: u8,
    pub audio_format: AudioFormat,
    /// GIF: loop forever.
    pub gif_loop: bool,
    /// HEVC / AV1 options.
    pub codec: VideoCodecOptions,
    /// WebM video codec.
    pub webm_codec: WebmVideoCodec,
    /// WebM Opus audio bitrate (stereo total), kbit/s.
    pub opus_bitrate_kbps: u32,
    pub opus_application: OpusApplication,
    /// Include Project Link: kept for parity; EffectCraft's writers have no place to store a
    /// link back to the project, so it changes nothing in the file.
    pub include_project_link: bool,
}

impl Default for OutputModule {
    fn default() -> Self {
        OutputModule {
            name: String::new(),
            format: OutputFormat::H264,
            channels: Channels::Rgb,
            alpha_mode: AlphaMode::Straight,
            output: DEFAULT_TEMPLATE.into(),
            quality: 90,
            bitrate_kbps: 10_000,
            webm_bitrate: false,
            keyframe_interval: 0,
            prores_profile: ProResProfile::Hq,
            crop: Crop::default(),
            resize: Resize::default(),
            audio: AudioOutput::Auto,
            audio_sample_rate: 48_000,
            audio_channels: 2,
            audio_format: AudioFormat::S16,
            gif_loop: true,
            codec: VideoCodecOptions::default(),
            webm_codec: WebmVideoCodec::Vp9,
            opus_bitrate_kbps: 192,
            opus_application: OpusApplication::Audio,
            include_project_link: true,
        }
    }
}

impl OutputModule {
    /// The size of the frames this module writes for frames rendered at `w`×`h` with render
    /// scale `scale`: after Crop and Resize (before codec rounding).
    pub fn frame_size(&self, w: u32, h: u32, scale: f64) -> (u32, u32) {
        let (t, l, b, r) = self.crop.edges(w, h, scale);
        let cw = (w as i32).saturating_sub(l).saturating_sub(r).max(1) as u32;
        let ch = (h as i32).saturating_sub(t).saturating_sub(b).max(1) as u32;
        if !self.resize.enabled {
            return (cw, ch);
        }
        let (mut rw, mut rh) = (self.resize.width.max(1), self.resize.height.max(1));
        if self.resize.lock_aspect {
            // Width rules; height follows the cropped frame's aspect.
            rh = ((rw as f64 * ch as f64 / cw as f64).round() as u32).max(1);
        }
        rw = rw.min(30_000);
        rh = rh.min(30_000);
        (rw, rh)
    }
    /// The written frame size of an item (Render Settings resolution, Crop, Resize, codec
    /// rounding).
    pub fn output_size(&self, comp: &Comp, settings: &RenderSettings) -> (u32, u32) {
        let (w, h) = settings.output_size(comp);
        let (w, h) = self.frame_size(w, h, settings.resolution.clamp(0.01, 4.0));
        self.format.coded_size(w, h)
    }
    /// Every setting as `(label, value)` lines (render logs).
    pub fn describe(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![
            ("Template", if self.name.is_empty() { "Custom".into() } else { self.name.clone() }),
            ("Format", self.format.label().into()),
            ("Channels", self.channels.label().into()),
            ("Output To", self.output.clone()),
        ];
        if self.channels == Channels::Rgba {
            v.insert(3, ("Color", format!("{:?}", self.alpha_mode)));
        }
        match self.format {
            OutputFormat::H264 => v.push(("Bitrate", format!("{} kbps", self.bitrate_kbps))),
            OutputFormat::ProRes => v.push(("Codec", self.prores_profile.label().into())),
            OutputFormat::WebM if self.webm_bitrate => v.push(("Bitrate", format!("{} kbps", self.bitrate_kbps))),
            OutputFormat::WebM | OutputFormat::JpegSequence => v.push(("Quality", self.quality.to_string())),
            _ => {}
        }
        if self.crop.enabled {
            let c = &self.crop;
            v.push((
                "Crop",
                if c.use_roi { format!("Region of Interest {:?}", c.roi) } else { format!("T {} L {} B {} R {}", c.top, c.left, c.bottom, c.right) },
            ));
        }
        if self.resize.enabled {
            let r = &self.resize;
            v.push(("Resize", format!("{}×{} ({:?} quality{})", r.width, r.height, r.quality, if r.lock_aspect { ", aspect locked" } else { "" })));
        }
        if self.format.supports_audio() {
            v.push((
                "Audio Output",
                format!(
                    "{:?} · {} Hz · {} · {}",
                    self.audio,
                    self.audio_sample_rate,
                    if self.audio_channels == 1 { "Mono" } else { "Stereo" },
                    self.audio_format.label()
                ),
            ));
        }
        v.push(("Include Project Link", if self.include_project_link { "On" } else { "Off" }.into()));
        v
    }
    pub fn for_format(format: OutputFormat) -> OutputModule {
        let mut m = OutputModule { format, ..Default::default() };
        if format.is_sequence() {
            m.output = DEFAULT_SEQUENCE_TEMPLATE.into();
        }
        m
    }
    /// Change the format, keeping the output name but fixing its extension / frame-number token.
    pub fn set_format(&mut self, format: OutputFormat) {
        let old = self.format;
        self.format = format;
        if !self.supports_alpha() && self.channels == Channels::Rgba {
            self.channels = Channels::Rgb;
        }
        if self.output.contains("[fileExtension]") {
            if format.is_sequence() && !self.output.contains('#') {
                self.output = self.output.replace(".[fileExtension]", "_[#####].[fileExtension]");
            } else if !format.is_sequence() && old.is_sequence() {
                self.output = self.output.replace("_[#####]", "").replace("[#####]", "");
            }
            return;
        }
        let mut stem = format.output_stem(&self.output).to_string();
        if format.is_sequence() && !stem.contains('#') {
            stem.push_str("_[#####]");
        } else if !format.is_sequence() {
            stem = stem.replace("_[#####]", "").replace("[#####]", "");
        }
        self.output = format!("{stem}.{}", format.extension());
    }
    /// Whether this module can write alpha (the format can, and WebM uses VP9).
    pub fn supports_alpha(&self) -> bool {
        self.format.supports_alpha() && !(self.format == OutputFormat::WebM && self.webm_codec == WebmVideoCodec::Av1)
    }
    /// The key-frame interval in frames at `fps` ([`OutputModule::keyframe_interval`]; automatic: 2 s).
    pub fn keyint(&self, fps: f64) -> u32 {
        if self.keyframe_interval > 0 { self.keyframe_interval } else { (fps * 2.0).round().max(1.0) as u32 }
    }
    /// Codec options summary: `Main · 10000 kbps · Level Auto · Key every auto`.
    fn codec_summary(&self) -> String {
        let c = &self.codec;
        let rate = match c.rate_control {
            RateControlMode::Bitrate => format!("{} kbps", self.bitrate_kbps),
            RateControlMode::Quality => format!("Quality {}", c.quality),
        };
        let gop = if self.keyframe_interval == 0 { "auto".to_string() } else { self.keyframe_interval.to_string() };
        format!("{} · {rate} · Level {} · Key every {gop}", c.profile.label(), c.level_label())
    }
    /// One-line summary shown next to "Output Module:".
    pub fn summary(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        let ch = self.channels.label();
        match self.format {
            OutputFormat::H264 => format!("H.264 · {} kbps", self.bitrate_kbps),
            OutputFormat::ProRes => format!(
                "{} · {ch}",
                if self.channels == Channels::Rgba && !self.prores_profile.is_4444() { "Apple ProRes 4444" } else { self.prores_profile.label() }
            ),
            OutputFormat::Hevc | OutputFormat::Av1 => format!("{} · {}", self.format.label(), self.codec_summary()),
            OutputFormat::WebM if self.webm_codec == WebmVideoCodec::Av1 => {
                format!("WebM (AV1 + Opus {} kbps) · {}", self.opus_bitrate_kbps, self.codec_summary())
            }
            f => format!("{} · {ch}", f.label()),
        }
    }
}

/// Render Queue item status (the Status column).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum RenderStatus {
    /// Render checkbox off.
    Unqueued,
    #[default]
    Queued,
    NeedsOutput,
    Rendering,
    Done,
    UserStopped,
    Failed(String),
}

impl RenderStatus {
    pub fn label(&self) -> &str {
        match self {
            RenderStatus::Unqueued => "Unqueued",
            RenderStatus::Queued => "Queued",
            RenderStatus::NeedsOutput => "Needs Output",
            RenderStatus::Rendering => "Rendering",
            RenderStatus::Done => "Done",
            RenderStatus::UserStopped => "User Stopped",
            RenderStatus::Failed(_) => "Failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderQueueItem {
    /// Stable id (unique within the queue).
    pub id: u64,
    pub comp: ItemId,
    /// The Render checkbox.
    pub render: bool,
    #[serde(default)]
    pub status: RenderStatus,
    #[serde(default)]
    pub settings: RenderSettings,
    #[serde(default)]
    pub output: OutputModule,
    /// Unix seconds when the last render of this item started.
    #[serde(default)]
    pub started: Option<u64>,
    /// Seconds the last render took.
    #[serde(default)]
    pub render_time: Option<f64>,
    /// Resolved path of the last render (first file for sequences).
    #[serde(default)]
    pub last_output: Option<String>,
    /// Further Output Modules (Composition ▸ Add Output Module): the same frames are encoded
    /// once per module.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_outputs: Vec<OutputModule>,
    /// Post-Render Action of the first output module.
    #[serde(default, skip_serializing_if = "PostRenderAction::is_none")]
    pub post_render: PostRenderAction,
    /// The Log menu: what the render log next to the output records.
    #[serde(default, skip_serializing_if = "RenderLog::is_default")]
    pub log: RenderLog,
}

/// Render Queue item ▸ Log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderLog {
    /// A log only when the render fails.
    #[default]
    ErrorsOnly,
    /// Always: the result plus every Render Settings and Output Module setting.
    PlusSettings,
    /// Also one line per rendered frame.
    PlusPerFrameInfo,
}

impl RenderLog {
    pub const ALL: [RenderLog; 3] = [RenderLog::ErrorsOnly, RenderLog::PlusSettings, RenderLog::PlusPerFrameInfo];
    pub fn is_default(&self) -> bool {
        *self == RenderLog::ErrorsOnly
    }
    pub fn label(self) -> &'static str {
        match self {
            RenderLog::ErrorsOnly => "Errors Only",
            RenderLog::PlusSettings => "Plus Settings",
            RenderLog::PlusPerFrameInfo => "Plus Per Frame Info",
        }
    }
    pub fn parse(s: &str) -> Option<RenderLog> {
        match norm(s).as_str() {
            "errors" | "errorsonly" => Some(RenderLog::ErrorsOnly),
            "settings" | "plussettings" => Some(RenderLog::PlusSettings),
            "perframe" | "plusperframe" | "plusperframeinfo" | "frames" => Some(RenderLog::PlusPerFrameInfo),
            _ => None,
        }
    }
}

/// The render log's path for an output: next to it, `<name>_RenderLog.txt` (a sequence's
/// frame-number run is dropped).
pub fn log_path(output: &str) -> String {
    let p = std::path::Path::new(output);
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut stem = stem;
    if let Some(i) = stem.find('#') {
        // The run of `#`s, with its brackets when it has them (`[###]`, `[#####]`).
        let n = stem[i..].chars().take_while(|c| *c == '#').count();
        let (a, b) = if stem[..i].ends_with('[') && stem[i + n..].starts_with(']') { (i - 1, i + n + 1) } else { (i, i + n) };
        stem.replace_range(a..b, "");
    }
    let stem = stem.trim_end_matches(['_', '.', ' ']);
    let name = format!("{}_RenderLog.txt", if stem.is_empty() { "Render" } else { stem });
    match p.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(d) => d.join(name).to_string_lossy().to_string(),
        None => name,
    }
}

/// Storage hook (Render Settings ▸ Use Storage Overflow): whether `bytes` more fit on the volume
/// holding `path`. The desktop app has no quota by default; tests and hosts with quotas supply
/// one.
pub trait StorageQuota: Send + Sync {
    fn has_room(&self, path: &str, bytes: u64) -> bool;
}

/// Project-wide Render Queue preferences.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderQueuePrefs {
    /// Notify when the render finishes (a sound and a notification in the desktop app).
    pub notify: bool,
    /// Storage overflow folders, used in order when the output volume is full.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overflow_folders: Vec<String>,
}

impl RenderQueuePrefs {
    pub fn is_default(&self) -> bool {
        *self == RenderQueuePrefs::default()
    }
}

/// What happens after an item renders (Output Module ▸ Post-Render Action).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PostRenderAction {
    #[default]
    None,
    /// Import the rendered file into the project.
    Import,
    /// Import it and replace every use of the rendered composition (Composition ▸ Pre-render).
    ImportAndReplace,
    /// Set the rendered file as the composition's proxy (File ▸ Create Proxy).
    SetProxy,
}

impl PostRenderAction {
    pub fn is_none(&self) -> bool {
        *self == PostRenderAction::None
    }
    pub fn label(self) -> &'static str {
        match self {
            PostRenderAction::None => "None",
            PostRenderAction::Import => "Import",
            PostRenderAction::ImportAndReplace => "Import & Replace Usage",
            PostRenderAction::SetProxy => "Set Proxy",
        }
    }
}

impl RenderQueueItem {
    pub fn new(id: u64, comp: ItemId) -> RenderQueueItem {
        RenderQueueItem {
            id,
            comp,
            render: true,
            status: RenderStatus::Queued,
            settings: RenderSettings::default(),
            output: OutputModule::default(),
            started: None,
            render_time: None,
            last_output: None,
            extra_outputs: vec![],
            post_render: PostRenderAction::None,
            log: RenderLog::ErrorsOnly,
        }
    }
    /// Every output module, the first one first.
    pub fn output_modules(&self) -> impl Iterator<Item = &OutputModule> {
        std::iter::once(&self.output).chain(self.extra_outputs.iter())
    }
    /// Will be rendered by the next Render.
    pub fn is_queued(&self) -> bool {
        self.render && matches!(self.status, RenderStatus::Queued)
    }
}

/// Values for template tokens.
pub struct TemplateVars<'a> {
    pub comp_name: &'a str,
    pub project_name: &'a str,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub start_frame: i64,
    pub end_frame: i64,
}

/// Expand template tokens (all but the `#` frame-number run).
pub fn expand_template(template: &str, format: OutputFormat, v: &TemplateVars) -> String {
    let fps = if (v.frame_rate - v.frame_rate.round()).abs() < 1e-6 { format!("{}", v.frame_rate.round() as i64) } else { format!("{:.2}", v.frame_rate) };
    template
        .replace("[compName]", &sanitize(v.comp_name))
        .replace("[projectName]", &sanitize(v.project_name))
        .replace("[fileExtension]", format.extension())
        .replace("[width]", &v.width.to_string())
        .replace("[height]", &v.height.to_string())
        .replace("[frameRate]", &fps)
        .replace("[startFrame]", &v.start_frame.to_string())
        .replace("[endFrame]", &v.end_frame.to_string())
        .replace("[outputModuleName]", format.label())
}

/// Replace the first run of `#` (optionally in brackets, `[####]`) with the zero-padded frame
/// number. Paths without a run get `_NNNNN` before the extension.
pub fn sequence_path(path: &str, frame: i64) -> String {
    if let Some(start) = path.find('#') {
        let len = path[start..].chars().take_while(|c| *c == '#').count();
        let (mut a, mut b) = (start, start + len);
        if a > 0 && path.as_bytes()[a - 1] == b'[' && path.as_bytes().get(b) == Some(&b']') {
            a -= 1;
            b += 1;
        }
        return format!("{}{:0width$}{}", &path[..a], frame, &path[b..], width = len);
    }
    let p = std::path::Path::new(path);
    match p.extension() {
        Some(ext) => format!("{}_{frame:05}.{}", p.with_extension("").to_string_lossy(), ext.to_string_lossy()),
        None => format!("{path}_{frame:05}"),
    }
}

/// Make a name safe as a file name component.
pub fn sanitize(name: &str) -> String {
    name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_and_sequences() {
        let v = TemplateVars { comp_name: "Main/Title", project_name: "P", width: 1920, height: 1080, frame_rate: 29.97, start_frame: 0, end_frame: 9 };
        assert_eq!(expand_template(DEFAULT_TEMPLATE, OutputFormat::H264, &v), "Main_Title.mp4");
        assert_eq!(expand_template("[compName]_[width]x[height]@[frameRate].[fileExtension]", OutputFormat::Gif, &v), "Main_Title_1920x1080@29.97.gif");
        assert_eq!(sequence_path("out/a_[#####].png", 42), "out/a_00042.png");
        assert_eq!(sequence_path("a###.tif", 7), "a007.tif");
        assert_eq!(sequence_path("a.png", 3), "a_00003.png");
    }

    #[test]
    fn set_format_fixes_names() {
        let mut m = OutputModule::default();
        m.set_format(OutputFormat::PngSequence);
        assert_eq!(m.output, "[compName]_[#####].[fileExtension]");
        m.set_format(OutputFormat::ProRes);
        assert_eq!(m.output, DEFAULT_TEMPLATE);
        m.output = "/tmp/x.mov".into();
        m.set_format(OutputFormat::JpegSequence);
        assert_eq!(m.output, "/tmp/x_[#####].jpg");
        assert_eq!(OutputFormat::from_name("TIFF"), Some(OutputFormat::TiffSequence));
        assert_eq!(ProResProfile::from_name("4444"), Some(ProResProfile::P4444));
        assert_eq!(ProResProfile::from_name("hq"), Some(ProResProfile::Hq));
    }

    /// Output names end in the format's extension exactly once (#293): a save dialog filtered to
    /// projects appended `.ecproj` (`Comp 1.mov.ecproj`, `Comp 1_00001.png.ecproj` for every
    /// frame), and a format change or a comp named `Comp 2.mov` doubled it (`Comp 2.mov.mov`).
    #[test]
    fn output_names_end_in_the_format_extension_once() {
        use OutputFormat::*;
        for (path, format, want) in [
            ("/out/Comp 1.mov.ecproj", ProRes, "/out/Comp 1.mov"),
            ("/out/Comp 1_[#####].png.ecproj", PngSequence, "/out/Comp 1_[#####].png"),
            ("/out/Comp 1.mov.ecprojx.ecproj", ProRes, "/out/Comp 1.mov"),
            ("/out/Comp 2.mov.mov", ProRes, "/out/Comp 2.mov"),
            ("/out/Comp 2.mov.mov.mov", ProRes, "/out/Comp 2.mov"),
            ("/out/render", ProRes, "/out/render.mov"),
            ("/out/frames_[#####]", PngSequence, "/out/frames_[#####].png"),
            ("/out/logo.png.mov", ProRes, "/out/logo.png.mov"),
            ("/out/v1.2", H264, "/out/v1.2.mp4"),
            ("/my.folder/Clip.MOV", ProRes, "/my.folder/Clip.MOV"),
            ("/out/x.mp4", Hevc, "/out/x.mp4"),
            ("[compName].[fileExtension]", ProRes, "[compName].[fileExtension]"),
        ] {
            assert_eq!(format.with_extension(path), want, "{path} as {format:?}");
        }
        assert_eq!(without_project_extension("C:/out/Comp 1.mov.ecproj"), "C:/out/Comp 1.mov");
        assert_eq!(without_project_extension("Comp 1.mov"), "Comp 1.mov");
        // Changing the format replaces one extension and drops the project one.
        let mut m = OutputModule { output: "/out/Comp 1.mov.ecproj".into(), ..OutputModule::for_format(ProRes) };
        m.set_format(H264);
        assert_eq!(m.output, "/out/Comp 1.mp4");
        m.output = "/out/Comp 2.mov.mov".into();
        m.set_format(ProRes);
        assert_eq!(m.output, "/out/Comp 2.mov");
        m.output = "/out/Comp 2.mov.ecproj".into();
        m.set_format(PngSequence);
        assert_eq!(m.output, "/out/Comp 2_[#####].png");
    }

    #[test]
    fn pulldown_cadence() {
        assert_eq!(Pulldown::Wwssw.pattern(), "WWSSW");
        for p in &Pulldown::ALL[1..] {
            assert_eq!(p.pattern(), p.label(), "{p:?}");
        }
        // 2:3 cadence: ten fields carry four film frames; every output frame starts on film 0.
        for p in &Pulldown::ALL[1..] {
            let mut fields = vec![];
            for k in 0..10 {
                let (a, b) = p.fields(k).unwrap();
                fields.extend([a, b]);
            }
            assert_eq!(fields[0], 0, "{p:?}");
            assert!(fields.windows(2).all(|w| w[1] == w[0] || w[1] == w[0] + 1), "{p:?}: {fields:?}");
            // 20 fields = 8 film frames' worth of cadence (a phase may start or end mid-frame).
            assert!((7..=8).contains(&(*fields.last().unwrap() - fields[0])), "{p:?} {fields:?}");
            // Each film frame appears in 2 or 3 consecutive fields, alternating.
            let mut runs = vec![];
            let mut n = 1;
            for w in fields.windows(2) {
                if w[1] == w[0] {
                    n += 1;
                } else {
                    runs.push(n);
                    n = 1;
                }
            }
            assert!(runs[1..].iter().all(|r| *r == 2 || *r == 3), "{p:?}: {runs:?}");
            assert!(runs[1..].windows(2).all(|w| w[0] != w[1]), "2 and 3 alternate: {p:?}: {runs:?}");
        }
        assert_eq!(Pulldown::parse("sswww"), Some(Pulldown::Sswww));
        // With field rendering at 29.97, film frames are 23.976 fps.
        let c = Comp::new(64, 48, FrameRate::new(30000, 1001), Tick::from_seconds_f64(2.0));
        let s = RenderSettings { field_render: FieldRender::UpperFirst, pulldown: Pulldown::Wssww, time_span: TimeSpan::LengthOfComp, ..Default::default() };
        let film = FrameRate::new(24000, 1001);
        match s.sample(&c, 1) {
            FrameSample::Fields { first, second, upper_first } => {
                assert!(upper_first);
                assert_eq!((first, second), (film.tick_of(0), film.tick_of(1)), "WSSWW: frame 1 is split between film frames 0 and 1");
            }
            f => panic!("{f:?}"),
        }
    }

    #[test]
    fn field_samples_are_half_a_frame_apart() {
        let c = Comp::new(64, 48, FrameRate::new(25, 1), Tick::from_seconds_f64(2.0));
        let s = RenderSettings { field_render: FieldRender::LowerFirst, time_span: TimeSpan::LengthOfComp, ..Default::default() };
        assert_eq!(s.sample(&c, 3), FrameSample::Fields { first: Tick::from_seconds_f64(0.12), second: Tick::from_seconds_f64(0.14), upper_first: false });
        let p = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
        assert_eq!(p.sample(&c, 3), FrameSample::Progressive(Tick::from_seconds_f64(0.12)));
    }

    #[test]
    fn crop_and_resize_sizes() {
        let c = Comp::new(1920, 1080, FrameRate::new(25, 1), Tick::from_seconds_f64(1.0));
        let rs = RenderSettings::default();
        let mut m = OutputModule::default();
        assert_eq!(m.output_size(&c, &rs), (1920, 1080));
        m.crop = Crop { enabled: true, top: 10, left: 20, bottom: 30, right: 41, ..Default::default() };
        assert_eq!(m.output_size(&c, &rs), (1858, 1040), "H.264 rounds the odd width down");
        m.format = OutputFormat::PngSequence;
        assert_eq!(m.output_size(&c, &rs), (1859, 1040));
        // Region of interest, at half resolution.
        m.crop = Crop { enabled: true, use_roi: true, roi: Some([100.0, 50.0, 800.0, 400.0]), ..Default::default() };
        let half = RenderSettings { resolution: 0.5, ..Default::default() };
        assert_eq!(m.output_size(&c, &half), (400, 200));
        // Negative crop pads.
        m.crop = Crop { enabled: true, top: -10, bottom: -10, ..Default::default() };
        assert_eq!(m.output_size(&c, &rs), (1920, 1100));
        // Resize: fixed, and with the aspect locked to the (cropped) frame.
        m.crop = Crop::default();
        m.resize = Resize { enabled: true, width: 1280, height: 999, lock_aspect: false, quality: ResizeQuality::High };
        assert_eq!(m.output_size(&c, &rs), (1280, 999));
        m.resize.lock_aspect = true;
        assert_eq!(m.output_size(&c, &rs), (1280, 720));
        m.crop = Crop { enabled: true, left: 480, right: 480, ..Default::default() };
        assert_eq!(m.output_size(&c, &rs), (1280, 1440), "960×1080 cropped, then 1280 wide at 8:9");
        assert_eq!(std::path::PathBuf::from(log_path("/out/Main_[#####].png")), std::path::PathBuf::from("/out/Main_RenderLog.txt"));
        assert_eq!(std::path::PathBuf::from(log_path("/out/a.mov")), std::path::PathBuf::from("/out/a_RenderLog.txt"));
        assert_eq!(log_path("seq###.tif"), "seq_RenderLog.txt");
        assert_eq!(std::path::PathBuf::from(log_path("gen/g_[###].png")), std::path::PathBuf::from("gen/g_RenderLog.txt"), "any bracketed run (M13.15)");
    }

    #[test]
    fn spans() {
        let mut c = Comp::new(100, 50, FrameRate::new(25, 1), Tick::from_seconds_f64(4.0));
        c.work_area = (Tick::from_seconds_f64(1.0), Tick::from_seconds_f64(2.0));
        let mut s = RenderSettings::default();
        assert_eq!(s.frame_count(&c), 25);
        assert_eq!(s.frame_time(&c, 0), Tick::from_seconds_f64(1.0));
        s.time_span = TimeSpan::LengthOfComp;
        assert_eq!(s.frame_count(&c), 100);
        s.frame_rate = Some(FrameRate::new(10, 1));
        assert_eq!(s.frame_count(&c), 40);
        s.resolution = 0.5;
        assert_eq!(s.output_size(&c), (50, 25));
        // NTSC: 1 s – 2 s holds the 30 frames that start inside it.
        let c = Comp::new(100, 50, FrameRate::new(30000, 1001), Tick::from_seconds_f64(4.0));
        let s = RenderSettings { time_span: TimeSpan::Custom { start: Tick::from_seconds_f64(1.0), end: Tick::from_seconds_f64(2.0) }, ..Default::default() };
        assert_eq!(s.frame_count(&c), 30);
        assert_eq!(s.first_frame(&c), 30);
        assert!(s.frame_time(&c, 0) >= Tick::from_seconds_f64(1.0));
    }

    #[test]
    fn hostile_roi_cannot_overflow_crop_math() {
        // `-1e12` saturates to `i32::MIN` on conversion: `w as i32 - l` must saturate,
        // not overflow, and still yield a usable (degenerate) frame size.
        let c = Comp::new(1920, 1080, FrameRate::new(30, 1), Tick::from_seconds_f64(1.0));
        let rs = RenderSettings::default();
        let mut m = OutputModule::for_format(OutputFormat::PngSequence);
        m.crop = Crop { enabled: true, use_roi: true, roi: Some([-1e12, -1e12, 200.0, 200.0]), ..Default::default() };
        let (w, h) = m.output_size(&c, &rs);
        assert!((w, h) == (1, 1), "degenerate but valid: {w}x{h}");
        // Sane ROIs are unaffected (existing behavior).
        m.crop = Crop { enabled: true, use_roi: true, roi: Some([100.0, 50.0, 800.0, 400.0]), ..Default::default() };
        let half = RenderSettings { resolution: 0.5, ..Default::default() };
        assert_eq!(m.output_size(&c, &half), (400, 200));
    }

    #[test]
    fn uncompressed_quicktime_format() {
        let f = OutputFormat::from_name("uncompressed").unwrap();
        assert_eq!(f, OutputFormat::Uncompressed);
        assert_eq!(OutputFormat::from_name(&format!("{f:?}")), Some(f));
        assert!(OutputFormat::ALL.contains(&f));
        assert!(f.is_movie() && f.supports_alpha() && f.supports_audio());
        assert_eq!(f.coded_size(3, 5), (3, 5));
        // `.mov` still guesses ProRes; switching keeps the name.
        assert_eq!(OutputFormat::from_path("a.mov"), Some(OutputFormat::ProRes));
        let mut m = OutputModule::for_format(OutputFormat::ProRes);
        m.output = "/out/Comp 1.mov".into();
        m.channels = Channels::Rgba;
        m.set_format(f);
        assert_eq!((m.output.as_str(), m.channels), ("/out/Comp 1.mov", Channels::Rgba));
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<OutputModule>(&json).unwrap().format, f);
    }

    #[test]
    fn hevc_av1_codec_options() {
        assert_eq!(OutputFormat::from_name("H.265"), Some(OutputFormat::Hevc));
        assert_eq!(OutputFormat::from_name("hevc"), Some(OutputFormat::Hevc));
        assert_eq!(OutputFormat::from_name("AV1"), Some(OutputFormat::Av1));
        assert_eq!(OutputFormat::Hevc.coded_size(33, 17), (32, 16));
        assert_eq!(OutputFormat::Av1.coded_size(33, 17), (33, 17));
        assert!(!OutputFormat::Hevc.supports_alpha() && OutputFormat::Av1.is_movie() && OutputFormat::Av1.supports_audio());
        assert_eq!(VideoCodecOptions::parse_level("4.1"), Some(Some(41)));
        assert_eq!(VideoCodecOptions::parse_level("51"), Some(Some(51)));
        assert_eq!(VideoCodecOptions::parse_level("auto"), Some(None));
        assert_eq!(VideoCodecOptions::parse_level("9"), None);
        let mut o = VideoCodecOptions { level: Some(41), ..Default::default() };
        assert_eq!((o.hevc_level_idc(), o.av1_level_idx()), (Some(123), Some(9)));
        o.level = Some(20);
        assert_eq!(o.av1_level_idx(), Some(0));
        o.quality = 100;
        assert_eq!((o.hevc_qp(), o.av1_qindex()), (4, 4));
        o.quality = 1;
        assert!(o.hevc_qp() <= 51 && o.av1_qindex() >= 240);
        // WebM with AV1 has no alpha; switching drops RGB + Alpha.
        let mut m = OutputModule::for_format(OutputFormat::WebM);
        m.channels = Channels::Rgba;
        assert!(m.supports_alpha());
        m.webm_codec = WebmVideoCodec::Av1;
        assert!(!m.supports_alpha());
        assert!(m.summary().starts_with("WebM (AV1 + Opus 192 kbps)"), "{}", m.summary());
        let mut h = OutputModule::for_format(OutputFormat::Hevc);
        assert_eq!(h.keyint(29.97), 60);
        h.keyframe_interval = 1;
        assert_eq!(h.keyint(29.97), 1);
        h.keyframe_interval = 0;
        assert_eq!(h.summary(), "HEVC (H.265) · Main · 10000 kbps · Level Auto · Key every auto");
        // Older projects (no codec fields) load with the defaults.
        let old: OutputModule = serde_json::from_str(r#"{"format":"H264","bitrate_kbps":5000}"#).unwrap();
        assert_eq!((old.codec, old.webm_codec, old.opus_bitrate_kbps), (VideoCodecOptions::default(), WebmVideoCodec::Vp9, 192));
    }
}
