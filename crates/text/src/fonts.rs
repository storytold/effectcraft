//! The font database: bundled fonts (always available, also on the web) plus fonts discovered by
//! [`FontSource`]s (system font folders on native platforms).
//!
//! Faces are addressed by [`FaceId`] (an index that never changes for the life of the process).
//! [`resolve`] maps a family + style name (as stored in projects) to a face and the synthetic
//! bold/italic needed when the family lacks that style; a full name (`Yu Gothic Bold`) or
//! PostScript name (`YuGothic-Bold`, how After Effects scripts name fonts) is accepted too.
//! Unknown families fall back to Inter. [`fallback_for`] finds a face for a character the chosen
//! face lacks: bundled faces first, then the system's usual family for the character's script
//! (CJK), then any installed face that covers it.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

use crate::sfnt::{FaceNames, NamedInstance, read_faces, read_faces_bytes};

/// Bundled fonts (SIL OFL 1.1; see `assets/fonts/*.attribution` and ATTRIBUTION.md).
pub static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
pub static INTER_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
pub static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
pub static INTER_BOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-Bold.ttf");
pub static INTER_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/Inter-Italic.ttf");
pub static JETBRAINS_MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
pub static NOTO_SERIF_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/NotoSerif-Regular.ttf");

/// The default family for new text.
pub const DEFAULT_FAMILY: &str = "Inter";

/// Index of a face in the database.
pub type FaceId = usize;

/// Where a face's bytes come from.
#[derive(Clone, Debug)]
pub enum FaceData {
    Static(&'static [u8]),
    Shared(Arc<Vec<u8>>),
    /// A file read on first use.
    File(PathBuf),
}

/// Description of a face offered by a [`FontSource`].
#[derive(Clone, Debug)]
pub struct FaceInfo {
    pub family: String,
    pub style: String,
    pub weight: u16,
    pub italic: bool,
    pub index: u32,
    pub data: FaceData,
    /// "bundled", "system", …
    pub origin: &'static str,
    /// The family name in the font's own (non-English) language, when it has one.
    pub native_family: Option<String>,
    /// The full name (`Yu Gothic Bold`) and PostScript name (`YuGothic-Bold`), so a face can also
    /// be requested the way After Effects scripts name fonts.
    pub full_name: String,
    pub postscript: Option<String>,
    /// The face has variation axes (an `fvar` table), read from the table directory at scan time.
    pub variable: bool,
    /// A variable face's named instances (`Light` … `Black`), read from `fvar` at scan time.
    pub instances: Vec<NamedInstance>,
}

/// Something that can list fonts (system folders, a web font picker, a project's font folder…).
pub trait FontSource: Send + Sync {
    fn faces(&self) -> Vec<FaceInfo>;
}

/// Scans font folders on disk (reading only name tables). Native only; on the web there are no
/// folders and the scan finds nothing.
pub struct DirectorySource {
    pub dirs: Vec<PathBuf>,
}

impl DirectorySource {
    /// The platform's standard font folders.
    pub fn system() -> Self {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if cfg!(target_os = "macos") {
            dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from));
            if let Some(h) = &home {
                dirs.push(h.join("Library/Fonts"));
            }
        } else if cfg!(windows) {
            let win = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
            dirs.push(win.join("Fonts"));
            if let Some(l) = std::env::var_os("LOCALAPPDATA") {
                dirs.push(PathBuf::from(l).join("Microsoft\\Windows\\Fonts"));
            }
        }
        // Linux/BSD: the usual folders, the XDG data folders, a Flatpak sandbox's host font
        // folders (/run/host/…) and the folders the fontconfig configuration names.
        #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
        dirs.extend(crate::font_dirs::unix_font_dirs(&crate::font_dirs::FontEnv::from_process()));
        dirs.extend(extra_dirs(std::env::var_os("EFFECTCRAFT_FONT_DIRS").as_deref()));
        Self { dirs }
    }
}

/// Extra font folders from `EFFECTCRAFT_FONT_DIRS` (a path list, `:`-separated, `;` on Windows), so a
/// tool can hand EffectCraft its own font library without installing the fonts system-wide.
fn extra_dirs(var: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    var.map(|v| std::env::split_paths(v).filter(|p| !p.as_os_str().is_empty()).collect()).unwrap_or_default()
}

fn walk(dir: &std::path::Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            // Bounded, so a symlink loop ends.
            if depth < 8 {
                walk(&p, depth + 1, out);
            }
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc")) {
            out.push(p);
        }
    }
}

impl FontSource for DirectorySource {
    fn faces(&self) -> Vec<FaceInfo> {
        let mut files = Vec::new();
        for d in &self.dirs {
            walk(d, 0, &mut files);
        }
        files.sort();
        files.dedup();
        let mut out = Vec::new();
        for f in files {
            let Ok(mut file) = std::fs::File::open(&f) else { continue };
            for n in read_faces(&mut file) {
                out.push(info_from(n, FaceData::File(f.clone()), "system"));
            }
        }
        out
    }
}

fn info_from(n: FaceNames, data: FaceData, origin: &'static str) -> FaceInfo {
    FaceInfo {
        family: n.family,
        style: n.style,
        weight: n.weight,
        italic: n.italic,
        index: n.index,
        data,
        origin,
        native_family: n.native_family,
        full_name: n.full_name,
        postscript: n.postscript,
        variable: n.variable,
        instances: n.instances,
    }
}

/// A loaded (or loadable) face.
pub struct Face {
    pub id: FaceId,
    pub info: FaceInfo,
    bytes: OnceLock<Option<FaceBytes>>,
    shaper: OnceLock<harfrust::ShaperData>,
}

#[derive(Clone)]
enum FaceBytes {
    Static(&'static [u8]),
    Shared(Arc<Vec<u8>>),
}

impl FaceBytes {
    fn get(&self) -> &[u8] {
        match self {
            FaceBytes::Static(b) => b,
            FaceBytes::Shared(b) => b,
        }
    }
}

/// Vertical metrics at a pixel size (all positive distances, y down from the baseline for the
/// underline position).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub cap_height: f32,
    pub x_height: f32,
    pub underline_pos: f32,
    pub underline_thickness: f32,
}

impl Face {
    fn bytes(&self) -> Option<&[u8]> {
        self.bytes
            .get_or_init(|| match &self.info.data {
                FaceData::Static(b) => Some(FaceBytes::Static(b)),
                FaceData::Shared(b) => Some(FaceBytes::Shared(b.clone())),
                FaceData::File(p) => std::fs::read(p).ok().map(|v| FaceBytes::Shared(Arc::new(v))),
            })
            .as_ref()
            .map(FaceBytes::get)
    }
    /// The parsed font (None if the file is missing or unreadable).
    pub fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(self.bytes()?, self.info.index).ok()
    }
    pub(crate) fn shaper_data(&self) -> Option<&harfrust::ShaperData> {
        let f = self.font()?;
        Some(self.shaper.get_or_init(|| harfrust::ShaperData::new(&f)))
    }
    pub fn units_per_em(&self) -> f32 {
        self.font().map(|f| f.metrics(Size::unscaled(), LocationRef::default()).units_per_em as f32).filter(|u| *u > 0.0).unwrap_or(1000.0)
    }
    /// Glyph for a character (None when the face does not cover it).
    pub fn glyph(&self, c: char) -> Option<u32> {
        self.font()?.charmap().map(c).map(|g| g.to_u32()).filter(|g| *g != 0)
    }
    /// Whether the face can draw `c`: it maps `c` to a glyph that has an outline, a colour glyph
    /// or a bitmap. Some fonts map whole scripts to empty glyphs (#285); those don't count, so
    /// fallback moves on to a face that draws the character. Characters that are blank by nature
    /// (spaces, format characters) count as covered by any mapping.
    pub fn has_char(&self, c: char) -> bool {
        self.font().is_some_and(|f| covers(&f, c))
    }
    /// Horizontal advance and vertical origin above the baseline, in pixels.
    pub(crate) fn vertical_origin(&self, gid: u32, px: f32) -> Option<(f64, f64)> {
        use skrifa::raw::TableProvider;
        let font = self.font()?;
        let glyph = skrifa::GlyphId::new(gid);
        let metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
        let origin = match font.vorg() {
            Ok(table) => f32::from(table.vertical_origin_y(glyph)),
            Err(_) => metrics.bounds(glyph)?.y_max + f32::from(font.vmtx().ok()?.side_bearing(glyph)?),
        };
        let scale = f64::from(px / self.units_per_em());
        let width = f64::from(metrics.advance_width(glyph)?) * scale;
        let top = f64::from(origin) * scale;
        (width.is_finite() && top.is_finite() && top.abs() < f64::from(px) * 4.0).then_some((width, top))
    }

    pub fn metrics(&self, px: f32) -> VMetrics {
        let Some(f) = self.font() else {
            return VMetrics {
                ascent: px * 0.8,
                descent: px * 0.2,
                cap_height: px * 0.7,
                x_height: px * 0.5,
                underline_pos: px * 0.1,
                underline_thickness: px * 0.06,
                ..Default::default()
            };
        };
        let m = f.metrics(Size::new(px), LocationRef::default());
        VMetrics {
            ascent: m.ascent,
            descent: -m.descent,
            line_gap: m.leading,
            cap_height: m.cap_height.unwrap_or(m.ascent * 0.7),
            x_height: m.x_height.unwrap_or(m.ascent * 0.5),
            underline_pos: m.underline.map_or(px * 0.1, |u| -u.offset),
            underline_thickness: m.underline.map_or(px * 0.06, |u| u.thickness.max(px * 0.03)),
        }
    }
    /// The OpenType layout features the face offers (GSUB and GPOS tags, sorted, deduplicated).
    pub fn features(&self) -> Vec<String> {
        use skrifa::raw::TableProvider;
        let Some(f) = self.font() else { return vec![] };
        let mut out: Vec<String> = vec![];
        if let Ok(list) = f.gsub().and_then(|g| g.feature_list()) {
            out.extend(list.feature_records().iter().map(|r| String::from_utf8_lossy(&r.feature_tag().to_be_bytes()).into_owned()));
        }
        if let Ok(list) = f.gpos().and_then(|g| g.feature_list()) {
            out.extend(list.feature_records().iter().map(|r| String::from_utf8_lossy(&r.feature_tag().to_be_bytes()).into_owned()));
        }
        out.sort();
        out.dedup();
        out
    }
    /// Whether the face's GSUB offers an OpenType feature (e.g. `smcp`).
    pub fn has_feature(&self, tag: &[u8; 4]) -> bool {
        use skrifa::raw::TableProvider;
        let Some(f) = self.font() else { return false };
        let Ok(gsub) = f.gsub() else { return false };
        let Ok(list) = gsub.feature_list() else { return false };
        list.feature_records().iter().any(|r| r.feature_tag().to_be_bytes() == *tag)
    }
}

