//! The layer header (`ldta`) and the composition header (`cdta`, `PRin`).
//!
//! Every field here has a verified layout: the decoded value equals the value the generated corpus
//! was made with (see `docs/aep-format.md`, sections 4 and 5, for the evidence per field). Fields that
//! the corpus does not decide are not decoded; the raw bytes stay reachable through the chunk.

use crate::bytes::{array, be_i32, be_u16, be_u32, cstr, text, until_nul};
use crate::typed::{Comp, Item, Layer, TimeBase, utf8_child};

/// An exact fraction. Used for times (`seconds = num / den`) and for the pixel aspect ratio. The
/// fields hold the stored values unreduced; [`Rational::reduced`] normalises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    pub num: i64,
    pub den: i64,
}

impl Rational {
    /// A fraction with a positive denominator, reduced; `None` for a zero denominator.
    pub fn new(num: i64, den: i64) -> Option<Self> {
        Self { num, den }.reduced()
    }

    /// The value as `f64`, `None` for a zero denominator.
    pub fn to_f64(&self) -> Option<f64> {
        (self.den != 0).then(|| self.num as f64 / self.den as f64)
    }

    /// The same value in lowest terms with a positive denominator, `None` for a zero denominator.
    pub fn reduced(&self) -> Option<Self> {
        if self.den == 0 {
            return None;
        }
        let (mut a, mut b) = (i128::from(self.num).unsigned_abs(), i128::from(self.den).unsigned_abs());
        let (mut x, mut y) = (a, b);
        while y != 0 {
            (x, y) = (y, x % y);
        }
        if x == 0 {
            x = 1;
        }
        a /= x;
        b /= x;
        let negative = (self.num < 0) != (self.den < 0);
        let num = i64::try_from(a).ok()?;
        let den = i64::try_from(b).ok()?;
        Some(Self { num: if negative { num.checked_neg()? } else { num }, den })
    }

    /// `self + other * factor`, exact, `None` on a zero denominator or overflow.
    fn plus_scaled(&self, other: &Rational, factor: &Rational) -> Option<Rational> {
        if self.den == 0 || other.den == 0 || factor.den == 0 {
            return None;
        }
        // other * factor = (other.num * factor.num) / (other.den * factor.den)
        let scaled_num = i128::from(other.num).checked_mul(i128::from(factor.num))?;
        let scaled_den = i128::from(other.den).checked_mul(i128::from(factor.den))?;
        let den = i128::from(self.den).checked_mul(scaled_den)?;
        let num = i128::from(self.num).checked_mul(scaled_den)?.checked_add(scaled_num.checked_mul(i128::from(self.den))?)?;
        Rational { num: i64::try_from(num).ok()?, den: i64::try_from(den).ok()? }.reduced()
    }
}

/// The layer quality switch, `ldta` byte 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// `1`
    Draft,
    /// `2`
    Best,
    /// Any other value (`0` on cameras and lights; wireframe was not sampled).
    Other(u8),
}

/// The blend mode, `ldta` byte 99. Six modes were sampled; any other mode is kept as its raw code.
/// The codes 2, 4, 5, 6 and 7 coincide with the numbers of the After Effects plug-in transfer
/// modes; Difference is 26.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    /// `2`
    Normal,
    /// `4`
    Add,
    /// `5`
    Multiply,
    /// `6`
    Screen,
    /// `7`
    Overlay,
    /// `26`
    Difference,
    /// Any other value (not sampled; `0` on cameras and lights).
    Other(u8),
}

impl BlendMode {
    /// The mode for a code.
    pub fn from_code(code: u8) -> Self {
        match code {
            2 => Self::Normal,
            4 => Self::Add,
            5 => Self::Multiply,
            6 => Self::Screen,
            7 => Self::Overlay,
            26 => Self::Difference,
            other => Self::Other(other),
        }
    }

    /// The code as stored.
    pub fn code(&self) -> u8 {
        match self {
            Self::Normal => 2,
            Self::Add => 4,
            Self::Multiply => 5,
            Self::Screen => 6,
            Self::Overlay => 7,
            Self::Difference => 26,
            Self::Other(code) => *code,
        }
    }
}

