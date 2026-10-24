//! Effect instances: match name, instance name, the parameter list from `parT`/`pard` and the
//! values stored with the instance.
//!
//! An effect on a layer is a `LIST sspc` under `ADBE Effect Parade`. It holds `fnam` (the
//! instance name), `LIST parT` (the parameter definitions: `parn`, then a `tdmn` and a 148-byte
//! `pard` per parameter, a `pdnm` after a popup), `LIST tdgp` (the values) and `pgui`. Only
//! parameters that differ from their `pard` default have a value record in the `tdgp` (the Layer
//! Control and button records are the exceptions: they always exist).
//!
//! When several layers carry an effect of the same type, only one instance has a full `parT`; the
//! others have an empty `LIST parT` (case 383). Their parameter definitions are the ones in the
//! project-level `LIST EfdG` (one `EfDf` per effect type: a `tdmn` with the match name, then a copy
//! of one instance's `sspc`), see [`Project::effect_params`]. That copy also carries the values of
//! the instance it was copied from, so values are only ever read from the instance's own `tdgp`.

use crate::Chunk;
use crate::bytes::{array, be_f32, be_f64, be_i32, be_u16, be_u32, cstr, text, until_nul};
use crate::typed::{KeyList, Layer, Project, PropGroup, PropNode, Property};

/// The parameter type, the `u32` at byte 12 of `pard`. The numbering matches the plug-in
/// parameter types of the After Effects effect interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// `0`: a layer reference (also the implicit first parameter, the input layer).
    Layer,
    /// `1`: an integer slider (Fast Box Blur's Iterations, 412-414): the default is an `i32` at
    /// `pard` byte 56 (3), the valid maximum an `i32` at byte 128 (50), the value record a plain
    /// `f64` (1.0).
    IntSlider,
    /// `2`: a fixed-point (16.16) slider.
    FixedSlider,
    /// `3`: an angle (16.16 degrees).
    Angle,
    /// `4`
    Checkbox,
    /// `5`
    Color,
    /// `6`
    Point,
    /// `7`: a popup menu (the items are in the `pdnm` chunk).
    Popup,
    /// `9`: no data (the group-end entry `ADBE Effect Built In Params`).
    NoData,
    /// `10`: a floating-point slider.
    FloatSlider,
    /// `11`: arbitrary data (Hue/Saturation's Channel Range, Levels' Histogram, 443, 511): the
    /// value record is a 4-byte `cdat`, the data itself the `aRbp` of the `LIST aRbs` after it,
    /// see [`Effect::arbitrary_data`].
    ArbitraryData,
    /// `12`: a path / mask reference.
    Path,
    /// `13`: the start of a twirl-down group (Exposure's Master, Red, Green and Blue, 389). It
    /// holds no value; its value record always exists with `cdat` 0.
    GroupStart,
    /// `14`: the end of a twirl-down group (its name repeats the group's name, 389). It holds no
    /// value; its value record always exists with `cdat` 0.
    GroupEnd,
    /// `15`: a button (Tint's and Gradient Ramp's Swap Colors). It holds no value; its value
    /// record always exists with `cdat` 0 (356, 368).
    Button,
    /// Any other value.
    Other(u32),
}

impl ParamKind {
    fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::Layer,
            1 => Self::IntSlider,
            2 => Self::FixedSlider,
            3 => Self::Angle,
            4 => Self::Checkbox,
            5 => Self::Color,
            6 => Self::Point,
            7 => Self::Popup,
            9 => Self::NoData,
            10 => Self::FloatSlider,
            11 => Self::ArbitraryData,
            12 => Self::Path,
            13 => Self::GroupStart,
            14 => Self::GroupEnd,
            15 => Self::Button,
            other => Self::Other(other),
        }
    }
}