struct Db {
    faces: Vec<Arc<Face>>,
    scanned: bool,
}

fn db() -> &'static RwLock<Db> {
    static DB: OnceLock<RwLock<Db>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = Db { faces: Vec::new(), scanned: false };
        for b in [INTER_REGULAR, INTER_MEDIUM, INTER_SEMIBOLD, INTER_BOLD, INTER_ITALIC, JETBRAINS_MONO_REGULAR, NOTO_SERIF_REGULAR] {
            for n in read_faces_bytes(b) {
                push(&mut db, info_from(n, FaceData::Static(b), "bundled"));
            }
        }
        RwLock::new(db)
    })
}

fn push(db: &mut Db, info: FaceInfo) -> FaceId {
    let id = db.faces.len();
    db.faces.push(Arc::new(Face { id, info, bytes: OnceLock::new(), shaper: OnceLock::new() }));
    id
}

/// Add faces from a source (skipping family+style pairs already present). Returns how many were
/// added.
pub fn add_source(src: &dyn FontSource) -> usize {
    let faces = src.faces();
    let mut d = db().write().unwrap_or_else(|e| e.into_inner());
    let mut n = 0;
    for f in faces {
        let dup = d.faces.iter().any(|g| g.info.family.eq_ignore_ascii_case(&f.family) && g.info.style.eq_ignore_ascii_case(&f.style));
        if !dup {
            push(&mut d, f);
            n += 1;
        }
    }
    // Cache hits may read the database while holding a fallback-cache read lock.
    // Release the database before invalidating either cache to avoid the reverse lock order.
    drop(d);
    // A new face can belong to one of a script's usual families or cover a cached missing glyph.
    family_faces_cache().write().unwrap_or_else(|e| e.into_inner()).clear();
    fallback_cache().write().unwrap_or_else(|e| e.into_inner()).clear();
    n
}

/// Register a font from memory (e.g. a font file the user picked on the web). Returns its faces.
pub fn add_font_data(data: Vec<u8>) -> Vec<FaceId> {
    let data = Arc::new(data);
    let names = read_faces_bytes(&data);
    let mut d = db().write().unwrap_or_else(|e| e.into_inner());
    let ids = names.into_iter().map(|n| push(&mut d, info_from(n, FaceData::Shared(data.clone()), "user"))).collect();
    // Match add_source's database-before-cache lock release order.
    drop(d);
    family_faces_cache().write().unwrap_or_else(|e| e.into_inner()).clear();
    fallback_cache().write().unwrap_or_else(|e| e.into_inner()).clear();
    ids
}

/// Held for a whole system scan: a concurrent caller waits for the complete list instead of
/// returning early and resolving against a half-filled database.
static SCAN: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Scan the system font folders once (native only; a no-op on wasm). Returns the number of faces
/// added by this call.
pub fn scan_system() -> usize {
    if system_scanned() {
        return 0;
    }
    let _scan = SCAN.lock().unwrap_or_else(|e| e.into_inner());
    if system_scanned() {
        return 0;
    }
    let n = if cfg!(target_arch = "wasm32") { 0 } else { add_source(&DirectorySource::system()) };
    db().write().unwrap_or_else(|e| e.into_inner()).scanned = true;
    n
}

/// Scan the system font folders again, adding fonts installed since the last scan (faces already
/// known are kept, so font ids stay valid). Returns the number of faces added.
pub fn rescan_system() -> usize {
    let _scan = SCAN.lock().unwrap_or_else(|e| e.into_inner());
    let n = if cfg!(target_arch = "wasm32") { 0 } else { add_source(&DirectorySource::system()) };
    db().write().unwrap_or_else(|e| e.into_inner()).scanned = true;
    n
}

/// Start the system font scan on a background thread, so the font menus are complete by the
/// time someone opens them without the first open waiting for the scan. Native only; on wasm
/// (no threads, no font folders) this does nothing.
pub fn scan_system_in_background() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if system_scanned() {
            return;
        }
        // A failed spawn only means the scan happens on first use instead.
        let _ = std::thread::Builder::new().name("font-scan".into()).spawn(|| {
            scan_system();
        });
    }
}

pub fn system_scanned() -> bool {
    db().read().unwrap_or_else(|e| e.into_inner()).scanned
}

/// A face by id (panics never: unknown ids give the default face).
pub fn face(id: FaceId) -> Arc<Face> {
    let d = db().read().unwrap_or_else(|e| e.into_inner());
    // The bundled fonts are compiled in and always parse (`tests::bundled_families_and_resolution` checks them), so
    // the database is never empty.
    #[allow(clippy::expect_used)]
    d.faces.get(id).or_else(|| d.faces.first()).cloned().expect("bundled fonts present")
}

/// All faces (bundled first).
pub fn all_faces() -> Vec<Arc<Face>> {
    db().read().unwrap_or_else(|e| e.into_inner()).faces.clone()
}

/// Faces are only appended, so their count identifies the available font set without cloning it.
/// Cached layouts must be resolved again when registration or discovery adds a face.
pub(crate) fn database_revision() -> usize {
    db().read().unwrap_or_else(|e| e.into_inner()).faces.len()
}

/// Family names in the fonts' own languages: English family → native family (only families
/// that have one). Scans the system fonts first if that hasn't happened yet.
pub fn native_families() -> std::collections::BTreeMap<String, String> {
    scan_system();
    all_faces().into_iter().filter_map(|f| Some((f.info.family.clone(), f.info.native_family.clone()?))).collect()
}

/// Families with at least one variable face (variation axes), for the font menu's Variable
/// Fonts filter. Scans the system fonts first if that hasn't happened yet.
pub fn variable_families() -> std::collections::BTreeSet<String> {
    scan_system();
    all_faces().into_iter().filter(|f| f.info.variable).map(|f| f.info.family.clone()).collect()
}

/// Families with their style names, sorted by family; styles in weight order. This is what font
/// menus list, so it scans the system fonts first if that hasn't happened yet (before, only a
/// project asking for a font the bundled ones lack started the scan, and the menus offered the
/// three bundled families alone).
pub fn families() -> Vec<(String, Vec<String>)> {
    scan_system();
    let mut m: std::collections::BTreeMap<String, Vec<(bool, u16, String)>> = Default::default();
    for f in all_faces() {
        let v = m.entry(f.info.family.clone()).or_default();
        // A variable face lists its named instances (`Light` … `Black`) as styles, as After
        // Effects' font menu does; its own style name is usually one of them.
        let styles = std::iter::once((f.info.italic, f.info.weight, f.info.style.clone()))
            .chain(f.info.instances.iter().map(|i| (i.italic(f.info.italic), i.weight(f.info.weight), i.style.clone())));
        for (italic, weight, style) in styles {
            if !v.iter().any(|(_, _, s)| s.eq_ignore_ascii_case(&style)) {
                v.push((italic, weight, style));
            }
        }
    }
    m.into_iter()
        .map(|(k, mut v)| {
            v.sort();
            (k, v.into_iter().map(|(_, _, s)| s).collect())
        })
        .collect()
}

/// The face chosen for a family/style request, plus synthetic emboldening / slant when the family
/// lacks a real bold / italic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Resolved {
    pub face: FaceId,
    pub synth_bold: bool,
    pub synth_italic: bool,
    /// The requested family was not found (shown as missing in the UI).
    pub missing: bool,
    /// The named instance of a variable face the request asked for (an index into the face's
    /// [`FaceInfo::instances`]); its axis values are the text's base variations (see
    /// [`instance_variations`]).
    pub instance: Option<u16>,
}

/// The axis values a resolved request implies: the named instance's coordinates (sorted by tag),
/// or none for a static face or the face's own default.
pub fn instance_variations(r: &Resolved) -> Vec<(String, f32)> {
    let Some(i) = r.instance else { return Vec::new() };
    face(r.face).info.instances.get(i as usize).map(|n| n.coords.clone()).unwrap_or_default()
}

