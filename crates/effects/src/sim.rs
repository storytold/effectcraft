//! Simulation effects (particles, physics, liquids): CC Rainfall, CC Snowfall, CC Bubbles,
//! CC Drizzle, CC Star Burst, CC Particle World, CC Particle Systems II and CC Mr. Mercury.
//!
//! Written from the public descriptions of the effects' behaviour. Weather/bubble/star effects
//! are closed-form functions of time (every frame independent); particle systems are stepped
//! from layer time 0 at a fixed rate through [`SimCache`], so any frame can be rendered
//! directly and equals the frame reached by scrubbing.
//!
//! Shared here (also used by `sim2`): a band-bucketed rasteriser for many small items
//! ([`raster`]), particle sprites ([`Sprite`]) and transfer-mode compositing ([`combine`]).

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::generate::value_noise;
use crate::util::{SimCache, hash1, params_key, params_key_except, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

mod common;
mod engine;
mod particle_fx;
mod registry;
mod sprites;
mod weather;

pub(crate) use common::{h, hs, pct, pt, spec};
pub use engine::particle_state;
#[cfg(test)]
pub(crate) use engine::{PState, PW_ANIMS, Phys, simulate};
#[cfg(test)]
use engine::{PW_EFFECT_CAMERA, sim_key};
pub(crate) use engine::{SPS, steps_at};
pub use particle_fx::{Blob, mercury_blobs};
pub use registry::{specs, sprite_plan};
pub use sprites::{Acc, Post, Shape, Sprite, SpritePlan, Tint};
pub(crate) use sprites::{combine, raster, splat};
pub use weather::{Bubble, bubble_list, drizzle_drops};

#[cfg(test)]
mod tests;
