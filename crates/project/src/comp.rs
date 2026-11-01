use super::item::LayerId;
use super::layer::{Layer, LayerSource};
use super::settings::{Marker, yes};
use super::{Deserialize, FrameRate, Node, PropGroup, Serialize, Tick, essential, styles};

/// 3D renderer of a composition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Renderer {
    #[default]
    Classic3D,
    Advanced3D,
}

/// A composition's Resolution (the Composition panel's Resolution/Down Sample Factor popup, View ▸
/// Resolution): how many of its pixels the viewer renders. Auto follows the magnification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    #[default]
    Auto,
    Full,
    Half,
    Third,
    Quarter,
    /// View ▸ Resolution ▸ Custom: render every n-th pixel (2..=40).
    Custom(u8),
}

impl Resolution {
    pub const ALL: [Resolution; 5] = [Resolution::Auto, Resolution::Full, Resolution::Half, Resolution::Third, Resolution::Quarter];
    pub fn label(self) -> &'static str {
        match self {
            Resolution::Auto => "Auto",
            Resolution::Full => "Full",
            Resolution::Half => "Half",
            Resolution::Third => "Third",
            Resolution::Quarter => "Quarter",
            Resolution::Custom(_) => "Custom",
        }
    }
    pub fn is_auto(&self) -> bool {
        *self == Resolution::Auto
    }
    /// The down-sample factor (render every n-th pixel); `None` for Auto.
    pub fn factor(self) -> Option<u8> {
        match self {
            Resolution::Auto => None,
            Resolution::Full => Some(1),
            Resolution::Half => Some(2),
            Resolution::Third => Some(3),
            Resolution::Quarter => Some(4),
            Resolution::Custom(n) => Some(n),
        }
    }
    /// The resolution that renders every `n`-th pixel (1 = Full … 4 = Quarter; up to 40).
    pub fn from_factor(n: u64) -> Resolution {
        match n.clamp(1, 40) {
            1 => Resolution::Full,
            2 => Resolution::Half,
            3 => Resolution::Third,
            4 => Resolution::Quarter,
            n => Resolution::Custom(n as u8),
        }
    }
    /// Render scale for a viewer magnification (and display pixel density).
    pub fn scale(self, zoom: f32, ppp: f32) -> f64 {
        match self.factor() {
            Some(n) => 1.0 / f64::from(n.clamp(1, 40)),
            // As in After Effects, Auto renders the pixels the magnification needs: the coarsest
            // whole factor that still gives every screen pixel a rendered one (Full above 50 %,
            // Half down to 33.3 %, Third down to 25 %, Quarter below), never fewer (#417).
            None => {
                let n = (1.0 / (zoom * ppp) as f64 + 1e-3).floor();
                if n >= 1.0 { 1.0 / n.min(4.0) } else { 1.0 }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comp {
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: f64,
    pub frame_rate: FrameRate,
    pub duration: Tick,
    /// Start timecode/frame shown for time 0.
    pub display_start: Tick,
    pub background: [f32; 3],
    pub work_area: (Tick, Tick),
    /// Layer #1 first (top of the stack). Each layer is shared between undo states until edited.
    pub layers: super::Layers,
    #[serde(default)]
    pub markers: Vec<Marker>,
    pub shutter_angle: f64,
    pub shutter_phase: f64,
    /// Samples Per Frame: the minimum motion-blur samples (and the count for 3D layers).
    pub motion_blur_samples: u32,
    /// Adaptive Sample Limit (Composition Settings ▸ Advanced): the most samples a 2D layer gets
    /// when it moves far within the shutter.
    #[serde(default = "adaptive_limit_default")]
    pub motion_blur_adaptive_limit: u32,
    #[serde(default)]
    pub renderer: Renderer,
    /// Comp switches (timeline toolbar).
    #[serde(default)]
    pub hide_shy: bool,
    #[serde(default = "yes")]
    pub enable_motion_blur: bool,
    #[serde(default = "yes")]
    pub enable_frame_blending: bool,
    #[serde(default)]
    pub draft_3d: bool,
    /// Composition Settings ▸ Advanced ▸ Preserve frame rate when nested or in render queue:
    /// nested in another comp, or rendered at another rate, it shows only its own frames.
    #[serde(default)]
    pub preserve_frame_rate: bool,
    /// Composition Settings ▸ Advanced ▸ Preserve resolution when nested: nested in a comp
    /// previewed at a lower resolution, it still renders at full size.
    #[serde(default)]
    pub preserve_resolution: bool,
    #[serde(default)]
    pub poster_time: Tick,
    /// Global Light for layer styles (Layer ▸ Layer Styles ▸ Blending Options).
    #[serde(default)]
    pub global_light: styles::GlobalLight,
    /// Viewer guides (View ▸ Add Guide…), in comp pixels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guides: Vec<Guide>,
    /// Essential Graphics: the controls this comp exposes to instances and templates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub essential: Option<essential::EssentialGraphics>,
    /// The Composition panel's Resolution, saved with the project as in After Effects. It changes
    /// what the viewer renders, not the comp's pixels ([`Comp::same_pixels`]), and stays out of
    /// the undo history.
    #[serde(default, skip_serializing_if = "Resolution::is_auto")]
    pub resolution: Resolution,
}

/// A viewer guide line: vertical guides sit at an x position, horizontal ones at a y position.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub vertical: bool,
    /// Comp pixels, or percent of the comp's width (vertical) / height (horizontal) when
    /// [`Guide::percent`] is set.
    pub position: f64,
    /// A percentage guide (After Effects 26.5): `position` is a percent of the comp's size, so the
    /// guide keeps its relative place when the comp is resized. Absent in older projects.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub percent: bool,
}

impl Guide {
    /// The comp's extent along this guide's axis, never zero.
    fn extent(&self, width: u32, height: u32) -> f64 {
        f64::from(if self.vertical { width } else { height }).max(1.0)
    }

    /// The guide's position in comp pixels for a comp of this size.
    pub fn px(&self, width: u32, height: u32) -> f64 {
        if self.percent { self.position / 100.0 * self.extent(width, height) } else { self.position }
    }

    /// Place the guide at `px` comp pixels, keeping its unit.
    pub fn set_px(&mut self, px: f64, width: u32, height: u32) {
        self.position = if self.percent { px / self.extent(width, height) * 100.0 } else { px };
    }

    /// Switch between pixels and percent without moving the guide on the comp.
    pub fn set_percent(&mut self, percent: bool, width: u32, height: u32) {
        let px = self.px(width, height);
        self.percent = percent;
        self.set_px(px, width, height);
    }
}

fn adaptive_limit_default() -> u32 {
    128
}

impl Comp {
    pub fn new(width: u32, height: u32, frame_rate: FrameRate, duration: Tick) -> Comp {
        Comp {
            width,
            height,
            pixel_aspect: 1.0,
            frame_rate,
            duration,
            display_start: Tick::ZERO,
            background: [0.0, 0.0, 0.0],
            work_area: (Tick::ZERO, duration),
            layers: super::Layers::new(),
            markers: vec![],
            shutter_angle: 180.0,
            shutter_phase: -90.0,
            motion_blur_samples: 16,
            motion_blur_adaptive_limit: 128,
            renderer: Renderer::Classic3D,
            hide_shy: false,
            enable_motion_blur: true,
            enable_frame_blending: true,
            draft_3d: false,
            preserve_frame_rate: false,
            preserve_resolution: false,
            poster_time: Tick::ZERO,
            guides: vec![],
            global_light: styles::GlobalLight::default(),
            essential: None,
            resolution: Resolution::Auto,
        }
    }

    /// Whether the composition has a frame at its time `t`: nested, it shows nothing before it
    /// starts or after it ends (a precomp layer trimmed or remapped past it).
    pub fn covers(&self, t: Tick) -> bool {
        t >= Tick::ZERO && t < self.duration
    }

    /// A new, empty composition with this one's settings (frame rate, pixel aspect, background,
    /// motion blur, frame blending and the 3D renderer), as Pre-compose makes.
    pub fn nested_like(&self, width: u32, height: u32, duration: Tick) -> Comp {
        Comp {
            pixel_aspect: self.pixel_aspect,
            background: self.background,
            shutter_angle: self.shutter_angle,
            shutter_phase: self.shutter_phase,
            motion_blur_samples: self.motion_blur_samples,
            motion_blur_adaptive_limit: self.motion_blur_adaptive_limit,
            renderer: self.renderer,
            enable_motion_blur: self.enable_motion_blur,
            enable_frame_blending: self.enable_frame_blending,
            preserve_frame_rate: self.preserve_frame_rate,
            preserve_resolution: self.preserve_resolution,
            ..Comp::new(width, height, self.frame_rate, duration)
        }
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.find_mut(|l| l.id == id)
    }

    /// 1-based layer number (#) of a layer.
    pub fn index_of(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id).map(|i| i + 1)
    }

    pub fn layer_by_name(&self, name: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.name == name)
    }

    pub fn frame_duration(&self) -> Tick {
        self.frame_rate.frame_duration()
    }

    /// Any 3D layer present (camera/lights or a 3D switch).
    pub fn has_3d(&self) -> bool {
        self.layers.iter().any(|l| l.switches.three_d || matches!(l.source, LayerSource::Camera | LayerSource::Light { .. }))
    }

    /// The active camera at `t`: the topmost enabled camera layer active at that time.
    pub fn active_camera(&self, t: Tick) -> Option<&Layer> {
        self.layers.iter().find(|l| matches!(l.source, LayerSource::Camera) && l.switches.video && l.is_active_at(t))
    }

    /// Unique layer name with a numeric suffix (`Shape Layer 2`).
    pub fn unique_layer_name(&self, base: &str) -> String {
        if !self.layers.iter().any(|l| l.name == base) {
            return base.to_string();
        }
        let stem = base.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end();
        (2..).map(|i| format!("{stem} {i}")).find(|n| !self.layers.iter().any(|l| &l.name == n)).unwrap_or_default()
    }
}