/// `explicit` axis values over the instance's `base` ones: a tag set explicitly wins, the others
/// keep the instance's value. Sorted by tag, as `TextDoc` keeps them.
pub fn merge_variations(base: &[(String, f32)], explicit: &[(String, f32)]) -> Vec<(String, f32)> {
    let mut out: Vec<(String, f32)> = base.iter().filter(|(t, _)| !explicit.iter().any(|(e, _)| e.trim_end() == t.trim_end())).cloned().collect();
    out.extend(explicit.iter().cloned());
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn style_wants(style: &str) -> (u16, bool) {
    let s = style.to_ascii_lowercase().replace([' ', '-'], "");
    let italic = s.contains("italic") || s.contains("oblique");
    let w = if s.contains("thin") || s.contains("hairline") {
        100
    } else if s.contains("extralight") || s.contains("ultralight") {
        200
    } else if s.contains("light") {
        300
    } else if s.contains("medium") {
        500
    } else if s.contains("semibold") || s.contains("demibold") {
        600
    } else if s.contains("extrabold") || s.contains("ultrabold") {
        800
    } else if s.contains("black") || s.contains("heavy") {
        900
    } else if s.contains("bold") {
        700
    } else {
        400
    };
    (w, italic)
}

/// Resolve a family + style name. Family matching is case-insensitive; a style that the family
/// lacks picks the nearest weight and synthesises bold (≥ 600 requested, ≤ 500 found) or italic.
pub fn resolve(family: &str, style: &str) -> Resolved {
    if let Some(r) = resolve_in(family, style).or_else(|| resolve_by_name(&all_faces(), family)) {
        return r;
    }
    if !system_scanned() && !family.is_empty() && !family.eq_ignore_ascii_case(DEFAULT_FAMILY) {
        scan_system();
    }
    resolve_available(&all_faces(), family, style)
}

/// Resolve against a fixed font list after discovery, including aliases and the bundled fallback.
/// Keeping discovery outside this step also lets tests model installed and missing families.
fn resolve_available(faces: &[Arc<Face>], family: &str, style: &str) -> Resolved {
    if let Some(r) = resolve_in_faces(faces, family, style).or_else(|| resolve_by_name(faces, family)) {
        return r;
    }
    if let Some(r) = resolve_alias(faces, family, style) {
        return r;
    }
    // Inter is bundled; face 0 (the first bundled face) is the last resort.
    let mut r =
        resolve_in_faces(faces, DEFAULT_FAMILY, style).unwrap_or(Resolved { face: 0, synth_bold: false, synth_italic: false, missing: true, instance: None });
    r.missing = !family.is_empty() && !family.eq_ignore_ascii_case(DEFAULT_FAMILY);
    r
}

/// Metric-compatible substitutes for common proprietary families, in order of preference (the
/// same pairs fontconfig's `30-metric-aliases.conf` uses). Text keeps its widths and line breaks.
const METRIC_ALIASES: &[(&str, &[&str])] = &[
    ("Calibri", &["Carlito"]),
    ("Cambria", &["Caladea"]),
    ("Arial", &["Liberation Sans", "Arimo"]),
    ("Helvetica", &["Liberation Sans", "Arimo"]),
    ("Arial Narrow", &["Liberation Sans Narrow"]),
    ("Times New Roman", &["Liberation Serif", "Tinos"]),
    ("Times", &["Liberation Serif", "Tinos"]),
    ("Courier New", &["Liberation Mono", "Cousine"]),
    ("Courier", &["Liberation Mono", "Cousine"]),
];

/// An installed metric-compatible substitute for a missing family. The result stays `missing`,
/// so Find Missing Fonts still lists the requested name.
fn resolve_alias(faces: &[Arc<Face>], family: &str, style: &str) -> Option<Resolved> {
    let family = family.trim();
    let (_, subs) = METRIC_ALIASES.iter().find(|(name, _)| name.eq_ignore_ascii_case(family))?;
    subs.iter().find_map(|s| resolve_in_faces(faces, s, style)).map(|r| Resolved { missing: true, ..r })
}

fn resolve_in(family: &str, style: &str) -> Option<Resolved> {
    resolve_in_faces(&all_faces(), family, style)
}

/// Find the requested family and nearest style within a fixed font list.
fn resolve_in_faces(faces: &[Arc<Face>], family: &str, style: &str) -> Option<Resolved> {
    let fam: Vec<&Arc<Face>> = faces.iter().filter(|f| f.info.family.eq_ignore_ascii_case(family)).collect();
    if fam.is_empty() {
        return None;
    }
    let instance_named = |f: &Face, name: &str| f.info.instances.iter().position(|i| i.style.eq_ignore_ascii_case(name)).and_then(|i| u16::try_from(i).ok());
    // A face's own style (a variable face's is its default instance) needs no axis values.
    if let Some(f) = fam.iter().find(|f| f.info.style.eq_ignore_ascii_case(style)) {
        return Some(Resolved { face: f.id, synth_bold: false, synth_italic: false, missing: false, instance: None });
    }
    // A named instance of a variable face (`Figtree` `Black`): that face at the instance's axis
    // values, not the nearest static weight made bold synthetically.
    if let Some((f, i)) = fam.iter().find_map(|f| Some((f, instance_named(f, style)?))) {
        return Some(Resolved { face: f.id, synth_bold: false, synth_italic: false, missing: false, instance: Some(i) });
    }
    // The nearest weight among the faces and the named instances.
    let (w, it) = style_wants(style);
    let candidates = fam.iter().flat_map(|f| {
        std::iter::once((f, None, f.info.weight, f.info.italic)).chain(
            f.info
                .instances
                .iter()
                .enumerate()
                .filter_map(move |(i, n)| Some((f, Some(u16::try_from(i).ok()?), n.weight(f.info.weight), n.italic(f.info.italic)))),
        )
    });
    let (best, instance, weight, italic) = candidates.min_by_key(|(_, _, weight, italic)| {
        let italic_pen = if *italic == it { 0 } else { 1000 };
        italic_pen + (*weight as i32 - w as i32).unsigned_abs()
    })?;
    Some(Resolved { face: best.id, synth_bold: w >= 600 && weight <= 500, synth_italic: it && !italic, missing: false, instance })
}

/// A face whose full name or PostScript name is `name` (case-insensitive), the way After Effects
/// scripts (`textDocument.font = "YuGothic-Bold"`) and `.aep`-derived data refer to fonts.
fn resolve_by_name(faces: &[Arc<Face>], name: &str) -> Option<Resolved> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let found = |f: &Arc<Face>, instance: Option<usize>| Resolved {
        face: f.id,
        synth_bold: false,
        synth_italic: false,
        missing: false,
        instance: instance.and_then(|i| u16::try_from(i).ok()),
    };
    if let Some(f) = faces
        .iter()
        .find(|f| f.info.postscript.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(name)))
        .or_else(|| faces.iter().find(|f| f.info.full_name.eq_ignore_ascii_case(name)))
    {
        // A variable face's own name is its default instance's (`Figtree-Light`).
        return Some(found(f, None));
    }
    // A named instance of a variable face, by its PostScript name (`Figtree-Black`, what After
    // Effects scripts use) or its full name (`Figtree Black`).
    faces.iter().find_map(|f| {
        let i = f
            .info
            .instances
            .iter()
            .position(|i| i.postscript.eq_ignore_ascii_case(name) || format!("{} {}", f.info.family, i.style).eq_ignore_ascii_case(name))?;
        Some(found(f, Some(i)))
    })
}

/// The writing systems that need a dedicated fallback family: the bundled fonts cover none of
/// them, and picking "any face with the glyph" gives Chinese glyph shapes to Japanese text (or the
/// other way round), since Han ideographs are shared, and a calligraphic display face to Arabic,
/// Hebrew, Indic or Thai text when that is the first installed face that covers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum FallbackScript {
    /// Kana: unambiguously Japanese.
    Japanese,
    /// Hangul: unambiguously Korean.
    Korean,
    /// Han ideographs, CJK punctuation and full-width forms: shared by Japanese, Chinese and
    /// Korean; the locale decides which family to try first.
    Han,
    Arabic,
    Hebrew,
    /// Devanagari, Bengali, Gurmukhi, Gujarati, Oriya, Tamil, Telugu, Kannada, Malayalam and
    /// Sinhala (one family covers them all on Windows; elsewhere each has its own).
    Indic,
    Thai,
    Other,
}

impl FallbackScript {
    /// Kana, Hangul or Han: these share the Han ideographs, so each tries the others' families
    /// next.
    fn is_cjk(self) -> bool {
        matches!(self, FallbackScript::Japanese | FallbackScript::Korean | FallbackScript::Han)
    }
}

fn script_of(c: char) -> FallbackScript {
    match c as u32 {
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => FallbackScript::Japanese,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF => FallbackScript::Korean,
        0x2E80..=0x2FDF
        | 0x3000..=0x303F
        | 0x3100..=0x312F
        | 0x3190..=0x31EF
        | 0x3200..=0x9FFF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF => FallbackScript::Han,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFE => FallbackScript::Arabic,
        0x0590..=0x05FF | 0xFB1D..=0xFB4F => FallbackScript::Hebrew,
        0x0900..=0x0DFF | 0xA8E0..=0xA8FF => FallbackScript::Indic,
        0x0E00..=0x0E7F => FallbackScript::Thai,
        _ => FallbackScript::Other,
    }
}

/// Families usually installed for each CJK language, in the order the platforms' own fallback
/// tables prefer them (Windows, macOS, then the Linux/Noto names). Missing ones cost one name
/// lookup each.
const JAPANESE_FAMILIES: &[&str] = &[
    "Yu Gothic",
    "Meiryo",
    "BIZ UDPGothic",
    "MS Gothic",
    "Yu Gothic UI",
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    "Noto Sans CJK JP",
    "Noto Sans JP",
    "Source Han Sans JP",
    "Source Han Sans",
    "IPAGothic",
    "VL Gothic",
];
/// Han ideograph families for Simplified Chinese, in the order the platforms' own fallback tables
/// prefer them (Windows, macOS, then the Linux/Noto names). Missing ones cost one name lookup each.
const SIMPLIFIED_CHINESE_FAMILIES: &[&str] =
    &["Microsoft YaHei", "SimSun", "PingFang SC", "Noto Sans CJK SC", "Noto Sans SC", "Source Han Sans SC", "WenQuanYi Micro Hei"];
