//! Import and decompile Adobe After Effects project files (`.aep` binary RIFX format and
//! `.aepx` XML format).
//!
//! # Clean-Room Provenance & AGENTS.md Compliance
//! The `.aep` binary RIFX container layout and `.aepx` XML tags are derived entirely from
//! community reverse-engineering publications and permissively licensed specifications:
//! - `boltframe/aftereffects-aep-parser` (RIFX Big-Endian chunk tree layout, Kaitai Struct schema, MIT)
//! - `inlife/aftereffects-project-research` (Egg! container FourCC codes and bdata XML tags, MIT)
//! - `forticheprod/py-aep` (Structural Item and Composition models, MIT)
//!
//! No Adobe binaries were disassembled, decompiled, executed, or inspected.
//!
//! # Container Structure
//! An `.aep` file is a RIFX (big-endian RIFF) or RIFF container with form type `Egg!`. Inside
//! is a chunk hierarchy:
//! - `Fold`: Root project folder
//! - `Item`: Project items (folders, compositions, footage, solids)
//! - `Sfdr`: Subfolder containing child `Item`s
//! - `cdta`: Composition metadata (width, height, frame rate, duration in frames)
//! - `Layr` / `SLay` / `CLay`: Layers (AVLayer, Solid Layer, Camera Layer)
//! - `ldta`: Layer metadata (source item id, layer index, timings, layer name)
//! - `tdgp`: Property stream groups (transform, effects, text, shape contents)
//!
//! An `.aepx` file is the XML serialization counterpart. Both are decoded into standard
//! [`effectcraft_project::Project`] documents.

use std::collections::HashMap;
use std::sync::Arc;

use effectcraft_color::Label;
use effectcraft_project::build;
use effectcraft_project::{Comp, Footage, FootageKind, Item, ItemId, ItemKind, LayerSource, Project, Resolution, Solid};
use effectcraft_time::{FrameRate, Tick};

/// Maximum allowed recursion depth for chunk trees.
const MAX_CHUNK_DEPTH: usize = 64;
/// Maximum total chunks allowed in a single parsed tree (guards against DoS/OOM).
const MAX_CHUNK_NODES: usize = 100_000;
/// Maximum total compositions and items allowed in a project.
const MAX_PROJECT_ITEMS: usize = 20_000;

#[inline]
fn get_u16_be(data: &[u8], offset: usize) -> Option<u16> {
    let s = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_be_bytes([s[0], s[1]]))
}

#[inline]
fn get_u32_be(data: &[u8], offset: usize) -> Option<u32> {
    let s = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

#[inline]
fn get_u32_le(data: &[u8], offset: usize) -> Option<u32> {
    let s = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Detailed report on assets, fonts, and plugins found during `.aep` import.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct AepReport {
    /// Project file path if known.
    pub project_path: Option<String>,
    /// Font families referenced in the project.
    pub fonts: Vec<String>,
    /// Fonts missing from the local system.
    pub missing_fonts: Vec<String>,
    /// Paths or names of missing footage files.
    pub missing_footage: Vec<String>,
    /// Total footage items discovered.
    pub total_footage_count: usize,
    /// Number of missing footage items.
    pub missing_footage_count: usize,
    /// Total compositions discovered.
    pub comp_count: usize,
    /// All effects/plugins used in the project (friendly display names).
    pub plugins: Vec<String>,
    /// Custom / third-party plugins detected (e.g. Cycore CC, Trapcode, Sapphire, Video Copilot, etc.).
    pub custom_plugins: Vec<String>,
}

impl AepReport {
    /// Whether the report has any missing assets, missing fonts, or custom plugins to alert the user about.
    pub fn has_issues(&self) -> bool {
        !self.custom_plugins.is_empty() || self.missing_footage_count > 0 || !self.missing_fonts.is_empty()
    }
}

/// Whether `bytes` has the binary After Effects project signature (RIFX/RIFF `Egg!`).
pub fn is_aep(bytes: &[u8]) -> bool {
    let Some(magic) = bytes.get(0..4) else { return false };
    let Some(form) = bytes.get(8..12) else { return false };
    (magic == b"RIFX" || magic == b"RIFF") && form == b"Egg!"
}

/// Whether `bytes` looks like an After Effects XML project (`.aepx`).
pub fn is_aepx(bytes: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(bytes) else { return false };
    let t = s.trim_start_matches('\u{feff}').trim_start();
    (t.starts_with("<?xml") || t.starts_with("<AfterEffects") || t.starts_with("<Egg"))
        && (t.contains("AfterEffects") || t.contains("Egg") || t.contains("bdata") || t.contains("string id=\"Utf8\""))
}

/// Whether `bytes` still opens like a project whatever the file is called: the RIFF/RIFX container
/// signature, or an XML document (`<` first, past a BOM, leading space or the NULs of a wide
/// encoding — the `.aepx` case).
fn looks_like_project(bytes: &[u8]) -> bool {
    let Some(magic) = bytes.get(0..4) else { return false };
    if magic == b"RIFX" || magic == b"RIFF" {
        return true;
    }
    let rest = match bytes {
        b if b.starts_with(b"\xEF\xBB\xBF") => &b[3..],
        b if b.starts_with(b"\xFE\xFF") || b.starts_with(b"\xFF\xFE") => &b[2..],
        b => b,
    };
    matches!(rest.iter().copied().find(|b| !b.is_ascii_whitespace() && *b != 0), Some(b'<'))
}

/// Whether a file path or buffer matches After Effects binary (.aep) or XML (.aepx) format.
///
/// The name alone is not enough: `AI Product Promo (converted).aep` is a 30 MB EffectCraft JSON
/// project saved under an `.aep` name, and claiming it here sent it to [`parse_rifx`], which
/// answered "not a RIFF/RIFX file" — while the JSON reader in `commands::file::open`, the branch
/// that can actually open the file, was never reached. A name now only adds an AEPX whose header
/// the sniffers above don't recognise; the content still has to look like a project.
pub fn is_aep_or_aepx(path: &str, bytes: &[u8]) -> bool {
    if is_aep(bytes) || is_aepx(bytes) {
        return true;
    }
    let p = path.to_ascii_lowercase();
    (p.ends_with(".aep") || p.ends_with(".aepx")) && looks_like_project(bytes)
}

/// A node in a parsed RIFF / RIFX chunk tree.
#[derive(Clone, Debug)]
pub enum ChunkNode<'a> {
    Chunk {
        id: [u8; 4],
        data: &'a [u8],
    },
    List {
        id: [u8; 4],
        list_type: [u8; 4],
        children: Vec<ChunkNode<'a>>,
    },
}

/// Parse a RIFX or RIFF binary stream into a tree of chunks.
pub fn parse_rifx(bytes: &[u8]) -> Result<Vec<ChunkNode<'_>>, String> {
    if bytes.len() < 12 {
        return Err("AEP file too small".into());
    }
    let magic = bytes.get(0..4).ok_or("cannot read header magic")?;
    if magic != b"RIFX" && magic != b"RIFF" {
        return Err(format!("not a RIFF/RIFX file: {:?}", magic));
    }
    let be = magic == b"RIFX";
    let form = bytes.get(8..12).ok_or("cannot read form type")?;
    if form != b"Egg!" {
        return Err(format!("not an After Effects Egg! container: {:?}", form));
    }
    let payload = bytes.get(12..).unwrap_or(&[]);
    let mut total_nodes = 0;
    parse_chunks(payload, be, 0, &mut total_nodes)
}

