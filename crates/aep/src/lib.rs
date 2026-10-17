//! After Effects project (`.aep`) container reader and writer.
//!
//! An `.aep` is a `RIFX` container (big-endian RIFF) with form type `Egg!`. This crate reads it
//! into a lossless chunk tree ([`AepFile`]) and writes it back byte for byte. See
//! `docs/aep-format.md` for the observed format and how it was established (clean room: only
//! files we generated ourselves, one change per file).
//!
//! [`Project::new`] opens a typed, read-only view over the tree: items, compositions
//! ([`CompHeader`]), layers ([`LayerHeader`], [`Transform`], effects, masks, text documents,
//! markers), properties, keyframes and footage. Only fields whose layout was verified against the
//! values the test files were made with are decoded; every accessor returns `None` (or an empty
//! list) on damaged data instead of failing.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod bytes;
mod camera;
mod chunk;
mod effects;
mod error;
mod footage;
mod header;
mod keys;
mod masks;
mod read;
mod shapes;
mod textdoc;
mod typed;
mod values;
mod write;

pub use camera::{CameraPosition, DEFAULT_CAMERA_ZOOM};
pub use chunk::{AepFile, Body, Chunk, LIST, NESTED_IDS, Walk};
pub use effects::{Effect, EffectParam, LayerRef, ParamKind, ParamValue};
pub use error::AepError;
pub use footage::Footage;
pub use header::{BlendMode, CompHeader, LayerHeader, LayerType, LightType, Quality, Rational, Renderer};
pub use keys::{ExpressionFlags, Interpolation, KeyEase, KeyframeKind, SpatialKey};
pub use masks::{Mask, MaskMode, PathVertex, ShapePath};
pub use read::{Limits, parse, parse_with_limits};
pub use shapes::{GradientType, PolystarType, ShapeDetail, ShapeGroup, ShapeItem, Stroke, StrokeCap, StrokeJoin};
pub use textdoc::{Caps, CueType, Justification, Marker, ParagraphRun, StyleRun, TextBox, TextDocument, TextValue, parse_text_dict};
pub use typed::{Comp, Item, ItemKind, KeyLayout, KeyList, Keyframe, Layer, ListHeader, Project, PropEntry, PropGroup, PropNode, Property, TimeBase};
pub use values::{TimeRemap, TimeRemapKey, Tracked, Transform, ValueSource};
pub use write::{write, write_to};
