//! FBX 6.x and 7.x scenes, binary and ASCII, read with `craft-mesh` (`craft_mesh::fbx`):
//! `Geometry` meshes, `Model` nodes with the SDK transform and geometric transforms, `Material`
//! (Lambert/Phong mapped onto metallic-roughness), embedded or relative `Texture` images,
//! joined by `Connections`. A root node turns the file's `GlobalSettings` axes to +Y up; units
//! stay the file's own (like OBJ). Meshes without normals get smooth ones.

use crate::scene::{MissingNormals, error, limits, mat4, to_model};
use crate::{Model, ModelError, Node, Result};

pub use craft_mesh::fbx::BINARY_MAGIC;

/// Parse an FBX file; `resolve(path)` reads texture files the file names but does not embed.
pub fn parse(bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Model> {
    let limits = limits();
    let doc = craft_mesh::fbx::read_document(bytes, &limits).map_err(error)?;
    let mut textures = |path: &str| resolve(path);
    let scene = doc.scene(&limits, &mut textures).map_err(error)?;
    if scene.meshes.is_empty() {
        return Err(ModelError::Parse("FBX file without meshes".into()));
    }
    let mut m = to_model(scene, MissingNormals::Smooth, &|_| None)?;
    // The axes node goes first: every node index moves up by one.
    for n in &mut m.nodes {
        n.children.iter_mut().for_each(|c| *c += 1);
    }
    m.skins.iter_mut().flat_map(|s| s.joints.iter_mut()).for_each(|j| *j += 1);
    m.animations.iter_mut().flat_map(|a| a.channels.iter_mut()).for_each(|c| c.node += 1);
    let roots = m.roots.iter().map(|r| r + 1).collect();
    m.nodes.insert(0, Node { name: "FBX axes".into(), matrix: Some(mat4(&doc.axes_to_y_up())), children: roots, ..Node::default() });
    m.roots = vec![0];
    Ok(m)
}

#[cfg(test)]
mod tests;
