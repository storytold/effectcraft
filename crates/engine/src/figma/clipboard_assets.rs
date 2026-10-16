//! Validate embedded bitmap assets before the importer may write files or change the project.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits};
use serde_json::{Map, Value, json};

use super::{COMMAND, bad};
use crate::Result;

const MAX_ASSETS: usize = 2048;
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_PIXELS: u64 = 512 * 1024 * 1024;

pub(super) fn prepare(value: Option<&Value>, nodes: &mut [Value]) -> Result<Map<String, Value>> {
    let Some(value) = value else { return Ok(Map::new()) };
    let assets = value.as_object().ok_or_else(|| bad(COMMAND, "assets must be an object"))?;
    if assets.len() > MAX_ASSETS {
        return Err(bad(COMMAND, "clipboard contains more than 2048 image assets"));
    }
    let mut reserved = BTreeSet::new();
    let mut pending: Vec<&Value> = nodes.iter().collect();
    while let Some(node) = pending.pop() {
        if let Some(id) = node.get("id").and_then(Value::as_str) {
            reserved.insert(id.to_string());
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            pending.extend(children);
        }
    }
    let mut next_key = 0usize;
    let mut images = Map::new();
    let mut dimensions = BTreeMap::new();
    let mut total_pixels = 0u64;
    for (id, entry) in assets {
        if id.is_empty() || id.len() > 256 {
            return Err(bad(COMMAND, "image asset id must contain 1–256 bytes"));
        }
        let entry = entry.as_object().ok_or_else(|| bad(COMMAND, "image asset must be an object"))?;
        if entry.keys().any(|key| !matches!(key.as_str(), "mime" | "data" | "width" | "height")) {
            return Err(bad(COMMAND, "image assets accept only mime, data, width and height"));
        }
        let format = match entry.get("mime").and_then(Value::as_str) {
            Some("image/png") => ImageFormat::Png,
            Some("image/jpeg") => ImageFormat::Jpeg,
            _ => return Err(bad(COMMAND, "clipboard image must be PNG or JPEG")),
        };
        let encoded = entry.get("data").and_then(Value::as_str).ok_or_else(|| bad(COMMAND, "image asset needs base64 data"))?;
        if encoded.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
            return Err(bad(COMMAND, "image asset exceeds 32 MiB"));
        }
        let bytes = super::super::fetch::base64_decode(encoded).map_err(|e| bad(COMMAND, format!("invalid image base64: {e}")))?;
        if bytes.len() > MAX_IMAGE_BYTES || effectcraft_track::roto::rle::base64_encode(&bytes) != encoded {
            return Err(bad(COMMAND, "invalid or oversized canonical image base64"));
        }
        if image::guess_format(&bytes).ok() != Some(format) {
            return Err(bad(COMMAND, "image signature does not match its MIME type"));
        }
        let (width, height) =
            ImageReader::with_format(Cursor::new(&bytes), format).into_dimensions().map_err(|e| bad(COMMAND, format!("invalid image header: {e}")))?;
        let pixels = u64::from(width) * u64::from(height);
        total_pixels = total_pixels.saturating_add(pixels);
        if width == 0 || height == 0 || width > 16384 || height > 16384 || pixels > MAX_IMAGE_PIXELS || total_pixels > MAX_TOTAL_PIXELS {
            return Err(bad(COMMAND, "image dimensions exceed clipboard limits (16384 per side, 64M pixels each, 512M total)"));
        }
        if entry.get("width").and_then(Value::as_u64) != Some(u64::from(width)) || entry.get("height").and_then(Value::as_u64) != Some(u64::from(height)) {
            return Err(bad(COMMAND, "image dimensions do not match its encoded data"));
        }
        let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
        let mut limits = Limits::default();
        limits.max_image_width = Some(16384);
        limits.max_image_height = Some(16384);
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);
        reader.decode().map_err(|e| bad(COMMAND, format!("invalid image pixels: {e}")))?;
        // Caller-supplied identifiers never become paths; each paste has its own generated directory.
        let key = loop {
            let key = format!("asset-{next_key}");
            next_key = next_key.saturating_add(1);
            if reserved.insert(key.clone()) {
                break key;
            }
        };
        dimensions.insert(id.as_str(), (key.clone(), width, height));
        images.insert(key, json!({"base64":encoded}));
    }
    bind(nodes, &dimensions, 0, &mut 0)?;
    Ok(images)
}