/// The light type, `ldta` `u32` at byte 136 of a light layer (0 on every other layer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightType {
    /// `0` (701)
    Parallel,
    /// `1`
    Spot,
    /// `2`
    Point,
    /// `3` (702; a light created as an environment light by script without the Advanced 3D
    /// renderer is stored as an ambient light too, 704)
    Ambient,
    /// Any other value (an environment light was not sampled).
    Other(u32),
}

/// The layer type, `ldta` `u32` at byte 128.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerType {
    /// `0`: solid, footage, precomp or null object (an AV layer).
    Av,
    /// `1`
    Light,
    /// `2`
    Camera,
    /// `3`
    Text,
    /// `4`
    Shape,
    /// Any other value.
    Other(u32),
}

/// The decoded fields of a layer's `ldta` chunk (164 bytes; the fields below need the first 140).
///
/// Times are stored as a numerator and a denominator (the denominator is the composition's
/// timescale, 1 in a precomp layer). The in and out points are stored **relative to the start**:
/// the absolute in point is `start + in * stretch`, see [`LayerHeader::in_point`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerHeader {
    /// The layer id (unique in the project); parent links use it.
    pub id: u32,
    pub quality: Quality,
    /// The numerator of the time stretch (signed `i32` at byte 8). The stretch is the exact ratio
    /// `stretch / stretch_den`: `1/1` = 100 %, `2/1` = 200 %, `1/2` = 50 %, `3/2` = 150 %, `-1/1` =
    /// -100 % (a time-reversed layer, 981), `-1/2` = -50 % (983), see
    /// [`LayerHeader::stretch_ratio`].
    pub stretch: i32,
    /// The denominator of the time stretch (`u32` at byte 108; 1 in every file before 50 % was
    /// sampled).
    pub stretch_den: u32,
    /// The layer start time, as stored.
    pub start: Rational,
    /// The in point relative to the start, as stored.
    pub in_relative: Rational,
    /// The out point relative to the start, as stored.
    pub out_relative: Rational,
    /// Bytes 37, 38 and 39 of `ldta`: the switch bits (see the `is_*` methods).
    pub flag_bytes: [u8; 3],
    /// The id of the source item (`ldta` byte 40), `None` for the values `0` (shape, text, camera)
    /// and `0xffff_ffff` (light).
    pub source_id: Option<u32>,
    /// The label colour index (`ldta` byte 61: 1 red, 5, 8 blue, ...).
    pub label: u8,
    /// The copy of the layer name at byte 64 (31 characters at most; the layer's `Utf8` child holds
    /// the full name).
    pub name_copy: String,
    pub blend_mode: BlendMode,
    pub layer_type: LayerType,
    /// The id of the parent layer, `None` when there is none.
    pub parent_id: Option<u32>,
    /// The `u32` at byte 136 (the light type of a light layer, 0 elsewhere), see
    /// [`LayerHeader::light_type`].
    pub light_type_code: u32,
}

impl LayerHeader {
    /// Decodes an `ldta` body; `None` when it is shorter than 140 bytes.
    pub fn parse(ldta: &[u8]) -> Option<Self> {
        let time = |at: usize| Some(Rational { num: i64::from(be_i32(ldta, at)?), den: i64::from(be_u32(ldta, at.checked_add(4)?)?) });
        let source = be_u32(ldta, 40)?;
        let parent = be_u32(ldta, 132)?;
        Some(Self {
            id: be_u32(ldta, 0)?,
            quality: match *ldta.get(5)? {
                1 => Quality::Draft,
                2 => Quality::Best,
                other => Quality::Other(other),
            },
            stretch: be_i32(ldta, 8)?,
            stretch_den: be_u32(ldta, 108)?,
            start: time(12)?,
            in_relative: time(20)?,
            out_relative: time(28)?,
            flag_bytes: array::<3>(ldta, 37)?,
            source_id: (source != 0 && source != u32::MAX).then_some(source),
            label: *ldta.get(61)?,
            name_copy: text(until_nul(ldta.get(64..96)?)),
            blend_mode: BlendMode::from_code(*ldta.get(99)?),
            layer_type: match be_u32(ldta, 128)? {
                0 => LayerType::Av,
                1 => LayerType::Light,
                2 => LayerType::Camera,
                3 => LayerType::Text,
                4 => LayerType::Shape,
                other => LayerType::Other(other),
            },
            parent_id: (parent != 0).then_some(parent),
            light_type_code: be_u32(ldta, 136)?,
        })
    }