/// A decoded parameter value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamValue {
    /// A slider or angle value as stored: float sliders, fixed sliders and angles keep a plain
    /// `f64` in their value record (an angle in degrees; a fixed slider in its own scale, see
    /// [`EffectParam::valid_max`]); defaults read from `pard` are 16.16 decoded for fixed sliders
    /// and angles.
    Number(f64),
    /// A checkbox: stored as 1.0 when on; an absent record means off.
    Bool(bool),
    /// A point control as stored: a fraction of the layer size (`[111, 222]` px on a 1920x1080
    /// layer is `[111/1920, 222/1080]`).
    Point([f64; 2]),
    /// A layer control: the id of the chosen layer (the `tdpi` chunk next to the value record).
    /// In the implicit first parameter it is the id of the layer that owns the effect.
    Layer(u32),
    /// A colour as ARGB fractions 0..1 (stored as doubles 0..255 in the order alpha, red, green,
    /// blue: a colour `[0.2, 0.6, 1]` is `[255, 51, 153, 255]`, with float rounding).
    Argb([f64; 4]),
    /// A popup's item index, 1-based (Gradient Ramp's Radial Ramp is 2, 377): the default in
    /// `pard`, or the stored selection.
    Index(u32),
}

/// What a Layer Control points at, given the id of the layer that owns the effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerRef {
    /// The layer itself (the default, "Layer 1" on the first layer): the stored `tdpi` equals the
    /// owning layer's id.
    This,
    /// None: the stored `tdpi` is 0.
    Nothing,
    /// Another layer, by id.
    Layer(u32),
}

/// An effect instance on a layer.
#[derive(Debug, Clone)]
pub struct Effect<'f, 'a> {
    match_name: String,
    chunk: &'f Chunk<'a>,
}

/// Where the parameter definitions of an effect instance come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionSource {
    /// The instance's own `parT`.
    Instance,
    /// The project-level `EfdG/EfDf` copy for the effect's match name (the instance's own `parT`
    /// is empty, 383).
    Project,
    /// Neither has any: the effect has no readable parameter definitions.
    Missing,
}

/// One parameter of an effect.
#[derive(Debug, Clone)]
pub struct EffectParam<'f, 'a> {
    /// The position in `parT` (0 is the implicit input layer).
    pub index: usize,
    /// The `tdmn` match name (`<effect match name>-NNNN`, the last one is
    /// `ADBE Effect Built In Params`).
    pub match_name: String,
    pub kind: ParamKind,
    /// The parameter name (`Blurriness`, `Slider`, ...; empty for the implicit parameters).
    pub name: String,
    /// The popup items (`pdnm`, split at `|`; for a button its label, e.g. `Swap Colors`).
    pub items: Option<Vec<String>>,
    /// True when the definition is the project-level copy (see [`DefinitionSource::Project`]).
    pub borrowed_definition: bool,
    pard: &'f [u8],
    stored: Option<Property<'f, 'a>>,
}

impl<'f, 'a> Effect<'f, 'a> {
    /// The effect's match name (`ADBE Slider Control`, `ADBE Gaussian Blur 2`, ...).
    pub fn match_name(&self) -> &str {
        &self.match_name
    }

    /// The underlying `LIST sspc` chunk.
    pub fn chunk(&self) -> &'f Chunk<'a> {
        self.chunk
    }

    /// The instance name shown in the effect controls (`fnam`).
    pub fn name(&self) -> Option<String> {
        self.chunk.child(b"fnam")?.children().iter().find(|c| c.is(b"Utf8"))?.bytes().map(text)
    }

    /// The name the user gave the instance: the `tdsn` of the instance's value group when it is
    /// not the placeholder `-_0_/-` (610: `My Tint`; `fnam` keeps the effect's own name `Tint`).
    /// `None` while the instance shows the effect's name.
    pub fn user_name(&self) -> Option<String> {
        let values = self.chunk.child_list(b"tdgp")?;
        let name = values.child(b"tdsn")?.children().iter().find(|c| c.is(b"Utf8"))?.bytes().map(text)?;
        (name != "-_0_/-" && !name.is_empty()).then_some(name)
    }

    /// The name After Effects shows for the instance: [`Effect::user_name`], else [`Effect::name`].
    pub fn display_name(&self) -> Option<String> {
        self.user_name().or_else(|| self.name())
    }

