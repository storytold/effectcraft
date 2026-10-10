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

use crate::sfnt::{FaceNames, read_faces, read_faces_bytes};

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
        } else if cfg!(unix) && !cfg!(target_arch = "wasm32") {
            dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
            if let Some(h) = &home {
                dirs.push(h.join(".local/share/fonts"));
                dirs.push(h.join(".fonts"));
            }
        }
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
            if depth < 4 {
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
    n
}

/// Register a font from memory (e.g. a font file the user picked on the web). Returns its faces.
pub fn add_font_data(data: Vec<u8>) -> Vec<FaceId> {
    let data = Arc::new(data);
    let names = read_faces_bytes(&data);
    let mut d = db().write().unwrap_or_else(|e| e.into_inner());
    names.into_iter().map(|n| push(&mut d, info_from(n, FaceData::Shared(data.clone()), "user"))).collect()
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

/// Family names in the fonts' own languages: English family → native family (only families
/// that have one). Scans the system fonts first if that hasn't happened yet.
pub fn native_families() -> std::collections::BTreeMap<String, String> {
    scan_system();
    all_faces().into_iter().filter_map(|f| Some((f.info.family.clone(), f.info.native_family.clone()?))).collect()
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
        if !v.iter().any(|(_, _, s)| s == &f.info.style) {
            v.push((f.info.italic, f.info.weight, f.info.style.clone()));
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
    // Inter is bundled; face 0 (the first bundled face) is the last resort.
    let mut r = resolve_in(DEFAULT_FAMILY, style).unwrap_or(Resolved { face: 0, synth_bold: false, synth_italic: false, missing: true });
    r.missing = !family.is_empty() && !family.eq_ignore_ascii_case(DEFAULT_FAMILY);
    r
}

fn resolve_in(family: &str, style: &str) -> Option<Resolved> {
    let faces = all_faces();
    let fam: Vec<&Arc<Face>> = faces.iter().filter(|f| f.info.family.eq_ignore_ascii_case(family)).collect();
    if fam.is_empty() {
        return None;
    }
    if let Some(f) = fam.iter().find(|f| f.info.style.eq_ignore_ascii_case(style)) {
        return Some(Resolved { face: f.id, synth_bold: false, synth_italic: false, missing: false });
    }
    let (w, it) = style_wants(style);
    let best = fam.iter().min_by_key(|f| {
        let italic_pen = if f.info.italic == it { 0 } else { 1000 };
        italic_pen + (f.info.weight as i32 - w as i32).unsigned_abs()
    })?;
    Some(Resolved { face: best.id, synth_bold: w >= 600 && best.info.weight <= 500, synth_italic: it && !best.info.italic, missing: false })
}

/// A face whose full name or PostScript name is `name` (case-insensitive), the way After Effects
/// scripts (`textDocument.font = "YuGothic-Bold"`) and `.aep`-derived data refer to fonts.
fn resolve_by_name(name: &str) -> Option<Resolved> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let faces = all_faces();
    let f = faces
        .iter()
        .find(|f| f.info.postscript.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(name)))
        .or_else(|| faces.iter().find(|f| f.info.full_name.eq_ignore_ascii_case(name)))?;
    Some(Resolved { face: f.id, synth_bold: false, synth_italic: false, missing: false })
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
const CHINESE_FAMILIES: &[&str] = &[
    "Microsoft YaHei",
    "Microsoft JhengHei",
    "SimSun",
    "MingLiU",
    "PingFang SC",
    "PingFang TC",
    "Noto Sans CJK SC",
    "Noto Sans CJK TC",
    "Noto Sans SC",
    "Noto Sans TC",
    "Source Han Sans SC",
    "Source Han Sans TC",
    "WenQuanYi Micro Hei",
];
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
    // The fallback cache answers per character block and preferred face, so a change of language
    // must drop it.
    fallback_cache().write().unwrap_or_else(|e| e.into_inner()).clear();
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

fn families_for(s: FallbackScript) -> &'static [&'static str] {
    match s {
        FallbackScript::Japanese => JAPANESE_FAMILIES,
        FallbackScript::Han => CHINESE_FAMILIES,
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

/// Results of the expensive part of [`fallback_for`], per 256-character block and preferred face
/// (so bold text gets the bold cut of the fallback family). `None` records that nothing installed
/// covers the block, so the search over every font runs once per block, not once per character.
fn fallback_cache() -> &'static RwLock<std::collections::HashMap<(u32, FaceId), Option<FaceId>>> {
    static CACHE: OnceLock<RwLock<std::collections::HashMap<(u32, FaceId), Option<FaceId>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// A face that covers `c`, preferring `prefer`. Bundled faces and faces already loaded are tried
/// first (cheap); then, for CJK characters, the platform's usual family for that language, in the
/// preferred face's style; then any installed face that covers the character. `prefer` when
/// nothing does (the glyph renders as the missing-glyph box).
pub fn fallback_for(c: char, prefer: FaceId) -> FaceId {
    let p = face(prefer);
    if p.has_char(c) || c.is_control() {
        return prefer;
    }
    let faces = all_faces();
    // prefer a face of the same italic-ness that is bundled (cheap), then anything loaded already
    for f in faces.iter().filter(|f| f.info.origin == "bundled") {
        if f.has_char(c) {
            return f.id;
        }
    }
    for f in faces.iter().filter(|f| f.info.origin != "bundled" && f.bytes.get().is_some()) {
        if f.has_char(c) {
            return f.id;
        }
    }
    let key = (c as u32 >> 8, prefer);
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

/// The script-aware then exhaustive search behind [`fallback_for`].
fn system_fallback(c: char, style: &str) -> Option<FaceId> {
    let script = script_of(c);
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
    for s in order {
        for fam in families_for(s) {
            if let Some(r) = resolve_in(fam, style)
                && face(r.face).has_char(c)
            {
                return Some(r.face);
            }
        }
    }
    // Anything installed, in scan order; the style's weight and slant break ties among the faces
    // of the first covering family.
    let faces = all_faces();
    let first = faces.iter().find(|f| f.info.origin != "bundled" && file_covers(f, c))?;
    let r = resolve_in(&first.info.family, style)?;
    Some(if face(r.face).has_char(c) { r.face } else { first.id })
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
        let strings: Vec<Vec<u8>> = ["Blank Glyph Test", "Regular"].iter().map(|s| s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()).collect();
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