    fn flag(&self, byte: usize, mask: u8) -> bool {
        self.flag_bytes.get(byte).is_some_and(|b| b & mask != 0)
    }

    /// Byte 37 bit `0x01`: the layer has a name set by the user (clear on a default-named camera or
    /// light and on a precomp layer, whose name is the source's).
    pub fn name_is_user_set(&self) -> bool {
        self.flag(0, 0x01)
    }

    /// Byte 37 bit `0x02`: guide layer.
    pub fn is_guide(&self) -> bool {
        self.flag(0, 0x02)
    }

    /// Byte 38 bit `0x02`: adjustment layer.
    pub fn is_adjustment(&self) -> bool {
        self.flag(1, 0x02)
    }

    /// Byte 38 bit `0x04`: the 3D layer switch.
    pub fn is_three_d(&self) -> bool {
        self.flag(1, 0x04)
    }

    /// Byte 38 bit `0x40`: the layer orients itself towards its point of interest. Set on two-node
    /// cameras (byte 38 is `0x44`) and on all lights, clear on a one-node camera (`0x04`).
    pub fn is_auto_oriented(&self) -> bool {
        self.flag(1, 0x40)
    }

    /// Byte 38 bit `0x08`: solo.
    pub fn is_solo(&self) -> bool {
        self.flag(1, 0x08)
    }

    /// Byte 38 bit `0x80`: the layer was created as a null object.
    pub fn is_null_object(&self) -> bool {
        self.flag(1, 0x80)
    }

    /// Byte 39 bit `0x01`: the video switch (the layer's eye icon; `enabled` in the scripting API).
    pub fn is_video_enabled(&self) -> bool {
        self.flag(2, 0x01)
    }

    /// The same as [`LayerHeader::is_video_enabled`].
    pub fn is_enabled(&self) -> bool {
        self.is_video_enabled()
    }

    /// Byte 39 bit `0x02`: the audio switch (`audioEnabled`).
    pub fn is_audio_enabled(&self) -> bool {
        self.flag(2, 0x02)
    }

    /// Byte 39 bit `0x04`: the effects switch (`effectsActive`).
    pub fn is_effects_enabled(&self) -> bool {
        self.flag(2, 0x04)
    }

    /// Byte 39 bit `0x08`: the motion blur switch.
    pub fn is_motion_blur(&self) -> bool {
        self.flag(2, 0x08)
    }

    /// Byte 39 bit `0x20`: locked.
    pub fn is_locked(&self) -> bool {
        self.flag(2, 0x20)
    }

    /// Byte 39 bit `0x40`: shy.
    pub fn is_shy(&self) -> bool {
        self.flag(2, 0x40)
    }

    /// Byte 39 bit `0x80`: collapse transformations / continuously rasterize. Shape and text
    /// layers have it set from creation.
    pub fn collapses_transformations(&self) -> bool {
        self.flag(2, 0x80)
    }

    /// The light type of a light layer: parallel 0, spot 1, point 2, ambient 3 (`None` for any
    /// other layer type).
    pub fn light_type(&self) -> Option<LightType> {
        (self.layer_type == LayerType::Light).then_some(match self.light_type_code {
            0 => LightType::Parallel,
            1 => LightType::Spot,
            2 => LightType::Point,
            3 => LightType::Ambient,
            other => LightType::Other(other),
        })
    }

    /// The time stretch as an exact reduced ratio (`stretch / stretch_den`: 50 % is `1/2`, 150 %
    /// is `3/2`); `None` for a zero denominator.
    pub fn stretch_ratio(&self) -> Option<Rational> {
        Rational::new(i64::from(self.stretch), i64::from(self.stretch_den))
    }

    /// The time stretch in percent (`stretch / stretch_den * 100`); a zero denominator reads as 1.
    pub fn stretch_percent(&self) -> f64 {
        let den = if self.stretch_den == 0 { 1.0 } else { f64::from(self.stretch_den) };
        f64::from(self.stretch) / den * 100.0
    }

    /// The start time in seconds, `None` for a zero denominator.
    pub fn start_seconds(&self) -> Option<f64> {
        self.start.to_f64()
    }

    /// The absolute in point, `start + in * stretch`, exact and reduced.
    pub fn in_point(&self) -> Option<Rational> {
        self.start.plus_scaled(&self.in_relative, &self.stretch_ratio()?)
    }

