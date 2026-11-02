//! Simulation effects, part 2: layer-shattering and surface simulations — CC Ball Action,
//! CC Pixel Polly, CC Scatterize, Card Dance, Shatter (planar pieces in perspective), Caustics,
//! Wave World (stepped 2D wave equation) and Foam (stepped bubble sim).
//!
//! Written from the public descriptions of the effects' behaviour. Pieces are textured convex
//! polygons placed in 3D and drawn through a per-piece inverse homography, far to near.

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::card3d::{Lighting, Proj};
use crate::generate::value_noise;
use crate::sim::{Acc, Post, Shape, Sprite, SpritePlan, h, hs, pct, pt, raster, spec, splat};
use crate::util::{Plane, SimCache, gauss_plane, layer_or_self, params_key, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

mod foam;
mod pieces;
mod pixels;
mod registry;
mod shatter;
mod surfaces;

pub(crate) use foam::foam_plan;
#[cfg(test)]
use foam::foam_pop_age;
pub use foam::{FoamDisc, FoamEnv, FoamPlan, foam_full_plan};
#[cfg(test)]
use pieces::Cam;
#[cfg(test)]
pub(crate) use pieces::draw_pieces;
pub use pieces::{Piece, PiecePlan, PieceTex, piece_plan};
pub(crate) use pieces::{rot_axis, sort_far_first};
pub use pixels::{PixelPlan, pixel_plan};
pub use registry::specs;
#[cfg(test)]
use shatter::SHATTER_PATTERNS;
pub use surfaces::{CausticsSetup, WavePlan, Waves, caustics_setup, wave_world_plan};

#[cfg(test)]
mod tests;
