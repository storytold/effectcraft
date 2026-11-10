//! OCIO-style colour transforms (Color Correction ▸ OCIO CDL / Color Space / Display / File / Look
//! Transform), Color Profile Converter (Utility) and Color Stabilizer (Color Correction).
//!
//! The OCIO effects run on a **built-in minimal configuration** written from published
//! standards, not from OpenColorIO's source:
//!
//! * colour spaces are defined by primaries, white point and transfer function: ACES2065-1 (AP0)
//!   and ACEScg / ACEScct (AP1) from SMPTE ST 2065-1 and Academy S-2014-004 / S-2016-001, sRGB
//!   (IEC 61966-2-1), Rec. 709 / Rec. 2020 with the BT.1886 gamma 2.4 EOTF, Display P3 (SMPTE
//!   EG 432-1 primaries, D65, sRGB curve), their linear variants and CIE XYZ D65;
//! * the reference space is CIE XYZ D65; spaces with another white point (the ACES white,
//!   x 0.32168 y 0.33767) are adapted with the Bradford chromatic adaptation transform, as in the
//!   published ACES configurations;
//! * displays (sRGB, Rec.1886 Rec.709, Display P3, Rec.2100-PQ via SMPTE ST 2084) and views
//!   (Standard = clip-free encode, Tone Mapped = extended Reinhard roll-off, Raw);
//! * looks are ASC CDLs applied in ACEScct;
//! * CDL maths follows the ASC CDL v1.2 specification (slope, offset, power, then saturation with
//!   Rec. 709 luma weights); the "No Clamp" style mirrors power around zero;
//! * File Transform reads `.cube` (Adobe/Resolve), `.3dl` (Autodesk Lustre/Flame integer
//!   lattices), `.csp` (cineSpace, with its per-channel pre-LUT shaper), Sony Imageworks
//!   `.spi1d` / `.spi3d` / `.spimtx` and ASC `.cc` / `.ccc` / `.cdl` XML, with nearest, trilinear
//!   or tetrahedral interpolation and an inverse direction (exact for 1D/CDL/matrices, iterative
//!   for 3D lattices);
//! * Configuration ▸ Custom reads a `.ocio` file instead ([`crate::ocio_config`]); what it can't
//!   apply is reported by [`warning`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_color::ColorSpace;
use effectcraft_color::space::{invert, mul, mul_vec, rgb_to_xyz};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use rayon::prelude::*;

use crate::util::unpremul;
use crate::utility::{Lut, parse_cube};
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

mod cdl;
mod effects;
mod lut;
mod program;
mod registry;
mod space;

pub use cdl::Cdl;
pub(crate) use effects::CUSTOM_CONFIG_FX;
#[cfg(test)]
pub use effects::parse_looks;
pub use effects::{color_stabilizer_maps, warning};
#[cfg(test)]
use effects::{tonemap, tonemap_inv};
pub use lut::{FileXform, load_file};
#[cfg(test)]
pub use lut::{parse_file, parse_spi1d, parse_spi3d, parse_spimtx};
pub use program::{ColorOp, Straight, color_program};
pub use registry::specs;
#[cfg(test)]
use space::M3;
pub(crate) use space::{ACES_CG, ACES2065_1, D65, P3, REC709, REC2020, sp};
#[cfg(test)]
pub use space::{SPACES, Xform, bradford, space_by_name};
pub use space::{Space, Tf, matrix};

#[cfg(test)]
mod tests;
