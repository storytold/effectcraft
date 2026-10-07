//! The property tree: groups and properties with stable uids and match ids, addressed by paths.

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};

pub type Uid = u64;

/// How a property is presented and edited.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ParamUi {
    /// Plain number with an optional valid range and a (narrower) slider range.
    #[default]
    Number,
    Slider {
        min: f64,
        max: f64,
        slider_min: f64,
        slider_max: f64,
        decimals: u8,
    },
    Percent,
    /// Angle in degrees, shown as `revolutions x +degrees`.
    Angle,
    Pixels,
    Point,
    Point3,
    Color,
    Checkbox,
    Popup {
        options: Vec<String>,
    },
    Layer,
    /// A mask of the effect's own layer by 1-based index, 0 = none (a scalar value): an effect's
    /// "Path" popup.
    Mask,
    Path,
    Text,
    Gradient,
    /// Not shown in Effect Controls / the timeline (internal data).
    Hidden,
}

/// Expression attached to a property.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Expression {
    pub text: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Property {
    pub uid: Uid,
    /// Stable id within its parent kind (`position`, `blurriness`…).
    #[serde(rename = "match")]
    pub match_id: String,
    pub name: String,
    /// Static value (used when there are no keyframes).
    pub value: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<Keyframe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr: Option<Expression>,
    #[serde(default)]
    pub ui: ParamUi,
    /// Motion-path semantics (Position, Anchor Point, points).
    #[serde(default)]
    pub spatial: bool,
    /// Only hold keyframes (Source Text, checkboxes, popups).
    #[serde(default)]
    pub hold_only: bool,
    /// Valid only on 3D layers (Orientation, X/Y Rotation…).
    #[serde(default)]
    pub three_d_only: bool,
    /// Valid only on 2D layers.
    #[serde(default)]
    pub two_d_only: bool,
    /// Dimensions shown/edited (2 for 2D position etc.); 0 = all.
    #[serde(default)]
    pub shown_dims: u8,
    /// Can't be keyframed (header-like values, e.g. Mask Mode). Shown without a stopwatch.
    #[serde(default)]
    pub static_only: bool,
}

impl Property {
    pub fn new(uid: Uid, match_id: &str, name: &str, value: Value) -> Property {
        let hold_only = !value.interpolates();
        Property {
            uid,
            match_id: match_id.into(),
            name: name.into(),
            value,
            keys: vec![],
            expr: None,
            ui: ParamUi::Number,
            spatial: false,
            hold_only,
            three_d_only: false,
            two_d_only: false,
            shown_dims: 0,
            static_only: false,
        }
    }
    pub fn with_ui(mut self, ui: ParamUi) -> Property {
        self.ui = ui;
        self
    }
    pub fn spatial(mut self) -> Property {
        self.spatial = true;
        self
    }
    pub fn is_animated(&self) -> bool {
        !self.keys.is_empty()
    }
    pub fn has_expression(&self) -> bool {
        self.expr.as_ref().is_some_and(|e| e.enabled && !e.text.trim().is_empty())
    }
    /// Keyframed value at `t` (no expression).
    pub fn value_at(&self, t: Tick) -> Value {
        if self.keys.is_empty() {
            return self.value.clone();
        }
        effectcraft_keyframe::evaluate(&self.keys, t, self.spatial).unwrap_or_else(|| self.value.clone())
    }
    /// Set the value at `t`: adds/replaces a key when animated, else sets the static value.
    pub fn set_value_at(&mut self, t: Tick, v: Value) {
        if self.keys.is_empty() {
            self.value = v;
        } else {
            let mut k = Keyframe::new(t, v);
            if self.hold_only {
                k = k.hold();
            }
            effectcraft_keyframe::set_key(&mut self.keys, k);
        }
    }
    /// Toggle the stopwatch: on → one key at `t` with the current value; off → keep value at `t`.
    pub fn set_animated(&mut self, on: bool, t: Tick) {
        if on && self.keys.is_empty() {
            let mut k = Keyframe::new(t, self.value.clone());
            if self.hold_only {
                k = k.hold();
            }
            self.keys.push(k);
        } else if !on && !self.keys.is_empty() {
            self.value = self.value_at(t);
            self.keys.clear();
        }
    }
}