/// A protected time marker with a duration sets the comp's timing (Responsive Design), so it is
/// part of the pixels even where plain markers are not.
fn is_timing_marker(m: &Marker) -> bool {
    m.protected && m.duration > Tick::ZERO
}

fn timing_markers(m: &[Marker]) -> impl Iterator<Item = (Tick, Tick)> + '_ {
    m.iter().filter(|m| is_timing_marker(m)).map(|m| (m.time, m.duration))
}

impl Comp {
    /// Draws the same pixels as `o`: equal but for its layers' Audio, Lock and Shy switches, Hide
    /// Shy Layers and the viewer's Resolution.
    pub fn same_pixels(&self, o: &Comp) -> bool {
        self.same_pixels_with(o, true)
    }

    /// [`Self::same_pixels`], with the comp's and its layers' markers counting only when
    /// `with_markers`: only expressions read them, so while none does, adding or moving a marker
    /// (even during playback) leaves every rendered frame valid. Protected time markers with a
    /// duration set the timing (Responsive Design), so those always count.
    pub fn same_pixels_with(&self, o: &Comp, with_markers: bool) -> bool {
        self.same_settings(o, with_markers)
            && self.layers.len() == o.layers.len()
            && self.layers.iter().zip(&o.layers).all(|(a, b)| a.same_pixels_with(b, with_markers))
    }

