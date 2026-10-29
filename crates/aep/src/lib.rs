//! After Effects projects: the `.aep` / `.aepx` container, import into an EffectCraft project and
//! export back to `.aep`, in one crate (like `effectcraft-lottie`).
//!
//! - [`container`]: the RIFX `Egg!` chunk tree (lossless, byte-identical round trip) and its typed,
//!   read-only view. Its API is re-exported at the crate root.
//! - [`import`]: maps the typed view into an EffectCraft project and reports what it drops.
//! - [`export`]: writes an EffectCraft project as an `.aep`, every chunk built in code.
//!
//! See `docs/aep-format.md`, `docs/aep-import.md` and `docs/aep-export.md`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod container;
pub mod export;
pub mod import;

pub use container::{
    AepError, AepFile, AutoKern, BlendMode, Body, CURVE_IDENTITY, CameraPosition, Caps, Chunk, Comp, CompHeader, CueType, CurvesData, DEFAULT_CAMERA_ZOOM,
    DefinitionSource, Effect, EffectParam, ExpressionFlags, FontBaseline, Footage, GradientType, HueSaturationData, Interpolation, Item, ItemKind,
    Justification, KernRun, KeyEase, KeyLayout, KeyList, Keyframe, KeyframeKind, LIST, Layer, LayerHeader, LayerRef, LayerType, LevelsData, LightType, Limits,
    ListHeader, Marker, Mask, MaskAtom, MaskMode, MatteMode, MeshData, NESTED_IDS, Orientation, OrientationKey, ParagraphRun, ParamKind, ParamValue,
    PathVertex, PolystarType, Project, ProjectSettings, PropEntry, PropGroup, PropNode, Property, Quality, Rational, Renderer, SequenceInfo, ShapeDetail,
    ShapeGroup, ShapeItem, ShapePath, SpatialKey, Stroke, StrokeCap, StrokeJoin, StyleRun, TextBox, TextDocument, TextValue, TimeBase, TimeDisplay, TimeRemap,
    TimeRemapKey, TrackMatte, Tracked, Transform, ValueSource, Walk, WorkingSpace, curve_lut, effect_map, is_aepx, parse, parse_any, parse_text_dict,
    parse_with_limits, read_aepx, read_aepx_with_limits, style_map, write, write_to,
};
