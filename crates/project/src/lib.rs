//! The EffectCraft document model.
//!
//! `Project` → items (folders, compositions, footage, solids) → `Comp` → `Layer` → property tree
//! ([`props`]). Compositions sit behind `Arc` so undo snapshots share everything that did not
//! change.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod build;
pub mod essential;
mod file_check;
pub mod limits;
pub mod props;
pub mod render_queue;
pub mod render_templates;
pub mod sequence;
pub mod styles;
pub mod tracking;

use std::collections::BTreeMap;
use std::sync::Arc;

pub use effectcraft_color::ColorSpace;
use effectcraft_color::{BlendMode, Label};
use effectcraft_time::{FrameRate, Tick};
pub use props::{Expression, FeatherFalloff, GroupKind, MaskMode, MaskMotionBlur, Node, ParamUi, PropGroup, Property, Uid, parse_path};
use serde::{Deserialize, Serialize};

pub use effectcraft_keyframe as keyframe;
pub use effectcraft_keyframe::{Keyframe, Value};

mod comp;
mod footage;
mod item;
mod layer;
mod settings;

pub use comp::{Comp, Guide, Renderer, Resolution};
pub use footage::{AlphaMode, FieldOrder, Footage, FootageKind, Proxy, Solid, SourceLayer};
pub use item::{Item, ItemId, ItemKind, LayerId};
pub use layer::{AutoOrient, FrameBlend, Layer, LayerSource, LightKind, MatteKind, PrimitiveKind, Quality, Sampling, Switches, TrackMatte};
pub use settings::{
    APP_VERSION, BitDepth, ColorEngine, CuePoint, HdrMode, Marker, ProjectError, ProjectSettings, SCHEMA_EFFECT_SPACE, SCHEMA_VERSION, SavedView,
    TimeDisplayStyle, is_newer_version, saved_by, saved_view,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub schema: u32,
    pub settings: ProjectSettings,
    pub items: BTreeMap<ItemId, Item>,
    pub next_id: u64,
    /// The Render Queue (Composition ▸ Add to Render Queue).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub render_queue: Vec<render_queue::RenderQueueItem>,
    /// Render Settings / Output Module templates and their defaults.
    #[serde(default, skip_serializing_if = "render_templates::RenderTemplates::is_default")]
    pub render_templates: render_templates::RenderTemplates,
    /// Render Queue preferences (Notify, storage overflow folders).
    #[serde(default, skip_serializing_if = "render_queue::RenderQueuePrefs::is_default")]
    pub render_prefs: render_queue::RenderQueuePrefs,
}

#[cfg(test)]
mod tests;
