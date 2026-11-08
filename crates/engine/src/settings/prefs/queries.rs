use super::sections::parse_hex;
use super::{Label, Prefs, Serialize, Tick};

impl Prefs {
    /// Display name of a label (Labels settings).
    pub fn label_name(&self, l: Label) -> String {
        match label_index(l) {
            Some(i) => self.labels.get(i).map(|p| p.name.clone()).unwrap_or_else(|| l.name().to_string()),
            None => l.name().to_string(),
        }
    }

    /// sRGB colour of a label (Labels settings).
    pub fn label_rgb(&self, l: Label) -> [u8; 3] {
        label_index(l).and_then(|i| self.labels.get(i)).and_then(|p| parse_hex(&p.color)).unwrap_or(l.rgb())
    }

    /// A label by its built-in name or its (renamed) display name.
    pub fn label_from_name(&self, s: &str) -> Option<Label> {
        Label::from_name(s).or_else(|| Label::ALL.iter().skip(1).copied().find(|l| self.label_name(*l).eq_ignore_ascii_case(s)))
    }

    /// The adaptive resolution limit as a render-scale floor (1/2 → 0.5).
    pub fn adaptive_limit(&self) -> f64 {
        match self.previews.adaptive_resolution_limit.as_str() {
            "1/2" => 0.5,
            "1/4" => 0.25,
            _ => 0.125,
        }
    }

    /// The memory EffectCraft's caches share (Settings ▸ Memory & CPU): the layer cache, the
    /// footage frames, the decoded OpenEXR planes and the RAM preview frames together, kept within
    /// it by the memory accountant (`effectcraft_render::memory::Accountant`), which decides
    /// what each holds. It is [`MEMORY_HEADROOM_PERCENT`] of the installed RAM left after **RAM
    /// Reserved for Other Applications** (the rest is room for EffectCraft's own working memory),
    /// halved while the system is low on memory (`low`) when **Reduce Cache Size When System Is
    /// Low on Memory** is on, and never below [`MIN_MEMORY_BUDGET`]. `mem` = the system's memory
    /// (`None` = unknown: [`FALLBACK_MEMORY_BUDGET`], in the browser [`WEB_MEMORY_BUDGET`]).
    pub fn memory_budget(&self, mem: Option<crate::sysinfo::SysMemory>, low: bool) -> MemoryBudget {
        let installed = mem.map(|m| m.total).filter(|t| *t > 0);
        let reserved = u64::from(self.memory.ram_reserved_gb) << 30;
        let mut bytes = match installed {
            // u128: `room × percent` cannot overflow.
            Some(total) => u64::try_from(u128::from(total.saturating_sub(reserved)) * u128::from(MEMORY_HEADROOM_PERCENT) / 100).unwrap_or(u64::MAX),
            None if cfg!(target_arch = "wasm32") => WEB_MEMORY_BUDGET,
            None => FALLBACK_MEMORY_BUDGET,
        };
        let reduced = installed.is_some() && low && self.memory.reduce_cache_when_low;
        if reduced {
            bytes /= 2;
        }
        let bytes = bytes.max(MIN_MEMORY_BUDGET);
        MemoryBudget { bytes: usize::try_from(bytes).unwrap_or(usize::MAX), installed, reduced }
    }

    /// Composition ▸ Motion Path: the layer-time span of a motion path to draw around `now`
    /// (layer time), for keyframes at `keys` (sorted). `None` = No Motion Path.
    pub fn motion_path_span(&self, keys: &[Tick], now: Tick) -> Option<(Tick, Tick)> {
        let (first, last) = (*keys.first()?, *keys.last()?);
        let c = &self.composition;
        match c.motion_path.as_str() {
            "none" => None,
            "seconds" => {
                let half = Tick::from_seconds_f64(c.motion_path_seconds.max(0.0) / 2.0);
                let (a, b) = ((now - half).max(first), (now + half).min(last));
                (a < b).then_some((a, b))
            }
            "keyframes" => {
                let n = (c.motion_path_keyframes.max(1) as usize).min(keys.len());
                if n < 2 {
                    return None;
                }
                // The n keyframes centred on the current time.
                let at = keys.partition_point(|k| *k < now);
                let start = at.saturating_sub(n / 2).min(keys.len() - n);
                Some((keys[start], keys[start + n - 1]))
            }
            _ => Some((first, last)),
        }
    }

    /// Previews ▸ Viewer Zoom Quality: smooth (bilinear) scaling of the viewer image, or nearest
    /// neighbour ("Faster").
    pub fn viewer_zoom_smooth(&self) -> bool {
        self.previews.zoom_quality != "faster"
    }

    /// Import ▸ Interpret Unlabeled Alpha As, for footage whose file has an alpha channel that
    /// doesn't state how it is stored. `None` = Ask User (keep straight and open Interpret
    /// Footage); `guess` takes premultiplied for movies (codecs with alpha are usually written
    /// premultiplied) and straight for stills.
    pub fn unlabeled_alpha(&self, movie: bool) -> Option<effectcraft_project::AlphaMode> {
        use effectcraft_project::AlphaMode::*;
        Some(match self.import.unlabeled_alpha.as_str() {
            "ignore" => Ignore,
            "straight" => Straight,
            "premultiplied" => Premultiplied,
            "guess" if movie => Premultiplied,
            "guess" => Straight,
            _ => return None,
        })
    }