    /// [`Self::same_pixels_with`] but for the layers.
    fn same_settings(&self, o: &Comp, with_markers: bool) -> bool {
        // Every field (no `..`: a new field must be considered here).
        let Comp {
            width,
            height,
            pixel_aspect,
            frame_rate,
            duration,
            display_start,
            background,
            work_area,
            layers: _,
            markers,
            shutter_angle,
            shutter_phase,
            motion_blur_samples,
            motion_blur_adaptive_limit,
            renderer,
            hide_shy: _,
            enable_motion_blur,
            enable_frame_blending,
            draft_3d,
            preserve_frame_rate,
            preserve_resolution,
            poster_time,
            global_light,
            guides,
            essential,
            resolution: _,
        } = self;
        *width == o.width
            && *height == o.height
            && *pixel_aspect == o.pixel_aspect
            && *frame_rate == o.frame_rate
            && *duration == o.duration
            && *display_start == o.display_start
            && *background == o.background
            && *work_area == o.work_area
            && (if with_markers { *markers == o.markers } else { timing_markers(markers).eq(timing_markers(&o.markers)) })
            && *shutter_angle == o.shutter_angle
            && *shutter_phase == o.shutter_phase
            && *motion_blur_samples == o.motion_blur_samples
            && *motion_blur_adaptive_limit == o.motion_blur_adaptive_limit
            && *renderer == o.renderer
            && *enable_motion_blur == o.enable_motion_blur
            && *enable_frame_blending == o.enable_frame_blending
            && *draft_3d == o.draft_3d
            && *preserve_frame_rate == o.preserve_frame_rate
            && *preserve_resolution == o.preserve_resolution
            && *poster_time == o.poster_time
            && *global_light == o.global_light
            && *guides == o.guides
            && *essential == o.essential
    }

