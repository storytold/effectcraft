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

    /// Layer / media / preview cache budgets in bytes.
    pub fn layer_cache_bytes(&self) -> usize {
        (self.memory.layer_cache_mb as usize) << 20
    }

    pub fn media_cache_bytes(&self) -> usize {
        (self.memory.media_cache_mb as usize) << 20
    }

    pub fn preview_cache_bytes(&self) -> usize {
        (self.memory.preview_cache_mb as usize) << 20
    }

    /// The adaptive resolution limit as a render-scale floor (1/2 → 0.5).
    pub fn adaptive_limit(&self) -> f64 {
        match self.previews.adaptive_resolution_limit.as_str() {
            "1/2" => 0.5,
            "1/4" => 0.25,
            _ => 0.125,
        }
    }

    /// The Memory & CPU cache budgets in effect: the configured layer / footage / preview
    /// budgets, scaled down so together they leave **RAM Reserved for Other Applications** free,
    /// and halved while the system is low on memory when **Reduce Cache Size When System Is Low
    /// on Memory** is on. `mem` = the system's memory (`None` = unknown: configured budgets).
    /// In the browser the layer and footage caches also stay within [`WEB_LAYER_CACHE`] and
    /// [`WEB_MEDIA_CACHE`].
    pub fn cache_budgets(&self, mem: Option<crate::sysinfo::SysMemory>) -> CacheBudgets {
        self.cache_budgets_in(mem, cfg!(target_arch = "wasm32"))
    }

    /// [`Prefs::cache_budgets`], in the browser's memory (`web`) or not.
    pub(crate) fn cache_budgets_in(&self, mem: Option<crate::sysinfo::SysMemory>, web: bool) -> CacheBudgets {
        let (mut l, mut m, mut p) = (self.layer_cache_bytes() as u64, self.media_cache_bytes() as u64, self.preview_cache_bytes() as u64);
        if web {
            (l, m) = (l.min(WEB_LAYER_CACHE), m.min(WEB_MEDIA_CACHE));
        }
        let floor = 64u64 << 20;
        let mut capped = false;
        let mut reduced = false;
        if let Some(mem) = mem.filter(|m| m.total > 0) {
            let reserved = (self.memory.ram_reserved_gb as u64) << 30;
            let cap = mem.total.saturating_sub(reserved).max(3 * floor);
            let sum = l + m + p;
            if sum > cap {
                let k = cap as f64 / sum as f64;
                (l, m, p) = ((l as f64 * k) as u64, (m as f64 * k) as u64, (p as f64 * k) as u64);
                capped = true;
            }
            if self.memory.reduce_cache_when_low && mem.is_low() {
                (l, m, p) = (l / 2, m / 2, p / 2);
                reduced = true;
            }
        }
        CacheBudgets { layer: l.max(floor) as usize, media: m.max(floor) as usize, preview: p.max(floor) as usize, capped, reduced }
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

/// The most the layer cache holds in the browser. A wasm32 page addresses at most 4 GiB, which
/// these caches share with the app and with the decoders' own decoded frames (FilmCraft keeps
/// up to 384 MB per open movie): with the desktop's 1 GB defaults, eight stacked 1080p clips ran
/// the page out of memory a few seconds into playback.
pub const WEB_LAYER_CACHE: u64 = 512 << 20;

/// The most the footage frame cache holds in the browser (see [`WEB_LAYER_CACHE`]).
pub const WEB_MEDIA_CACHE: u64 = 256 << 20;

/// Effective cache budgets in bytes (see [`Prefs::cache_budgets`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct CacheBudgets {
    pub layer: usize,
    pub media: usize,
    pub preview: usize,
    /// Scaled down to leave the reserved RAM free.
    pub capped: bool,
    /// Halved because the system is low on memory.
    pub reduced: bool,
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