/// The same for Traditional Chinese. The two scripts share almost all of their code points but not
/// the glyph forms (`东` against `東`'s neighbours, `直`, `骨`), so one list for both drew a
/// Traditional interface with Simplified forms - the faces do not differ in *coverage*, which is
/// what the fallback search checks, so the script has to pick the list.
const TRADITIONAL_CHINESE_FAMILIES: &[&str] =
    &["Microsoft JhengHei", "MingLiU", "PingFang TC", "Heiti TC", "Hiragino Sans CNS", "Noto Sans CJK TC", "Noto Sans TC", "Source Han Sans TC"];
const KOREAN_FAMILIES: &[&str] = &["Malgun Gothic", "Gulim", "Apple SD Gothic Neo", "Noto Sans CJK KR", "Noto Sans KR", "Source Han Sans KR", "NanumGothic"];
/// The platforms' plain text families for the other scripts the bundled fonts lack, in the same
/// order (Windows, macOS, then the Linux/Noto names).
const ARABIC_FAMILIES: &[&str] = &["Segoe UI", "Tahoma", "Geeza Pro", "Noto Sans Arabic", "Noto Naskh Arabic", "DejaVu Sans", "Arial"];
const HEBREW_FAMILIES: &[&str] = &["Segoe UI", "Tahoma", "Arial Hebrew", "Noto Sans Hebrew", "DejaVu Sans", "Arial"];
const INDIC_FAMILIES: &[&str] = &[
    "Nirmala UI",
    "Kohinoor Devanagari",
    "Kohinoor Bangla",
    "Kohinoor Gujarati",
    "Kohinoor Telugu",
    "Devanagari Sangam MN",
    "Bangla Sangam MN",
    "Gurmukhi Sangam MN",
    "Gujarati Sangam MN",
    "Oriya Sangam MN",
    "Tamil Sangam MN",
    "Telugu Sangam MN",
    "Kannada Sangam MN",
    "Malayalam Sangam MN",
    "Sinhala Sangam MN",
    "Noto Sans Devanagari",
    "Noto Sans Bengali",
    "Noto Sans Gurmukhi",
    "Noto Sans Gujarati",
    "Noto Sans Oriya",
    "Noto Sans Tamil",
    "Noto Sans Telugu",
    "Noto Sans Kannada",
    "Noto Sans Malayalam",
    "Noto Sans Sinhala",
];
const THAI_FAMILIES: &[&str] = &["Leelawadee UI", "Tahoma", "Thonburi", "Noto Sans Thai", "Noto Sans Thai UI", "Loma", "Garuda"];

/// The CJK locale the host asked fallbacks to prefer, when it has one (see [`set_cjk_locale`]).
static REQUESTED_CJK_LOCALE: RwLock<Option<String>> = RwLock::new(None);

/// Ask CJK font fallbacks to prefer `tag`'s language for Han ideographs instead of the locale
/// environment, so a Chinese interface is drawn with a Chinese face on a machine whose locale is
/// Japanese (and the other way round). The interface sets this from its language setting; an empty
/// tag, `c` or `posix` clears the request and returns to the environment. Process-global, like the
/// environment it replaces: every layout in the process follows it, so composition text is drawn
/// with the same language's Han forms as the interface, not only the interface chrome.
pub fn set_cjk_locale(tag: &str) {
    let tag = tag.trim().to_ascii_lowercase();
    let requested = if tag.is_empty() || tag == "c" || tag == "posix" { None } else { Some(tag) };
    *REQUESTED_CJK_LOCALE.write().unwrap_or_else(|e| e.into_inner()) = requested;
    // The order of the CJK families follows the language, so a change of language drops the cached
    // fallbacks.
    fallback_cache().write().unwrap_or_else(|e| e.into_inner()).clear();
    family_faces_cache().write().unwrap_or_else(|e| e.into_inner()).clear();
}

fn requested_cjk_locale() -> Option<String> {
    REQUESTED_CJK_LOCALE.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The CJK locale to use: [`set_cjk_locale`] when the host asked for one, otherwise the locale
/// environment (`LANG`, `LC_ALL`, `LC_CTYPE`; `EFFECTCRAFT_CJK_LOCALE` overrides them).
fn active_cjk_locale() -> String {
    requested_cjk_locale().unwrap_or_else(|| {
        ["EFFECTCRAFT_CJK_LOCALE", "LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .map(|v| v.trim().to_ascii_lowercase())
            .find(|v| !v.is_empty() && v != "c" && v != "posix")
            .unwrap_or_default()
    })
}

/// The CJK language to try first for Han ideographs. Japanese when nothing says otherwise: the
/// ambiguity only matters for ideographs, and kana / hangul in the same text pick their language
/// regardless.
fn han_order_for(tag: &str) -> [FallbackScript; 3] {
    let tag = tag.trim().to_ascii_lowercase();
    if tag.starts_with("zh") {
        [FallbackScript::Han, FallbackScript::Japanese, FallbackScript::Korean]
    } else if tag.starts_with("ko") {
        [FallbackScript::Korean, FallbackScript::Japanese, FallbackScript::Han]
    } else {
        [FallbackScript::Japanese, FallbackScript::Han, FallbackScript::Korean]
    }
}

/// The order for the locale in force ([`active_cjk_locale`]).
fn han_order() -> [FallbackScript; 3] {
    han_order_for(&active_cjk_locale())
}

/// Whether a CJK tag asks for Traditional Chinese: a `Hant` script, or a Taiwan, Hong Kong or
/// Macao region (`zh-TW`, `zh_HK`, `zh_MO.UTF-8`). Anything else Chinese - including a bare `zh` -
/// is Simplified, as in the interface's own locale mapping.
fn asks_traditional_chinese(tag: &str) -> bool {
    tag.to_ascii_lowercase().split(['-', '_', '.']).any(|p| matches!(p, "hant" | "tw" | "hk" | "mo"))
}

/// The Chinese families to try first for `tag`: one list per script, since coverage does not tell
/// the two apart - a Simplified face covers the Traditional glyphs' code points, so the fallback
/// search would keep it and draw the wrong forms.
fn chinese_families_for(tag: &str) -> &'static [&'static str] {
    if asks_traditional_chinese(tag) { TRADITIONAL_CHINESE_FAMILIES } else { SIMPLIFIED_CHINESE_FAMILIES }
}

/// [`chinese_families_for`] for the locale in force ([`active_cjk_locale`]).
fn chinese_families() -> &'static [&'static str] {
    chinese_families_for(&active_cjk_locale())
}

fn families_for(s: FallbackScript) -> &'static [&'static str] {
    match s {
        FallbackScript::Japanese => JAPANESE_FAMILIES,
        FallbackScript::Han => chinese_families(),
        FallbackScript::Korean => KOREAN_FAMILIES,
        FallbackScript::Arabic => ARABIC_FAMILIES,
        FallbackScript::Hebrew => HEBREW_FAMILIES,
        FallbackScript::Indic => INDIC_FAMILIES,
        FallbackScript::Thai => THAI_FAMILIES,
        FallbackScript::Other => &[],
    }
}

/// Whether a not-yet-loaded face covers `c`, reading its file without keeping the bytes (the
/// last-resort search may look at every installed font once).
fn file_covers(f: &Face, c: char) -> bool {
    if f.bytes.get().is_some() {
        return f.has_char(c);
    }
    let FaceData::File(p) = &f.info.data else { return f.has_char(c) };
    let Ok(data) = std::fs::read(p) else { return false };
    FontRef::from_index(&data, f.info.index).ok().is_some_and(|font| covers(&font, c))
}

/// Characters that legitimately draw nothing: whitespace and the default-ignorable code points
/// (soft hyphen, joiners, bidi controls, variation selectors…).
fn blank_by_nature(c: char) -> bool {
    c.is_whitespace()
        || matches!(c as u32,
            0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F | 0x200B..=0x200F
            | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
}

/// Whether `font` maps `c` to a glyph it can draw (see [`Face::has_char`]).
fn covers(font: &FontRef, c: char) -> bool {
    let Some(gid) = font.charmap().map(c).filter(|g| g.to_u32() != 0) else { return false };
    blank_by_nature(c) || glyph_draws(font, gid)
}

/// Whether a glyph has an outline, a colour glyph or a bitmap in any strike.
fn glyph_draws(font: &FontRef, gid: skrifa::GlyphId) -> bool {
    use skrifa::outline::{DrawSettings, OutlinePen};
    /// Counts the drawing segments of an outline.
    struct Segments(usize);
    impl OutlinePen for Segments {
        fn move_to(&mut self, _: f32, _: f32) {}
        fn line_to(&mut self, _: f32, _: f32) {
            self.0 += 1;
        }
        fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
            self.0 += 1;
        }
        fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
            self.0 += 1;
        }
        fn close(&mut self) {}
    }
    if let Some(outline) = font.outline_glyphs().get(gid) {
        let mut pen = Segments(0);
        if outline.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen).is_ok() && pen.0 > 0 {
            return true;
        }
    }
    if font.color_glyphs().get(gid).is_some() {
        return true;
    }
    let strikes = font.bitmap_strikes();
    (0..strikes.len()).any(|i| strikes.get(i).is_some_and(|s| s.get(gid).is_some()))
}