    /// The comp-time spans where `self` may draw other pixels than `o` after an edit of some of
    /// its layers (moved, trimmed, split, edited, added, removed): where the active layers differ
    /// or a changed one draws differently, widened by the shutter. A trim reaches only the part it
    /// cut, a split only the copy's half (#1641). `None` when the edit can reach any frame: a comp
    /// setting, a layer reorder, Solo turning on or off, an environment layer, an expression (it
    /// reads anything), or a changed layer another layer parents to, mattes with or picks in an
    /// effect or tracker.
    pub fn changed_spans(&self, o: &Comp, with_markers: bool) -> Option<Vec<(Tick, Tick)>> {
        fn has_expression(g: &PropGroup) -> bool {
            g.children.iter().any(|c| match c {
                Node::Prop(p) => p.expr.is_some(),
                Node::Group(g) => has_expression(g),
            })
        }
        // Solo hides every other layer at every frame.
        if !self.same_settings(o, with_markers) || self.any_solo() != o.any_solo() {
            return None;
        }
        let common = |a: &Comp, b: &Comp| a.layers.iter().map(|l| l.id).filter(|id| b.layer(*id).is_some()).collect::<Vec<_>>();
        if common(self, o) != common(o, self) {
            return None;
        }
        // Changed, added or removed layers (each from both sides: its old and its new span), and
        // those that draw the same apart from their span (a trim or split). Ids count: they seed
        // randomness (noise, text randomisers), so a split's copy may draw differently.
        let mut changed = Vec::new();
        let mut respanned = Vec::new();
        let mut cuts = Vec::new();
        for (l, other) in self.layers.iter().map(|l| (l, o.layer(l.id))).chain(o.layers.iter().map(|l| (l, self.layer(l.id)))) {
            if other.is_some_and(|b| l.same_pixels_with(b, with_markers)) {
                continue;
            }
            if l.environment || l.environment_background {
                return None;
            }
            if let Some(b) = other
                && (Layer { in_point: b.in_point, out_point: b.out_point, name: b.name.clone(), label: b.label, comment: b.comment.clone(), ..l.clone() })
                    .same_pixels_with(b, with_markers)
            {
                respanned.push(l.id);
            }
            changed.push(l.id);
            cuts.extend([l.in_point, l.out_point]);
        }
        let reaches = |l: &Layer| has_expression(&l.props) || l.referenced_layers().iter().any(|r| *r != l.id && changed.contains(r));
        if self.layers.iter().chain(&o.layers).any(reaches) {
            return None;
        }
        // A frame at `t` samples cameras, lights and transforms over its shutter,
        // `t + (phase + angle * 0..=1) / 360` frames, and draws the layers active at `t` itself.
        let (a, p) = (self.shutter_angle / 360.0, self.shutter_phase / 360.0);
        let fd = self.frame_duration().seconds();
        let (before, after) = (Tick::from_seconds_f64(p.max(p + a).max(0.0) * fd), Tick::from_seconds_f64(-p.min(p + a).min(0.0) * fd));
        // Between two cuts the same layers are active; outside them no changed layer is.
        cuts.sort_unstable();
        cuts.dedup();
        let draws_same = |l: &Layer| !changed.contains(&l.id) || respanned.contains(&l.id);
        let spans = cuts
            .windows(2)
            .filter_map(|w| {
                let (&s, &e) = (w.first()?, w.last()?);
                let active = |c: &Comp| c.layers.iter().filter(|l| l.is_active_at(s)).map(|l| l.id).collect::<Vec<_>>();
                let same = active(self) == active(o) && self.layers.iter().filter(|l| l.is_active_at(s)).all(draws_same);
                (!same).then(|| (s - before, e + after))
            })
            .collect();
        Some(spans)
    }

    /// Some layer is soloed: only the soloed layers draw.
    pub fn any_solo(&self) -> bool {
        self.layers.iter().any(|l| l.switches.solo && l.source.is_av())
    }

    /// The comp with those switches and the Resolution at their defaults (borrowed when they
    /// already are), for keys that hash the whole comp.
    pub fn pixel_form(&self) -> std::borrow::Cow<'_, Comp> {
        self.pixel_form_with(true)
    }

    /// [`Self::pixel_form`], also without the comp's plain markers and its layers' markers when not
    /// `with_markers` (see [`Self::same_pixels_with`] and [`Project::reads_markers`]).
    pub fn pixel_form_with(&self, with_markers: bool) -> std::borrow::Cow<'_, Comp> {
        if !self.hide_shy
            && self.resolution.is_auto()
            && self.layers.iter().all(|l| l.switches == l.switches.pixels())
            && (with_markers || (self.markers.iter().all(is_timing_marker) && self.layers.iter().all(|l| l.markers.is_empty())))
        {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut c = self.clone();
        c.hide_shy = false;
        c.resolution = Resolution::Auto;
        if !with_markers {
            c.markers.retain(is_timing_marker);
        }
        for l in &mut c.layers {
            l.switches = l.switches.pixels();
            if !with_markers {
                l.markers.clear();
            }
        }
        std::borrow::Cow::Owned(c)
    }
}
