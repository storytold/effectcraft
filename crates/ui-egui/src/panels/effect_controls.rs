//! Effect Controls: the selected layer's effects with all parameters, in After Effects'
//! conventions:
//!
//! - the tab's lock icon keeps the panel on the layer it shows whatever is selected next (a
//!   pick whip from another layer's property can then reach its parameters); deleting that
//!   layer or replacing the project unlocks it (see [`set_lock`]). The lock holds while the
//!   locked layer's comp is the active comp: with another comp active the panel shows that
//!   comp's selection (its edits address layers of the active comp), and the locked layer
//!   again when its comp is back. The Effect menu of a right-click on empty space applies to
//!   the selected layers, as in After Effects;
//! - effect headers with the fx switch, twirl, Reset and About…; click selects the effect
//!   (Edit ▸ Copy / Paste / Duplicate and Delete then act on it), drag reorders the stack, the
//!   context menu copies, pastes, duplicates, resets and removes;
//! - stopwatch per parameter and a keyframe navigator (◀ ◆ ▶) on animated ones;
//! - sliders and angle dials twirl down; angles read `0x+45.0°`;
//! - point parameters have the crosshair button (then click in the Composition viewer) and an
//!   on-viewer ⊕ control while their effect is selected (see [`viewer_hook`]);
//! - colour parameters have a swatch and an eyedropper that samples the composited frame;
//! - layer parameters have a layer popup and a Source / Masks / Effects & Masks popup; mask
//!   ("Path") parameters list the layer's masks;
//! - Curves has its graph editor, Levels its histogram with input/output triangles, Auto
//!   Levels a histogram; Lumetri's curves, Colorama's output cycle, Glow's colour map and
//!   Reshape's correspondence points have visual editors ([`super::fx_editors`]).

use effectcraft_engine::geom::Mat3;
use effectcraft_engine::keyframe::Value;
use effectcraft_engine::project::{GroupKind, Layer, Node, ParamUi, PropGroup, Property};
use effectcraft_engine::render::EvalCtx;
use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use super::fx_widgets as fw;
use crate::icons::{self, Icon};
use crate::state::FxPick;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

mod common;
mod editors;
mod panel;
mod rows;
mod viewer;

pub use common::{choose_files_after_apply, reveal_applied, set_lock};
pub(crate) use common::{gesture_key, scope_lock, shown_layer_id};
pub(crate) use panel::handle_disclosure_key;
pub use panel::{show, start_rename};
pub(crate) use rows::toggle_pick;
pub use viewer::viewer_hook;