    /// The number of parameters declared by `parn` (the implicit input layer and the built-in
    /// entry included).
    pub fn declared_param_count(&self) -> Option<u32> {
        be_u32(self.chunk.child_list(b"parT")?.child(b"parn")?.bytes()?, 0)
    }

    /// True when the instance's own `parT` holds parameter definitions (a `pard`). A second
    /// instance of an effect type has an empty `parT` (383).
    pub fn has_own_definitions(&self) -> bool {
        self.chunk.child_list(b"parT").is_some_and(has_pard)
    }

    /// The parameters in the order of the instance's own `parT`. Empty for an instance whose
    /// `parT` is empty; [`Project::effect_params`] falls back to the project's definitions.
    pub fn params(&self) -> Vec<EffectParam<'f, 'a>> {
        match self.chunk.child_list(b"parT") {
            Some(part) => self.params_from(part, false),
            None => Vec::new(),
        }
    }

    /// The parameters described by `part` (a `LIST parT`) with the values of this instance.
    fn params_from(&self, part: &'f Chunk<'a>, borrowed: bool) -> Vec<EffectParam<'f, 'a>> {
        let mut out: Vec<EffectParam<'f, 'a>> = Vec::new();
        let values = self.chunk.child_list(b"tdgp").and_then(PropGroup::from_chunk);
        let mut pending: Option<String> = None;
        for c in part.children() {
            if c.is(b"tdmn") {
                pending = c.bytes().map(|b| text(until_nul(b)));
            } else if c.is(b"pard") {
                let Some(pard) = c.bytes() else { continue };
                let match_name = pending.take().unwrap_or_default();
                let stored = values.as_ref().and_then(|g| match g.get(&match_name) {
                    Some(PropNode::Property(p)) => Some(p),
                    _ => None,
                });
                out.push(EffectParam {
                    index: out.len(),
                    match_name,
                    kind: ParamKind::from_u32(be_u32(pard, 12).unwrap_or(u32::MAX)),
                    name: cstr(pard, 16, 32).unwrap_or_default(),
                    items: None,
                    borrowed_definition: borrowed,
                    pard,
                    stored,
                });
            } else if c.is(b"pdnm")
                && let Some(last) = out.last_mut()
                && let Some(u) = c.children().iter().find(|u| u.is(b"Utf8"))
                && let Some(b) = u.bytes()
            {
                last.items = Some(text(b).split('|').map(str::to_string).collect());
            }
        }
        out
    }
}

fn has_pard(part: &Chunk<'_>) -> bool {
    part.children().iter().any(|c| c.is(b"pard"))
}

impl<'f, 'a> Project<'f, 'a> {
    /// The project-level effect definitions (`LIST EfdG`): one entry per effect type, the match
    /// name and the `LIST parT` of the copied instance, in file order (the stack order of the
    /// types, 365/366). The `EfDC` count before them is little-endian and is not needed to read
    /// them. A type whose copy has no definitions is left out.
    pub fn effect_definitions(&self) -> Vec<(String, &'f Chunk<'a>)> {
        let mut out: Vec<(String, &'f Chunk<'a>)> = Vec::new();
        let Some(defs) = self.effect_defs else { return out };
        for df in defs.children().iter().filter(|c| c.is_list(b"EfDf")) {
            let mut name: Option<String> = None;
            for c in df.children() {
                if c.is(b"tdmn") {
                    name = c.bytes().map(|b| text(until_nul(b)));
                } else if c.is_list(b"sspc")
                    && let Some(n) = name.take()
                    && let Some(part) = c.child_list(b"parT").filter(|p| has_pard(p))
                    && !out.iter().any(|(m, _)| *m == n)
                {
                    out.push((n, part));
                }
            }
        }
        out
    }

    /// Where the parameter definitions of `effect` come from.
    pub fn definition_source(&self, effect: &Effect<'f, 'a>) -> DefinitionSource {
        if effect.has_own_definitions() {
            DefinitionSource::Instance
        } else if self.effect_definitions().iter().any(|(m, _)| m == effect.match_name()) {
            DefinitionSource::Project
        } else {
            DefinitionSource::Missing
        }
    }