/// What a group represents, with its non-animatable header data.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum GroupKind {
    #[default]
    Plain,
    /// An instance in an indexed list (masks, effects, shape contents, animators): these can be
    /// renamed, reordered, duplicated and deleted.
    Indexed,
    Mask {
        mode: MaskMode,
        inverted: bool,
        color: [u8; 3],
        locked: bool,
        /// Layer ▸ Mask ▸ Motion Blur.
        #[serde(default)]
        motion_blur: MaskMotionBlur,
        /// Layer ▸ Mask ▸ Feather Falloff.
        #[serde(default)]
        feather_falloff: FeatherFalloff,
        /// Layer ▸ Mask and Shape Path ▸ RotoBezier: tangents follow the vertices automatically.
        #[serde(default)]
        roto_bezier: bool,
    },
    Effect {
        /// Effect spec id, e.g. `ec.blur.gaussian`.
        effect: String,
    },
    /// A motion tracker (Motion Trackers ▸ Tracker n) with its Tracker panel settings.
    Tracker { settings: Box<crate::tracking::TrackerSettings> },
    /// The Essential Properties of a precomp layer (master properties): one child per control
    /// of the nested comp's Essential Graphics; `overridden` lists the children (uids) whose
    /// values this instance overrides.
    Essential {
        #[serde(default)]
        overridden: Vec<Uid>,
    },
}

/// Mask motion blur (sub-frame samples of an animated mask path).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MaskMotionBlur {
    /// Blur when the layer's Motion Blur switch is on.
    #[default]
    SameAsLayer,
    On,
    Off,
}

/// How a mask's feather ramps: Smooth (Gaussian) or Linear.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeatherFalloff {
    #[default]
    Smooth,
    Linear,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MaskMode {
    None,
    #[default]
    Add,
    Subtract,
    Intersect,
    Lighten,
    Darken,
    Difference,
}