/// Results of the search over every installed font in [`fallback_for`], per 256-character block and
/// preferred face (so bold text gets the bold cut of the fallback family) and database revision.
/// A search finishing after registration cannot cache a miss for the new revision. `None` records that
/// nothing installed covers the block, so the search over every font runs once per block, not once
/// per character.
fn fallback_cache() -> &'static RwLock<std::collections::HashMap<(u32, FaceId, usize), Option<FaceId>>> {
    static CACHE: OnceLock<RwLock<std::collections::HashMap<(u32, FaceId, usize), Option<FaceId>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The results of [`family_faces`], per script, preferred face and database revision.
fn family_faces_cache() -> &'static RwLock<std::collections::HashMap<(FallbackScript, FaceId, usize), Arc<Vec<FaceId>>>> {
    static CACHE: OnceLock<RwLock<std::collections::HashMap<(FallbackScript, FaceId, usize), Arc<Vec<FaceId>>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// A face that covers `c`, preferring `prefer`. Bundled faces are tried first (cheap). For a
/// character of a script with usual families (CJK, Arabic, Hebrew, Indic, Thai), the platform's
/// usual families for that script are tried next, in the preferred face's style. Faces already
/// loaded are tried after that (cheap), and any installed face that covers the character is tried
/// last. `prefer` is returned when nothing covers it (the glyph renders as the missing-glyph box).
pub fn fallback_for(c: char, prefer: FaceId) -> FaceId {
    let p = face(prefer);
    if p.has_char(c) || c.is_control() {
        return prefer;
    }
    let faces = all_faces();
    // prefer a face of the same italic-ness that is bundled (cheap)
    for f in faces.iter().filter(|f| f.info.origin == "bundled") {
        if f.has_char(c) {
            return f.id;
        }
    }
    // The usual families of the character's script come before the faces that are loaded already
    // and before the search results of its block. Which faces are loaded depends on every earlier
    // lookup, and Arial Unicode MS, the first face in the macOS scan, covers every script with
    // usual families except Sinhala.
    let script = script_of(c);
    if script != FallbackScript::Other {
        if cfg!(not(target_arch = "wasm32")) && !system_scanned() {
            scan_system();
        }
        if let Some(&id) = family_faces(script, &p).iter().find(|&&id| face(id).has_char(c)) {
            return id;
        }
    }
    // Faces that are loaded already come next (cheap).
    for f in faces.iter().filter(|f| f.info.origin != "bundled" && f.bytes.get().is_some()) {
        if f.has_char(c) {
            return f.id;
        }
    }
    let key = (c as u32 >> 8, prefer, database_revision());
    if let Some(cached) = fallback_cache().read().unwrap_or_else(|e| e.into_inner()).get(&key).copied() {
        match cached {
            Some(id) if face(id).has_char(c) => return id,
            Some(_) => {}
            None => return prefer,
        }
    }
    if cfg!(not(target_arch = "wasm32")) && !system_scanned() {
        scan_system();
    }
    let found = system_fallback(c, &p.info.style);
    fallback_cache().write().unwrap_or_else(|e| e.into_inner()).insert(key, found);
    found.unwrap_or(prefer)
}

/// The script-aware part of [`fallback_for`]: the installed faces of the platform's usual families
/// for `script`, in the order they are tried, each in the style of `prefer`. For CJK, the other CJK
/// languages' families follow in the locale's order.
fn family_faces(script: FallbackScript, prefer: &Face) -> Arc<Vec<FaceId>> {
    // Capture before any face snapshots; a late insertion must keep its original revision.
    let key = (script, prefer.id, database_revision());
    if let Some(faces) = family_faces_cache().read().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return faces.clone();
    }
    let order: Vec<FallbackScript> = match script {
        FallbackScript::Other => vec![],
        FallbackScript::Han => han_order().to_vec(),
        s if s.is_cjk() => {
            let mut v = vec![s];
            v.extend(han_order().iter().copied().filter(|o| *o != s));
            v
        }
        s => vec![s],
    };
    let style = &prefer.info.style;
    let faces: Arc<Vec<FaceId>> = Arc::new(order.into_iter().flat_map(families_for).filter_map(|fam| resolve_in(fam, style)).map(|r| r.face).collect());
    #[cfg(test)]
    BEFORE_FAMILY_CACHE_INSERT.with_borrow_mut(|hook| {
        if let Some(hook) = hook.take() {
            hook();
        }
    });
    family_faces_cache().write().unwrap_or_else(|e| e.into_inner()).insert(key, faces.clone());
    faces
}

/// The exhaustive search behind [`fallback_for`]: anything installed, in scan order; the style's
/// weight and slant break ties among the faces of the first covering family.
fn system_fallback(c: char, style: &str) -> Option<FaceId> {
    let faces = all_faces();
    let first = faces.iter().find(|f| f.info.origin != "bundled" && file_covers(f, c))?;
    let r = resolve_in(&first.info.family, style)?;
    Some(if face(r.face).has_char(c) { r.face } else { first.id })
}

