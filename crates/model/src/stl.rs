//! STL (stereolithography) meshes, binary and ASCII, read with `craft-mesh`
//! (`craft_mesh::stl`): every facet keeps its own three vertices and its face normal, and binary
//! colour extensions become one plain material per colour.
//!
//! STL files are Z-up (3D printing and CAD); they are turned to the +Y-up convention of
//! [`crate::Model`] (`(x, y, z)` → `(x, z, −y)`, a proper rotation, so winding is kept).

use crate::scene::{MissingNormals, error, limits, to_model};
use crate::{Model, Result};

/// The Z-up → Y-up turn.
fn y_up(v: [f64; 3]) -> [f64; 3] {
    [v[0], v[2], -v[1]]
}

/// Parse an STL file (binary or ASCII, detected from the contents).
pub fn parse(bytes: &[u8]) -> Result<Model> {
    let mut scene = craft_mesh::stl::read(bytes, &limits()).map_err(error)?;
    for m in &mut scene.meshes {
        m.positions.iter_mut().chain(m.normals.iter_mut()).for_each(|v| *v = y_up(*v));
    }
    to_model(scene, MissingNormals::Flat, &|_| None)
}