fn bind(nodes: &mut [Value], assets: &BTreeMap<&str, (String, u32, u32)>, depth: usize, placements: &mut usize) -> Result<()> {
    if depth > 64 {
        return Err(bad(COMMAND, "clipboard image nesting exceeds 64 levels"));
    }
    for node in nodes {
        for channel in ["fills", "strokes"] {
            if let Some(paints) = node.get_mut(channel).and_then(Value::as_array_mut) {
                for paint in paints {
                    if paint.get("type").and_then(Value::as_str) != Some("IMAGE") {
                        continue;
                    }
                    *placements = placements.saturating_add(1);
                    if *placements > 20_000 {
                        return Err(bad(COMMAND, "clipboard contains more than 20000 image paint placements"));
                    }
                    let reference =
                        paint.get("imageRef").and_then(Value::as_str).ok_or_else(|| bad(COMMAND, "image paint needs an embedded asset reference"))?;
                    let (key, width, height) = assets.get(reference).ok_or_else(|| bad(COMMAND, "image paint refers to a missing embedded asset"))?;
                    let rows = paint
                        .get("figcraftImageTransform")
                        .and_then(Value::as_array)
                        .filter(|rows| rows.len() == 2)
                        .ok_or_else(|| bad(COMMAND, "image paint needs a 2×3 pixel transform"))?;
                    let mut coefficients = Vec::with_capacity(6);
                    for row in rows {
                        let row = row.as_array().filter(|row| row.len() == 3).ok_or_else(|| bad(COMMAND, "image paint needs a 2×3 pixel transform"))?;
                        for value in row {
                            coefficients.push(
                                value
                                    .as_f64()
                                    .filter(|x| x.is_finite() && x.abs() <= 1e9)
                                    .ok_or_else(|| bad(COMMAND, "image transform must be finite and bounded"))?,
                            );
                        }
                    }
                    let determinant = coefficients.first().copied().unwrap_or(0.0) * coefficients.get(4).copied().unwrap_or(0.0)
                        - coefficients.get(1).copied().unwrap_or(0.0) * coefficients.get(3).copied().unwrap_or(0.0);
                    if determinant.abs() < 1e-12 {
                        return Err(bad(COMMAND, "image transform is singular"));
                    }
                    paint["imageRef"] = json!(key);
                    paint["figcraftImageSize"] = json!([width, height]);
                }
            }
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            bind(children, assets, depth + 1, placements)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn fixture() -> (Value, Value) {
        let image = image::RgbaImage::from_fn(4, 2, |x, _| if x < 2 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) });
        let mut png = Cursor::new(Vec::new());
        image.write_to(&mut png, ImageFormat::Png).unwrap();
        let data = effectcraft_track::roto::rle::base64_encode(png.get_ref());
        let asset = json!({"source-image":{"mime":"image/png","width":4,"height":2,"data":data}});
        let node = json!({"id":"image-frame","type":"FRAME","figcraft":true,"name":"Photo with title", "size":{"x":40,"y":20},
            "clipsContent":true, "cornerRadius":4, "absoluteTransform":[[1,0,0],[0,1,0]],
            "fills":[{"type":"IMAGE","imageRef":"source-image", "figcraftImageTransform":[[10,0,-10],[0,10,0]]}],
            "children":[{"id":"title","type":"TEXT","name":"Editable title","characters":"Hello", "size":{"x":20,"y":10},
            "absoluteTransform":[[1,0,4],[0,1,2]],"style":{"fontSize":6}}]});
        (asset, node)
    }

    #[test]
    fn embeds_original_pixels_and_keeps_frame_children_and_crop_transform() {
        let (assets, node) = fixture();
        let mut nodes = vec![node];
        let images = prepare(Some(&assets), &mut nodes).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images["asset-0"]["base64"], assets["source-image"]["data"]);
        let plan = super::super::map::plan(&nodes, &super::super::map::Options { target: super::super::map::Target::NewComp });
        assert_eq!(plan.rasters.len(), 1);
        let layers: Vec<_> = plan.comps.iter().flat_map(|c| &c.layers).collect();
        let image = layers.iter().find(|l| l.original_image).unwrap();
        assert_eq!(image.anchor, [2.0, 1.0]);
        assert_eq!(image.scale, [1000.0, 1000.0]);
        assert_eq!(image.position, [10.0, 10.0]);
        assert!(image.matte.is_some());
        assert!(layers.iter().any(|l| l.name == "Editable title" && matches!(l.kind, super::super::map::PKind::Text(_))));
        assert!(layers.iter().any(|l| l.is_matte && matches!(l.kind, super::super::map::PKind::Shape(_))));
        let frame = layers.iter().find(|l| l.name == "Photo with title").unwrap();
        assert_eq!(frame.masks.len(), 1);
        let super::super::map::PKind::Precomp(inner) = frame.kind else { panic!("frame must clip through its precomp") };
        assert_eq!((plan.comps[inner].width, plan.comps[inner].height), (40, 20));
        let mut outside = nodes.clone();
        outside[0]["children"][0]["absoluteTransform"] = json!([[1, 0, 100], [0, 1, 100]]);
        let clipped = super::super::map::plan(&outside, &super::super::map::Options { target: super::super::map::Target::NewComp });
        assert_eq!((clipped.comps[inner].width, clipped.comps[inner].height), (40, 20));
        assert!(clipped.comps[inner].layers.iter().any(|l| l.name == "Editable title" && l.position[0] > 40.0));
    }

    #[test]
    fn jpeg_bytes_are_preserved_and_repeated_paints_share_one_footage_source() {
        let image = image::RgbImage::from_pixel(4, 2, image::Rgb([30, 100, 220]));
        let mut jpeg = Cursor::new(Vec::new());
        image.write_to(&mut jpeg, ImageFormat::Jpeg).unwrap();
        let data = effectcraft_track::roto::rle::base64_encode(jpeg.get_ref());
        let assets = json!({"source-image":{"mime":"image/jpeg", "data":data, "width":4, "height":2}});
        let mut node = fixture().1;
        let repeated = node["fills"][0].clone();
        node["fills"].as_array_mut().unwrap().push(repeated);
        let mut nodes = [node];
        let images = prepare(Some(&assets), &mut nodes).unwrap();
        assert_eq!(images["asset-0"]["base64"], data);
        let plan = super::super::map::plan(&nodes, &super::super::map::Options { target: super::super::map::Target::NewComp });
        assert_eq!(plan.rasters.len(), 1);
        assert_eq!(plan.comps.iter().flat_map(|c| &c.layers).filter(|l| l.original_image).count(), 2);
    }

    #[test]
    fn repeated_image_paints_cannot_expand_beyond_the_layer_budget() {
        let (assets, mut node) = fixture();
        node["fills"] = json!(vec![node["fills"][0].clone(); 20_001]);
        let error = prepare(Some(&assets), &mut [node]).unwrap_err().to_string();
        assert!(error.contains("20000 image paint placements"));
    }

    #[test]
    fn rejects_paths_wrong_dimensions_missing_assets_and_singular_transforms() {
        let (assets, node) = fixture();
        for (assets, node) in [
            (
                {
                    let mut a = assets.clone();
                    a["source-image"]["path"] = json!("/private/file.png");
                    a
                },
                node.clone(),
            ),
            (
                {
                    let mut a = assets.clone();
                    a["source-image"]["url"] = json!("http://127.0.0.1/secret");
                    a
                },
                node.clone(),
            ),
            (
                {
                    let mut a = assets.clone();
                    a["source-image"]["width"] = json!(40000);
                    a
                },
                node.clone(),
            ),
            (
                {
                    let mut a = assets.clone();
                    a["source-image"]["mime"] = json!("image/jpeg");
                    a
                },
                node.clone(),
            ),
            (
                {
                    let mut a = assets.clone();
                    a["source-image"]["data"] = json!("%%%bad");
                    a
                },
                node.clone(),
            ),
            (json!({}), node.clone()),
            (assets.clone(), {
                let mut n = node.clone();
                n["fills"][0]["figcraftImageTransform"] = json!([[0, 0, 0], [0, 0, 0]]);
                n
            }),
            (assets.clone(), {
                let mut n = node;
                n["fills"][0]["figcraftImageTransform"] = json!([[1, 0, 0], [0, 1, 1e100]]);
                n
            }),
        ] {
            assert!(prepare(Some(&assets), &mut [node]).is_err());
        }
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use crate::{Importer, Session};
    use effectcraft_project::{Footage, FootageKind, ItemKind, LayerSource, Project};
    use std::sync::Arc;

    struct BitmapImporter;
    impl Importer for BitmapImporter {
        fn probe(&self, path: &str) -> std::result::Result<Footage, String> {
            let (width, height) = image::image_dimensions(path).map_err(|e| e.to_string())?;
            Ok(Footage { path: path.into(), kind: FootageKind::Still, width, height, has_video: true, ..Footage::default() })
        }
    }

    #[test]
    fn pasted_bitmap_assets_survive_undo_redo_and_project_reopen() {
        let (assets, node) = super::tests::fixture();
        let payload = json!({"format":"figcraft.clipboard", "version":2,"nodes":[node],"assets":assets});
        let mut session = Session { importer: Some(Arc::new(BitmapImporter)), ..Session::default() };
        let dir = std::env::temp_dir().join(format!("effectcraft-clipboard-images-{}", super::super::paste_id()));
        session.path = Some(dir.join("Project.ecproj").to_string_lossy().into_owned());
        let result = session.execute_checked(COMMAND, json!({"clipboard":payload.to_string()})).unwrap();
        assert_eq!(result["images"], 1);
        assert_eq!(session.history.undo.len(), 1);
        let layers: Vec<_> = session.project.comps().flat_map(|(_, c)| c.layers.iter()).collect();
        let image = layers.iter().find(|l| matches!(l.source, LayerSource::Footage { .. })).unwrap();
        assert_eq!(image.props.prop("transform/scale").unwrap().value.as_vec3(), [1000.0, 1000.0, 100.0]);
        assert!(image.track_matte.is_some());
        assert!(layers.iter().any(|l| matches!(l.source, LayerSource::Text)));
        let serialized = session.project.to_file_json().unwrap();
        let reopened = Project::from_json(&serialized).unwrap();
        let footage = reopened
            .items
            .values()
            .find_map(|item| match &item.kind {
                ItemKind::Footage(f) => Some(f),
                _ => None,
            })
            .unwrap();
        assert_eq!(image::image_dimensions(&footage.path).unwrap(), (4, 2));
        session.execute_checked("edit.undo", json!({})).unwrap();
        assert!(session.project.items.is_empty());
        session.execute_checked("edit.redo", json!({})).unwrap();
        assert_eq!(session.project.to_file_json().unwrap(), serialized);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_or_malformed_assets_leave_the_project_untouched() {
        let (assets, node) = super::tests::fixture();
        for assets in [
            None,
            Some(json!({})),
            Some({
                let mut a = assets;
                a["source-image"]["width"] = json!(99999);
                a
            }),
        ] {
            let mut payload = json!({"format":"figcraft.clipboard", "version":2, "nodes":[node.clone()]});
            if let Some(assets) = assets {
                payload["assets"] = assets;
            }
            let mut session = Session::default();
            assert!(session.execute_checked(COMMAND, json!({"clipboard":payload.to_string()})).is_err());
            assert!(session.project.items.is_empty());
            assert!(session.history.undo.is_empty());
            assert_eq!(session.revision, 0);
        }
    }

    #[test]
    fn later_image_probe_failure_preserves_all_prior_application_state() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct FailSecond(AtomicUsize);
        impl Importer for FailSecond {
            fn probe(&self, path: &str) -> std::result::Result<Footage, String> {
                if self.0.fetch_add(1, Ordering::Relaxed) == 1 { Err("synthetic second image failure".into()) } else { BitmapImporter.probe(path) }
            }
        }
        let (mut assets, mut node) = super::tests::fixture();
        assets["second-image"] = assets["source-image"].clone();
        let mut second = node["fills"][0].clone();
        second["imageRef"] = json!("second-image");
        node["fills"].as_array_mut().unwrap().push(second);
        let payload = json!({"format":"figcraft.clipboard", "version":2,"nodes":[node],"assets":assets});
        let importer = Arc::new(FailSecond(AtomicUsize::new(0)));
        let mut session = Session { importer: Some(importer.clone()), ..Session::default() };
        session.execute_checked("comp.new", json!({"name":"Existing comp"})).unwrap();
        session.execute_checked("layer.newNull", json!({"name":"Keep selected"})).unwrap();
        session.mark_saved();
        let dir = std::env::temp_dir().join(format!("effectcraft-clipboard-failure-{}", super::super::paste_id()));
        session.path = Some(dir.join("Project.ecproj").to_string_lossy().into_owned());
        let project = session.project.clone();
        let history = session.history.clone();
        let state = serde_json::to_value(&session.state).unwrap();
        let counters = (session.revision, session.saved_revision);
        let events = session.events.clone();
        let journal = session.journal.clone();
        let error = session.execute_checked(COMMAND, json!({"clipboard":payload.to_string()})).unwrap_err().to_string();
        assert!(error.contains("synthetic second image failure"));
        assert_eq!(importer.0.load(Ordering::Relaxed), 2);
        assert!(Arc::ptr_eq(&session.project, &project));
        assert_eq!(serde_json::to_value(&session.state).unwrap(), state);
        assert_eq!((session.revision, session.saved_revision), counters);
        assert!(!session.is_dirty());
        assert_eq!(session.events, events);
        assert_eq!(session.journal, journal);
        assert_eq!(session.history.undo.len(), history.undo.len());
        for ((label, project), (old_label, old_project)) in session.history.undo.iter().zip(&history.undo) {
            assert_eq!(label, old_label);
            assert!(Arc::ptr_eq(project, old_project));
        }
        assert_eq!(session.history.redo.len(), history.redo.len());
        assert_eq!(session.history.branches.len(), history.branches.len());
        assert_eq!(session.history.merge_key, history.merge_key);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_payload_above_old_two_megabyte_limit_is_accepted() {
        let mut node = super::tests::fixture().1;
        node["fills"] = json!([]);
        node["name"] = json!("x".repeat(2 * 1024 * 1024 + 1));
        let payload = json!({"format":"figcraft.clipboard", "version":2, "nodes":[node], "assets":{}});
        let mut session = Session::default();
        assert!(session.execute_checked(COMMAND, json!({"clipboard":payload.to_string()})).is_ok());
        assert!(!session.project.items.is_empty());
    }
}