fn parse_chunks<'a>(data: &'a [u8], be: bool, depth: usize, total_nodes: &mut usize) -> Result<Vec<ChunkNode<'a>>, String> {
    if depth > MAX_CHUNK_DEPTH || *total_nodes >= MAX_CHUNK_NODES {
        return Ok(Vec::new());
    }
    let mut nodes = Vec::new();
    let mut pos = 0usize;
    while let Some(hdr) = data.get(pos..pos.saturating_add(8)) {
        *total_nodes = total_nodes.saturating_add(1);
        if *total_nodes > MAX_CHUNK_NODES {
            break;
        }
        let mut id = [0u8; 4];
        id.copy_from_slice(&hdr[0..4]);
        let len = if be {
            u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize
        } else {
            u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize
        };
        let cstart = pos.saturating_add(8);
        let cend = cstart.saturating_add(len).min(data.len());
        let payload = data.get(cstart..cend).unwrap_or(&[]);
        let pad = len % 2;
        let next_pos = cend.saturating_add(pad);
        if next_pos <= pos {
            break;
        }
        pos = next_pos;

        if &id == b"LIST" {
            if let Some(ltype_bytes) = payload.get(0..4) {
                let mut ltype = [0u8; 4];
                ltype.copy_from_slice(ltype_bytes);
                let sub_data = payload.get(4..).unwrap_or(&[]);
                let sub = parse_chunks(sub_data, be, depth.saturating_add(1), total_nodes).unwrap_or_default();
                nodes.push(ChunkNode::List { id, list_type: ltype, children: sub });
            }
        } else {
            nodes.push(ChunkNode::Chunk { id, data: payload });
        }
        if pos >= data.len() {
            break;
        }
    }
    Ok(nodes)
}

/// Extracted composition information before turning into project items.
#[derive(Clone, Debug)]
pub struct DecodedComp {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: f64,
    pub frame_rate: f64,
    pub duration_frames: u32,
    pub background: [f32; 3],
    pub layers: Vec<DecodedLayer>,
    pub raw_item_id: u32,
    pub parent_folder: Option<String>,
}

/// Extracted footage / media information.
#[derive(Clone, Debug)]
pub struct DecodedFootage {
    pub id: u32,
    pub name: String,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub duration_frames: u32,
    pub parent_folder: Option<String>,
    pub is_solid: bool,
}

/// Per-layer transform values read from a layer's `tdgp` property group. Every field is optional:
/// a property is absent when After Effects stored it as an animation (this importer does not
/// keyframe transforms yet), and `None` leaves the layer's built-in default in place.
#[derive(Clone, Debug, Default)]
pub struct DecodedTransform {
    /// Anchor Point, in comp pixels.
    pub anchor: Option<[f64; 3]>,
    /// Position, in comp pixels.
    pub position: Option<[f64; 3]>,
    /// Percent (After Effects writes scale as a 1.0 = 100% factor).
    pub scale: Option<[f64; 3]>,
    /// Orientation, X/Y/Z degrees.
    pub orientation: Option<[f64; 3]>,
    /// Z rotation, in degrees.
    pub rotation: Option<f64>,
    /// X rotation, in degrees.
    pub rotation_x: Option<f64>,
    /// Y rotation, in degrees.
    pub rotation_y: Option<f64>,
    /// Percent (After Effects writes opacity as a 0.0..1.0 factor).
    pub opacity: Option<f64>,
    /// The layer lives in 3D space: it has an orientation or X/Y rotation, or a position Z offset.
    /// (Cameras are 3D through their layer type instead, see the layer build in [`from_aep`].)
    pub three_d: bool,
}

