//! Timeline documents → compositions (File ▸ Import ▸ Adobe Premiere Pro Project / Timeline
//! Interchange…).

use std::collections::{BTreeMap, HashMap, HashSet};

use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::build::Ids;
use effectcraft_project::{AlphaMode, Comp, Footage, FootageKind, ItemId, ItemKind, Layer, LayerSource, Marker, Node, ParamUi, Project, Property, Solid};
use effectcraft_time::{FrameRate, Tick};
use filmcraft_media::{Generator, MediaKind};
use filmcraft_project as fp;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, TimelineFormat, rate_in, tick_in};

/// Import options.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ImportOptions {
    /// Directory relative media paths are resolved against (the document's folder).
    pub base_dir: Option<String>,
    /// Name of the Project panel folder the import goes in (the document's file name).
    pub name: String,
    /// EDLs carry no frame rate: the rate to read timecode at (default: guessed).
    pub edl_frame_rate: Option<f64>,
}

/// What an import created.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ImportResult {
    pub format: Option<TimelineFormat>,
    /// The folder holding everything imported.
    pub folder: ItemId,
    /// Compositions of the document's top-level sequences, in document order.
    pub comps: Vec<ItemId>,
    /// Every composition created (nested sequences included).
    pub all_comps: Vec<ItemId>,
    /// Footage and solid items created.
    pub items: Vec<ItemId>,
    /// Media files that could not be found or read (imported as placeholders).
    pub missing: Vec<String>,
    /// Everything that could not be represented exactly.
    pub warnings: Vec<String>,
}

/// Import a timeline document into `project`. `probe` reads a media file's footage metadata
/// (None = missing or unreadable: the item becomes a placeholder with the document's metadata).
/// The deepest element / array nesting accepted in XML and OTIO documents. Real timelines nest
/// a few levels per nested sequence; parsing several thousand levels overflowed the stack.
pub const MAX_DEPTH: usize = 256;