    /// The absolute out point, `start + out * stretch`, exact and reduced. For a footage layer the
    /// stored denominator is the footage time base (the sample rate of an audio layer, the frame
    /// rate of an image sequence), not the composition timescale.
    pub fn out_point(&self) -> Option<Rational> {
        self.start.plus_scaled(&self.out_relative, &self.stretch_ratio()?)
    }

    /// The absolute in point in seconds.
    pub fn in_point_seconds(&self) -> Option<f64> {
        self.in_point()?.to_f64()
    }

    /// The absolute out point in seconds.
    pub fn out_point_seconds(&self) -> Option<f64> {
        self.out_point()?.to_f64()
    }
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The layer id (`ldta` byte 0).
    pub fn id(&self) -> Option<u32> {
        be_u32(self.chunk.child(b"ldta")?.bytes()?, 0)
    }

    /// The decoded `ldta`, see [`LayerHeader`].
    pub fn header(&self) -> Option<LayerHeader> {
        LayerHeader::parse(self.chunk.child(b"ldta")?.bytes()?)
    }

    /// The video switch (byte 39 bit `0x01`), `enabled` in the scripting API.
    pub fn is_enabled(&self) -> Option<bool> {
        self.header().map(|h| h.is_video_enabled())
    }

    /// The audio switch (byte 39 bit `0x02`), `audioEnabled`.
    pub fn is_audio_enabled(&self) -> Option<bool> {
        self.header().map(|h| h.is_audio_enabled())
    }

    /// The effects switch (byte 39 bit `0x04`), `effectsActive`.
    pub fn is_effects_enabled(&self) -> Option<bool> {
        self.header().map(|h| h.is_effects_enabled())
    }
}

/// The renderer of a composition (`LIST PRin` / `prin`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renderer {
    /// The match name (`ADBE Escher` for the Classic 3D renderer in the base project,
    /// `ADBE Calder` for Advanced 3D).
    pub match_name: String,
    /// The display name (`Classic 3D`, `Advanced 3D`).
    pub display_name: String,
}

/// The decoded fields of a composition's `cdta` chunk (204 bytes; the fields below need 176).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompHeader {
    /// Frame duration (`u32` at byte 4) and timescale (`u32` at byte 8).
    pub time_base: TimeBase,
    /// The frame rate as 16.16 fixed point (`u32` at byte 156).
    pub frame_rate_fixed: u32,
    /// The composition duration in time units.
    pub duration_units: u32,
    /// The work area start in time units.
    pub work_area_start_units: u32,
    /// The work area end (start + duration) in time units; `None` when it is the whole
    /// composition (stored as `0xffff_ffff`).
    pub work_area_end_units: Option<u32>,
    /// The background colour as R, G, B (bytes 52 to 54), 8 bits per channel: 0.5 is stored as 128
    /// and reads back as 128/255.
    pub background: [u8; 3],
    /// The shutter angle in degrees.
    pub shutter_angle: u32,
    pub width: u16,
    pub height: u16,
    /// The pixel aspect ratio as stored: numerator and denominator.
    pub pixel_aspect: Rational,
    /// The composition's motion blur switch.
    pub motion_blur: bool,
    /// The shutter phase in degrees (`i32` at byte 180, 0 by default, -90 in the sample); `None`
    /// when the chunk ends before byte 184.
    pub shutter_phase: Option<i32>,
    /// The display start time (the comp start timecode) as `num / den` seconds: `u32` at byte 164
    /// over `u32` at byte 168 (0/1 by default; display start frame 100 at 25 fps is 102400/25600).
    /// `None` when the chunk ends before byte 172.
    pub display_start: Option<Rational>,
}