/// The static value of a `tdbs` property block: its `cdat` chunk read as big-endian f64s. After
/// Effects writes Point3D values as three doubles and scalar properties as five (the value first,
/// then fuzz/reference values), so only the leading components are meaningful.
fn cdat_values(nodes: &[ChunkNode<'_>]) -> Option<Vec<f64>> {
    for n in nodes {
        if let ChunkNode::Chunk { id, data } = n
            && id == b"cdat"
        {
            let v: Vec<f64> = data.as_chunks::<8>().0.iter().map(|c| f64::from_be_bytes(*c)).collect();
            return (!v.is_empty()).then_some(v);
        }
    }
    None
}

/// Read a layer's transform group (`tdgp`): each property is a `tdmn` match name followed by a
/// `tdbs` list holding its value. Angles are degrees -- real projects store `180`, `-45`,
/// `-21.1752` -- positions are comp pixels, and scale/opacity are 1.0-based factors.
fn read_transform(chunks: &[ChunkNode<'_>], out: &mut DecodedTransform) {
    let mut pending: Option<String> = None;
    for c in chunks {
        match c {
            ChunkNode::Chunk { id, data } if id == b"tdmn" => {
                let name = String::from_utf8_lossy(data);
                pending = name.split('\0').next().map(|s| s.trim().to_string());
            }
            ChunkNode::List { list_type, children, .. } if list_type == b"tdbs" => {
                let Some(name) = pending.take() else { continue };
                let Some(v) = cdat_values(children) else { continue };
                let xyz = || -> Option<[f64; 3]> { Some([*v.first()?, *v.get(1)?, *v.get(2)?]) };
                match name.as_str() {
                    "ADBE Anchor Point" => out.anchor = xyz(),
                    "ADBE Position" => {
                        if let Some(p) = xyz() {
                            out.three_d |= p[2] != 0.0;
                            out.position = Some(p);
                        }
                    }
                    "ADBE Scale" => out.scale = xyz().map(|s| [s[0] * 100.0, s[1] * 100.0, s[2] * 100.0]),
                    "ADBE Orientation" => {
                        out.orientation = xyz();
                        out.three_d = true;
                    }
                    "ADBE Rotate X" => {
                        out.rotation_x = v.first().copied();
                        out.three_d = true;
                    }
                    "ADBE Rotate Y" => {
                        out.rotation_y = v.first().copied();
                        out.three_d = true;
                    }
                    "ADBE Rotate Z" => out.rotation = v.first().copied(),
                    "ADBE Opacity" => out.opacity = v.first().map(|o| o * 100.0),
                    _ => {}
                }
            }
            ChunkNode::List { children, .. } => read_transform(children, out),
            _ => {}
        }
    }
}

/// Write a transform property's static value, leaving the built-in default when the group or the
/// property itself is missing.
fn set_vec3(layer: &mut effectcraft_project::Layer, path: &str, v: [f64; 3]) {
    if let Some(p) = layer.props.prop_mut(path) {
        p.value = effectcraft_keyframe::Value::Vec3(v);
    }
}

fn set_scalar(layer: &mut effectcraft_project::Layer, path: &str, v: f64) {
    if let Some(p) = layer.props.prop_mut(path) {
        p.value = effectcraft_keyframe::Value::Scalar(v);
    }
}


/// Extracted layer information.
#[derive(Clone, Debug)]
pub struct DecodedLayer {
    pub name: String,
    pub layer_type: [u8; 4],
    pub in_sec: f64,
    pub out_sec: f64,
    pub start_sec: f64,
    pub source_item_id: u32,
    pub three_d: bool,
    /// Transform values read from the layer's `tdgp` group.
    pub transform: DecodedTransform,
    pub is_text: bool,
    pub is_shape: bool,
    pub text_font: Option<String>,
    pub effects: Vec<String>,
}

/// Scan bytes for `/CoolTypeFont` declarations and extract font family names.
pub fn extract_aep_fonts(bytes: &[u8]) -> Vec<String> {
    let mut fonts = Vec::new();
    let marker = b"/CoolTypeFont";
    let mut pos = 0usize;
    while pos.saturating_add(marker.len()) <= bytes.len() {
        let Some(window_slice) = bytes.get(pos..) else { break };
        if let Some(idx) = window_slice.windows(marker.len()).position(|w| w == marker) {
            let start = pos.saturating_add(idx).saturating_add(marker.len());
            pos = start;
            let search_len = 128usize.min(bytes.len().saturating_sub(start));
            let Some(window) = bytes.get(start..start.saturating_add(search_len)) else { break };
            if let Some(bom_idx) = window.windows(2).position(|w| w == b"\xfe\xff") {
                let text_start = start.saturating_add(bom_idx).saturating_add(2);
                let max_end = text_start.saturating_add(256).min(bytes.len());
                let Some(text_slice) = bytes.get(text_start..max_end) else { continue };
                if let Some(close_idx) = text_slice.iter().position(|&b| b == b')') {
                    let end_pos = text_start.saturating_add(close_idx);
                    if let Some(u16_bytes) = bytes.get(text_start..end_pos) {
                        let u16s: Vec<u16> = u16_bytes
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|c| u16::from_be_bytes(*c))
                            .collect();
                        let font_name = String::from_utf16_lossy(&u16s).trim().to_string();
                        if !font_name.is_empty() && font_name != "AdobeInvisFont" && !fonts.contains(&font_name) {
                            fonts.push(font_name);
                        }
                    }
                }
            }
        } else {
            break;
        }
    }
    fonts
}

/// Extract all effects and plugins referenced in the project from `Pefl` (pjef) and `EfdG` (EfDf).
pub fn extract_aep_plugins(bytes: &[u8]) -> (Vec<String>, Vec<String>) {
    let mut all_plugins = Vec::new();
    let mut custom_plugins = Vec::new();

    let match_map: HashMap<&str, &str> = [
        ("ADBE 4ColorGradient", "4-Color Gradient"),
        ("ADBE Box Blur2", "Fast Box Blur"),
        ("ADBE Checkbox Control", "Checkbox Control"),
        ("ADBE Color Control", "Color Control"),
        ("ADBE CurvesCustom", "Curves"),
        ("ADBE Drop Shadow", "Drop Shadow"),
        ("ADBE Easy Levels2", "Levels"),
        ("ADBE Fill", "Fill"),
        ("ADBE Gaussian Blur 2", "Gaussian Blur"),
        ("ADBE Geometry2", "Transform"),
        ("ADBE Glo2", "Glow"),
        ("ADBE Grid", "Grid"),
        ("ADBE Linear Wipe", "Linear Wipe"),
        ("ADBE Noise2", "Noise"),
        ("ADBE PhotoFilterPS", "Photo Filter"),
        ("ADBE Sharpen", "Sharpen"),
        ("ADBE Slider Control", "Slider Control"),
        ("ADBE Tile", "Motion Tile"),
        ("ADBE Tint", "Tint"),
        ("ADBE Tritone", "Tritone"),
        ("ADBE Vibrance", "Vibrance"),
        ("CS Composite", "CC Composite"),
    ]
    .into_iter()
    .collect();

    // Dynamically extract friendly display names from EfDf lists in EfdG
    let mut dynamic_names: HashMap<String, String> = HashMap::new();
    let mut pos = 0usize;
    while pos.saturating_add(4) <= bytes.len() {
        let Some(slice) = bytes.get(pos..) else { break };
        if let Some(idx) = slice.windows(4).position(|w| w == b"EfDf") {
            let efdf_start = pos.saturating_add(idx);
            pos = efdf_start.saturating_add(4);
            let window_len = 3000usize.min(bytes.len().saturating_sub(efdf_start));
            let Some(window) = bytes.get(efdf_start..efdf_start.saturating_add(window_len)) else { continue };

            let mut match_name = String::new();
            if let Some(mn_pos) = window.windows(4).position(|w| w == b"tdmn")
                && let Some(len) = get_u32_be(window, mn_pos.saturating_add(4))
            {
                let len_usize = len as usize;
                let m_start = mn_pos.saturating_add(8);
                let m_end = m_start.saturating_add(len_usize).min(window.len());
                if let Some(data) = window.get(m_start..m_end) {
                    match_name = String::from_utf8_lossy(data).trim_matches('\0').trim().to_string();
                }
            }

            let mut display_name = String::new();
            if let Some(fn_pos) = window.windows(4).position(|w| w == b"fnam")
                && let Some(sub) = window.get(fn_pos..)
                && let Some(u_pos) = sub.windows(4).position(|w| w == b"Utf8")
            {
                let u_start = fn_pos.saturating_add(u_pos);
                if let Some(len) = get_u32_be(window, u_start.saturating_add(4)) {
                    let len_usize = len as usize;
                    let u_data_start = u_start.saturating_add(8);
                    let u_end = u_data_start.saturating_add(len_usize).min(window.len());
                    if let Some(data) = window.get(u_data_start..u_end) {
                        display_name = String::from_utf8_lossy(data).trim_matches('\0').trim().to_string();
                    }
                }
            }

            if !match_name.is_empty() && !display_name.is_empty() {
                dynamic_names.insert(match_name, display_name);
            }
        } else {
            break;
        }
    }

    // Scan pjef chunks (Project Effect List)
    let marker = b"pjef";
    pos = 0usize;
    while pos.saturating_add(8) <= bytes.len() {
        let Some(slice) = bytes.get(pos..) else { break };
        if let Some(idx) = slice.windows(4).position(|w| w == marker) {
            let chunk_pos = pos.saturating_add(idx);
            pos = chunk_pos.saturating_add(4);
            if let Some(len) = get_u32_be(bytes, chunk_pos.saturating_add(4)) {
                let len_usize = len as usize;
                let data_start = chunk_pos.saturating_add(8);
                let data_end = data_start.saturating_add(len_usize).min(bytes.len());
                let raw_name = bytes
                    .get(data_start..data_end)
                    .map(|d| String::from_utf8_lossy(d).trim_matches('\0').trim().to_string())
                    .unwrap_or_default();
                pos = data_end.saturating_add(len_usize % 2);

                if !raw_name.is_empty() {
                    let friendly = dynamic_names
                        .get(&raw_name)
                        .map(|s| s.as_str())
                        .or_else(|| match_map.get(raw_name.as_str()).copied())
                        .unwrap_or(raw_name.as_str())
                        .to_string();

                    if !all_plugins.contains(&friendly) {
                        all_plugins.push(friendly.clone());
                    }

                    let is_custom = !raw_name.starts_with("ADBE ")
                        || raw_name.starts_with("CC ")
                        || raw_name.starts_with("CS ")
                        || friendly.starts_with("CC ")
                        || is_third_party_name(&friendly);

                    if is_custom && !custom_plugins.contains(&friendly) {
                        custom_plugins.push(friendly);
                    }
                }
            }
        } else {
            break;
        }
    }

    (all_plugins, custom_plugins)
}

fn is_third_party_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("cc ")
        || lower.starts_with("rg ")
        || lower.starts_with("tc_")
        || lower.starts_with("s_")
        || lower.contains("particular")
        || lower.contains("form")
        || lower.contains("optical flares")
        || lower.contains("element 3d")
        || lower.contains("saber")
        || lower.contains("twitch")
        || lower.contains("plexus")
        || lower.contains("sapphire")
        || lower.contains("magic bullet")
        || lower.contains("boris")
        || lower.contains("mocha")
}

/// Decode an After Effects `.aep` project file into an [`effectcraft_project::Project`].
pub fn from_aep(bytes: &[u8]) -> Result<Project, String> {
    from_aep_with_report(bytes, None).map(|(p, _)| p)
}

/// Decode an After Effects `.aep` project file and produce a detailed report.
pub fn from_aep_with_report(bytes: &[u8], project_path: Option<&std::path::Path>) -> Result<(Project, AepReport), String> {
    let tree = parse_rifx(bytes)?;
    let mut comps = Vec::new();
    let mut folders = Vec::new();
    let mut footage = Vec::new();

    scan_nodes(&tree, None, project_path, &mut comps, &mut folders, &mut footage);

    if comps.is_empty() {
        return Err("no compositions found in After Effects project".into());
    }

    let detected_fonts = extract_aep_fonts(bytes);
    let (all_plugins, custom_plugins) = extract_aep_plugins(bytes);
    build_project(comps, folders, footage, detected_fonts, all_plugins, custom_plugins, project_path)
}

fn parse_alas_json(data: &[u8]) -> Option<String> {
    let key = b"\"fullpath\":\"";
    let idx = data.windows(key.len()).position(|w| w == key)?;
    let start = idx.saturating_add(key.len());
    let mut end = start;
    let mut escaped = false;
    while end < data.len() {
        let Some(&byte) = data.get(end) else { break };
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            break;
        }
        end = end.saturating_add(1);
    }
    let raw = std::str::from_utf8(data.get(start..end)?).ok()?;
    let unescaped = raw.replace(r"\\", r"\").replace(r#"\""#, r#"""#);
    if !unescaped.is_empty() {
        Some(unescaped)
    } else {
        None
    }
}

