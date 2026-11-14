//! glTF 2.0 import (`.gltf` with external or embedded buffers, `.glb`), read with `craft-mesh`
//! (`craft_mesh::gltf`): meshes, PBR metallic-roughness materials, the node hierarchy, skins,
//! animations, cameras and `KHR_lights_punctual` lights. External buffers and images are read
//! through the caller's resolver by their percent-decoded URI. Primitives without normals get
//! flat normals (glTF 2.0 §3.7.2.1); normal-mapped ones get tangents from their UVs.

use crate::scene::{MissingNormals, error, limits, to_model};
use crate::{Model, Result};

pub use craft_mesh::base64_decode;
pub use craft_mesh::gltf::{split_glb, uri_decode};

/// Parse a `.gltf` (JSON) or `.glb` file; `resolve(uri)` reads external buffers and images.
pub fn parse(bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Model> {
    let mut buffers = |uri: &str| resolve(uri);
    let scene = craft_mesh::gltf::read(bytes, &limits(), &mut buffers).map_err(error)?;
    to_model(scene, MissingNormals::Flat, &|im| {
        let uri = im.uri.as_deref()?;
        match uri.strip_prefix("data:") {
            Some(_) => None,
            // craft-mesh decodes external image URIs while constructing the scene. Decode only
            // once here; a second pass changes literal percent escapes in valid filenames.
            None => resolve(uri),
        }
    })
}