    /// The parameters of `effect`: from its own `parT`, or, when that is empty, from the
    /// project-level definition of its match name ([`EffectParam::borrowed_definition`]). Values
    /// always come from the instance's own records (absent = the `pard` default); the project
    /// copy's values belong to another instance and are never read.
    pub fn effect_params(&self, effect: &Effect<'f, 'a>) -> Vec<EffectParam<'f, 'a>> {
        if effect.has_own_definitions() {
            return effect.params();
        }
        match self.effect_definitions().into_iter().find(|(m, _)| m == effect.match_name()) {
            Some((_, part)) => effect.params_from(part, true),
            None => Vec::new(),
        }
    }
}

impl<'f, 'a> EffectParam<'f, 'a> {
    /// The raw 148-byte `pard` record (it contains stale memory after the name).
    pub fn pard(&self) -> &'f [u8] {
        self.pard
    }

    /// The value record stored with the instance (`<match>-NNNN` in the instance's `tdgp`), which
    /// exists only for parameters that differ from the default.
    pub fn stored(&self) -> Option<Property<'f, 'a>> {
        self.stored
    }

    /// True when the parameter's value record has keyframes ([`EffectParam::keys`]); then
    /// [`EffectParam::stored_value`] is `None` and [`EffectParam::value`] is not the value.
    pub fn is_animated(&self) -> bool {
        self.stored.is_some_and(|p| p.is_animated())
    }

    /// The keyframes of an animated parameter: plain 48-byte records for sliders (363), 152-byte
    /// colour records (364) and 104-byte spatial records for points (382), with the values in the
    /// stored units of [`EffectParam::stored_value`].
    pub fn keys(&self) -> Option<KeyList<'f>> {
        self.stored?.keys()
    }

    /// The value in the value record: float and fixed sliders, angles, checkboxes, points,
    /// colours, popups and layer controls. A record counts when its `tdsb` is 1 and it has no
    /// keyframes; a layer control reads the `tdpi` chunk; a button has no value.
    pub fn stored_value(&self) -> Option<ParamValue> {
        let p = self.stored?;
        if self.kind == ParamKind::Layer {
            return be_u32(p.chunk().child(b"tdpi")?.bytes()?, 0).map(ParamValue::Layer);
        }
        if p.flags() != Some(1) || p.is_animated() {
            return None;
        }
        let v = p.static_value()?;
        match self.kind {
            ParamKind::FloatSlider | ParamKind::FixedSlider | ParamKind::Angle | ParamKind::IntSlider => v.first().copied().map(ParamValue::Number),
            ParamKind::Checkbox => v.first().map(|x| ParamValue::Bool(*x != 0.0)),
            ParamKind::Point => {
                let [x, y] = *v.as_slice().first_chunk::<2>()?;
                Some(ParamValue::Point([x, y]))
            }
            ParamKind::Popup => {
                let index = v.first().copied()?;
                (0.0..=f64::from(u32::MAX)).contains(&index).then(|| ParamValue::Index(index.round() as u32))
            }
            ParamKind::Color => {
                let [a, r, g, b] = *v.as_slice().first_chunk::<4>()?;
                Some(ParamValue::Argb([a / 255.0, r / 255.0, g / 255.0, b / 255.0]))
            }
            _ => None,
        }
    }

    /// The default stored in `pard` at byte 56: a float slider's `f64`, a fixed slider's or angle's
    /// 16.16 value, a colour's ARGB bytes, a popup's item index, a point's two 16.16 fractions of
    /// the layer size (Point Control 0.5, 0.5: the layer centre, also on a 100x100 layer;
    /// Gradient Ramp Start 0.5, 0 and End 0.5, 1), an integer slider's `i32` (Iterations 3, 412),
    /// a checkbox's `u32` (1 = on: Repeat Edge Pixels of Fast Box Blur and Gaussian Blur, read back
    /// on, 412, 417; 0 = off: Shadow Only 420, Bypass Linear Light Conversion 389); other types:
    /// `None`.
    ///
    /// It is the default and not the current value: Gaussian Blur with Blurriness 40 still has 25
    /// here and a value record with 40 (After Effects itself reads a fresh Gaussian Blur back as 25).
    /// Exception: selecting a popup item also writes the selection to this slot. So a popup whose
    /// definition was borrowed from the project copy ([`EffectParam::borrowed_definition`]) has no
    /// reliable default here (the slot holds the copied instance's selection): `None`.
    pub fn pard_default(&self) -> Option<ParamValue> {
        match self.kind {
            ParamKind::Popup if self.borrowed_definition => None,
            ParamKind::FloatSlider => be_f64(self.pard, 56).map(ParamValue::Number),
            ParamKind::FixedSlider | ParamKind::Angle => be_i32(self.pard, 56).map(|v| ParamValue::Number(f64::from(v) / 65536.0)),
            ParamKind::IntSlider => be_i32(self.pard, 56).map(|v| ParamValue::Number(f64::from(v))),
            ParamKind::Checkbox => be_u32(self.pard, 56).map(|v| ParamValue::Bool(v != 0)),
            ParamKind::Popup => be_u32(self.pard, 56).map(ParamValue::Index),
            ParamKind::Point => {
                let p = [f64::from(be_i32(self.pard, 56)?) / 65536.0, f64::from(be_i32(self.pard, 60)?) / 65536.0];
                // A fresh instance stores fractions of the layer (0.5, 0.5); After Effects' save of
                // a project it opened stores percent (50, 50: the re-saves of export tests e1 and
                // e2, where Gradient Ramp's Start and End and every other point default came back
                // as 50 / 0 and 50 / 100). Point
                // defaults lie inside the layer, so a component beyond 1 tells percent.
                let percent = p.iter().any(|c| c.abs() > 1.0 + 1e-9);
                Some(ParamValue::Point(if percent { p.map(|c| c / 100.0) } else { p }))
            }
            ParamKind::Color => {
                let [a, r, g, b] = array::<4>(self.pard, 56)?.map(|c| f64::from(c) / 255.0);
                Some(ParamValue::Argb([a, r, g, b]))
            }
            _ => None,
        }
    }

    /// True when the parameter is at its default: it has no value record (a record exists exactly
    /// when the value differs from the `pard` default; setting a value back to the default removes
    /// the record again), or the record equals the default. `None` when that cannot be decided
    /// (unknown parameter type).
    pub fn is_default(&self) -> Option<bool> {
        match (self.stored, self.kind) {
            // A Layer Control always has its record, and a popup selection overwrites the default
            // in `pard`. Buttons and group markers hold no value at all.
            (_, ParamKind::Layer | ParamKind::Button | ParamKind::GroupStart | ParamKind::GroupEnd) => None,
            (None, _) => Some(true),
            (Some(_), ParamKind::Popup) => Some(false),
            (Some(_), _) => {
                let stored = self.stored_value()?;
                // A checkbox's default is in `pard` too (on for Repeat Edge Pixels, 412).
                Some(self.pard_default().is_some_and(|d| d == stored))
            }
        }
    }

    /// A point control in pixels of a layer of this size (the stored point is a fraction of the
    /// layer size; the default is the layer centre). `None` for other parameter types.
    pub fn point_px(&self, layer_width: f64, layer_height: f64) -> Option<[f64; 2]> {
        match self.value()? {
            ParamValue::Point([x, y]) if self.kind == ParamKind::Point => Some([x * layer_width, y * layer_height]),
            _ => None,
        }
    }

    /// For a Layer Control: what the stored layer id means for the layer that owns the effect.
    /// `None` for other types and for the implicit first parameter of an effect.
    pub fn layer_ref(&self, owner_layer_id: u32) -> Option<LayerRef> {
        if self.kind != ParamKind::Layer || self.index == 0 {
            return None;
        }
        match self.stored_value()? {
            ParamValue::Layer(0) => Some(LayerRef::Nothing),
            ParamValue::Layer(id) if id == owner_layer_id => Some(LayerRef::This),
            ParamValue::Layer(id) => Some(LayerRef::Layer(id)),
            _ => None,
        }
    }

    /// The largest valid value of a slider, from `pard`: a fixed slider's 16.16 at byte 128 (Tint
    /// Amount to Tint 100, Gradient Ramp Ramp Scatter 512 and Blend With Original 1.0, Drop Shadow
    /// Opacity 255), a float slider's `f32` at byte 108 (Blurriness 30000, Exposure 100, Exposure's
    /// Gamma Correction 10). The stored scale of a
    /// slider differs per parameter (Amount 37.5 % is stored 37.5, Blend 25 % is stored 0.25), so
    /// this is what tells a reader the scale. `None` for other types.
    pub fn valid_max(&self) -> Option<f64> {
        match self.kind {
            ParamKind::FixedSlider => be_i32(self.pard, 128).map(|v| f64::from(v) / 65536.0),
            ParamKind::FloatSlider => be_f32(self.pard, 108).map(f64::from),
            ParamKind::IntSlider => be_i32(self.pard, 128).map(f64::from),
            _ => None,
        }
    }

    /// The smallest valid value of a float slider, the `f32` at byte 104 of `pard` (Exposure
    /// -100, Offset -2, Gamma Correction 0.1: 389). `None` for other types (a fixed slider's
    /// minimum is not decoded).
    pub fn valid_min(&self) -> Option<f64> {
        match self.kind {
            ParamKind::FloatSlider => be_f32(self.pard, 104).map(f64::from),
            _ => None,
        }
    }

    /// The range of a float slider's slider (what the control's drag covers, inside the valid
    /// range): the `f32` pair at bytes 112 and 116 of `pard` (Exposure -4..4, Offset -0.5..0.5,
    /// Gamma Correction 0.1..10: 389). Each value record repeats it as `tdum` / `tduM`. `None`
    /// for other types.
    pub fn slider_range(&self) -> Option<(f64, f64)> {
        match self.kind {
            ParamKind::FloatSlider => Some((f64::from(be_f32(self.pard, 112)?), f64::from(be_f32(self.pard, 116)?))),
            _ => None,
        }
    }

    /// For a popup: the number of items (`u16` at byte 60 of `pard`).
    pub fn popup_item_count(&self) -> Option<u16> {
        if self.kind != ParamKind::Popup {
            return None;
        }
        be_u16(self.pard, 60)
    }

    /// For a popup: the selected item index (1-based): the stored selection, else the default.
    pub fn popup_selected_index(&self) -> Option<u32> {
        match self.value()? {
            ParamValue::Index(i) if self.kind == ParamKind::Popup => Some(i),
            _ => None,
        }
    }

    /// For a popup: the text of the selected item.
    pub fn popup_selected_item(&self) -> Option<&str> {
        let index = usize::try_from(self.popup_selected_index()?).ok()?.checked_sub(1)?;
        self.items.as_ref()?.get(index).map(String::as_str)
    }

    /// The effective value: the stored value when there is one, else the `pard` default (absent =
    /// default); a checkbox without a record is off.
    pub fn value(&self) -> Option<ParamValue> {
        self.stored_value().or_else(|| self.pard_default()).or(match self.kind {
            ParamKind::Checkbox if self.stored.is_none() => Some(ParamValue::Bool(false)),
            _ => None,
        })
    }
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The effects on the layer, in stack order (`ADBE Effect Parade`).
    pub fn effects(&self) -> Vec<Effect<'f, 'a>> {
        let Some(root) = self.properties() else { return Vec::new() };
        let Some(PropNode::Group(parade)) = root.get("ADBE Effect Parade") else { return Vec::new() };
        parade
            .entries()
            .into_iter()
            .filter_map(|e| match e.node {
                PropNode::Other(chunk) if chunk.is_list(b"sspc") => Some(Effect { match_name: e.match_name, chunk }),
                _ => None,
            })
            .collect()
    }
}