fn scan_nodes(
    nodes: &[ChunkNode<'_>],
    parent_folder: Option<String>,
    project_path: Option<&std::path::Path>,
    comps: &mut Vec<DecodedComp>,
    folders: &mut Vec<(String, Option<String>)>,
    footage: &mut Vec<DecodedFootage>,
) {
    if comps.len().saturating_add(footage.len()).saturating_add(folders.len()) >= MAX_PROJECT_ITEMS {
        return;
    }
    for node in nodes {
        if let ChunkNode::List { list_type, children, .. } = node {
            if list_type == b"Item" {
                scan_item(children, parent_folder.clone(), project_path, comps, folders, footage);
            } else {
                scan_nodes(children, parent_folder.clone(), project_path, comps, folders, footage);
            }
        }
    }
}

fn extract_alas_path(chunks: &[ChunkNode<'_>]) -> Option<String> {
    for c in chunks {
        match c {
            ChunkNode::Chunk { id, data } => {
                if id == b"alas"
                    && let Some(p) = parse_alas_json(data)
                {
                    return Some(p);
                }
            }
            ChunkNode::List { children, .. } => {
                if let Some(p) = extract_alas_path(children) {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn scan_item(
    chunks: &[ChunkNode<'_>],
    parent_folder: Option<String>,
    project_path: Option<&std::path::Path>,
    comps: &mut Vec<DecodedComp>,
    folders: &mut Vec<(String, Option<String>)>,
    footage: &mut Vec<DecodedFootage>,
) {
    let mut name = String::new();
    let mut cdta: Option<&[u8]> = None;
    let mut idta: Option<&[u8]> = None;
    let mut iide: Option<u32> = None;
    let mut subfolder: Option<&[ChunkNode<'_>]> = None;
    let mut layer_chunks = Vec::new();
    let mut fnam_str = String::new();

    for c in chunks {
        match c {
            ChunkNode::Chunk { id, data } => {
                if id == b"Utf8" {
                    let s = String::from_utf8_lossy(data).trim_matches('\0').trim().to_string();
                    if !s.is_empty() {
                        name = s;
                    }
                } else if id == b"Utf1" && name.is_empty() {
                    let u16s: Vec<u16> = data
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| u16::from_be_bytes(*b))
                        .collect();
                    let s = String::from_utf16_lossy(&u16s).trim_matches('\0').trim().to_string();
                    if !s.is_empty() {
                        name = s;
                    }
                } else if id == b"fnam" {
                    let s = String::from_utf8_lossy(data).trim_matches('\0').trim().to_string();
                    if !s.is_empty() {
                        fnam_str = s;
                    }
                } else if id == b"cdta" {
                    cdta = Some(data);
                } else if id == b"idta" {
                    idta = Some(data);
                } else if id == b"iide"
                    && let Some(id_val) = get_u32_le(data, 0)
                {
                    iide = Some(id_val);
                }
            }
            ChunkNode::List { list_type, children, .. } => {
                if list_type == b"Sfdr" {
                    subfolder = Some(children);
                } else if list_type == b"Layr"
                    || list_type == b"SLay"
                    || list_type == b"CLay"
                    || list_type == b"SecL"
                    || list_type == b"AVLa"
                    || list_type.ends_with(b"Lay")
                {
                    layer_chunks.push((list_type, children.as_slice()));
                }
            }
        }
    }

    let raw_item_id = if let Some(d) = idta {
        if let Some(sub_id) = get_u32_be(d, 16).filter(|&v| v > 0) {
            sub_id
        } else if let Some(lid) = iide {
            lid
        } else {
            get_u32_be(d, 0).unwrap_or(0)
        }
    } else {
        iide.unwrap_or(0)
    };

    if let Some(sub) = subfolder {
        let folder_name = if name.is_empty() { "Folder".to_string() } else { name };
        folders.push((folder_name.clone(), parent_folder));
        scan_nodes(sub, Some(folder_name), project_path, comps, folders, footage);
    } else if let Some(cd) = cdta {
        let comp_name = if name.is_empty() { format!("Comp {}", comps.len().saturating_add(1)) } else { name };
        let (w, h, fps, dur_frames, bg) = parse_cdta(cd);

        let mut decoded_layers = Vec::new();
        for (ltype, lchildren) in layer_chunks {
            let layer = parse_layer(*ltype, lchildren);
            decoded_layers.push(layer);
        }

        comps.push(DecodedComp {
            name: comp_name,
            width: w,
            height: h,
            pixel_aspect: 1.0,
            frame_rate: fps,
            duration_frames: dur_frames,
            background: bg,
            layers: decoded_layers,
            raw_item_id,
            parent_folder,
        });
    } else {
        let mut file_path = extract_alas_path(chunks).unwrap_or_default();
        if file_path.is_empty() && !fnam_str.is_empty() {
            file_path = fnam_str;
        }

        let is_solid = parent_folder.as_deref() == Some("Solids") || (file_path.is_empty() && name.to_ascii_lowercase().contains("solid"));
        let fname = if !name.is_empty() {
            name
        } else if !file_path.is_empty() {
            let norm = file_path.replace('\\', "/");
            norm.rsplit('/').next().unwrap_or("Footage").to_string()
        } else {
            "Item".into()
        };

        footage.push(DecodedFootage {
            id: raw_item_id,
            name: fname,
            path: file_path,
            width: 1920,
            height: 1080,
            frame_rate: 30.0,
            duration_frames: 300,
            parent_folder,
            is_solid,
        });
    }
}

/// Extract composition metadata from the `cdta` chunk safely with bounds checks.
fn parse_cdta(cdta: &[u8]) -> (u32, u32, f64, u32, [f32; 3]) {
    let mut w = 1920u32;
    let mut h = 1080u32;
    let mut fps = 30.0f64;
    let mut duration_frames = 300u32;
    let mut bg = [0.0f32, 0.0f32, 0.0f32];

    if let (Some(&r), Some(&g), Some(&b)) = (cdta.get(52), cdta.get(53), cdta.get(54)) {
        bg = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];
    }

    if let (Some(cw), Some(ch)) = (get_u16_be(cdta, 140), get_u16_be(cdta, 142)) {
        let cw_u32 = cw as u32;
        let ch_u32 = ch as u32;
        if cw_u32 > 0 && ch_u32 > 0 && cw_u32 <= 32768 && ch_u32 <= 32768 {
            w = cw_u32;
            h = ch_u32;
        }
    }

    if let (Some(int_fps), Some(frac_fps)) = (get_u16_be(cdta, 156), get_u16_be(cdta, 158)) {
        let raw_fps = int_fps as f64 + (frac_fps as f64 / 65536.0);
        if (1.0..=240.0).contains(&raw_fps) {
            fps = raw_fps;
        }
    } else if let Some(raw_fps) = get_u32_be(cdta, 168)
        && (1..=240).contains(&raw_fps)
    {
        fps = raw_fps as f64;
    }

    let mut time_base = 30720.0f64;
    if let Some(tb) = get_u16_be(cdta, 10) {
        let tb_f = tb as f64;
        if (100.0..=100_000.0).contains(&tb_f) {
            time_base = tb_f;
        }
    }

    if let Some(dur_ticks) = get_u32_be(cdta, 44)
        && dur_ticks > 0 && dur_ticks < 100_000_000
    {
        let dur_sec = dur_ticks as f64 / time_base;
        duration_frames = (dur_sec * fps).round().max(1.0) as u32;
    }
    if (duration_frames == 0 || duration_frames > 1_000_000)
        && let Some(df) = get_u32_be(cdta, 16)
        && df > 0 && df < 1_000_000
    {
        duration_frames = df;
    }

    if w == 0 || h == 0 {
        w = 1920;
        h = 1080;
    }

    (w, h, fps, duration_frames.max(1), bg)
}

/// Extract layer metadata from a layer's child chunks.
fn parse_layer(layer_type: [u8; 4], chunks: &[ChunkNode<'_>]) -> DecodedLayer {
    let mut name = String::new();
    let mut ldta: Option<&[u8]> = None;
    let mut is_text = false;
    let mut is_shape = false;
    let mut three_d = layer_type == *b"CLay";
    let mut transform = DecodedTransform::default();
    let mut effects = Vec::new();
    let mut text_font = None;

    for c in chunks {
        match c {
            ChunkNode::Chunk { id, data } => {
                if id == b"Utf8" && !data.is_empty() {
                    let s = String::from_utf8_lossy(data).trim_matches('\0').trim().to_string();
                    if !s.is_empty() {
                        name = s;
                    }
                } else if id == b"ldta" {
                    ldta = Some(data);
                }
            }
            ChunkNode::List { list_type, children, .. } => {
                if list_type == b"tdgp" {
                    inspect_tdgp(children, &mut is_text, &mut is_shape, &mut three_d, &mut effects, &mut text_font);
                    read_transform(children, &mut transform);
                }
            }
        }
    }

    let mut time_base = 30720u32;
    let mut in_ticks = 0u32;
    let mut out_ticks = 30720u32 * 10;
    let mut out_time_base = 30720u32;
    let mut source_item_id = 0u32;

    if let Some(ld) = ldta {
        if let Some(tb) = get_u32_be(ld, 16).filter(|&v| v > 0) {
            time_base = tb;
        }
        if let Some(it) = get_u32_be(ld, 20) {
            in_ticks = it;
        }
        if let Some(otb) = get_u32_be(ld, 24).filter(|&v| v > 0) {
            out_time_base = otb;
        } else {
            out_time_base = time_base;
        }
        if let Some(ot) = get_u32_be(ld, 28) {
            out_ticks = ot;
        }
        if let Some(sid) = get_u32_be(ld, 40) {
            source_item_id = sid;
        }
        if name.is_empty()
            && let Some(sub) = ld.get(64..96)
        {
            let end = sub.iter().position(|&b| b == 0).unwrap_or(sub.len());
            if let Some(slice) = sub.get(..end) {
                let cand = String::from_utf8_lossy(slice).trim().to_string();
                if !cand.is_empty() {
                    name = cand;
                }
            }
        }
    }

    if name.is_empty() {
        name = match &layer_type {
            b"SLay" => "Solid".into(),
            b"CLay" => "Camera".into(),
            b"SecL" => "Markers".into(),
            _ if is_text => "Text".into(),
            _ if is_shape => "Shape Layer".into(),
            _ => "Layer".into(),
        };
    }

    let in_sec = in_ticks as f64 / time_base.max(1) as f64;
    let mut out_sec = out_ticks as f64 / out_time_base.max(1) as f64;
    if out_sec <= in_sec {
        out_sec = in_sec + 10.0;
    }

    DecodedLayer {
        name,
        layer_type,
        in_sec,
        out_sec,
        start_sec: in_sec,
        source_item_id,
        three_d: three_d || transform.three_d,
        transform,
        is_text,
        is_shape,
        text_font,
        effects,
    }
}

fn inspect_tdgp(
    chunks: &[ChunkNode<'_>],
    is_text: &mut bool,
    is_shape: &mut bool,
    _three_d: &mut bool,
    effects: &mut Vec<String>,
    text_font: &mut Option<String>,
) {
    for c in chunks {
        match c {
            ChunkNode::Chunk { id, data } => {
                if id == b"tdmn" {
                    let mn = String::from_utf8_lossy(data);
                    if mn.contains("ADBE Text Properties") {
                        *is_text = true;
                    } else if mn.contains("Shape Layer") || mn.contains("ADBE Vector") {
                        *is_shape = true;
                    }
                } else if id == b"tdsn" && !data.is_empty() {
                    let sn = String::from_utf8_lossy(data).trim_matches('\0').trim().to_string();
                    if !sn.is_empty() && sn != "Transform" && sn != "Compositing Options" && !effects.contains(&sn) {
                        effects.push(sn);
                    }
                } else if id == b" /98" && text_font.is_none() {
                    let fonts = extract_aep_fonts(data);
                    if let Some(first) = fonts.into_iter().next() {
                        *text_font = Some(first);
                    }
                }
            }
            ChunkNode::List { children, .. } => {
                inspect_tdgp(children, is_text, is_shape, _three_d, effects, text_font);
            }
        }
    }
}

/// Build an [`effectcraft_project::Project`] and [`AepReport`] from the decoded components.
fn build_project(
    decoded_comps: Vec<DecodedComp>,
    decoded_folders: Vec<(String, Option<String>)>,
    decoded_footage: Vec<DecodedFootage>,
    fonts: Vec<String>,
    plugins: Vec<String>,
    custom_plugins: Vec<String>,
    project_path: Option<&std::path::Path>,
) -> Result<(Project, AepReport), String> {
    let mut project = Project::default();
    let comp_count = decoded_comps.len();
    let mut folder_ids: HashMap<String, ItemId> = HashMap::new();
    let mut missing_footage = Vec::new();

    // 1. Create folders
    for (fname, parent_name) in decoded_folders {
        let parent = parent_name.as_deref().and_then(|p| folder_ids.get(p)).copied();
        let fid = ItemId(project.alloc());
        project.items.insert(
            fid,
            Item {
                id: fid,
                name: fname.clone(),
                label: Label::Yellow,
                comment: String::new(),
                parent,
                kind: ItemKind::Folder,
                proxy: None,
            },
        );
        folder_ids.insert(fname, fid);
    }

    // Default Solids folder for any solid layers
    let solids_folder = if let Some(&id) = folder_ids.get("Solids") {
        id
    } else {
        let fid = ItemId(project.alloc());
        project.items.insert(
            fid,
            Item {
                id: fid,
                name: "Solids".into(),
                label: Label::Yellow,
                comment: String::new(),
                parent: None,
                kind: ItemKind::Folder,
                proxy: None,
            },
        );
        folder_ids.insert("Solids".into(), fid);
        fid
    };

    // 2. Pre-allocate comp item IDs so precomposing references resolve
    let mut comp_ids: HashMap<String, ItemId> = HashMap::new();
    let mut comp_by_raw_id: HashMap<u32, ItemId> = HashMap::new();
    for c in &decoded_comps {
        let cid = ItemId(project.alloc());
        comp_ids.insert(c.name.clone(), cid);
        if c.raw_item_id > 0 {
            comp_by_raw_id.insert(c.raw_item_id, cid);
        }
    }

    // 3. Create footage items
    let mut footage_ids: HashMap<u32, ItemId> = HashMap::new();
    let mut footage_by_name: HashMap<String, ItemId> = HashMap::new();
    let total_footage = decoded_footage.len();

    for dfoot in decoded_footage {
        let parent = dfoot.parent_folder.as_deref().and_then(|p| folder_ids.get(p)).copied();
        let fid = ItemId(project.alloc());

        if dfoot.is_solid {
            let solid = Solid {
                color: [0.5, 0.5, 0.5],
                width: dfoot.width,
                height: dfoot.height,
                pixel_aspect: 1.0,
            };
            project.items.insert(
                fid,
                Item {
                    id: fid,
                    name: dfoot.name.clone(),
                    label: Label::Red,
                    comment: String::new(),
                    parent: parent.or(Some(solids_folder)),
                    kind: ItemKind::Solid(solid),
                    proxy: None,
                },
            );
        } else {
            let p_lower = dfoot.path.to_ascii_lowercase();
            let kind = if p_lower.ends_with(".wav") || p_lower.ends_with(".mp3") || p_lower.ends_with(".aac") || p_lower.ends_with(".m4a") {
                FootageKind::Audio
            } else if p_lower.ends_with(".mp4") || p_lower.ends_with(".mov") || p_lower.ends_with(".avi") || p_lower.ends_with(".mkv") {
                FootageKind::Video
            } else {
                FootageKind::Still
            };

            let label = match kind {
                FootageKind::Audio => Label::SeaFoam,
                FootageKind::Video => Label::Aqua,
                FootageKind::Still => Label::Lavender,
                _ => Label::Lavender,
            };

            if dfoot.path.is_empty() {
                missing_footage.push(dfoot.name.clone());
            }

            let footage_obj = Footage {
                path: dfoot.path.clone(),
                kind,
                width: dfoot.width,
                height: dfoot.height,
                pixel_aspect: 1.0,
                frame_rate: FrameRate::from_f64(dfoot.frame_rate),
                native_rate: None,
                duration: Tick::from_seconds_f64(10.0),
                has_video: kind != FootageKind::Audio,
                has_audio: kind == FootageKind::Audio || kind == FootageKind::Video,
                alpha: effectcraft_project::AlphaMode::Straight,
                premul_color: [0.0, 0.0, 0.0],
                loop_count: 1,
                codec: String::new(),
                missing: false,
                sequence: Vec::new(),
                color_profile: None,
                layer: None,
                fields: Default::default(),
                invert_alpha: false,
                linear_light: false,
                preserve_rgb: false,
                data: None,
                page: 0,
                alphabetical: false,
                start_timecode: None,
            };

            project.items.insert(
                fid,
                Item {
                    id: fid,
                    name: dfoot.name.clone(),
                    label,
                    comment: String::new(),
                    parent,
                    kind: ItemKind::Footage(footage_obj),
                    proxy: None,
                },
            );
        }

        if dfoot.id > 0 {
            footage_ids.insert(dfoot.id, fid);
        }
        footage_by_name.insert(dfoot.name, fid);
    }

    // 4. Build each composition
    for dcomp in decoded_comps {
        let cid = match comp_ids.get(&dcomp.name) {
            Some(&id) => id,
            None => ItemId(project.alloc()),
        };
        let frame_rate = FrameRate::from_f64(dcomp.frame_rate);
        let duration = frame_rate.tick_of(dcomp.duration_frames.max(1) as i64);

        let mut comp = Comp {
            width: dcomp.width,
            height: dcomp.height,
            pixel_aspect: dcomp.pixel_aspect,
            frame_rate,
            duration,
            display_start: Tick::ZERO,
            background: dcomp.background,
            work_area: (Tick::ZERO, duration),
            layers: Vec::new(),
            markers: Vec::new(),
            shutter_angle: 180.0,
            shutter_phase: -90.0,
            motion_blur_samples: 16,
            motion_blur_adaptive_limit: 128,
            renderer: Default::default(),
            hide_shy: false,
            enable_motion_blur: true,
            enable_frame_blending: true,
            draft_3d: false,
            preserve_frame_rate: false,
            preserve_resolution: false,
            poster_time: Tick::ZERO,
            global_light: Default::default(),
            guides: Vec::new(),
            essential: None,
            resolution: Resolution::Auto,
        };

        for dlayer in dcomp.layers {
            // After Effects stores its three built-in custom views (View ▸ Switch 3D View) as
            // camera entries in a comp's layer list. They are view bookmarks, not layers: importing
            // them would list them in the timeline and make a saved 3/4 view the comp's active
            // camera, looking past the comp. EffectCraft has its own custom views.
            if dlayer.layer_type == *b"CLay" && dlayer.name.starts_with("Custom View") {
                continue;
            }
            let source = match dlayer.layer_type.as_slice() {
                b"SLay" => {
                    let sid = ItemId(project.alloc());
                    let s = Solid {
                        color: [0.5, 0.5, 0.5],
                        width: dcomp.width,
                        height: dcomp.height,
                        pixel_aspect: 1.0,
                    };
                    project.items.insert(
                        sid,
                        Item {
                            id: sid,
                            name: dlayer.name.clone(),
                            label: Label::Red,
                            comment: String::new(),
                            parent: Some(solids_folder),
                            kind: ItemKind::Solid(s),
                            proxy: None,
                        },
                    );
                    LayerSource::Solid { item: sid }
                }
                b"CLay" => LayerSource::Camera,
                _ if dlayer.is_text => LayerSource::Text,
                _ if dlayer.is_shape => LayerSource::Shape,
                _ => {
                    if let Some(&target_ftg) = footage_ids.get(&dlayer.source_item_id) {
                        LayerSource::Footage { item: target_ftg }
                    } else if let Some(&target_comp) = comp_by_raw_id.get(&dlayer.source_item_id).filter(|&&id| id != cid) {
                        LayerSource::Comp { item: target_comp }
                    } else if let Some(&target_ftg) = footage_by_name.get(&dlayer.name) {
                        LayerSource::Footage { item: target_ftg }
                    } else if let Some(&target_comp) = comp_ids.get(&dlayer.name).filter(|&&id| id != cid) {
                        LayerSource::Comp { item: target_comp }
                    } else {
                        LayerSource::Null
                    }
                }
            };

            let mut layer = build::layer(
                &mut project,
                &comp,
                &dlayer.name,
                source,
                (dcomp.width, dcomp.height),
                Some(duration),
            );

            layer.start_time = Tick::from_seconds_f64(dlayer.start_sec);
            layer.in_point = Tick::from_seconds_f64(dlayer.in_sec);
            layer.out_point = Tick::from_seconds_f64(dlayer.out_sec.max(dlayer.in_sec + 0.033));
            layer.switches.three_d = dlayer.three_d || layer.is_camera();
            layer.switches.motion_blur = true;

            // Transform values: positions in comp pixels, scale/opacity 1.0-based, angles in
            // degrees. Properties After Effects stored as animation have no static value yet, so
            // those keep the layer's built-in defaults.
            let tr = &dlayer.transform;
            if layer.is_camera() {
                // Cameras keep the Point of Interest where AV layers keep the anchor, and an After
                // Effects camera is two-node by default: it looks at that point.
                if let Some(v) = tr.anchor {
                    set_vec3(&mut layer, "transform/poi", v);
                }
                layer.auto_orient = effectcraft_project::AutoOrient::TowardsPointOfInterest;
            } else if let Some(v) = tr.anchor {
                set_vec3(&mut layer, "transform/anchor", v);
            }
            if let Some(v) = tr.position {
                set_vec3(&mut layer, "transform/position", v);
            }
            if let Some(v) = tr.scale {
                set_vec3(&mut layer, "transform/scale", v);
            }
            if let Some(v) = tr.orientation {
                set_vec3(&mut layer, "transform/orientation", v);
            }
            if let Some(v) = tr.rotation {
                set_scalar(&mut layer, "transform/rotation", v);
            }
            if let Some(v) = tr.rotation_x {
                set_scalar(&mut layer, "transform/rotationX", v);
            }
            if let Some(v) = tr.rotation_y {
                set_scalar(&mut layer, "transform/rotationY", v);
            }
            if let Some(v) = tr.opacity {
                set_scalar(&mut layer, "transform/opacity", v);
            }

            if dlayer.is_text {
                let font_name = dlayer.text_font.as_deref()
                    .or_else(|| fonts.first().map(String::as_str))
                    .unwrap_or("SegoeUI")
                    .to_string();
                let doc = effectcraft_keyframe::TextDoc {
                    text: dlayer.name.clone(),
                    font: font_name,
                    size: 48.0,
                    fill: [1.0, 1.0, 1.0, 1.0],
                    apply_fill: true,
                    ..Default::default()
                };
                if let Some(pr) = layer.props.prop_mut("text/sourceText") {
                    pr.value = effectcraft_keyframe::Value::Text(Box::new(doc));
                }
            }

            comp.layers.push(layer);
        }

        let parent = dcomp.parent_folder.as_deref().and_then(|p| folder_ids.get(p)).copied();
        project.items.insert(
            cid,
            Item {
                id: cid,
                name: dcomp.name,
                label: Label::Sandstone,
                comment: String::new(),
                parent,
                kind: ItemKind::Comp(Arc::new(comp)),
                proxy: None,
            },
        );
    }

    let missing_count = missing_footage.len();
    let report = AepReport {
        project_path: project_path.map(|p| p.to_string_lossy().to_string()),
        fonts,
        missing_fonts: Vec::new(),
        missing_footage,
        total_footage_count: total_footage,
        missing_footage_count: missing_count,
        comp_count,
        plugins,
        custom_plugins,
    };

    Ok((project, report))
}

/// Decode an After Effects XML project (`.aepx`) into an [`effectcraft_project::Project`].
pub fn from_aepx(bytes: &[u8]) -> Result<Project, String> {
    let s = std::str::from_utf8(bytes).map_err(|e| format!("invalid UTF-8 in AEPX: {e}"))?;
    let t = s.trim_start_matches('\u{feff}').trim_start();
    if !is_aepx(bytes) && !t.starts_with("<?xml") {
        return Err("not an After Effects XML project".into());
    }

    let mut project = Project::default();
    let comp_w = extract_xml_num(t, "width").unwrap_or(1920) as u32;
    let comp_h = extract_xml_num(t, "height").unwrap_or(1080) as u32;
    let fps = extract_xml_num(t, "frameRate").unwrap_or(30) as f64;
    let dur_secs = extract_xml_num(t, "duration").unwrap_or(10) as f64;

    let frame_rate = FrameRate::from_f64(fps);
    let duration = Tick::from_seconds_f64(dur_secs);

    let comp = Comp {
        width: comp_w,
        height: comp_h,
        pixel_aspect: 1.0,
        frame_rate,
        duration,
        display_start: Tick::ZERO,
        background: [0.0, 0.0, 0.0],
        work_area: (Tick::ZERO, duration),
        layers: Vec::new(),
        markers: Vec::new(),
        shutter_angle: 180.0,
        shutter_phase: -90.0,
        motion_blur_samples: 16,
        motion_blur_adaptive_limit: 128,
        renderer: Default::default(),
        hide_shy: false,
        enable_motion_blur: true,
        enable_frame_blending: true,
        draft_3d: false,
        preserve_frame_rate: false,
        preserve_resolution: false,
        poster_time: Tick::ZERO,
        global_light: Default::default(),
        guides: Vec::new(),
        essential: None,
        resolution: Resolution::Auto,
    };

    let cid = ItemId(project.alloc());
    project.items.insert(
        cid,
        Item {
            id: cid,
            name: "Composition 1".into(),
            label: Label::Sandstone,
            comment: String::new(),
            parent: None,
            kind: ItemKind::Comp(Arc::new(comp)),
            proxy: None,
        },
    );

    Ok(project)
}

fn extract_xml_num(xml: &str, tag: &str) -> Option<i64> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(start) = xml.find(&open) {
        let val_start = start.saturating_add(open.len());
        if let Some(end) = xml.get(val_start..)?.find(&close) {
            let text = xml.get(val_start..val_start.saturating_add(end))?.trim();
            return text.parse::<f64>().ok().map(|v| v as i64);
        }
    }
    None
}

/// Decode an After Effects project from either binary `.aep` or XML `.aepx` format.
pub fn from_aep_or_aepx(bytes: &[u8]) -> Result<Project, String> {
    from_aep_or_aepx_with_report(bytes, None).map(|(p, _)| p)
}

/// Decode an After Effects project with a detailed report on assets, fonts, and plugins.
pub fn from_aep_or_aepx_with_report(
    bytes: &[u8],
    project_path: Option<&std::path::Path>,
) -> Result<(Project, AepReport), String> {
    if is_aep(bytes) {
        from_aep_with_report(bytes, project_path)
    } else if is_aepx(bytes) {
        let p = from_aepx(bytes)?;
        let report = AepReport {
            comp_count: p.comps().count(),
            project_path: project_path.map(|p| p.to_string_lossy().to_string()),
            ..Default::default()
        };
        Ok((p, report))
    } else {
        Err("unrecognized After Effects project format (not RIFX or AEPX)".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Constructs a valid minimal synthetic RIFX Egg! AEP byte stream entirely in memory.
    fn make_synthetic_aep() -> Vec<u8> {
        let mut buf = Vec::new();
        // 1. Build Fold child chunks (an Item with cdta and Layr)
        let mut comp_item_payload = Vec::new();

        // Item name: Utf8
        let name_bytes = b"Main Comp\0";
        comp_item_payload.extend_from_slice(b"Utf8");
        comp_item_payload.extend_from_slice(&(name_bytes.len() as u32).to_be_bytes());
        comp_item_payload.extend_from_slice(name_bytes);
        if name_bytes.len() % 2 != 0 {
            comp_item_payload.push(0);
        }

        // cdta: 180 bytes of comp metadata
        let mut cdta_bytes = vec![0u8; 180];
        // Timebase: 30720
        cdta_bytes[10] = 0x78;
        cdta_bytes[11] = 0x00;
        // Width: 1920 (0x0780) at 140
        cdta_bytes[140] = 0x07;
        cdta_bytes[141] = 0x80;
        // Height: 1080 (0x0438) at 142
        cdta_bytes[142] = 0x04;
        cdta_bytes[143] = 0x38;
        // FPS: 30.0 at 156
        cdta_bytes[156] = 0x00;
        cdta_bytes[157] = 0x1e;

        comp_item_payload.extend_from_slice(b"cdta");
        comp_item_payload.extend_from_slice(&(cdta_bytes.len() as u32).to_be_bytes());
        comp_item_payload.extend_from_slice(&cdta_bytes);

        // LIST Layr child
        let mut layr_payload = Vec::new();
        let lay_name = b"Layer 1\0";
        layr_payload.extend_from_slice(b"Utf8");
        layr_payload.extend_from_slice(&(lay_name.len() as u32).to_be_bytes());
        layr_payload.extend_from_slice(lay_name);
        if lay_name.len() % 2 != 0 {
            layr_payload.push(0);
        }

        let mut ldta_bytes = vec![0u8; 96];
        ldta_bytes[16] = 0x00;
        ldta_bytes[17] = 0x00;
        ldta_bytes[18] = 0x78;
        ldta_bytes[19] = 0x00; // timebase 30720
        ldta_bytes[28] = 0x00;
        ldta_bytes[29] = 0x04;
        ldta_bytes[30] = 0xb0;
        ldta_bytes[31] = 0x00; // out_ticks: 307200 (10s)

        layr_payload.extend_from_slice(b"ldta");
        layr_payload.extend_from_slice(&(ldta_bytes.len() as u32).to_be_bytes());
        layr_payload.extend_from_slice(&ldta_bytes);

        // Append Layr LIST to comp_item_payload
        comp_item_payload.extend_from_slice(b"LIST");
        let layr_len = (layr_payload.len() + 4) as u32;
        comp_item_payload.extend_from_slice(&layr_len.to_be_bytes());
        comp_item_payload.extend_from_slice(b"Layr");
        comp_item_payload.extend_from_slice(&layr_payload);

        // LIST Item
        let mut fold_payload = Vec::new();
        fold_payload.extend_from_slice(b"LIST");
        let item_len = (comp_item_payload.len() + 4) as u32;
        fold_payload.extend_from_slice(&item_len.to_be_bytes());
        fold_payload.extend_from_slice(b"Item");
        fold_payload.extend_from_slice(&comp_item_payload);

        // LIST Fold
        let mut egg_payload = Vec::new();
        egg_payload.extend_from_slice(b"LIST");
        let fold_len = (fold_payload.len() + 4) as u32;
        egg_payload.extend_from_slice(&fold_len.to_be_bytes());
        egg_payload.extend_from_slice(b"Fold");
        egg_payload.extend_from_slice(&fold_payload);

        // Entire RIFX container: RIFX + size + Egg! + egg_payload
        buf.extend_from_slice(b"RIFX");
        let container_size = (egg_payload.len() + 4) as u32;
        buf.extend_from_slice(&container_size.to_be_bytes());
        buf.extend_from_slice(b"Egg!");
        buf.extend_from_slice(&egg_payload);

        buf
    }

    #[test]
    fn detects_aep_signature() {
        let mut sample = Vec::new();
        sample.extend_from_slice(b"RIFX");
        sample.extend_from_slice(&100u32.to_be_bytes());
        sample.extend_from_slice(b"Egg!");
        assert!(is_aep(&sample));

        sample[0..4].copy_from_slice(b"RIFF");
        assert!(is_aep(&sample));

        sample[8..12].copy_from_slice(b"WAVE");
        assert!(!is_aep(&sample));
    }

    /// A layer's transform group is read from `tdgp` (a `tdmn` match name followed by a `tdbs`
    /// list holding the value in its `cdat` chunk): positions are comp pixels, scale and opacity
    /// are 1.0-based factors, angles are degrees, and a position Z offset means the layer is 3D.
    #[test]
    fn layer_transform_is_read_from_tdgp() {
        fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(id);
            v.extend_from_slice(&(data.len() as u32).to_be_bytes());
            v.extend_from_slice(data);
            if data.len() % 2 == 1 {
                v.push(0);
            }
            v
        }
        fn list(ltype: &[u8; 4], children: &[u8]) -> Vec<u8> {
            let mut body = Vec::new();
            body.extend_from_slice(ltype);
            body.extend_from_slice(children);
            chunk(b"LIST", &body)
        }
        fn doubles(vals: &[f64]) -> Vec<u8> {
            vals.iter().flat_map(|v| v.to_be_bytes()).collect()
        }
        let tdmn = |name: &str| {
            let mut d = vec![0u8; 40];
            d[..name.len()].copy_from_slice(name.as_bytes());
            chunk(b"tdmn", &d)
        };

        let mut tg = Vec::new();
        for (name, vals) in [
            ("ADBE Anchor Point", [540.0, 960.0, 0.0]),
            ("ADBE Position", [540.0, 960.0, -5000.0]),
            ("ADBE Scale", [1.0, 1.0, 1.0]),
        ] {
            tg.extend_from_slice(&tdmn(name));
            tg.extend_from_slice(&list(b"tdbs", &chunk(b"cdat", &doubles(&vals))));
        }
        for (name, v) in [("ADBE Rotate Z", 180.0), ("ADBE Opacity", 0.5)] {
            tg.extend_from_slice(&tdmn(name));
            tg.extend_from_slice(&list(b"tdbs", &chunk(b"cdat", &doubles(&[v]))));
        }

        let mut children = Vec::new();
        children.extend_from_slice(&chunk(b"Utf8", b"Front"));
        children.extend_from_slice(&list(b"tdgp", &tg));

        let mut file = Vec::new();
        file.extend_from_slice(b"RIFX");
        file.extend_from_slice(&0u32.to_be_bytes());
        file.extend_from_slice(b"Egg!");
        file.extend_from_slice(&list(b"Layr", &children));

        let tree = parse_rifx(&file).expect("parse");
        let ChunkNode::List { children: layer, .. } = &tree[0] else {
            panic!("expected a layer list");
        };
        let l = parse_layer(*b"Layr", layer);
        assert_eq!(l.name, "Front");
        assert_eq!(l.transform.anchor, Some([540.0, 960.0, 0.0]));
        assert_eq!(l.transform.position, Some([540.0, 960.0, -5000.0]));
        assert_eq!(l.transform.scale, Some([100.0, 100.0, 100.0]));
        assert_eq!(l.transform.rotation, Some(180.0));
        assert_eq!(l.transform.opacity, Some(50.0));
        assert!(l.transform.three_d, "a non-zero position Z means the layer is 3D");
    }

    #[test]
    fn parses_synthetic_aep_fixture() {
        let bytes = make_synthetic_aep();
        assert!(is_aep(&bytes));
        let (proj, report) = from_aep_or_aepx_with_report(&bytes, None).expect("decodes synthetic AEP");
        assert_eq!(proj.comps().count(), 1);
        assert_eq!(report.comp_count, 1);
        let comp = proj.items.values().find(|it| it.name == "Main Comp").expect("Main Comp exists");
        let c = comp.as_comp().expect("is comp");
        assert_eq!(c.width, 1920);
        assert_eq!(c.height, 1080);
        assert_eq!(c.layers.len(), 1);
        assert_eq!(c.layers[0].name, "Layer 1");
    }

    #[test]
    fn parses_synthetic_aepx_fixture() {
        let aepx_str = r#"<?xml version="1.0" encoding="UTF-8"?>
<AfterEffectsProject>
    <Composition>
        <width>1280</width>
        <height>720</height>
        <frameRate>24</frameRate>
        <duration>5</duration>
    </Composition>
</AfterEffectsProject>"#;
        let bytes = aepx_str.as_bytes();
        assert!(is_aepx(bytes));
        let (proj, report) = from_aep_or_aepx_with_report(bytes, None).expect("decodes synthetic AEPX");
        assert_eq!(proj.comps().count(), 1);
        assert_eq!(report.comp_count, 1);
        let (_id, comp) = proj.comps().next().expect("has comp");
        assert_eq!(comp.width, 1280);
        assert_eq!(comp.height, 720);
        assert_eq!(comp.duration.seconds(), 5.0);
    }

    #[test]
    fn never_panics_on_truncated_or_fuzzed_inputs() {
        let valid_aep = make_synthetic_aep();
        // Test every single prefix from 0..=len
        for len in 0..=valid_aep.len() {
            let slice = &valid_aep[..len];
            let _ = is_aep(slice);
            let _ = is_aepx(slice);
            let _ = is_aep_or_aepx("f.aep", slice);
            let _ = from_aep_or_aepx_with_report(slice, None);
        }

        // Test random garbage
        let garbage = vec![0xaa; 512];
        for len in 0..=garbage.len() {
            let slice = &garbage[..len];
            let _ = is_aep(slice);
            let _ = is_aepx(slice);
            let _ = is_aep_or_aepx("f.aepx", slice);
            let _ = from_aep_or_aepx_with_report(slice, None);
        }
    }

    #[test]
    fn json_project_named_aep_is_not_routed_to_the_binary_reader() {
        // The real file: `AI Product Promo (converted).aep`, an EffectCraft project saved as JSON.
        let json = br#"{ "savedBy": "0.4.0", "schema": 1, "settings": {}, "items": {} }"#;
        assert!(!is_aep_or_aepx("AI Product Promo (converted).aep", json));
        assert!(!is_aep_or_aepx("promo.aepx", json));
        assert!(!is_aep_or_aepx("promo.AEP", json));
        assert!(!is_aep_or_aepx("promo.aep", b""));
        assert!(!is_aep_or_aepx("promo.aep", b"not a project at all"));
        // Same bytes, a name that never claimed to be an AEP: unchanged, and not claimed now either.
        assert!(!is_aep_or_aepx("promo.ecproj", b"<?xml version=\"1.0\"?><Foo/>"));
    }

    #[test]
    fn renamed_aep_and_aepx_are_still_claimed() {
        // Content, not name: a synthetic binary .aep under any extension.
        let aep = make_synthetic_aep();
        assert!(is_aep_or_aepx("x.aep", &aep));
        assert!(is_aep_or_aepx("x.bin", &aep));
        // An AEPX saved as `.aep`, with and without its XML declaration.
        assert!(is_aep_or_aepx("x.aep", b"<?xml version=\"1.0\"?><AfterEffectsProject/>"));
        assert!(is_aep_or_aepx("x.aep", b"\r\n  <AfterEffectsProject/>"));
        // XML the sniffers don't recognise but the name does: still claimed.
        assert!(is_aep_or_aepx("x.aepx", b"<?xml version=\"1.0\"?><Foo/>"));
        // A widened (UTF-16) AEPX: BOM, then NUL-interleaved markup.
        let wide: Vec<u8> = [0xFE, 0xFF].into_iter().chain(b"<?xml version=\"1.0\"?><AfterEffectsProject/>".iter().flat_map(|b| [0u8, *b])).collect();
        assert!(is_aep_or_aepx("x.aepx", &wide));
        // Truncated to the BOM alone: no project, nothing claimed.
        assert!(!is_aep_or_aepx("x.aepx", &[0xFE, 0xFF]));
    }
}