impl CompHeader {
    /// Decodes a `cdta` body; `None` when it is shorter than 176 bytes.
    pub fn parse(cdta: &[u8]) -> Option<Self> {
        let work_end = be_u32(cdta, 36)?;
        Some(Self {
            time_base: TimeBase { frame_duration: be_u32(cdta, 4)?, timescale: be_u32(cdta, 8)? },
            frame_rate_fixed: be_u32(cdta, 156)?,
            duration_units: be_u32(cdta, 44)?,
            work_area_start_units: be_u32(cdta, 28)?,
            work_area_end_units: (work_end != u32::MAX).then_some(work_end),
            background: array::<3>(cdta, 52)?,
            shutter_angle: be_u32(cdta, 172)?,
            width: be_u16(cdta, 140)?,
            height: be_u16(cdta, 142)?,
            pixel_aspect: Rational { num: i64::from(be_u32(cdta, 144)?), den: i64::from(be_u32(cdta, 148)?) },
            motion_blur: cdta.get(139)? & 0x08 != 0,
            shutter_phase: be_i32(cdta, 180),
            display_start: be_u32(cdta, 164).zip(be_u32(cdta, 168)).map(|(num, den)| Rational { num: i64::from(num), den: i64::from(den) }),
        })
    }

    /// Frames per second from the time base (`timescale / frame_duration`).
    pub fn frame_rate(&self) -> Option<f64> {
        self.time_base.frame_rate()
    }

    /// The frame rate from the 16.16 fixed-point field (25 is `0x0019_0000`).
    pub fn frame_rate_from_fixed(&self) -> f64 {
        f64::from(self.frame_rate_fixed) / 65536.0
    }

    /// The duration in seconds.
    pub fn duration_seconds(&self) -> Option<f64> {
        self.time_base.seconds(i32::try_from(self.duration_units).ok()?)
    }

    /// The work area start in seconds.
    pub fn work_area_start_seconds(&self) -> Option<f64> {
        self.time_base.seconds(i32::try_from(self.work_area_start_units).ok()?)
    }

    /// The work area end in seconds; the composition duration when the work area is the whole
    /// composition.
    pub fn work_area_end_seconds(&self) -> Option<f64> {
        match self.work_area_end_units {
            Some(end) => self.time_base.seconds(i32::try_from(end).ok()?),
            None => self.duration_seconds(),
        }
    }

    /// The background colour as fractions 0..1 (`byte / 255`).
    pub fn background_rgb(&self) -> [f64; 3] {
        self.background.map(|c| f64::from(c) / 255.0)
    }

    /// The pixel aspect ratio as a number.
    pub fn pixel_aspect_ratio(&self) -> Option<f64> {
        self.pixel_aspect.to_f64()
    }

    /// The display start time in seconds (the start timecode), `None` when absent or the
    /// denominator is zero.
    pub fn display_start_seconds(&self) -> Option<f64> {
        self.display_start?.to_f64()
    }

    /// The display start as a frame number (`display start * frame rate`, rounded); 100 at 25 fps
    /// for a display start of 4 s.
    pub fn display_start_frame(&self) -> Option<i64> {
        let rate = self.frame_rate()?;
        Some((self.display_start_seconds()? * rate).round() as i64)
    }

    /// The frame rate as an exact fraction `timescale / frame_duration`, as typed: 29.97 fps is
    /// `23976 / 800` (not `30000 / 1001`), 23.976 fps is `23976 / 1000`.
    pub fn frame_rate_exact(&self) -> Option<Rational> {
        Rational::new(i64::from(self.time_base.timescale), i64::from(self.time_base.frame_duration))
    }

    /// The duration in whole frames (`duration / frame_duration`). After Effects snaps a duration
    /// to a whole number of frames: 5 s at 29.97 fps becomes 150 frames (5.005 s).
    pub fn duration_frames(&self) -> Option<u32> {
        self.duration_units.checked_div(self.time_base.frame_duration)
    }
}

impl<'f, 'a> Comp<'f, 'a> {
    /// The decoded `cdta`, see [`CompHeader`].
    pub fn header(&self) -> Option<CompHeader> {
        CompHeader::parse(self.item.child(b"cdta")?.bytes()?)
    }

    /// The composition name (the item's `Utf8` child).
    pub fn name(&self) -> Option<String> {
        utf8_child(self.item).filter(|n| !n.is_empty())
    }

    /// The item this composition data belongs to.
    pub fn item(&self) -> Item<'f, 'a> {
        Item { chunk: self.item }
    }

    /// The renderer (`LIST PRin` / `prin`: match name at byte 4, display name at byte 52, 32
    /// bytes each).
    pub fn renderer(&self) -> Option<Renderer> {
        let prin = self.item.child_list(b"PRin")?.child(b"prin")?.bytes()?;
        Some(Renderer { match_name: cstr(prin, 4, 32)?, display_name: cstr(prin, 52, 32)? })
    }
}