/// The deepest element nesting of an XML document (a quick scan that skips comments, CDATA,
/// processing instructions, declarations and quoted attribute values).
pub(crate) fn xml_depth(b: &[u8]) -> usize {
    let (mut i, mut depth, mut max) = (0usize, 0usize, 0usize);
    let skip_to = |from: usize, end: &[u8]| b.get(from..).and_then(|r| r.windows(end.len()).position(|w| w == end)).map_or(b.len(), |p| from + p + end.len());
    while let Some(&c) = b.get(i) {
        if c != b'<' {
            i += 1;
            continue;
        }
        let rest = b.get(i..).unwrap_or_default();
        if rest.starts_with(b"<!--") {
            i = skip_to(i + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = skip_to(i + 9, b"]]>");
        } else if rest.starts_with(b"<?") {
            i = skip_to(i + 2, b"?>");
        } else if rest.starts_with(b"<!") {
            i = skip_to(i + 2, b">");
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = skip_to(i + 2, b">");
        } else {
            // A start tag: find its end outside quotes; `/>` closes it at once.
            let mut j = i + 1;
            let mut quote = None;
            while let Some(&c) = b.get(j) {
                match quote {
                    Some(q) if c == q => quote = None,
                    Some(_) => {}
                    None if c == b'"' || c == b'\'' => quote = Some(c),
                    None if c == b'>' => break,
                    None => {}
                }
                j += 1;
            }
            if b.get(j.wrapping_sub(1)) != Some(&b'/') {
                depth += 1;
                max = max.max(depth);
            }
            i = j + 1;
        }
    }
    max
}

/// The deepest array / object nesting of a JSON document (strings skipped).
pub(crate) fn json_depth(b: &[u8]) -> usize {
    let (mut depth, mut max, mut in_str, mut esc) = (0usize, 0usize, false, false);
    for &c in b {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

pub fn import(
    project: &mut Project,
    bytes: &[u8],
    format: Option<TimelineFormat>,
    opts: &ImportOptions,
    probe: &mut dyn FnMut(&str) -> Option<Footage>,
) -> Result<ImportResult> {
    let format = match format {
        Some(f) => f,
        None => TimelineFormat::detect(bytes, Some(&opts.name)).ok_or(Error::UnknownFormat)?,
    };
    // The XML and JSON parsers recurse per nesting level: a small file nested thousands of levels
    // deep overflowed the stack and aborted the app.
    let depth = match format {
        TimelineFormat::Fcp7Xml | TimelineFormat::Fcpxml => xml_depth(bytes),
        TimelineFormat::Otio => json_depth(bytes),
        _ => 0,
    };
    if depth > MAX_DEPTH {
        return Err(Error::TooDeep(MAX_DEPTH));
    }
    // FilmCraft's parser expands internal entities without a node limit (#2129); Final Cut Pro
    // and FCPXML exports never declare any.
    if matches!(format, TimelineFormat::Fcp7Xml | TimelineFormat::Fcpxml) && bytes.windows(8).any(|w| w == b"<!ENTITY") {
        return Err(Error::XmlEntities);
    }
    let fc_opts = filmcraft_interchange::ImportOptions {
        base_dir: opts.base_dir.clone(),
        edl_frame_rate: opts.edl_frame_rate.map(|f| {
            let r = FrameRate::from_f64(f);
            crate::rate_out(r)
        }),
        name: Some(opts.name.trim_end_matches(|c| c != '.').trim_end_matches('.').to_string()).filter(|s| !s.is_empty()),
    };
    let (mut imported, report) = filmcraft_interchange::import_with(bytes, format.fc(), &fc_opts)?;
    // Each file is probed once, though an EDL's media are probed before the layers are made.
    let mut probed: HashMap<String, Option<Footage>> = HashMap::new();
    // An EDL's guessed paths that leave the list's folder are not probed (see `guessed_outside`).
    let outside = if format == TimelineFormat::Edl { guessed_outside(&imported.project, opts.base_dir.as_deref()) } else { HashSet::new() };
    let mut probe_once = |path: &str| if outside.contains(path) { None } else { probed.entry(path.to_string()).or_insert_with(|| probe(path)).clone() };
    let left_on_timecode = if format == TimelineFormat::Edl { rebase_to_file_timecode(&mut imported.project, &mut probe_once) } else { Vec::new() };
    let mut res = ImportResult { format: Some(format), ..Default::default() };
    for e in &report.entries {
        // The reader's "rebase after probing" note is done above: name only the media it wasn't.
        if format == TimelineFormat::Edl && e.message.starts_with(EDL_MEDIA_TIME_NOTE) {
            if !left_on_timecode.is_empty() {
                res.warnings
                    .push(format!("EDL source timecodes stay media time for {}: the file is missing or has no start timecode", left_on_timecode.join(", ")));
            }
            continue;
        }
        if e.count > 1 { res.warnings.push(format!("{} (×{})", e.message, e.count)) } else { res.warnings.push(e.message.clone()) }
    }
    let folder_name = if opts.name.is_empty() { "Imported Timeline".to_string() } else { opts.name.clone() };
    res.folder = project.add_item(&folder_name, Label::None, None, ItemKind::Folder);
    let warned = res.warnings.iter().cloned().collect();
    let mut im = Importer {
        src: &imported.project,
        project,
        res,
        warned,
        items: HashMap::new(),
        solids: HashMap::new(),
        solids_folder: None,
        bins: HashMap::new(),
        probe_offline: format == TimelineFormat::Edl,
    };
    im.run(&imported.sequences, &mut probe_once);
    Ok(im.res)
}

/// The start of the note FilmCraft's EDL reader adds when source timecodes look like real
/// timecode (an hour or more) and it has kept them as media time.
const EDL_MEDIA_TIME_NOTE: &str = "EDL source timecodes were imported as media time";

/// An EDL's source times are its source timecodes, which FilmCraft's reader keeps as media time.
/// The largest start timecode, in frames, a file may have: a day at 1000 fps.
const MAX_FILE_TIMECODE: i64 = 24 * 3600 * 1000;

/// The paths an EDL reader guessed (media it flags offline: a line without `SOURCE FILE` names
/// only its clip) that are not inside the list's folder: a clip name of `../x.mov` or an absolute
/// one would link a file elsewhere on the disk. They stay placeholders. A `SOURCE FILE` path is
/// the list's own choice and is kept, also when another clip guesses the same path.
fn guessed_outside(project: &fp::Project, base_dir: Option<&str>) -> HashSet<String> {
    let inside = |path: &str| {
        let p = std::path::Path::new(path);
        let rest = match base_dir.filter(|b| !b.is_empty()) {
            Some(b) => p.strip_prefix(b).ok(),
            None => Some(p),
        };
        rest.is_some_and(|r| r.components().all(|c| matches!(c, std::path::Component::Normal(_))))
    };
    let (mut guessed, mut named) = (HashSet::new(), HashSet::new());
    for it in project.items.values() {
        if let fp::ItemKind::Media(m) = &it.kind
            && let fp::MediaRef::File { path } = &m.media
        {
            if m.offline && !inside(path) {
                guessed.insert(path.clone());
            } else {
                named.insert(path.clone());
            }
        }
    }
    guessed.retain(|p| !named.contains(p));
    guessed
}

/// Put the clips of each file that has a start timecode on the file's own time, as FilmCraft does
/// when it links the media of an imported EDL. Returns the names of the media that stay on
/// timecode though their clips use an hour or more of it (the file is missing or has none).
fn rebase_to_file_timecode(project: &mut fp::Project, probe: &mut dyn FnMut(&str) -> Option<Footage>) -> Vec<String> {
    let mut on_timecode: HashSet<fp::ItemId> = HashSet::new();
    for it in project.items.values() {
        if let fp::ItemKind::Sequence(seq) = &it.kind {
            // 01:00:00:00 as the list counts it: 01:00:00;00 drop-frame at 29.97 fps is 3599.996 s
            let (rate, df) = (seq.settings.frame_rate, seq.settings.drop_frame);
            let hour = rate.tick_of(filmcraft_time::fields_to_frames(1, 0, 0, 0, rate, df));
            on_timecode.extend(seq.all_tracks().flat_map(|t| &t.items).filter(|ti| ti.source_in >= hour).map(|ti| ti.item));
        }
    }
    let files: Vec<(fp::ItemId, String, String)> = project
        .items
        .values()
        .filter_map(|it| match &it.kind {
            fp::ItemKind::Media(m) => match &m.media {
                fp::MediaRef::File { path } => Some((it.id, it.name.clone(), path.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let mut left = Vec::new();
    for (id, name, path) in files {
        match probe(&path) {
            // A corrupt timecode past a day of frames (at up to 1000 fps) would overflow the tick
            // arithmetic; such a file stays on timecode.
            Some(Footage { file_timecode: Some(tc), native_rate, frame_rate, .. }) if (0..=MAX_FILE_TIMECODE).contains(&tc) => {
                filmcraft_interchange::rebase_source_timecode(project, id, tc, crate::rate_out(native_rate.unwrap_or(frame_rate)));
            }
            _ if on_timecode.contains(&id) => left.push(name),
            _ => {}
        }
    }
    left.sort();
    left.dedup();
    left
}

struct Importer<'a> {
    src: &'a fp::Project,
    project: &'a mut Project,
    res: ImportResult,
    /// The warnings in `res`, to say each once without searching them all every time.
    warned: HashSet<String>,
    /// Source item → created item (footage, comp).
    items: HashMap<fp::ItemId, ItemId>,
    /// Generator / adjustment solids by (source item, width, height).
    solids: HashMap<(fp::ItemId, u32, u32), ItemId>,
    solids_folder: Option<ItemId>,
    /// Folder of each source item (from the bins).
    bins: HashMap<fp::ItemId, ItemId>,
    /// Probe media the reader flags offline too. An EDL line without `SOURCE FILE` names its clip and
    /// FilmCraft guesses the path beside the list; other readers flag media offline for other reasons
    /// (an OTIO missing reference also gets a guessed path), and a same-named file there is not
    /// the intended clip.
    probe_offline: bool,
}

impl Importer<'_> {
    fn warn(&mut self, m: impl Into<String>) {
        let m = m.into();
        if self.warned.insert(m.clone()) {
            self.res.warnings.push(m);
        }
    }

    fn run(&mut self, top: &[fp::ItemId], probe: &mut dyn FnMut(&str) -> Option<Footage>) {
        // Bins → folders.
        let root = self.src.root.clone();
        self.walk_bin(&root, self.res.folder);
        // Media → footage; sequences → (empty) comps first, so nested sequences resolve.
        let ids: Vec<fp::ItemId> = self.src.items.keys().copied().collect();
        for id in &ids {
            let it = &self.src.items[id];
            let folder = self.bins.get(id).copied().unwrap_or(self.res.folder);
            match &it.kind {
                fp::ItemKind::Media(m) => {
                    if let fp::MediaRef::File { path } = &m.media {
                        let f = self.footage(&it.name, path, m, probe);
                        let label = match f.kind {
                            FootageKind::Still | FootageKind::Sequence => Label::Lavender,
                            FootageKind::Audio => Label::SeaFoam,
                            _ => Label::Aqua,
                        };
                        let nid = self.project.add_item(&it.name, label, Some(folder), ItemKind::Footage(f));
                        self.items.insert(*id, nid);
                        self.res.items.push(nid);
                    }
                }
                fp::ItemKind::Sequence(seq) => {
                    let s = &seq.settings;
                    let rate = rate_in(s.frame_rate);
                    let dur = rate.snap_nearest(tick_in(seq.duration())).max(rate.frame_duration());
                    let mut c = Comp::new(s.width.max(1), s.height.max(1), rate, dur);
                    c.pixel_aspect = if s.par.1 > 0 { s.par.0 as f64 / s.par.1 as f64 } else { 1.0 };
                    c.display_start = rate.tick_of(seq.start_timecode);
                    let nid = self.project.add_item(&it.name, Label::Sandstone, Some(folder), ItemKind::Comp(std::sync::Arc::new(c)));
                    self.items.insert(*id, nid);
                    self.res.all_comps.push(nid);
                }
                _ => {}
            }
        }
        for id in &ids {
            if let fp::ItemKind::Sequence(seq) = &self.src.items[id].kind {
                let cid = self.items[id];
                self.fill_comp(cid, seq);
            }
        }
        self.res.comps = top.iter().filter_map(|s| self.items.get(s).copied()).collect();
        self.res.all_comps.sort_by_key(|c| (!self.res.comps.contains(c), self.res.comps.iter().position(|x| x == c), c.0));
    }

    fn walk_bin(&mut self, bin: &fp::Bin, folder: ItemId) {
        for e in &bin.children {
            match e {
                fp::BinEntry::Item(i) => {
                    self.bins.insert(*i, folder);
                }
                fp::BinEntry::Bin(b) => {
                    let f = self.project.add_item(&b.name, Label::None, Some(folder), ItemKind::Folder);
                    self.walk_bin(b, f);
                }
            }
        }
    }

    fn footage(&mut self, name: &str, path: &str, m: &fp::MediaClip, probe: &mut dyn FnMut(&str) -> Option<Footage>) -> Footage {
        // An EDL's offline media is probed: the clip is named and the file may sit beside the list.
        // A path that is not there probes to nothing and stays a placeholder.
        if (!m.offline || self.probe_offline)
            && let Some(mut f) = probe(path)
        {
            if let Some(r) = m.interpret.frame_rate
                && matches!(f.kind, FootageKind::Video | FootageKind::Sequence)
            {
                f.native_rate = Some(f.frame_rate);
                f.frame_rate = rate_in(r);
            }
            if m.interpret.ignore_alpha {
                f.alpha = AlphaMode::Ignore;
            }
            f.invert_alpha |= m.interpret.invert_alpha;
            return f;
        }
        // Placeholder from the document's metadata.
        if !self.res.missing.iter().any(|p| p == path) {
            self.res.missing.push(path.to_string());
        }
        let info = &m.info;
        let v = info.video.as_ref();
        let kind = match info.kind {
            MediaKind::AudioOnly => FootageKind::Audio,
            MediaKind::Still => FootageKind::Still,
            MediaKind::ImageSequence => FootageKind::Sequence,
            _ if v.is_none() && info.has_audio() => FootageKind::Audio,
            _ => FootageKind::Video,
        };
        let _ = name;
        Footage {
            path: path.to_string(),
            kind,
            width: v.map(|v| v.width).unwrap_or(0),
            height: v.map(|v| v.height).unwrap_or(0),
            pixel_aspect: v.map(|v| if v.par.1 > 0 { v.par.0 as f64 / v.par.1 as f64 } else { 1.0 }).unwrap_or(1.0),
            frame_rate: rate_in(info.frame_rate()),
            duration: tick_in(info.duration),
            has_video: kind != FootageKind::Audio,
            has_audio: info.has_audio() || kind == FootageKind::Audio,
            alpha: if v.is_some_and(|v| v.has_alpha) { AlphaMode::Straight } else { AlphaMode::Ignore },
            codec: v.map(|v| v.codec.clone()).unwrap_or_default(),
            missing: true,
            ..Default::default()
        }
    }

    /// A solid for a generator / adjustment layer item at a sequence's frame size.
    fn solid(&mut self, src: fp::ItemId, name: &str, color: [f32; 3], w: u32, h: u32) -> ItemId {
        if let Some(id) = self.solids.get(&(src, w, h)) {
            return *id;
        }
        let folder = match self.solids_folder {
            Some(f) => f,
            None => {
                let f = self.project.add_item("Solids", Label::None, Some(self.res.folder), ItemKind::Folder);
                self.solids_folder = Some(f);
                f
            }
        };
        let id = self.project.add_item(name, Label::Red, Some(folder), ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }));
        self.solids.insert((src, w, h), id);
        self.res.items.push(id);
        id
    }

    fn fill_comp(&mut self, cid: ItemId, seq: &fp::Sequence) {
        let mut comp = self.project.comp(cid).cloned().unwrap_or_else(|| Comp::new(1920, 1080, FrameRate::new(30, 1), Tick(1)));
        comp.markers = seq.markers.iter().map(marker_in).collect();
        // Video: the top track's clips first (layer #1); later clips of a track above earlier ones
        // so incoming clips of dissolves cover outgoing ones.
        for track in seq.video_tracks.iter().rev().chain(&seq.audio_tracks) {
            self.track_layers(&mut comp, seq, track);
        }
        if !seq.caption_tracks.is_empty() {
            self.warn("caption tracks are not imported");
        }
        if let Some(c) = self.project.comp_mut(cid) {
            *c = comp;
        }
    }

    /// The layer source for a clip's item (creating solids on demand).
    fn source_of(&mut self, ti: &fp::TrackItem, w: u32, h: u32) -> Option<(LayerSource, (u32, u32), bool)> {
        let mut id = ti.item;
        if let Some(fp::ItemKind::Subclip { parent, .. }) = self.src.item(id).map(|i| &i.kind) {
            id = *parent;
        }
        let it = self.src.item(id)?;
        match &it.kind {
            fp::ItemKind::Media(m) => match &m.media {
                fp::MediaRef::File { .. } => {
                    let nid = *self.items.get(&id)?;
                    let (sz, has_audio) = match self.project.item(nid).map(|i| &i.kind) {
                        Some(ItemKind::Footage(f)) => ((f.width, f.height), f.has_audio),
                        _ => ((w, h), false),
                    };
                    Some((LayerSource::Footage { item: nid }, sz, has_audio))
                }
                fp::MediaRef::Generator(g) => {
                    let color = match g {
                        Generator::ColorMatte { color } => [color[0], color[1], color[2]],
                        Generator::BlackVideo => [0.0; 3],
                        Generator::TransparentVideo | Generator::Tone { .. } => {
                            self.warn(format!("\"{}\" ({}) has no picture in a composition and was left out", ti.name, g.label()));
                            return None;
                        }
                        _ => {
                            self.warn(format!("generator \"{}\" was imported as a grey solid", g.label()));
                            [0.5; 3]
                        }
                    };
                    let sid = self.solid(id, &it.name, color, w, h);
                    Some((LayerSource::Solid { item: sid }, (w, h), false))
                }
            },
            fp::ItemKind::Sequence(s) => {
                let nid = *self.items.get(&id)?;
                if s.multicam.is_some() {
                    self.warn("multi-camera clips were imported as precomps of their source sequence");
                }
                Some((LayerSource::Comp { item: nid }, (s.settings.width, s.settings.height), !s.audio_tracks.is_empty()))
            }
            fp::ItemKind::AdjustmentLayer { .. } => {
                let sid = self.solid(id, &it.name, [1.0; 3], w, h);
                Some((LayerSource::Solid { item: sid }, (w, h), false))
            }
            fp::ItemKind::Graphic { .. } => {
                self.warn("graphics (titles, shapes) are not imported");
                None
            }
            fp::ItemKind::Subclip { .. } => None,
        }
    }

    /// A track's clips as layers of `comp`, the latest first.
    fn track_layers(&mut self, comp: &mut Comp, seq: &fp::Sequence, track: &fp::Track) {
        let mut items: Vec<&fp::TrackItem> = track.items.iter().collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.start));
        // Each clip's transitions, found once per track: scanning every transition of the track
        // for every clip made a long flat edit (many clips and many transitions) quadratic.
        let mut transitions: HashMap<fp::ClipId, Vec<&fp::Transition>> = HashMap::new();
        for tr in &track.transitions {
            for clip in [tr.from, tr.to].into_iter().flatten() {
                let list = transitions.entry(clip).or_default();
                // A transition from and to the same clip still counts once.
                if !list.last().is_some_and(|last| std::ptr::eq(*last, tr)) {
                    list.push(tr);
                }
            }
        }
        let mut any = false;
        for ti in items {
            let own = transitions.get(&ti.id).map(Vec::as_slice).unwrap_or_default();
            if let Some(l) = self.clip_layer(comp, seq, track, ti, own) {
                comp.layers.push(l);
                any = true;
            }
        }
        // What the track's transitions become, said once a clip of the track was imported.
        if any && track.kind != fp::TrackKind::Audio {
            for tr in &track.transitions {
                let id = tr.effect.effect.as_str();
                if !matches!(id, "cross_dissolve" | "dip_to_black" | "dip_to_white" | "film_dissolve" | "additive_dissolve" | "non_additive_dissolve") {
                    self.warn(format!("transition \"{}\" was imported as a cross dissolve", tr.effect.def().map(|d| d.name).unwrap_or(id)));
                }
            }
        }
        if any && track.transitions.iter().any(|tr| tr.effect.effect == "dip_to_white") {
            self.warn("Dip to White was imported as a dip to the background");
        }
    }

    fn clip_layer(&mut self, comp: &Comp, seq: &fp::Sequence, track: &fp::Track, ti: &fp::TrackItem, transitions: &[&fp::Transition]) -> Option<Layer> {
        let audio_track = track.kind == fp::TrackKind::Audio;
        let (source, size, has_audio) = self.source_of(ti, comp.width, comp.height)?;
        let is_adjustment = matches!(self.src.item(ti.item).map(|i| &i.kind), Some(fp::ItemKind::AdjustmentLayer { .. }));
        if audio_track && !has_audio {
            return None;
        }
        let mut l = effectcraft_project::build::layer(self.project, comp, &ti.name, source.clone(), size, None);
        if is_adjustment {
            l.switches.adjustment = true;
        }
        // ---- timing
        let start = tick_in(ti.start);
        let dur = tick_in(ti.duration);
        // A zero, tiny (below 1e-9) or non-finite speed plays at 100 %. Otherwise the layer's
        // stretch (100 / speed) is held to the range the app supports (±1 % to ±9900 %): a speed
        // of 1e300 or 1e-300 from a damaged file made the layer and audio time divide by ~0
        // (#3247). The speed follows the stretch so the start time below matches it.
        let raw = ti.speed.abs();
        let (speed, stretch) = if !raw.is_finite() || raw < 1e-9 {
            // A speed of exactly 0 keeps its old silent 100 %; a tiny or non-finite one is named.
            if !raw.is_finite() || raw > 0.0 {
                self.warn(format!("clip \"{}\": a speed of {} is not supported (100 % used)", ti.name, ti.speed));
            }
            (1.0, 100.0)
        } else {
            let stretch = effectcraft_project::limits::stretch(100.0 / raw);
            if stretch.to_bits() == (100.0 / raw).to_bits() {
                (raw, stretch)
            } else {
                let speed = 100.0 / stretch;
                self.warn(format!("clip \"{}\": a speed of {:e} % is outside what a layer supports ({} % used)", ti.name, raw * 100.0, speed * 100.0));
                (speed, stretch)
            }
        };
        let reverse = ti.reverse || ti.speed < 0.0;
        let src_in = tick_in(ti.source_in);
        let span = Tick((dur.0 as f64 * speed).round() as i64);
        l.stretch = if reverse { -stretch } else { stretch };
        l.start_time =
            if reverse { start + Tick(((src_in + span).0 as f64 / speed).round() as i64) } else { start - Tick((src_in.0 as f64 / speed).round() as i64) };
        l.in_point = start;
        l.out_point = start + dur;
        if (speed - 1.0).abs() > 1e-9 || reverse {
            // Keep the layer time of the in point exact (rounding of the start time).
            let want = if reverse { src_in + span } else { src_in };
            let got = l.layer_time(start);
            if got != want {
                l.start_time += Tick(((got - want).0 as f64 * l.stretch / 100.0).round() as i64);
            }
        }
        // Transitions extend the layer over their handles.
        let mut fades: Vec<(Tick, Tick, f64, f64)> = vec![];
        for tr in transitions {
            let (ts, te) = (tick_in(tr.start), tick_in(tr.end()));
            let dip = tr.effect.effect.starts_with("dip_to");
            let mid = ts + Tick((te - ts).0 / 2);
            if tr.to == Some(ti.id) {
                l.in_point = l.in_point.min(ts);
                if dip && tr.from.is_some() {
                    fades.push((mid, te, 0.0, 1.0));
                } else {
                    fades.push((ts, te, 0.0, 1.0));
                }
            }
            if tr.from == Some(ti.id) {
                l.out_point = l.out_point.max(te);
                if tr.to.is_none() || audio_track {
                    fades.push((ts, te, 1.0, 0.0));
                } else if dip {
                    fades.push((ts, mid, 1.0, 0.0));
                }
            }
        }
        l.in_point = l.in_point.max(Tick::ZERO);
        l.out_point = l.out_point.min(comp.duration).max(l.in_point + comp.frame_duration());
        // ---- switches
        if audio_track {
            l.switches.video = false;
            l.switches.audio = ti.enabled && !track.muted;
            l.name = comp.unique_layer_name(&format!("{} (audio)", ti.name));
        } else {
            l.switches.video = ti.enabled && track.enabled;
            if has_audio {
                // The clip's sound comes from its audio-track layer.
                l.switches.audio = false;
            }
        }
        if ti.frame_hold.is_some() || ti.effects.iter().any(|e| e.effect == "time_remap" && e.params.values().any(|p| p.is_animated())) {
            self.time_remap(&mut l, ti);
        }
        // Keyframe times: clip time → layer time.
        let to_layer = |l: &Layer, k: filmcraft_time::Tick| l.layer_time(start + tick_in(k));
        if audio_track {
            self.audio_levels(&mut l, ti, track, &fades, to_layer);
        } else {
            self.motion(&mut l, ti, comp, size, to_layer);
            self.opacity(&mut l, ti, &fades, to_layer);
            let other: Vec<&str> = ti
                .effects
                .iter()
                .filter(|e| !matches!(e.effect.as_str(), "motion" | "opacity" | "time_remap"))
                .filter_map(|e| e.def().map(|d| d.name))
                .collect();
            for n in other {
                self.warn(format!("effect \"{n}\" is not imported"));
            }
            if ti.effects.iter().any(|e| !e.masks.is_empty()) {
                self.warn("effect masks are not imported");
            }
        }
        let _ = seq;
        Some(l)
    }

    fn time_remap(&mut self, l: &mut Layer, ti: &fp::TrackItem) {
        if !matches!(l.source, LayerSource::Footage { .. } | LayerSource::Comp { .. }) {
            return;
        }
        let Some(h) = ti.frame_hold else {
            self.warn("variable speed (time remapping keyframes) is not imported; the clip's average speed was used");
            return;
        };
        let mut ids = Ids(&mut self.project.next_id);
        let mut p = ids.prop("timeRemap", "Time Remap", Value::Scalar(tick_in(h).seconds())).with_ui(ParamUi::Number);
        let (a, b) = (l.layer_time(l.in_point), l.layer_time(l.out_point));
        p.keys = vec![Keyframe::new(a.min(b), Value::Scalar(tick_in(h).seconds())).hold(), Keyframe::new(a.max(b), Value::Scalar(tick_in(h).seconds())).hold()];
        l.props.children.insert(0, Node::Prop(p));
    }

    fn motion(&mut self, l: &mut Layer, ti: &fp::TrackItem, comp: &Comp, size: (u32, u32), to_layer: impl Fn(&Layer, filmcraft_time::Tick) -> Tick) {
        let Some(m) = ti.effect("motion") else { return };
        if !m.enabled {
            return;
        }
        let fit = if ti.scale_to_frame && size.0 > 0 && size.1 > 0 { (comp.width as f64 / size.0 as f64).min(comp.height as f64 / size.1 as f64) } else { 1.0 };
        let lc = l.clone();
        let Some(tr) = l.transform_mut() else { return };
        // Position / anchor (NaN = centred).
        for (src, dst) in [("position", "position"), ("anchor", "anchor")] {
            let Some(p) = m.param(src) else { continue };
            let v2 = |v: &fp::ParamValue| v.as_vec2().filter(|v| v.x.is_finite() && v.y.is_finite()).map(|v| Value::Vec3([v.x, v.y, 0.0]));
            if let Some(dp) = tr.get_mut(dst) {
                set_prop(dp, p, |v| v2(v), &lc, &to_layer);
            }
        }
        // Scale: uniform, or Scale (height) + Scale Width.
        if let (Some(s), Some(dp)) = (m.param("scale"), tr.get_mut("scale")) {
            let uniform = m.param("uniform_scale").and_then(|p| p.value.as_bool()).unwrap_or(true);
            let sw = m.param("scale_width").filter(|_| !uniform);
            match sw {
                None => set_prop(dp, s, |v| v.as_f64().map(|x| Value::Vec3([x * fit, x * fit, 100.0])), &lc, &to_layer),
                Some(w) => {
                    let mut times: Vec<filmcraft_time::Tick> = s.keyframes.iter().chain(&w.keyframes).map(|k| k.time).collect();
                    times.sort();
                    times.dedup();
                    if times.is_empty() {
                        dp.value = Value::Vec3([w.f64_at(filmcraft_time::Tick::ZERO) * fit, s.f64_at(filmcraft_time::Tick::ZERO) * fit, 100.0]);
                    } else {
                        dp.keys =
                            times.iter().map(|t| Keyframe::new(to_layer(&lc, *t), Value::Vec3([w.f64_at(*t) * fit, s.f64_at(*t) * fit, 100.0]))).collect();
                        dp.value = dp.keys[0].value.clone();
                    }
                }
            }
        }
        if let (Some(r), Some(dp)) = (m.param("rotation"), tr.get_mut("rotation")) {
            set_prop(dp, r, |v| v.as_f64().map(Value::Scalar), &lc, &to_layer);
        }
    }

    fn opacity(&mut self, l: &mut Layer, ti: &fp::TrackItem, fades: &[(Tick, Tick, f64, f64)], to_layer: impl Fn(&Layer, filmcraft_time::Tick) -> Tick) {
        let lc = l.clone();
        if let Some(o) = ti.effect("opacity").filter(|e| e.enabled) {
            if let Some(b) = o.param("blend").and_then(|p| match p.value {
                fp::ParamValue::Choice(i) => fp::effect::BLEND_MODES.get(i as usize).copied(),
                _ => None,
            }) {
                match BlendMode::from_name(b) {
                    Some(m) => l.blend_mode = m,
                    None => self.warn(format!("blend mode \"{b}\" was imported as Normal")),
                }
            }
            if let (Some(p), Some(dp)) = (o.param("opacity"), l.transform_mut().and_then(|t| t.get_mut("opacity"))) {
                set_prop(dp, p, |v| v.as_f64().map(Value::Scalar), &lc, &to_layer);
            }
        }
        if !fades.is_empty() {
            let fades: Vec<(Tick, Tick, f64, f64)> = fades.iter().map(|f| (lc.layer_time(f.0), lc.layer_time(f.1), f.2, f.3)).collect();
            if let Some(dp) = l.transform_mut().and_then(|t| t.get_mut("opacity")) {
                apply_fades(dp, &fades, |v, f| Value::Scalar(v.as_f64() * f));
            }
        }
    }

    fn audio_levels(
        &mut self,
        l: &mut Layer,
        ti: &fp::TrackItem,
        track: &fp::Track,
        fades: &[(Tick, Tick, f64, f64)],
        to_layer: impl Fn(&Layer, filmcraft_time::Tick) -> Tick,
    ) {
        let lc = l.clone();
        let base = ti.gain_db + track.volume_db;
        let Some(dp) = l.props.prop_mut("audio/levels") else { return };
        if let Some(v) = ti.effect("volume").filter(|e| e.enabled).and_then(|e| e.param("level")) {
            set_prop(dp, v, |v| v.as_f64().map(|db| Value::Vec2([db + base, db + base])), &lc, &to_layer);
        } else if base != 0.0 {
            dp.value = Value::Vec2([base, base]);
        }
        if !fades.is_empty() {
            let fades: Vec<(Tick, Tick, f64, f64)> = fades.iter().map(|f| (lc.layer_time(f.0), lc.layer_time(f.1), f.2, f.3)).collect();
            apply_fades(dp, &fades, |v, f| {
                let g = 20.0 * f.max(1e-3).log10();
                let [a, b] = v.as_vec2();
                Value::Vec2([a + g, b + g])
            });
        }
    }
}

fn marker_in(m: &fp::Marker) -> Marker {
    let comment = match (m.name.is_empty(), m.comment.is_empty()) {
        (false, false) => format!("{}: {}", m.name, m.comment),
        (false, true) => m.name.clone(),
        _ => m.comment.clone(),
    };
    Marker {
        time: tick_in(m.start),
        duration: tick_in(m.duration),
        comment,
        chapter: if m.kind == fp::MarkerKind::Chapter { m.name.clone() } else { String::new() },
        ..Default::default()
    }
}

fn interp_in(k: Keyframe, i: fp::Interpolation) -> Keyframe {
    match i {
        fp::Interpolation::Linear => k,
        fp::Interpolation::Hold => {
            let mut k = k;
            k.out_interp = effectcraft_keyframe::Interp::Hold;
            k
        }
        _ => k.eased(),
    }
}

/// Copy a FilmCraft parameter (static value or clip-time keyframes) to a property.
fn set_prop(
    dp: &mut Property,
    p: &fp::Param,
    conv: impl Fn(&fp::ParamValue) -> Option<Value>,
    l: &Layer,
    to_layer: &impl Fn(&Layer, filmcraft_time::Tick) -> Tick,
) {
    if p.keyframes.is_empty() {
        if let Some(v) = conv(&p.value) {
            dp.value = v;
        }
        return;
    }
    let mut keys: BTreeMap<Tick, Keyframe> = BTreeMap::new();
    for k in &p.keyframes {
        if let Some(v) = conv(&k.value) {
            let t = to_layer(l, k.time);
            keys.insert(t, interp_in(Keyframe::new(t, v), k.interp));
        }
    }
    if keys.is_empty() {
        return;
    }
    dp.keys = keys.into_values().collect();
    // Keys from a reversed layer arrive in reverse order of clip time; holds stay on the left key.
    dp.value = dp.keys[0].value.clone();
}

/// Multiply a property by piecewise-linear fades `(t0, t1, f0, f1)` (layer time): keys at the fade
/// ends and at the property's own keys, valued `orig(t) ⊗ factor(t)`.
fn apply_fades(dp: &mut Property, fades: &[(Tick, Tick, f64, f64)], mul: impl Fn(&Value, f64) -> Value) {
    let orig = dp.clone();
    let factor = |t: Tick| -> f64 {
        let mut f = 1.0;
        for &(a, b, fa, fb) in fades {
            let (lo, hi, flo, fhi) = if a <= b { (a, b, fa, fb) } else { (b, a, fb, fa) };
            if t <= lo {
                // Before a fade-in the layer is invisible; before a fade-out it is full.
                if flo < fhi {
                    f *= flo;
                }
            } else if t >= hi {
                if fhi < flo {
                    f *= fhi;
                }
            } else {
                let u = (t - lo).0 as f64 / (hi - lo).0.max(1) as f64;
                f *= flo + (fhi - flo) * u;
            }
        }
        f
    };
    let mut times: Vec<Tick> = orig.keys.iter().map(|k| k.time).collect();
    for &(a, b, _, _) in fades {
        times.push(a);
        times.push(b);
    }
    times.sort();
    times.dedup();
    dp.keys = times.iter().map(|&t| Keyframe::new(t, mul(&orig.value_at(t), factor(t)))).collect();
    dp.value = dp.keys[0].value.clone();
}
