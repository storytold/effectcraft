//! Wavefront OBJ geometry and MTL materials, read with `craft-mesh` (`craft_mesh::obj`).
//!
//! The material library and the textures it names are read through the caller's resolver: a
//! texture is looked up by the name as written (a name containing spaces is one name), then by
//! its file name (an absolute path from another machine, #1274), then by the final token (the
//! options convention, `-bm 1 file.png`). Faces without normals get smooth normals within a
//! smoothing group and flat ones where smoothing is off.

use crate::scene::{MissingNormals, error, limits, to_model};
use crate::{Model, Result};

pub use craft_mesh::srgb_to_linear;

/// A material reference resolved against the model's folder: the name as written, then its file
/// name, then the final token.
fn resolve_reference(resolve: &dyn Fn(&str) -> Option<Vec<u8>>, name: &str) -> Option<Vec<u8>> {
    let mut tried: Vec<&str> = vec![];
    for candidate in [name, craft_mesh::obj::sibling_name(name).unwrap_or(name), name.split_whitespace().last().unwrap_or(name)] {
        if tried.contains(&candidate) {
            continue;
        }
        tried.push(candidate);
        if let Some(b) = resolve(candidate) {
            return Some(b);
        }
    }
    None
}

/// Parse OBJ text (and its MTL libraries and textures through `resolve`).
pub fn parse(bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Model> {
    // `craft-mesh` splits `mtllib` names and tries each complete library path itself. Applying
    // texture-option fallback here first can mistake the last library for the whole directive.
    let mut library = |name: &str| resolve(name);
    let scene = craft_mesh::obj::read(bytes, &limits(), &mut library).map_err(error)?;
    to_model(scene, MissingNormals::BySmoothing, &|im| resolve_reference(resolve, im.uri.as_deref()?.trim()))
}