#[cfg(test)]
thread_local! {
    // Pause one test thread after resolution, with no database/cache lock held.
    static BEFORE_FAMILY_CACHE_INSERT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_font_dirs_come_from_a_path_list() {
        assert!(extra_dirs(None).is_empty());
        let v = std::env::join_paths(["/a/fonts", "/b/more"]).unwrap_or_default();
        assert_eq!(extra_dirs(Some(&v)), vec![PathBuf::from("/a/fonts"), PathBuf::from("/b/more")]);
        assert!(extra_dirs(Some(std::ffi::OsStr::new(""))).is_empty());
    }

    #[test]
    fn bundled_families_and_resolution() {
        let fams = families();
        let inter = fams.iter().find(|(f, _)| f == "Inter").expect("Inter");
        for s in ["Regular", "Medium", "SemiBold", "Bold", "Italic"] {
            assert!(inter.1.iter().any(|x| x == s), "{s} in {:?}", inter.1);
        }
        assert!(fams.iter().any(|(f, _)| f == "Noto Serif"));
        assert!(fams.iter().any(|(f, _)| f == "JetBrains Mono"));
        let b = resolve("inter", "bold");
        assert_eq!(face(b.face).info.style, "Bold");
        assert!(!b.synth_bold);
        let faces = all_faces();
        assert!(faces.iter().any(|f| f.info.family == "Noto Serif" && f.info.style == "Regular" && f.info.origin == "bundled"));
        // A real installed Bold Italic takes priority. Partial installed families can instead
        // supply the nearest weight/slant, so neither synthesis flag is fixed by the OS.
        for family in ["Noto Serif", "Inter"] {
            let resolved = resolve(family, "Bold Italic");
            let selected = face(resolved.face);
            assert!(selected.info.family.eq_ignore_ascii_case(family));
            assert!(!resolved.missing);
            let candidates: Vec<_> = faces.iter().filter(|f| f.info.family.eq_ignore_ascii_case(family)).collect();
            if let Some(exact) = candidates.iter().find(|f| f.info.style.eq_ignore_ascii_case("Bold Italic")) {
                assert_eq!(resolved.face, exact.id, "real style wins for {family}");
                assert!(!resolved.synth_bold && !resolved.synth_italic);
            } else {
                // Italic mismatch dominates any supported font weight distance.
                let distance = |f: &Face| u32::from(f.info.weight.abs_diff(700)) + if f.info.italic { 0 } else { 1_000 };
                let nearest = candidates.iter().map(|f| distance(f)).min().unwrap();
                assert_eq!(distance(&selected), nearest, "closest available style for {family}");
                assert_eq!(resolved.synth_bold, selected.info.weight <= 500);
                assert_eq!(resolved.synth_italic, !selected.info.italic);
            }
        }
    }

    #[test]
    fn missing_family_falls_back_to_inter() {
        let r = resolve("No Such Font Family 123", "Regular");
        assert_eq!(face(r.face).info.family, "Inter");
        assert!(r.missing);
    }

    /// Native: the system scan reads name tables only and resolves families installed there.
    #[test]
    fn system_scan_finds_fonts() {
        let t = std::time::Instant::now();
        scan_system(); // (another test may have triggered the scan already)
        let n = all_faces().iter().filter(|f| f.info.origin == "system").count();
        eprintln!("system fonts: {n} faces in {:?}", t.elapsed());
        assert_eq!(scan_system(), 0, "scans once");
        if cfg!(target_os = "macos") {
            assert!(n > 20, "{n}");
            let r = resolve("Helvetica", "Bold");
            assert_eq!(face(r.face).info.family, "Helvetica");
            let l = crate::layout_text("Helvetica", &crate::TextStyle { family: "Helvetica".into(), size: 30.0, ..Default::default() }, &Default::default());
            assert!(!l.missing_font && l.glyphs.iter().all(|g| g.id != 0));
        }
    }

    #[test]
    fn postscript_and_full_names_resolve() {
        // Bundled Inter: name id 6 is `Inter-Bold`, name id 4 is `Inter Bold`.
        let ps = resolve("Inter-Bold", "");
        assert_eq!(face(ps.face).info.style, "Bold");
        assert!(!ps.missing && !ps.synth_bold);
        let full = resolve("inter semibold", "");
        assert_eq!(face(full.face).info.style, "SemiBold");
        assert!(!full.missing);
        // A family name still wins over a name lookup.
        assert_eq!(face(resolve("Inter", "Bold").face).info.style, "Bold");
    }

    /// #1563: a variable font's named instances are styles of its family. `fontStyle = "Black"`,
    /// the instance's PostScript name (`font = "Figtree-Black"`) and its full name select the
    /// instance's axis values instead of the default instance made bold synthetically, and the
    /// font menus list every instance.
    #[test]
    fn variable_named_instances_resolve_and_list() {
        const FAMILY: &str = "EffectCraft Instance Test";
        let instances = [
            ("Light", 300.0, Some("EffectCraftInstanceTest-Light")),
            ("Regular", 400.0, Some("EffectCraftInstanceTest-Regular")),
            ("Black", 900.0, Some("EffectCraftInstanceTest-Black")),
            ("Extra Bold", 800.0, None),
        ];
        assert_eq!(add_font_data(crate::sfnt::test_font::variable(FAMILY, &instances, true)).len(), 1);
        let wght = |r: &Resolved| instance_variations(r);
        // By style name.
        let black = resolve(FAMILY, "Black");
        assert!(!black.missing && !black.synth_bold && !black.synth_italic, "{black:?}");
        assert_eq!(wght(&black), vec![("wght".to_string(), 900.0)]);
        assert_eq!(wght(&resolve(FAMILY, "regular")), vec![("wght".to_string(), 400.0)]);
        // The face's own style is its default instance: no axis values to set.
        let light = resolve(FAMILY, "Light");
        assert_eq!((light.face, light.instance), (black.face, None));
        // A style the family lacks takes the nearest instance's weight, no synthetic bold.
        let bold = resolve(FAMILY, "Bold");
        assert!(!bold.synth_bold, "{bold:?}");
        assert_eq!(wght(&bold), vec![("wght".to_string(), 800.0)]);
        // By PostScript name, the font's own and the derived one, and by full name.
        let ps = resolve("EffectCraftInstanceTest-Black", "Regular");
        assert!(!ps.missing && ps.face == black.face, "{ps:?}");
        assert_eq!(wght(&ps), vec![("wght".to_string(), 900.0)]);
        assert_eq!(wght(&resolve("EffectCraftInstanceTest-ExtraBold", "")), vec![("wght".to_string(), 800.0)]);
        assert_eq!(wght(&resolve("effectcraft instance test extra bold", "")), vec![("wght".to_string(), 800.0)]);
        // The font menu lists every instance, in weight order.
        let fams = families();
        let styles = &fams.iter().find(|(f, _)| f == FAMILY).unwrap().1;
        assert_eq!(styles, &["Light", "Regular", "Extra Bold", "Black"]);
        // Static fonts are untouched.
        assert_eq!(resolve("Inter", "Bold").instance, None);
    }

    /// Axis values set on the text win over the instance's; the others keep the instance's.
    #[test]
    fn explicit_variations_override_the_instance() {
        let base = vec![("wdth".to_string(), 75.0), ("wght".to_string(), 900.0)];
        assert_eq!(merge_variations(&base, &[("wght".to_string(), 450.0)]), vec![("wdth".to_string(), 75.0), ("wght".to_string(), 450.0)]);
        assert_eq!(merge_variations(&base, &[]), base);
        assert_eq!(merge_variations(&[], &[("opsz".to_string(), 12.0)]), vec![("opsz".to_string(), 12.0)]);
    }

    #[test]
    fn scripts_are_classified() {
        assert_eq!(script_of('あ'), FallbackScript::Japanese);
        assert_eq!(script_of('ｶ'), FallbackScript::Japanese);
        assert_eq!(script_of('한'), FallbackScript::Korean);
        assert_eq!(script_of('水'), FallbackScript::Han);
        assert_eq!(script_of('、'), FallbackScript::Han);
        assert_eq!(script_of('Ａ'), FallbackScript::Han);
        assert_eq!(script_of('\u{20000}'), FallbackScript::Han);
        assert_eq!(script_of('A'), FallbackScript::Other);
        assert_eq!(script_of('م'), FallbackScript::Arabic);
        assert_eq!(script_of('\u{fefb}'), FallbackScript::Arabic);
        assert_eq!(script_of('\u{5d0}'), FallbackScript::Hebrew);
        assert_eq!(script_of('क'), FallbackScript::Indic);
        assert_eq!(script_of('த'), FallbackScript::Indic);
        assert_eq!(script_of('ก'), FallbackScript::Thai);
        assert_eq!(script_of('\u{feff}'), FallbackScript::Other, "the byte order mark is no Arabic letter");
    }

    /// Native: Arabic, Hebrew, Indic and Thai text in a Latin-only family falls back to the
    /// platform's plain text family for its script (#394), not to the first installed face that
    /// covers it (on Windows a calligraphic Arabic face sorts first). Skipped per script where no
    /// family from its list is installed.
    #[test]
    fn other_scripts_fall_back_to_a_plain_system_family() {
        scan_system();
        let inter = resolve("Inter", "Regular").face;
        for (c, families) in [('م', ARABIC_FAMILIES), ('א', HEBREW_FAMILIES), ('क', INDIC_FAMILIES), ('த', INDIC_FAMILIES), ('ก', THAI_FAMILIES)] {
            let covering = families.iter().any(|n| resolve_in(n, "Regular").is_some_and(|r| face(r.face).has_char(c)));
            if !covering {
                eprintln!("{c}: no family from the list is installed; skipping");
                continue;
            }
            let f = face(fallback_for(c, inter));
            assert!(f.has_char(c), "{c}: {} lacks it", f.info.family);
            assert!(families.iter().any(|n| f.info.family.eq_ignore_ascii_case(n)), "{c} → {}", f.info.family);
        }
    }

    /// Native: Japanese text in a Latin-only family falls back to an installed Japanese family
    /// (whichever the platform ships) instead of the missing-glyph box, without any layer having
    /// loaded a CJK font first. Skipped where no family from the list is installed.
    #[test]
    fn cjk_falls_back_to_a_system_family() {
        scan_system();
        let inter = resolve("Inter", "Bold").face;
        let installed = JAPANESE_FAMILIES.iter().any(|f| resolve_in(f, "Regular").is_some());
        if !installed {
            eprintln!("no Japanese family installed; skipping");
            return;
        }
        for c in ['水', 'あ', 'ア', '。'] {
            let id = fallback_for(c, inter);
            let f = face(id);
            assert!(f.has_char(c), "{c}: {} {} lacks it", f.info.family, f.info.style);
            assert!(JAPANESE_FAMILIES.iter().any(|n| f.info.family.eq_ignore_ascii_case(n)), "{c} → {}", f.info.family);
        }
        // The bold cut of the fallback family is chosen for bold text when the family has one.
        let f = face(fallback_for('水', inter));
        let (w, _) = style_wants(&f.info.style);
        let has_bold = all_faces().iter().any(|g| g.info.family == f.info.family && style_wants(&g.info.style).0 >= 600);
        assert!(!has_bold || w >= 600, "{} {}", f.info.family, f.info.style);
        // Cached: the second call is a lookup.
        assert_eq!(fallback_for('水', inter), f.id);
    }

    /// A minimal TrueType font built in code (no font file in the repo): U+0020 and U+E000 map to
    /// an empty glyph, U+E001 to a triangle.
    fn blank_glyph_font() -> Vec<u8> {
        test_font("Blank Glyph Test")
    }

    /// The blank-glyph test font under another family name.
    fn test_font(family: &str) -> Vec<u8> {
        fn u16s(v: &[u16]) -> Vec<u8> {
            v.iter().flat_map(|x| x.to_be_bytes()).collect()
        }
        let delta_space = 1u16.wrapping_sub(0x20);
        let cmap4 = {
            let mut t = u16s(&[4, 0, 0, 6, 4, 1, 2]);
            t.extend(u16s(&[0x20, 0xE001, 0xFFFF, 0, 0x20, 0xE000, 0xFFFF, delta_space, 0x2001, 1, 0, 0, 0]));
            let len = t.len() as u16;
            t[2..4].copy_from_slice(&len.to_be_bytes());
            t
        };
        let mut cmap = u16s(&[0, 1, 3, 1, 0, 12]);
        cmap.extend(cmap4);
        // glyph 2: one contour of three on-curve points (100,0) (500,0) (300,600)
        let mut glyf = u16s(&[1, 100, 0, 500, 600, 2, 0]);
        glyf.extend([1u8, 1, 1]);
        glyf.extend(u16s(&[100, 400, (-200i16) as u16, 0, 0, 600]));
        glyf.push(0);
        let loca = u16s(&[0, 0, 0, (glyf.len() / 2) as u16]);
        let mut head = u16s(&[1, 0, 1, 0, 0, 0, 0x5F0F, 0x3CF5, 0, 1000]);
        head.extend([0u8; 16]);
        head.extend(u16s(&[100, 0, 500, 600, 0, 8, 2, 0, 0]));
        let mut hhea = u16s(&[1, 0, 800, (-200i16) as u16, 0, 600, 0, 0, 500, 1, 0, 0]);
        hhea.extend([0u8; 8]);
        hhea.extend(u16s(&[0, 3]));
        let hmtx = u16s(&[600, 0, 600, 0, 600, 0]);
        let maxp = u16s(&[1, 0, 3, 3, 1, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0]);
        let strings: Vec<Vec<u8>> = [family, "Regular"].iter().map(|s| s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()).collect();
        let mut name = u16s(&[0, 2, 30]);
        let mut off = 0;
        for (id, st) in strings.iter().enumerate() {
            name.extend(u16s(&[3, 1, 0x409, id as u16 + 1, st.len() as u16, off]));
            off += st.len() as u16;
        }
        strings.iter().for_each(|st| name.extend(st));
        let tables: [(&[u8; 4], Vec<u8>); 8] =
            [(b"cmap", cmap), (b"glyf", glyf), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", loca), (b"maxp", maxp), (b"name", name)];
        let mut out = u16s(&[1, 0, tables.len() as u16, 128, 3, 0]);
        let mut offset = 12 + 16 * tables.len();
        let mut body = vec![];
        for (tag, data) in &tables {
            out.extend(*tag);
            out.extend([0u8; 4]);
            out.extend((offset as u32).to_be_bytes());
            out.extend((data.len() as u32).to_be_bytes());
            let padded = data.len().div_ceil(4) * 4;
            body.extend(data);
            body.resize(body.len() + padded - data.len(), 0);
            offset += padded;
        }
        out.extend(body);
        out
    }

    /// A face that maps a character to an empty glyph doesn't cover it, so fallback skips it
    /// (#285); a blank space still counts, and a drawn glyph is found as before.
    #[test]
    fn an_empty_glyph_does_not_cover_its_character() {
        let ids = add_font_data(blank_glyph_font());
        let &[id] = ids.as_slice() else { panic!("one face expected: {ids:?}") };
        let f = face(id);
        assert_eq!(f.info.family, "Blank Glyph Test");
        assert!(f.glyph('\u{e000}').is_some(), "the cmap maps it");
        assert!(!f.has_char('\u{e000}'), "but its glyph is empty");
        assert!(f.has_char('\u{e001}'), "a drawn glyph covers its character");
        assert!(f.has_char(' '), "a space is blank by nature");
        let inter = resolve("Inter", "Regular").face;
        assert_ne!(fallback_for('\u{e000}', inter), id, "fallback never picks the empty glyph");
        assert_eq!(fallback_for('\u{e001}', inter), id, "fallback finds the drawn one");
    }

    /// A family-order search paused before publication must not poison later fallbacks when
    /// registration clears the cache while that search is in flight.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn registration_during_family_search_preserves_preferred_fallback() {
        const CHILD: &str = "EFFECTCRAFT_TEST_FAMILY_SEARCH_REGISTRATION";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "fonts::tests::registration_during_family_search_preserves_preferred_fallback", "--nocapture"])
                .env(CHILD, "1")
                .output()
                .unwrap();
            eprintln!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert!(output.status.success());
            return;
        }
        db().write().unwrap().scanned = true;
        assert!(all_faces().iter().all(|f| f.info.origin == "bundled"));
        let c = 'あ';
        let triangle = |family| {
            let mut data = test_font(family);
            let table = data[12..12 + 8 * 16].as_chunks::<16>().0.iter().find(|t| &t[..4] == b"cmap").unwrap();
            let cmap = u32::from_be_bytes(table[8..12].try_into().unwrap()) as usize + 12;
            for (offset, value) in [(16, c as u16), (24, c as u16), (30, 2u16.wrapping_sub(c as u16))] {
                data[cmap + offset..cmap + offset + 2].copy_from_slice(&value.to_be_bytes());
            }
            data
        };
        let unrelated = add_font_data(triangle("Unrelated Loaded Fallback"))[0];
        assert!(face(unrelated).has_char(c), "load the lower-priority covering face first");
        let primary = resolve(DEFAULT_FAMILY, "Regular").face;
        let script = script_of(c);
        assert_eq!(script, FallbackScript::Japanese);
        let (ready, built) = std::sync::mpsc::sync_channel(0);
        let (resume, paused) = std::sync::mpsc::sync_channel(0);
        let worker = std::thread::spawn(move || {
            BEFORE_FAMILY_CACHE_INSERT.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move || {
                    ready.send(()).unwrap();
                    paused.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
                }));
            });
            family_faces(script, &face(primary))
        });
        built.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        let preferred = add_font_data(triangle("Yu Gothic"))[0];
        assert!(face(preferred).bytes.get().is_none(), "do not preload the preferred face");
        assert!(family_faces_cache().read().unwrap().is_empty(), "registration cleared the cache");
        resume.send(()).unwrap();
        assert!(worker.join().unwrap().is_empty(), "the paused search predates Yu Gothic");
        assert_eq!(family_faces_cache().read().unwrap().len(), 1, "the old search actually published its empty list after registration");
        assert_eq!(fallback_for(c, primary), preferred, "the late empty list must not send new requests to the unrelated loaded face");
        let layout = crate::layout_text(&c.to_string(), &crate::TextStyle::default(), &Default::default());
        assert!(!layout.glyphs.is_empty());
        assert!(layout.glyphs.iter().all(|g| g.id != 0 && g.face == preferred));
    }

    /// A cached missing glyph must be retried when either registration API adds an unloaded
    /// covering face. Each child uses only bundled/synthetic fonts and its own fallback cache.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn font_registration_refreshes_cached_fallback_misses() {
        const CHILD: &str = "EFFECTCRAFT_TEST_REGISTERED_FALLBACK";
        let Ok(mode) = std::env::var(CHILD) else {
            let mut failures = Vec::new();
            for mode in ["data", "source"] {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "fonts::tests::font_registration_refreshes_cached_fallback_misses", "--nocapture"])
                    .env(CHILD, mode)
                    .output()
                    .unwrap();
                eprintln!("{mode}:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
                if !output.status.success() {
                    failures.push(mode);
                }
            }
            assert!(failures.is_empty(), "registration modes failed: {failures:?}");
            return;
        };
        db().write().unwrap().scanned = true;
        assert!(all_faces().iter().all(|f| f.info.origin == "bundled"));
        let c = '\u{e001}';
        assert_eq!(script_of(c), FallbackScript::Other);
        let style = crate::TextStyle::default();
        let primary = resolve(&style.family, &style.style).face;
        let missing = crate::layout_text(&c.to_string(), &style, &Default::default());
        assert!(!missing.glyphs.is_empty());
        assert!(missing.glyphs.iter().all(|g| g.id == 0));
        let key = (c as u32 >> 8, primary, database_revision());
        assert_eq!(fallback_cache().read().unwrap().get(&key), Some(&None));
        let previous_revision = database_revision();
        let data = test_font("Registered Fallback Test");
        let id = if mode == "data" {
            add_font_data(data)[0]
        } else {
            assert_eq!(mode, "source");
            struct Source(FaceInfo);
            impl FontSource for Source {
                fn faces(&self) -> Vec<FaceInfo> {
                    vec![self.0.clone()]
                }
            }
            let data = Arc::new(data);
            let names = read_faces_bytes(&data).into_iter().next().unwrap();
            assert_eq!(add_source(&Source(info_from(names, FaceData::Shared(data), "test"))), 1);
            all_faces().last().unwrap().id
        };
        assert!(database_revision() > previous_revision);
        assert!(fallback_cache().read().unwrap().get(&key).is_none(), "registration must clear cached searches");
        // Model a pre-registration search that completes after the cache was cleared.
        // Its old-revision miss must not poison the next layout's fallback search.
        fallback_cache().write().unwrap().insert(key, None);
        // Do not call has_char or font here: loading the face would bypass the stale miss.
        assert!(face(id).bytes.get().is_none());
        let refreshed = crate::layout_text(&c.to_string(), &style, &Default::default());
        assert!(!Arc::ptr_eq(&missing, &refreshed), "the layout revision must change");
        assert!(!refreshed.glyphs.is_empty());
        assert!(
            refreshed.glyphs.iter().all(|g| g.id != 0 && g.face == id),
            "{mode}: refreshed layout must use the registered fallback, got {:?}",
            refreshed.glyphs
        );
        assert_eq!(fallback_for(c, primary), id);
    }

    /// A Microsoft family that isn't installed renders in its installed metric-compatible
    /// substitute instead of Inter, and still reports itself missing (#1677).
    #[test]
    fn microsoft_families_resolve_to_metric_compatible_substitutes() {
        // No system scan or shared database: another test may discover real Microsoft fonts.
        let mut fonts = Db { faces: Vec::new(), scanned: true };
        let mut add = |family: &str| {
            let data = Arc::new(test_font(family));
            for names in read_faces_bytes(&data) {
                push(&mut fonts, info_from(names, FaceData::Shared(data.clone()), "test"));
            }
        };
        for family in [DEFAULT_FAMILY, "Carlito", "Caladea"] {
            add(family);
        }
        // Without a substitute, a missing Microsoft family still uses the bundled default.
        let missing = resolve_available(&fonts.faces[..1], "Calibri", "Regular");
        assert_eq!(fonts.faces[missing.face].info.family, DEFAULT_FAMILY);
        assert!(missing.missing);
        for (requested, substitute, style, bold) in [("Calibri", "Carlito", "Regular", false), ("Cambria", "Caladea", "Bold", true)] {
            let r = resolve_available(&fonts.faces, requested, style);
            assert_eq!(fonts.faces[r.face].info.family, substitute);
            assert!(r.missing, "the original name stays missing for Find Missing Fonts");
            assert_eq!(r.synth_bold, bold);
        }
        let unknown = resolve_available(&fonts.faces, "No Such Family 1677", "Regular");
        assert_eq!(fonts.faces[unknown.face].info.family, DEFAULT_FAMILY);
        assert!(unknown.missing);

        // An installed original must win over its substitute and must not be marked missing.
        for family in ["Calibri", "Cambria"] {
            let data = Arc::new(test_font(family));
            for names in read_faces_bytes(&data) {
                push(&mut fonts, info_from(names, FaceData::Shared(data.clone()), "test"));
            }
            let r = resolve_available(&fonts.faces, family, "Regular");
            assert_eq!(fonts.faces[r.face].info.family, family);
            assert!(!r.missing);
        }
    }

    /// Exercise the public resolver with a known database, including glyph layout. A child keeps
    /// these deliberately installed original families out of other tests' font database.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn public_resolution_preserves_alias_style_and_original_precedence() {
        const CHILD: &str = "EFFECTCRAFT_TEST_ISOLATED_METRIC_FONTS";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "fonts::tests::public_resolution_preserves_alias_style_and_original_precedence", "--nocapture"])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            return;
        }
        // Model discovery having completed without any installed fonts. Do not consult the host.
        db().write().unwrap().scanned = true;
        assert!(all_faces().iter().all(|f| f.info.origin == "bundled"));
        for request in ["Calibri", "No Such Metric Font"] {
            let r = resolve(request, "Bold Italic");
            assert_eq!(face(r.face).info.family, DEFAULT_FAMILY);
            assert!(r.missing && r.synth_bold && !r.synth_italic, "{r:?}");
        }
        for request in ["", "inter", "Inter-Bold", " inter bold "] {
            let r = resolve(request, "Regular");
            assert_eq!(face(r.face).info.family, DEFAULT_FAMILY);
            assert!(!r.missing, "{request}: {r:?}");
        }
        for &(original, substitutes) in METRIC_ALIASES {
            // Try the second choice before the first. Some aliases share the same substitutes.
            for substitute in substitutes.iter().rev() {
                add_font_data(test_font(substitute));
                let r = resolve(original, "Regular");
                let expected = substitutes.iter().find(|s| all_faces().iter().any(|f| f.info.family == **s)).unwrap();
                assert_eq!(face(r.face).info.family, *expected, "{original}");
                assert!(r.missing && !r.synth_bold && !r.synth_italic, "{r:?}");
            }
            for (style, bold, italic) in [("Regular", false, false), ("Bold", true, false), ("Italic", false, true), ("Bold Italic", true, true)] {
                let r = resolve(&original.to_ascii_lowercase(), style);
                assert_eq!(face(r.face).info.family, substitutes[0], "{original}: {style}");
                assert_eq!((r.missing, r.synth_bold, r.synth_italic, r.instance), (true, bold, italic, None), "{original}: {style}");
            }
            let layout = crate::layout_text("\u{e001}", &crate::TextStyle { family: original.into(), size: 30.0, ..Default::default() }, &Default::default());
            assert!(layout.missing_font && !layout.glyphs.is_empty(), "{original}");
            assert!(layout.glyphs.iter().all(|g| g.id != 0 && face(g.face).info.family == substitutes[0]), "{original}");
            add_font_data(test_font(original));
            let r = resolve(original, "Bold Italic");
            assert_eq!(face(r.face).info.family, original);
            assert_eq!((r.missing, r.synth_bold, r.synth_italic), (false, true, true));
            let layout = crate::layout_text("\u{e001}", &crate::TextStyle { family: original.into(), size: 30.0, ..Default::default() }, &Default::default());
            assert!(!layout.missing_font && !layout.glyphs.is_empty(), "{original}");
            assert!(layout.glyphs.iter().all(|g| g.id != 0 && face(g.face).info.family == original), "{original}");
        }
        // Named variable instances must keep their index through the public name lookup.
        add_font_data(crate::sfnt::test_font::variable("Carlito", &[("Black", 900.0, Some("MetricTest-Black"))], true));
        let black = resolve("MetricTest-Black", "Regular");
        assert_eq!((black.instance, black.missing, black.synth_bold), (Some(0), false, false));
        assert_eq!(instance_variations(&black), vec![("wght".into(), 900.0)]);
        eprintln!("public matrix: {} aliases; absent/substitute/original, 4 styles, glyph layout and named instance passed", METRIC_ALIASES.len());
    }

    /// A fixed list must not consult or mutate the process database, even for name/instance
    /// lookup. Subsets deliberately retain sparse face ids, as production snapshots do.
    #[test]
    fn fixed_face_lists_keep_names_instances_and_missing_fonts_isolated() {
        let mut fonts = Db { faces: Vec::new(), scanned: true };
        let data = Arc::new(crate::sfnt::test_font::variable("Carlito", &[("Black", 900.0, Some("IsolatedMetric-Black"))], true));
        for names in read_faces_bytes(INTER_REGULAR) {
            push(&mut fonts, info_from(names, FaceData::Static(INTER_REGULAR), "test"));
        }
        for family in ["Unused Gap", "Calibri"] {
            let data = Arc::new(test_font(family));
            for names in read_faces_bytes(&data) {
                push(&mut fonts, info_from(names, FaceData::Shared(data.clone()), "test"));
            }
        }
        for names in read_faces_bytes(&data) {
            push(&mut fonts, info_from(names, FaceData::Shared(data.clone()), "test"));
        }
        let substitute: Vec<_> = fonts.faces.iter().filter(|f| f.info.family != "Calibri" && f.info.family != "Unused Gap").cloned().collect();
        let original: Vec<_> = fonts.faces.iter().filter(|f| f.info.family == "Inter" || f.info.family == "Calibri").cloned().collect();
        let absent = &fonts.faces[..1];
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..32 {
                        for request in ["Carlito", "IsolatedMetric-Black", "Carlito Black", "Calibri"] {
                            let r = resolve_available(&substitute, request, "Black");
                            assert_eq!(r.face, 3, "{request}");
                            assert_eq!((r.instance, r.missing, r.synth_bold, r.synth_italic), (Some(0), request == "Calibri", false, false));
                        }
                        let r = resolve_available(&original, "Calibri", "Regular");
                        assert_eq!((r.face, r.missing, r.instance), (2, false, None));
                        for request in ["Calibri", "IsolatedMetric-Black"] {
                            let r = resolve_available(absent, request, "Regular");
                            assert_eq!((r.face, r.missing, r.instance), (0, true, None));
                        }
                    }
                });
            }
        });
        assert_eq!(fonts.faces.len(), 4);
        assert!(fonts.scanned);
    }

    #[test]
    fn the_han_order_follows_the_cjk_locale() {
        // Chinese first for any Chinese tag, whether or not it names a script.
        for tag in ["zh", "zh-CN", "zh_Hans", "ZH-Hant-TW.UTF-8", "zh_MO"] {
            assert_eq!(han_order_for(tag)[0], FallbackScript::Han, "{tag}");
        }
        assert_eq!(han_order_for("ko-KR"), [FallbackScript::Korean, FallbackScript::Japanese, FallbackScript::Han]);
        // Japanese, and anything else, keeps the Japanese face first for Han text.
        for tag in ["ja_JP.UTF-8", "en-US", "de", ""] {
            assert_eq!(han_order_for(tag), [FallbackScript::Japanese, FallbackScript::Han, FallbackScript::Korean], "{tag}");
        }
        // The host's request is what `han_order` reads (the ui test covers the effect on the
        // registered fallbacks; this only checks that clearing it returns to the environment).
        assert_eq!(requested_cjk_locale(), None, "tests run without a request");
        assert_eq!(han_order(), han_order_for(&active_cjk_locale()));
    }

    #[test]
    fn the_chinese_family_list_follows_the_script_the_locale_asks_for() {
        for tag in ["zh", "zh-CN", "zh_Hans", "zh_CN.UTF-8", "zh-SG"] {
            assert_eq!(chinese_families_for(tag), SIMPLIFIED_CHINESE_FAMILIES, "{tag}");
        }
        for tag in ["zh-Hant", "zh-Hant-TW", "zh_TW", "zh-HK", "zh_MO.UTF-8"] {
            assert_eq!(chinese_families_for(tag), TRADITIONAL_CHINESE_FAMILIES, "{tag}");
        }
        // Each list carries its own script's faces and not the other's, which is the point of
        // splitting them: `PingFang SC` covers the same code points as `PingFang TC`, so the
        // fallback search would have kept it and drawn Simplified forms in a Traditional interface.
        assert!(SIMPLIFIED_CHINESE_FAMILIES.iter().any(|f| f.contains("PingFang SC") && !f.contains("Source")));
        assert!(!SIMPLIFIED_CHINESE_FAMILIES.iter().any(|f| f.contains("TC") || f.contains("JhengHei")));
        assert!(TRADITIONAL_CHINESE_FAMILIES.iter().any(|f| f.contains("PingFang TC")));
        assert!(!TRADITIONAL_CHINESE_FAMILIES.iter().any(|f| f.contains(" SC") || f.contains("YaHei") || f.contains("WenQuanYi")));
        assert!(TRADITIONAL_CHINESE_FAMILIES.iter().any(|f| f.contains("Heiti TC") || f.contains("Hiragino Sans CNS")));
        // The arm the fallback itself walks: Han hands over the list the locale's script picked.
        assert_eq!(families_for(FallbackScript::Han), chinese_families_for(&active_cjk_locale()));
        // Han text is the only thing these lists serve; the Japanese order does not change.
        assert_eq!(han_order_for("ja_JP.UTF-8")[0], FallbackScript::Japanese);
    }

    #[test]
    fn bundled_ui_fonts_cover_the_ukrainian_alphabet() {
        const ALPHABET: &str = "АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯабвгґдеєжзиіїйклмнопрстуфхцчшщьюя’«»";
        for (name, bytes) in
            [("Inter Regular", INTER_REGULAR), ("Inter Medium", INTER_MEDIUM), ("Inter SemiBold", INTER_SEMIBOLD), ("JetBrains Mono", JETBRAINS_MONO_REGULAR)]
        {
            let font = FontRef::new(bytes).unwrap();
            for c in ALPHABET.chars() {
                assert!(covers(&font, c), "{name} cannot draw {c}");
            }
        }
    }

    #[test]
    fn metrics_and_coverage() {
        let f = face(resolve("Inter", "Regular").face);
        let m = f.metrics(100.0);
        assert!(m.ascent > 80.0 && m.ascent < 110.0, "{m:?}");
        assert!(m.descent > 10.0 && m.descent < 40.0);
        assert!(m.cap_height > 60.0 && m.cap_height < 80.0);
        assert!(f.has_char('A') && f.has_char('Ж'));
        assert!(!f.has_char('\u{5d0}'), "Inter has no Hebrew");
        assert!(f.has_feature(b"liga") || f.has_feature(b"calt"));
    }
}