    /// Import ▸ Default Drag Import As, as `file.import`'s `importAs`.
    pub fn drag_import_as(&self) -> &'static str {
        match self.import.drag_import_as.as_str() {
            "comp" => "composition",
            "compLayerSizes" => "compositionLayerSizes",
            _ => "footage",
        }
    }

    /// Export ▸ Append Bit Depth to File Name: `Comp 1.png` → `Comp 1_16bpc.png` (sequence frame
    /// runs and folders are kept).
    pub fn output_name(&self, path: &str, bits: u32) -> String {
        if !self.export.append_bits_to_name {
            return path.to_string();
        }
        let p = std::path::Path::new(path);
        let (Some(stem), dir) = (p.file_stem().map(|s| s.to_string_lossy().to_string()), p.parent()) else { return path.to_string() };
        let tag = format!("_{bits}bpc");
        if stem.ends_with(&tag) {
            return path.to_string();
        }
        let name = match p.extension() {
            Some(e) => format!("{stem}{tag}.{}", e.to_string_lossy()),
            None => format!("{stem}{tag}"),
        };
        match dir.filter(|d| !d.as_os_str().is_empty()) {
            Some(d) => d.join(name).to_string_lossy().to_string(),
            None => name,
        }
    }

    /// Export ▸ Segment Sequences / Segment Movie Files: how many frames go in each segment of an
    /// output of `frames` frames (`None` = one file / folder). Sequences split every Files per
    /// Segment frames; movies every Segment Size MB at the output's data rate
    /// (`bytes_per_frame`, estimated from the format's bitrate).
    pub fn segment_frames(&self, frames: u64, sequence: bool, bytes_per_frame: f64) -> Option<u64> {
        let e = &self.export;
        let per = if sequence {
            e.segment_sequences.then_some(e.segment_sequence_files.max(1) as u64)?
        } else {
            if !e.segment_movies || bytes_per_frame <= 0.0 {
                return None;
            }
            (((e.segment_movie_mb.max(1) as f64) * 1_048_576.0 / bytes_per_frame).floor() as u64).max(1)
        };
        (per < frames).then_some(per)
    }
}

/// The most the layer cache holds in the browser, inside the common budget. A wasm32 page
/// addresses at most 4 GiB, which these caches share with the app and with the decoders' own
/// decoded frames (FilmCraft keeps up to 384 MB per open movie): with 1 GB of layers, eight
/// stacked 1080p clips ran the page out of memory a few seconds into playback. On the desktop
/// no cache has a limit of its own.
pub const WEB_LAYER_CACHE: u64 = 512 << 20;

/// The most the footage frame cache holds in the browser (see [`WEB_LAYER_CACHE`]).
pub const WEB_MEDIA_CACHE: u64 = 256 << 20;

/// The part (percent) of the installed RAM left after **RAM Reserved for Other Applications**
/// that EffectCraft's caches share ([`Prefs::memory_budget`]). The rest is headroom for
/// EffectCraft's own working memory (decoders, render buffers, undo, GPU staging, the interface),
/// so that does not eat into the memory reserved for other applications.
pub const MEMORY_HEADROOM_PERCENT: u64 = 80;

/// The caches' budget while the installed RAM is unknown (the browser, a sandbox, or the desktop
/// before its first background reading): 5 GB, what the three fixed cache budgets added up to
/// before they became one.
pub const FALLBACK_MEMORY_BUDGET: u64 = 5 << 30;

/// The caches' budget in the browser, where the installed RAM is unknown and a page addresses at
/// most 4 GiB, shared with the app and the decoders' own frames: 3.5 GB, a little more than the
/// RAM preview's 3 GB there, leaving at least half a gigabyte of the page to the rest. (The
/// separate budgets there added up to 4.25 GB before they became one, more than a page can
/// spare.)
pub const WEB_MEMORY_BUDGET: u64 = 3584 << 20;

/// The most the RAM preview frames hold in the browser, inside the common budget (as before).
pub const WEB_PREVIEW_CACHE: u64 = 3 << 30;

/// The least the caches share, however little RAM is left after the reserve: 192 MB, three times
/// the smallest budget one cache had before they became one.
pub const MIN_MEMORY_BUDGET: u64 = 192 << 20;

/// The memory EffectCraft's caches share (see [`Prefs::memory_budget`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MemoryBudget {
    /// The budget in bytes.
    pub bytes: usize,
    /// The installed RAM it was taken from (`None`: unknown, the fallback).
    pub installed: Option<u64>,
    /// Halved because the system is low on memory.
    pub reduced: bool,
}

impl MemoryBudget {
    /// What each big cache may hold by itself within the budget: on the desktop all of it (no
    /// cache has a share of its own, the memory accountant decides what is kept), in the browser
    /// (`web`) also [`WEB_LAYER_CACHE`], [`WEB_MEDIA_CACHE`] and [`WEB_PREVIEW_CACHE`].
    pub fn caps(&self, web: bool) -> CacheCaps {
        let cap = |c: u64| if web { usize::try_from(c).unwrap_or(usize::MAX).min(self.bytes) } else { self.bytes };
        CacheCaps { layer: cap(WEB_LAYER_CACHE), footage: cap(WEB_MEDIA_CACHE), preview: cap(WEB_PREVIEW_CACHE) }
    }
}

/// The most each big cache holds by itself ([`MemoryBudget::caps`]), in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheCaps {
    pub layer: usize,
    pub footage: usize,
    pub preview: usize,
}

/// Colour of the `k`-th mask of a layer: Appearance ▸ Cycle Mask Colors cycles through the mask
/// colours, otherwise every new mask takes the first one.
pub fn mask_color(cycle: bool, k: usize) -> [u8; 3] {
    use effectcraft_project::build::MASK_COLORS;
    if cycle { MASK_COLORS[k % MASK_COLORS.len()] } else { MASK_COLORS[0] }
}

fn label_index(l: Label) -> Option<usize> {
    Label::ALL.iter().position(|x| *x == l).and_then(|i| i.checked_sub(1))
}
