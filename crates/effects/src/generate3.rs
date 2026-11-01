//! Generate effects, batch 3: Fractal (Mandelbrot/Julia), mask-driven Scribble and Stroke,
//! Vegas (animated segments along image contours or masks), Write-on, Paint Bucket (flood
//! fill), Eyedropper Fill, CC Glue Gun, CC Threads, and the audio visualisers Audio Spectrum
//! and Audio Waveform (fed by the host's audio).
//!
//! Also home of the shared anti-aliased segment rasteriser ([`raster_segs`]) used by the text
//! effects.

use std::f64::consts::PI;

use effectcraft_color::{hsl_to_rgb, luminance, rgb_to_hsl};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::util::{Plane, gauss_plane, hash1, layer_or_self, morph_frac, poly_length, premul, smoothstep, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, Params, col, num, p, popup, slider};

mod audio;
mod common;
mod fill;
mod fractal;
mod paths;
mod raster;
mod registry;

#[cfg(test)]
pub(crate) use audio::fft;
pub use audio::{MarksPlan, marks_plan};
pub use fill::{eyedropper_color, paint_bucket_region};
#[cfg(test)]
use paths::contours;
pub use paths::{Coverage, PaintPlan, paint_plan, plan_reads_pixels, scribble_style};
pub use raster::{Seg, poly_segs};
pub(crate) use raster::{mask_px, raster_segs};
#[cfg(test)]
pub(crate) use raster::{raster_convex, stroke_outline};
pub use registry::specs;

#[cfg(test)]
mod tests;
