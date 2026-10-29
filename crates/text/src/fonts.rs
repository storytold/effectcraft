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
        } else if p.extension().and_then(std::ffi::OsStr::to_str).is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc")) {
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
    fallback: Arc<Face>,
    faces: Vec<Arc<Face>>,
    scanned: bool,
}

/// Keep the compiled-in regular face available independently of discovery results.
fn bundled_default_face() -> Arc<Face> {
    let info =
        read_faces_bytes(INTER_REGULAR).into_iter().next().map(|n| info_from(n, FaceData::Static(INTER_REGULAR), "bundled")).unwrap_or_else(|| FaceInfo {
            family: DEFAULT_FAMILY.into(),
            style: "Regular".into(),
            weight: 400,
            italic: false,
            index: 0,
            data: FaceData::Static(INTER_REGULAR),
            origin: "bundled",
            native_family: None,
            full_name: "Inter Regular".into(),
            postscript: Some("Inter-Regular".into()),
            variable: false,
            instances: Vec::new(),
        });
    Arc::new(Face { id: 0, info, bytes: OnceLock::new(), shaper: OnceLock::new() })
}

fn db() -> &'static RwLock<Db> {
    static DB: OnceLock<RwLock<Db>> = OnceLock::new();
    DB.get_or_init(|| {
        let fallback = bundled_default_face();
        let mut db = Db { faces: vec![fallback.clone()], fallback, scanned: false };
        for b in [INTER_MEDIUM, INTER_SEMIBOLD, INTER_BOLD, INTER_ITALIC, JETBRAINS_MONO_REGULAR, NOTO_SERIF_REGULAR] {
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
    let mut d = db().write().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut n = 0;
    for f in faces {
        let dup = d.faces.iter().any(|g| g.info.family.eq_ignore_ascii_case(&f.family) && g.info.style.eq_ignore_ascii_case(&f.style));
        if !dup {
            push(&mut d, f);
            n += 1;
        }
    }
    // A new face can belong to one of a script's usual families.
    family_faces_cache().write().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    n
}

/// Register a font from memory (e.g. a font file the user picked on the web). Returns its faces.
pub fn add_font_data(data: Vec<u8>) -> Vec<FaceId> {
    let data = Arc::new(data);
    let names = read_faces_bytes(&data);
    let mut d = db().write().unwrap_or_else(std::sync::PoisonError::into_inner);
    let ids = names.into_iter().map(|n| push(&mut d, info_from(n, FaceData::Shared(data.clone()), "user"))).collect();
    family_faces_cache().write().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
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
    let _scan = SCAN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if system_scanned() {
        return 0;
    }
    let n = if cfg!(target_arch = "wasm32") { 0 } else { add_source(&DirectorySource::system()) };
    db().write().unwrap_or_else(std::sync::PoisonError::into_inner).scanned = true;
    n
}

/// Scan the system font folders again, adding fonts installed since the last scan (faces already
/// known are kept, so font ids stay valid). Returns the number of faces added.
pub fn rescan_system() -> usize {
    let _scan = SCAN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let n = if cfg!(target_arch = "wasm32") { 0 } else { add_source(&DirectorySource::system()) };
    db().write().unwrap_or_else(std::sync::PoisonError::into_inner).scanned = true;
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
    db().read().unwrap_or_else(std::sync::PoisonError::into_inner).scanned
}

/// A face by id (panics never: unknown ids give the default face).
pub fn face(id: FaceId) -> Arc<Face> {
    let d = db().read().unwrap_or_else(std::sync::PoisonError::into_inner);
    d.faces.get(id).cloned().unwrap_or_else(|| d.fallback.clone())
}

/// All faces (bundled first).
pub fn all_faces() -> Vec<Arc<Face>> {
    db().read().unwrap_or_else(std::sync::PoisonError::into_inner).faces.clone()
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
    if let Some(r) = resolve_in(family, style).or_else(|| resolve_by_name(family)) {
        return r;
    }
    if !system_scanned() && !family.is_empty() && !family.eq_ignore_ascii_case(DEFAULT_FAMILY) {
        scan_system();
        if let Some(r) = resolve_in(family, style).or_else(|| resolve_by_name(family)) {
            return r;
        }
    }
    if let Some(r) = resolve_alias(family, style) {
        return r;
    }
    // Inter is bundled; face 0 (the first bundled face) is the last resort.
    let mut r = resolve_in(DEFAULT_FAMILY, style).unwrap_or(Resolved { face: 0, synth_bold: false, synth_italic: false, missing: true, instance: None });
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
fn resolve_alias(family: &str, style: &str) -> Option<Resolved> {
    let family = family.trim();
    let (_, subs) = METRIC_ALIASES.iter().find(|(name, _)| name.eq_ignore_ascii_case(family))?;
    subs.iter().find_map(|s| resolve_in(s, style)).map(|r| Resolved { missing: true, ..r })
}

fn resolve_in(family: &str, style: &str) -> Option<Resolved> {
    let faces = all_faces();
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
fn resolve_by_name(name: &str) -> Option<Resolved> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let faces = all_faces();
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
    /// Georgian: Mkhedruli, Asomtavruli and Nuskhuri, and Mtavruli capitals.
    Georgian,
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
        0x10A0..=0x10FF | 0x1C90..=0x1CBF | 0x2D00..=0x2D2F => FallbackScript::Georgian,
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
const GEORGIAN_FAMILIES: &[&str] = &["Sylfaen", ".SF Georgian", "SF Georgian", "Noto Sans Georgian", "DejaVu Sans"];

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
    *REQUESTED_CJK_LOCALE.write().unwrap_or_else(std::sync::PoisonError::into_inner) = requested;
    // The order of the CJK families follows the language, so a change of language drops the cached
    // fallbacks.
    fallback_cache().write().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    family_faces_cache().write().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
}

fn requested_cjk_locale() -> Option<String> {
    REQUESTED_CJK_LOCALE.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// The CJK locale to use: [`set_cjk_locale`] when the host asked for one, otherwise the locale
/// environment (`LANG`, `LC_ALL`, `LC_CTYPE`; `EFFECTCRAFT_CJK_LOCALE` overrides them).
fn active_cjk_locale() -> String {
    requested_cjk_locale().unwrap_or_else(environment_cjk_locale)
}

/// Read the environment independently of another UI's process-wide language request.
fn environment_cjk_locale() -> String {
    ["EFFECTCRAFT_CJK_LOCALE", "LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_ascii_lowercase())
        .find(|v| !v.is_empty() && v != "c" && v != "posix")
        .unwrap_or_default()
}

/// The four distinct fallback policies, rather than unbounded user-supplied locale cache keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CjkLocale {
    Japanese,
    Korean,
    Simplified,
    Traditional,
}

impl CjkLocale {
    fn from_tag(tag: &str) -> Self {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.starts_with("zh") {
            if asks_traditional_chinese(&tag) { Self::Traditional } else { Self::Simplified }
        } else if tag.starts_with("ko") {
            Self::Korean
        } else {
            Self::Japanese
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::Simplified => "zh-hans",
            Self::Traditional => "zh-hant",
        }
    }
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
#[cfg(test)]
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

fn families_for(s: FallbackScript, locale: CjkLocale) -> &'static [&'static str] {
    match s {
        FallbackScript::Japanese => JAPANESE_FAMILIES,
        FallbackScript::Han => chinese_families_for(locale.tag()),
        FallbackScript::Korean => KOREAN_FAMILIES,
        FallbackScript::Arabic => ARABIC_FAMILIES,
        FallbackScript::Hebrew => HEBREW_FAMILIES,
        FallbackScript::Indic => INDIC_FAMILIES,
        FallbackScript::Thai => THAI_FAMILIES,
        FallbackScript::Georgian => GEORGIAN_FAMILIES,
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
/// preferred face (so bold text gets the bold cut of the fallback family). `None` records that
/// nothing installed covers the block, so the search over every font runs once per block, not once
/// per character.
fn fallback_cache() -> &'static RwLock<std::collections::HashMap<(u32, FaceId), Option<FaceId>>> {
    static CACHE: OnceLock<RwLock<std::collections::HashMap<(u32, FaceId), Option<FaceId>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The results of [`family_faces`], per script, preferred face and captured language policy.
/// A lookup that finishes after a language change cannot populate another language's entry.
fn family_faces_cache() -> &'static RwLock<std::collections::HashMap<(FallbackScript, FaceId, CjkLocale), Arc<Vec<FaceId>>>> {
    static CACHE: OnceLock<RwLock<std::collections::HashMap<(FallbackScript, FaceId, CjkLocale), Arc<Vec<FaceId>>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// A face that covers `c`, preferring `prefer`. Bundled faces are tried first (cheap). For a
/// character of a script with usual families (CJK, Arabic, Hebrew, Indic, Thai), the platform's
/// usual families for that script are tried next, in the preferred face's style. Faces already
/// loaded are tried after that (cheap), and any installed face that covers the character is tried
/// last. `prefer` is returned when nothing covers it (the glyph renders as the missing-glyph box).
pub fn fallback_for(c: char, prefer: FaceId) -> FaceId {
    fallback_with_locale(c, prefer, || CjkLocale::from_tag(&active_cjk_locale()))
}

/// Choose a fallback for an explicit interface language, independent of concurrent global
/// language changes. Empty, `c` and `posix` use the OS environment rather than the global request.
pub fn fallback_for_locale(c: char, prefer: FaceId, tag: &str) -> FaceId {
    let tag = tag.trim();
    let locale = || {
        if tag.is_empty() || tag.eq_ignore_ascii_case("c") || tag.eq_ignore_ascii_case("posix") {
            CjkLocale::from_tag(&environment_cjk_locale())
        } else {
            CjkLocale::from_tag(tag)
        }
    };
    fallback_with_locale(c, prefer, locale)
}

/// Keep a single language snapshot through the family-order lookup and cache insertion. `locale`
/// is read only for a script with usual families: reading it takes a lock and may consult the
/// environment, and layout asks for a fallback for every character.
fn fallback_with_locale(c: char, prefer: FaceId, locale: impl FnOnce() -> CjkLocale) -> FaceId {
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
        if let Some(&id) = family_faces(script, &p, locale()).iter().find(|&&id| face(id).has_char(c)) {
            return id;
        }
    }
    // Faces that are loaded already come next (cheap).
    for f in faces.iter().filter(|f| f.info.origin != "bundled" && f.bytes.get().is_some()) {
        if f.has_char(c) {
            return f.id;
        }
    }
    let key = (c as u32 >> 8, prefer);
    if let Some(cached) = fallback_cache().read().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key).copied() {
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
    fallback_cache().write().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key, found);
    found.unwrap_or(prefer)
}

/// The script-aware part of [`fallback_for`]: the installed faces of the platform's usual families
/// for `script`, in the order they are tried, each in the style of `prefer`. For CJK, the other CJK
/// languages' families follow in the locale's order.
fn family_faces(script: FallbackScript, prefer: &Face, locale: CjkLocale) -> Arc<Vec<FaceId>> {
    if let Some(faces) = family_faces_cache().read().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(script, prefer.id, locale)) {
        return faces.clone();
    }
    let order: Vec<FallbackScript> = match script {
        FallbackScript::Other => vec![],
        FallbackScript::Han => han_order_for(locale.tag()).to_vec(),
        s if s.is_cjk() => {
            let mut v = vec![s];
            v.extend(han_order_for(locale.tag()).iter().copied().filter(|o| *o != s));
            v
        }
        s => vec![s],
    };
    let style = &prefer.info.style;
    let faces: Arc<Vec<FaceId>> =
        Arc::new(order.into_iter().flat_map(|s| families_for(s, locale)).filter_map(|fam| resolve_in(fam, style)).map(|r| r.face).collect());
    family_faces_cache().write().unwrap_or_else(std::sync::PoisonError::into_inner).insert((script, prefer.id, locale), faces.clone());
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
mod tests;