impl MaskMode {
    pub const ALL: [MaskMode; 7] =
        [MaskMode::None, MaskMode::Add, MaskMode::Subtract, MaskMode::Intersect, MaskMode::Lighten, MaskMode::Darken, MaskMode::Difference];
    pub fn label(self) -> &'static str {
        match self {
            MaskMode::None => "None",
            MaskMode::Add => "Add",
            MaskMode::Subtract => "Subtract",
            MaskMode::Intersect => "Intersect",
            MaskMode::Lighten => "Lighten",
            MaskMode::Darken => "Darken",
            MaskMode::Difference => "Difference",
        }
    }
    pub fn from_name(s: &str) -> Option<MaskMode> {
        MaskMode::ALL.into_iter().find(|m| m.label().eq_ignore_ascii_case(s))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropGroup {
    pub uid: Uid,
    #[serde(rename = "match")]
    pub match_id: String,
    pub name: String,
    #[serde(default)]
    pub kind: GroupKind,
    /// Presentation label of an effect instance; independent of its layer label.
    #[serde(default = "no_effect_label", skip_serializing_if = "effect_label_is_none")]
    pub effect_label: Label,
    /// fx switch for effects, eye for shape groups, etc.
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub children: Vec<Node>,
}

fn no_effect_label() -> Label {
    Label::None
}

fn effect_label_is_none(label: &Label) -> bool {
    *label == Label::None
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node")]
pub enum Node {
    Prop(Property),
    Group(PropGroup),
}

impl Node {
    pub fn uid(&self) -> Uid {
        match self {
            Node::Prop(p) => p.uid,
            Node::Group(g) => g.uid,
        }
    }
    pub fn match_id(&self) -> &str {
        match self {
            Node::Prop(p) => &p.match_id,
            Node::Group(g) => &g.match_id,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Node::Prop(p) => &p.name,
            Node::Group(g) => &g.name,
        }
    }
    pub fn as_prop(&self) -> Option<&Property> {
        if let Node::Prop(p) = self { Some(p) } else { None }
    }
    pub fn as_group(&self) -> Option<&PropGroup> {
        if let Node::Group(g) = self { Some(g) } else { None }
    }
    pub fn as_group_mut(&mut self) -> Option<&mut PropGroup> {
        if let Node::Group(g) = self { Some(g) } else { None }
    }
}

/// One step of a property path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seg {
    Uid(Uid),
    /// Match id and 1-based occurrence among siblings with that match id.
    Match(String, usize),
    /// 1-based child index.
    Index(usize),
}

/// Parse `transform/position`, `effects/#1/blurriness`, `effects/@57/blurriness`,
/// `contents/rect#2/size`.
/// `@57` (a whole path that is one uid).
fn bare_uid(path: &str) -> Option<Uid> {
    path.trim().strip_prefix('@')?.parse().ok()
}

pub fn parse_path(s: &str) -> Vec<Seg> {
    s.split(['/', '.'])
        .filter(|x| !x.is_empty())
        .map(|seg| {
            if let Some(u) = seg.strip_prefix('@').and_then(|u| u.parse().ok()) {
                Seg::Uid(u)
            } else if let Some(i) = seg.strip_prefix('#').and_then(|u| u.parse().ok()) {
                Seg::Index(i)
            } else if let Some((m, n)) = seg.rsplit_once('#')
                && let Ok(n) = n.parse()
            {
                Seg::Match(m.to_string(), n)
            } else {
                Seg::Match(seg.to_string(), 1)
            }
        })
        .collect()
}

impl PropGroup {
    pub fn new(uid: Uid, match_id: &str, name: &str) -> PropGroup {
        PropGroup { uid, match_id: match_id.into(), name: name.into(), kind: GroupKind::Plain, effect_label: Label::None, enabled: true, children: vec![] }
    }
    pub fn with(mut self, n: impl Into<Node>) -> PropGroup {
        self.children.push(n.into());
        self
    }

    fn child_index(&self, seg: &Seg) -> Option<usize> {
        match seg {
            Seg::Uid(u) => self.children.iter().position(|c| c.uid() == *u),
            Seg::Index(i) => (*i >= 1 && *i <= self.children.len()).then(|| i - 1),
            Seg::Match(m, n) => {
                self.children.iter().enumerate().filter(|(_, c)| c.match_id() == m || c.name().eq_ignore_ascii_case(m)).nth(n.saturating_sub(1)).map(|(i, _)| i)
            }
        }
    }

    pub fn node(&self, path: &str) -> Option<&Node> {
        let segs = parse_path(path);
        let (last, init) = segs.split_last()?;
        let mut g = self;
        for s in init {
            g = g.children.get(g.child_index(s)?)?.as_group()?;
        }
        g.children.get(g.child_index(last)?)
    }
    pub fn node_mut(&mut self, path: &str) -> Option<&mut Node> {
        let segs = parse_path(path);
        let (last, init) = segs.split_last()?;
        let mut g = self;
        for s in init {
            let i = g.child_index(s)?;
            g = g.children.get_mut(i)?.as_group_mut()?;
        }
        let i = g.child_index(last)?;
        g.children.get_mut(i)
    }
    /// A property by path; a bare `@uid` finds the property anywhere below this group.
    pub fn prop(&self, path: &str) -> Option<&Property> {
        if let Some(u) = bare_uid(path) {
            return self.find(u);
        }
        self.node(path)?.as_prop()
    }
    pub fn prop_mut(&mut self, path: &str) -> Option<&mut Property> {
        if let Some(u) = bare_uid(path) {
            return self.find_mut(u);
        }
        match self.node_mut(path)? {
            Node::Prop(p) => Some(p),
            _ => None,
        }
    }
    pub fn group(&self, path: &str) -> Option<&PropGroup> {
        self.node(path)?.as_group()
    }
    pub fn group_mut(&mut self, path: &str) -> Option<&mut PropGroup> {
        self.node_mut(path)?.as_group_mut()
    }
    /// Direct child property by match id.
    pub fn get(&self, match_id: &str) -> Option<&Property> {
        self.children.iter().find_map(|c| c.as_prop().filter(|p| p.match_id == match_id))
    }
    pub fn get_mut(&mut self, match_id: &str) -> Option<&mut Property> {
        self.children.iter_mut().find_map(|c| match c {
            Node::Prop(p) if p.match_id == match_id => Some(p),
            _ => None,
        })
    }
    pub fn sub(&self, match_id: &str) -> Option<&PropGroup> {
        self.children.iter().find_map(|c| c.as_group().filter(|g| g.match_id == match_id))
    }
    pub fn sub_mut(&mut self, match_id: &str) -> Option<&mut PropGroup> {
        self.children.iter_mut().find_map(|c| match c {
            Node::Group(g) if g.match_id == match_id => Some(g),
            _ => None,
        })
    }
    pub fn groups(&self) -> impl Iterator<Item = &PropGroup> {
        self.children.iter().filter_map(Node::as_group)
    }
    pub fn props(&self) -> impl Iterator<Item = &Property> {
        self.children.iter().filter_map(Node::as_prop)
    }

    /// Visit every property depth-first with its uid-path (`@g/@g/@p`).
    pub fn walk<'a>(&'a self, prefix: &str, f: &mut dyn FnMut(&str, &'a Property)) {
        for c in &self.children {
            let p = format!("{prefix}@{}", c.uid());
            match c {
                Node::Prop(pr) => f(&p, pr),
                Node::Group(g) => g.walk(&format!("{p}/"), f),
            }
        }
    }
    pub fn walk_mut(&mut self, f: &mut dyn FnMut(&mut Property)) {
        for c in &mut self.children {
            match c {
                Node::Prop(p) => f(p),
                Node::Group(g) => g.walk_mut(f),
            }
        }
    }
    /// Uid path (`@1/@2/@3`) of the node with `uid`.
    pub fn path_of(&self, uid: Uid) -> Option<String> {
        for c in &self.children {
            if c.uid() == uid {
                return Some(format!("@{uid}"));
            }
            if let Node::Group(g) = c
                && let Some(p) = g.path_of(uid)
            {
                return Some(format!("@{}/{p}", g.uid));
            }
        }
        None
    }
    /// Match-id path (`transform/position`, `effects/ec.blur.gaussian#2/blurriness`) of the node
    /// with `uid`: portable between layers with the same structure (keyframe paste, pick-whip).
    pub fn match_path_of(&self, uid: Uid) -> Option<String> {
        for (i, c) in self.children.iter().enumerate() {
            let n = self.children[..i].iter().filter(|o| o.match_id() == c.match_id()).count() + 1;
            let seg = if n == 1 { c.match_id().to_string() } else { format!("{}#{n}", c.match_id()) };
            if c.uid() == uid {
                return Some(seg);
            }
            if let Node::Group(g) = c
                && let Some(p) = g.match_path_of(uid)
            {
                return Some(format!("{seg}/{p}"));
            }
        }
        None
    }
    /// Chain of nodes from this group's children down to `uid` (inclusive).
    pub fn node_chain(&self, uid: Uid) -> Option<Vec<&Node>> {
        for c in &self.children {
            if c.uid() == uid {
                return Some(vec![c]);
            }
            if let Node::Group(g) = c
                && let Some(mut rest) = g.node_chain(uid)
            {
                rest.insert(0, c);
                return Some(rest);
            }
        }
        None
    }
    /// Human path (`Transform/Position`) of the node with `uid`.
    pub fn name_path_of(&self, uid: Uid) -> Option<String> {
        for c in &self.children {
            if c.uid() == uid {
                return Some(c.name().to_string());
            }
            if let Node::Group(g) = c
                && let Some(p) = g.name_path_of(uid)
            {
                return Some(format!("{}/{p}", g.name));
            }
        }
        None
    }
    /// Find a property anywhere in the tree by uid.
    pub fn find(&self, uid: Uid) -> Option<&Property> {
        for c in &self.children {
            match c {
                Node::Prop(p) if p.uid == uid => return Some(p),
                Node::Group(g) => {
                    if let Some(p) = g.find(uid) {
                        return Some(p);
                    }
                }
                _ => {}
            }
        }
        None
    }
    pub fn find_mut(&mut self, uid: Uid) -> Option<&mut Property> {
        for c in &mut self.children {
            match c {
                Node::Prop(p) if p.uid == uid => return Some(p),
                Node::Group(g) => {
                    if let Some(p) = g.find_mut(uid) {
                        return Some(p);
                    }
                }
                _ => {}
            }
        }
        None
    }
    pub fn find_group(&self, uid: Uid) -> Option<&PropGroup> {
        if self.uid == uid {
            return Some(self);
        }
        self.groups().find_map(|g| g.find_group(uid))
    }
    pub fn find_group_mut(&mut self, uid: Uid) -> Option<&mut PropGroup> {
        if self.uid == uid {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| match c {
            Node::Group(g) => g.find_group_mut(uid),
            _ => None,
        })
    }
    /// Parent group of the node with `uid`.
    pub fn parent_of(&self, uid: Uid) -> Option<&PropGroup> {
        if self.children.iter().any(|c| c.uid() == uid) {
            return Some(self);
        }
        self.groups().find_map(|g| g.parent_of(uid))
    }
    /// Parent group of the node with `uid`.
    pub fn parent_of_mut(&mut self, uid: Uid) -> Option<&mut PropGroup> {
        if self.children.iter().any(|c| c.uid() == uid) {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| match c {
            Node::Group(g) => g.parent_of_mut(uid),
            _ => None,
        })
    }
    /// Any property in this subtree animated (keys or expression).
    pub fn any_animated(&self) -> bool {
        self.children.iter().any(|c| match c {
            Node::Prop(p) => p.is_animated() || p.has_expression(),
            Node::Group(g) => g.any_animated(),
        })
    }
    /// Re-assign fresh uids (after duplicating a subtree).
    pub fn reassign_uids(&mut self, next: &mut Uid) {
        *next += 1;
        self.uid = *next;
        for c in &mut self.children {
            match c {
                Node::Prop(p) => {
                    *next += 1;
                    p.uid = *next;
                }
                Node::Group(g) => g.reassign_uids(next),
            }
        }
    }
    pub fn max_uid(&self) -> Uid {
        self.children
            .iter()
            .map(|c| match c {
                Node::Prop(p) => p.uid,
                Node::Group(g) => g.max_uid(),
            })
            .fold(self.uid, Uid::max)
    }
}

impl From<Property> for Node {
    fn from(p: Property) -> Node {
        Node::Prop(p)
    }
}
impl From<PropGroup> for Node {
    fn from(g: PropGroup) -> Node {
        Node::Group(g)
    }
}
