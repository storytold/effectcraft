//! `link.open`: import an OTIO payload, open the comp, move the playhead, select matching layers, write the sidecar.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use effectcraft_engine::{EngineError, Importer, Services, Session};
use effectcraft_interchange::{ImportOptions, TimelineFormat, import};
use effectcraft_project::{Comp, CraftLinkBinding, Footage, ItemId, LayerId};
use effectcraft_time::{FrameRate, Tick};
use serde::Serialize;
use serde_json::{Value, json};

/// `payload.data` larger than this is rejected before import and before any project file is written.
const MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Successful `link.open` body. `selection_applied` is not part of the protocol reply.
#[derive(Debug, Serialize)]
pub struct OpenedLink {
    pub comp: String,
    pub project: String,
    pub format: LinkFormat,
    pub unsupported: Vec<UnsupportedPart>,
    /// True only when the request had at least one range and at least one layer matched.
    #[serde(skip)]
    pub selection_applied: bool,
}

/// Composition format carried back to FilmCraft.
#[derive(Debug, Serialize)]
pub struct LinkFormat {
    pub w: u32,
    pub h: u32,
    pub rate_num: i64,
    pub rate_den: i64,
    pub duration_ticks: i64,
    /// Compositions composite with an alpha channel.
    pub alpha: bool,
}

/// One part of the hand-off the importer could not keep.
#[derive(Debug, Serialize)]
pub struct UnsupportedPart {
    pub what: String,
    pub name: String,
    pub kept_as: String,
}

/// Failure of [`open_link`]. [`Self::reply`] is the object the control channel returns.
#[derive(Debug)]
pub struct LinkOpenError {
    pub code: &'static str,
    pub message: String,
}

impl LinkOpenError {
    pub fn bad_params(message: impl Into<String>) -> Self {
        Self { code: "bad_params", message: message.into() }
    }
    pub fn payload_invalid(message: impl Into<String>) -> Self {
        Self { code: "payload_invalid", message: message.into() }
    }
    pub fn payload_too_large(len: usize) -> Self {
        Self { code: "payload_too_large", message: format!("payload data is {len} bytes; the limit is {MAX_PAYLOAD_BYTES} bytes") }
    }
    pub fn io(message: impl Into<String>) -> Self {
        Self { code: "io_error", message: message.into() }
    }
    pub fn reply(&self) -> Value {
        json!({
            "ok": false,
            "error": {"code": self.code, "message": self.message, "data": {}}
        })
    }
}

impl std::fmt::Display for LinkOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for LinkOpenError {}

/// One `place.selection` entry, already in comp ticks. `track_index` is the position in the
/// sender's track list: video tracks, then audio tracks (the OpenTimelineIO order).
struct SelectionRange {
    start: Tick,
    end: Tick,
    track_index: usize,
}

struct OpenParams {
    link_id: String,
    data: String,
    seconds: f64,
    selection: Vec<SelectionRange>,
    result_path: String,
    sidecar: PathBuf,
    source: Value,
}

struct Imported {
    comp: ItemId,
    warnings: Vec<String>,
    /// The clips of the payload's first sequence, in the order the importer makes layers from them.
    clips: Vec<ClipSpan>,
}

/// A clip of the payload: where it sat in the sender's track list and on the timeline.
#[derive(Clone, Debug, PartialEq)]
struct ClipSpan {
    /// Position in the sender's track list (video tracks, then audio tracks).
    track: usize,
    name: String,
    start: Tick,
    end: Tick,
}

/// Probes media the way the app does: through the session's importer, which is the one File >
/// Import and the footage check use. Footage it cannot probe stays a placeholder, flagged missing.
struct Prober {
    importer: Option<Arc<dyn Importer>>,
    services: Arc<dyn Services>,
    rate: FrameRate,
}

impl Prober {
    fn of(session: &Session) -> Self {
        Self { importer: session.importer.clone(), services: Arc::clone(&session.services), rate: session.sequence_rate() }
    }

    fn probe(&self, path: &str) -> Option<Footage> {
        let importer = self.importer.as_ref()?;
        match effectcraft_engine::sequence::probe_path(importer.as_ref(), self.services.as_ref(), path, self.rate) {
            Ok(footage) => Some(footage),
            Err(why) => {
                log::warn!("link.open: could not probe {path}: {why}");
                None
            }
        }
    }
}

/// Import `params` into `session` and write `<link_id>.ecproj` next to `result_path`.
pub fn open_link(session: &mut Session, params: &Value) -> Result<OpenedLink, LinkOpenError> {
    let parsed = parse_params(params)?;
    let prober = Prober::of(session);
    let mut failed: Option<LinkOpenError> = None;
    let edit_result = session.edit("link.open", None, |project, _state| match import_into(project, &parsed, &prober) {
        Ok(imported) => Ok(imported),
        Err(e) => {
            failed = Some(e);
            Err(EngineError::Other("link.open".into()))
        }
    });
    let imported = match edit_result {
        Ok(imported) => imported,
        Err(_) => return Err(failed.unwrap_or_else(|| LinkOpenError::io("link.open did not import"))),
    };

    session.open_comp(imported.comp);
    if session.active_comp_id() != Some(imported.comp) {
        return Err(LinkOpenError::payload_invalid("could not open the imported composition"));
    }
    session.set_time(Tick::from_seconds_f64(parsed.seconds));
    let selection_applied = apply_selection(session, imported.comp, &parsed.selection, &imported.clips);

    let (w, h, rate_num, rate_den, duration_ticks) = {
        let Some(comp) = session.project.comp(imported.comp) else {
            return Err(LinkOpenError::payload_invalid("imported composition is missing"));
        };
        (comp.width, comp.height, comp.frame_rate.num, comp.frame_rate.den, comp.duration.0)
    };
    write_sidecar(session, &parsed.sidecar)?;
    let project = parsed.sidecar.to_str().ok_or_else(|| LinkOpenError::io("the sidecar path is not utf-8"))?.to_string();
    Ok(OpenedLink {
        comp: imported.comp.0.to_string(),
        project,
        format: LinkFormat { w, h, rate_num, rate_den, duration_ticks, alpha: true },
        unsupported: unsupported_from(&imported.warnings),
        selection_applied,
    })
}

/// Half-open `[start, end)`: a touch at either edge is not a match.
fn apply_selection(session: &mut Session, comp_id: ItemId, ranges: &[SelectionRange], clips: &[ClipSpan]) -> bool {
    let selected = {
        let comp = session.project.comp(comp_id);
        matching_layers(comp, ranges, clips)
    };
    let applied = !ranges.is_empty() && !selected.is_empty();
    session.state.selected_layers = selected;
    applied
}

/// The layers made from the clips that a range selects. A range selects a clip when the clip sits
/// on the range's track and overlaps its time span.
fn matching_layers(comp: Option<&Comp>, ranges: &[SelectionRange], clips: &[ClipSpan]) -> Vec<LayerId> {
    let Some(comp) = comp else {
        return Vec::new();
    };
    if ranges.is_empty() {
        return Vec::new();
    }
    let mut selected = Vec::new();
    for (layer, clip) in comp.layers.iter().zip(clip_of_each_layer(comp, clips)) {
        let Some(clip) = clip.and_then(|index| clips.get(index)) else {
            continue;
        };
        let hit = ranges.iter().any(|range| clip.track == range.track_index && clip.start < range.end && clip.end > range.start);
        if hit && !selected.contains(&layer.id) {
            selected.push(layer.id);
        }
    }
    selected
}

/// For each layer of `comp`, the index of the clip it was made from, or `None` for a layer the
/// payload did not make (or one that cannot be told apart).
///
/// The importer makes layers from the clips in the order `clips` lists them and skips clips it
/// cannot represent. So the walk takes layers in order and, for each, the next clip with the same
/// name whose span overlaps the layer's. A skipped clip is stepped over. Transitions can stretch a
/// layer past its clip, which still overlaps.
fn clip_of_each_layer(comp: &Comp, clips: &[ClipSpan]) -> Vec<Option<usize>> {
    let mut next = 0usize;
    comp.layers
        .iter()
        .map(|layer| {
            let found =
                (next..clips.len()).find(|&i| clips.get(i).is_some_and(|c| c.name == layer.name && layer.in_point < c.end && layer.out_point > c.start));
            if let Some(i) = found {
                next = i + 1;
            }
            found
        })
        .collect()
}

fn by_start_desc(items: &[effectcraft_interchange::fc_project::TrackItem]) -> Vec<&effectcraft_interchange::fc_project::TrackItem> {
    let mut items: Vec<_> = items.iter().collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.start));
    items
}

/// The clips of the payload's first sequence in importer layer order: video tracks from the top
/// down, later clips first, then audio tracks in order. `track` counts video tracks first, then
/// audio tracks, as the sender does.
fn clip_spans(data: &[u8]) -> Vec<ClipSpan> {
    use effectcraft_interchange::fc;
    let imported = match fc::import_with(data, fc::Format::Otio, &fc::ImportOptions::default()) {
        Ok((imported, _report)) => imported,
        Err(why) => {
            log::warn!("link.open: could not read the clips of the payload: {why}");
            return Vec::new();
        }
    };
    let Some(sequence) = imported.sequences.first().and_then(|id| imported.project.sequence(*id)) else {
        return Vec::new();
    };
    let video = sequence.video_tracks.len();
    let mut spans = Vec::new();
    for (index, track) in sequence.video_tracks.iter().enumerate().rev() {
        for item in by_start_desc(&track.items) {
            spans.push(ClipSpan { track: index, name: item.name.clone(), start: Tick(item.start.0), end: Tick(item.start.0.saturating_add(item.duration.0)) });
        }
    }
    for (index, track) in sequence.audio_tracks.iter().enumerate() {
        for item in by_start_desc(&track.items) {
            spans.push(ClipSpan {
                track: video + index,
                name: item.name.clone(),
                start: Tick(item.start.0),
                end: Tick(item.start.0.saturating_add(item.duration.0)),
            });
        }
    }
    spans
}

fn parse_params(params: &Value) -> Result<OpenParams, LinkOpenError> {
    let link_id = required_str(params, "link_id")?;
    if !is_uuid(link_id) {
        return Err(LinkOpenError::bad_params("link_id must be a uuid"));
    }
    let payload = params.get("payload").filter(|v| !v.is_null()).ok_or_else(|| LinkOpenError::bad_params("missing payload"))?;
    let payload = payload.as_object().ok_or_else(|| LinkOpenError::payload_invalid("payload must be an object"))?;
    let format = payload.get("format").and_then(Value::as_str).ok_or_else(|| LinkOpenError::payload_invalid("payload.format is required"))?;
    if format != "otio-json" {
        return Err(LinkOpenError::payload_invalid(format!("unsupported payload format {format}")));
    }
    let data = payload.get("data").and_then(Value::as_str).ok_or_else(|| LinkOpenError::payload_invalid("payload.data must be a string"))?;
    if data.len() > MAX_PAYLOAD_BYTES {
        return Err(LinkOpenError::payload_too_large(data.len()));
    }
    let place = params.get("place").filter(|v| !v.is_null()).ok_or_else(|| LinkOpenError::bad_params("missing place"))?;
    let place = place.as_object().ok_or_else(|| LinkOpenError::bad_params("place must be an object"))?;
    let seconds = place.get("seconds").and_then(Value::as_f64).ok_or_else(|| LinkOpenError::bad_params("place.seconds must be a number"))?;
    if !seconds.is_finite() {
        return Err(LinkOpenError::bad_params("place.seconds must be finite"));
    }
    let selection = parse_selection(place)?;
    let result_path = required_str(params, "result_path")?;
    let result = crate::result::check_result_name(link_id, result_path).map_err(|e| LinkOpenError::bad_params(e.message))?;
    let Some(parent) = result.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return Err(LinkOpenError::bad_params("result_path has no directory"));
    };
    let source = params.get("source").filter(|v| !v.is_null()).ok_or_else(|| LinkOpenError::bad_params("missing source"))?.clone();
    if !source.is_object() {
        return Err(LinkOpenError::bad_params("source must be an object"));
    }
    Ok(OpenParams {
        link_id: link_id.to_string(),
        data: data.to_string(),
        seconds,
        selection,
        result_path: result_path.to_string(),
        sidecar: parent.join(format!("{link_id}.ecproj")),
        source,
    })
}

fn parse_selection(place: &serde_json::Map<String, Value>) -> Result<Vec<SelectionRange>, LinkOpenError> {
    let Some(value) = place.get("selection").filter(|v| !v.is_null()) else {
        return Ok(Vec::new());
    };
    let items = value.as_array().ok_or_else(|| LinkOpenError::bad_params("place.selection must be an array"))?;
    let mut ranges = Vec::with_capacity(items.len());
    for item in items {
        let obj = item.as_object().ok_or_else(|| LinkOpenError::bad_params("place.selection entries must be objects"))?;
        let start_s = selection_number(obj, "start_s")?;
        let end_s = selection_number(obj, "end_s")?;
        let track_index = selection_track_index(obj.get("track_index"))?;
        ranges.push(SelectionRange { start: Tick::from_seconds_f64(start_s), end: Tick::from_seconds_f64(end_s), track_index });
    }
    Ok(ranges)
}

fn selection_number(obj: &serde_json::Map<String, Value>, key: &str) -> Result<f64, LinkOpenError> {
    let number = obj.get(key).and_then(Value::as_f64).filter(|n| n.is_finite());
    number.ok_or_else(|| LinkOpenError::bad_params(format!("place.selection {key} must be a number")))
}

fn selection_track_index(value: Option<&Value>) -> Result<usize, LinkOpenError> {
    let Some(value) = value else {
        return Err(LinkOpenError::bad_params("place.selection track_index must be a number"));
    };
    if let Some(index) = value.as_u64() {
        return usize::try_from(index).map_err(|_| LinkOpenError::bad_params("place.selection track_index must be a non-negative integer"));
    }
    let Some(number) = value.as_f64().filter(|n| n.is_finite()) else {
        return Err(LinkOpenError::bad_params("place.selection track_index must be a number"));
    };
    if number.is_sign_negative() {
        return Err(LinkOpenError::bad_params("place.selection track_index must be a non-negative integer"));
    }
    let index = number as u64;
    if (index as f64).to_bits() != number.to_bits() {
        return Err(LinkOpenError::bad_params("place.selection track_index must be a non-negative integer"));
    }
    usize::try_from(index).map_err(|_| LinkOpenError::bad_params("place.selection track_index must be a non-negative integer"))
}

fn required_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, LinkOpenError> {
    params
        .get(key)
        .filter(|v| !v.is_null())
        .ok_or_else(|| LinkOpenError::bad_params(format!("missing {key}")))?
        .as_str()
        .ok_or_else(|| LinkOpenError::bad_params(format!("{key} must be a string")))
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, c) in b.iter().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if *c != b'-' {
                return false;
            }
        } else if !c.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn import_into(project: &mut effectcraft_project::Project, params: &OpenParams, prober: &Prober) -> Result<Imported, LinkOpenError> {
    let mut probe = |path: &str| prober.probe(path);
    let result = import(
        project,
        params.data.as_bytes(),
        Some(TimelineFormat::Otio),
        &ImportOptions { name: params.link_id.clone(), ..ImportOptions::default() },
        &mut probe,
    )
    .map_err(|e| LinkOpenError::payload_invalid(e.to_string()))?;
    let Some(comp) = result.comps.first().copied() else {
        return Err(LinkOpenError::payload_invalid("the timeline has no composition"));
    };
    if project.comp(comp).is_none() {
        return Err(LinkOpenError::payload_invalid("the imported item is not a composition"));
    }
    project.craft_links.insert(
        params.link_id.clone(),
        CraftLinkBinding { link_id: params.link_id.clone(), comp, source: params.source.clone(), result_path: params.result_path.clone(), epoch: 0 },
    );
    Ok(Imported { comp, warnings: result.warnings, clips: clip_spans(params.data.as_bytes()) })
}

fn unsupported_from(warnings: &[String]) -> Vec<UnsupportedPart> {
    warnings.iter().map(|name| UnsupportedPart { what: "import".to_string(), name: name.clone(), kept_as: "none".to_string() }).collect()
}

fn write_sidecar(session: &Session, sidecar: &Path) -> Result<(), LinkOpenError> {
    let Some(parent) = sidecar.parent() else {
        return Err(LinkOpenError::io("the sidecar path has no directory"));
    };
    std::fs::create_dir_all(parent).map_err(|e| LinkOpenError::io(format!("could not create the link directory: {e}")))?;
    let text = session.project.to_json();
    if text.is_empty() {
        return Err(LinkOpenError::io("the project could not be saved"));
    }
    let link_id = sidecar.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
    crate::sidecar::write_atomic(sidecar, link_id, text.as_bytes()).map_err(|e| LinkOpenError::io(format!("could not write the sidecar project: {e}")))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use effectcraft_engine::Session;
    use effectcraft_interchange::fc_media as fm;
    use effectcraft_interchange::fc_project as fp;
    use effectcraft_interchange::fc_time as ft;
    use effectcraft_interchange::{ImportOptions, TimelineFormat, import};
    use effectcraft_project::Project;
    use effectcraft_time::Tick;
    use serde_json::json;

    use super::*;

    const LINK_ID: &str = "11111111-1111-4111-8111-111111111111";

    fn source() -> serde_json::Value {
        json!({"app": "filmcraft", "doc_id": "22222222-2222-4222-8222-222222222222", "item": "12"})
    }

    fn otio_json() -> String {
        let mut doc = fp::Project::new("Link");
        let info = fm::MediaInfo {
            name: String::new(),
            kind: fm::MediaKind::Movie,
            duration: ft::Tick::from_seconds_f64(20.0),
            video: Some(fm::VideoStreamInfo {
                width: 320,
                height: 180,
                frame_rate: ft::FrameRate { num: 25, den: 1 },
                par: (1, 1),
                codec: "png".into(),
                pixel_format: String::new(),
                color: Default::default(),
                has_alpha: false,
                bitrate: None,
                hdr: None,
            }),
            audio_streams: Vec::new(),
            container: String::new(),
            start_timecode: None,
            file_size: None,
        };
        let clip = fp::MediaClip {
            media: fp::MediaRef::File { path: "/media/shot.mov".into() },
            info,
            interpret: Default::default(),
            mark_in: None,
            mark_out: None,
            markers: vec![],
            offline: false,
            proxy: None,
            identity: None,
        };
        let item = doc.add_item("shot", fp::Label::Iris, fp::ItemKind::Media(clip), None);
        let settings = fp::SequenceSettings { width: 320, height: 180, frame_rate: ft::FrameRate { num: 25, den: 1 }, ..Default::default() };
        let main = doc.new_sequence("Main", settings, 1, 0, None);
        let rate = ft::FrameRate { num: 25, den: 1 };
        let placed = doc
            .make_track_item(
                item,
                fp::TrackKind::Video,
                ft::Tick::from_seconds_f64(0.0),
                ft::TimeRange::new(ft::Tick::from_seconds_f64(0.0), ft::Tick::from_seconds_f64(2.0)),
                rate,
            )
            .unwrap();
        doc.sequence_mut(main).unwrap().video_tracks[0].items = vec![placed];
        let (bytes, _) = effectcraft_interchange::fc::export(&doc, main, effectcraft_interchange::fc::Format::Otio, &Default::default()).unwrap();
        String::from_utf8(bytes).unwrap()
    }

    fn direct_warnings(data: &str) -> Vec<String> {
        let mut project = Project::default();
        let mut probe = |_path: &str| -> Option<effectcraft_project::Footage> { None };
        let result =
            import(&mut project, data.as_bytes(), Some(TimelineFormat::Otio), &ImportOptions { name: LINK_ID.into(), ..Default::default() }, &mut probe)
                .unwrap();
        assert!(!result.comps.is_empty(), "the fixture must import a composition");
        result.warnings
    }

    #[test]
    fn link_open_imports_otio_and_writes_the_sidecar() {
        let data = otio_json();
        let warnings = direct_warnings(&data);
        let mut session = Session::default();
        let dir = format!("/tmp/ec-open-{}", std::process::id());
        let result_path = format!("{dir}/{LINK_ID}.mov");
        let params = json!({
            "link_id": LINK_ID,
            "payload": {"format": "otio-json", "data": data},
            "place": {
                "seconds": 1.0,
                "selection": [{"start_s": 0.0, "end_s": 1.0, "track_index": 0}],
                "view": {"zoom": 2.0}
            },
            "result_path": result_path,
            "source": source(),
        });
        let opened = open_link(&mut session, &params).unwrap();
        let reply = serde_json::to_value(&opened).unwrap();
        println!("OPEN_REPLY:{reply}");

        let comp_id = effectcraft_project::ItemId(reply["comp"].as_str().unwrap().parse().unwrap());
        assert_eq!(session.active_comp_id(), Some(comp_id));
        assert_eq!(opened.comp, comp_id.0.to_string());
        assert!(reply.get("selection_applied").is_none());

        let comp = session.project.comp(comp_id).unwrap();
        assert_eq!(session.project.item(comp_id).unwrap().name, "Main");
        let shot = comp.layers.iter().find(|layer| layer.name == "shot").unwrap();
        assert_eq!(session.state.selected_layers, vec![shot.id]);
        assert!(opened.selection_applied);
        println!("SELECTED_LAYER:{}", shot.id.0);
        let tick = Tick::from_seconds_f64(1.0);
        let snapped = comp.frame_rate.snap_nearest(tick.clamp(Tick::ZERO, comp.duration - comp.frame_duration()));
        assert_eq!(session.time(), snapped);
        assert_eq!(session.time(), Tick::from_seconds_f64(1.0));
        assert_eq!(reply["format"]["w"], 320);
        assert_eq!(reply["format"]["h"], 180);
        assert_eq!(reply["format"]["rate_num"], 25);
        assert_eq!(reply["format"]["rate_den"], 1);
        assert_eq!(reply["format"]["duration_ticks"], comp.duration.0);
        assert_eq!(reply["format"]["alpha"], true);

        let unsupported = reply["unsupported"].as_array().unwrap();
        assert_eq!(unsupported.len(), warnings.len());
        for (part, warning) in unsupported.iter().zip(&warnings) {
            assert_eq!(part["what"], "import");
            assert_eq!(part["name"], warning.as_str());
            assert_eq!(part["kept_as"], "none");
        }

        let project_path = reply["project"].as_str().unwrap();
        assert!(Path::new(project_path).is_absolute());
        assert!(project_path.ends_with(&format!("{LINK_ID}.ecproj")));
        assert!(Path::new(project_path).is_file());
        let text = std::fs::read_to_string(project_path).unwrap();
        assert!(text.contains(LINK_ID));
        let saved = Project::from_json(&text).unwrap();
        assert_eq!(saved.craft_links[LINK_ID].comp, comp_id);
        assert_eq!(saved.craft_links[LINK_ID].result_path, result_path);
        assert_eq!(session.project.craft_links[LINK_ID].comp, comp_id);
        assert_eq!(session.history.undo.len(), 1);
        assert_eq!(session.history.undo[0].0, "link.open");
    }

    /// Open the link in a session, then edit the comp without saving, as a user does before closing.
    fn open_then_edit(tag: &str) -> (serde_json::Value, String) {
        let (_session, reply, dir) = open_edit_session(tag);
        (reply, dir)
    }

    /// [`open_then_edit`], and the session that holds the edit.
    fn open_edit_session(tag: &str) -> (Session, serde_json::Value, String) {
        let dir = format!("/tmp/ec-open-{}-{tag}", std::process::id());
        let _ = std::fs::remove_dir_all(&dir);
        let mut session = Session::default();
        let params = open_params(otio_json(), &format!("{dir}/{LINK_ID}.mov"), json!([]));
        let reply = serde_json::to_value(open_link(&mut session, &params).unwrap()).unwrap();
        let comp = effectcraft_project::ItemId(reply["comp"].as_str().unwrap().parse().unwrap());
        session
            .edit("rename comp", None, |project, _state| {
                project.item_mut(comp).unwrap().name = "Edited, never saved".to_string();
                Ok(())
            })
            .unwrap();
        assert!(session.is_dirty());
        (session, reply, dir)
    }

    /// Attach `reply`'s link in a session that has never seen it and return the comp's name.
    fn reopened_name(reply: &serde_json::Value) -> String {
        let mut fresh = Session::default();
        let attach = json!({"link_id": LINK_ID, "project": reply["project"], "comp": reply["comp"]});
        crate::attach_link(&mut fresh, &attach).unwrap();
        let comp = effectcraft_project::ItemId(reply["comp"].as_str().unwrap().parse().unwrap());
        fresh.project.item(comp).unwrap().name.clone()
    }

    fn rename(session: &mut Session, reply: &serde_json::Value, name: &str) {
        let comp = effectcraft_project::ItemId(reply["comp"].as_str().unwrap().parse().unwrap());
        session
            .edit("rename comp", None, |project, _state| {
                project.item_mut(comp).unwrap().name = name.to_string();
                Ok(())
            })
            .unwrap();
    }

    /// Symptom 2: the user edits in EffectCraft, never saves and never presses Back, and the
    /// second `link.editIn` reopens the sidecar. The quiet period after the edit must have
    /// written the edit into it.
    #[test]
    fn an_edit_reaches_the_sidecar_after_the_quiet_period() {
        let (mut session, reply, dir) = open_edit_session("settle");
        let live = crate::live::Live::new(std::sync::Arc::new(|_, _| {}), std::sync::Arc::new(|| {}));
        let t0 = std::time::Instant::now();
        live.tick(&mut session, t0);
        rename(&mut session, &reply, "Edited twice");
        live.tick(&mut session, t0 + std::time::Duration::from_millis(10));
        assert_ne!(reopened_name(&reply), "Edited twice", "the sidecar must not be written before the quiet period ends");
        live.tick(&mut session, t0 + crate::sidecar::SETTLE + std::time::Duration::from_millis(50));
        live.finish_sidecar_writes();
        assert_eq!(reopened_name(&reply), "Edited twice");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The window closes at once: the quiet period has not passed. The synchronous write on
    /// close puts the edit in the sidecar, with no tick in between.
    #[test]
    fn an_edit_made_just_before_closing_is_in_the_sidecar_after_the_close_flush() {
        let (session, reply, dir) = open_edit_session("flush");
        assert_ne!(reopened_name(&reply), "Edited, never saved", "the edit reached the sidecar before the flush: the test proves nothing");
        let errors = crate::live::flush_sidecars(&session);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(reopened_name(&reply), "Edited, never saved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// FilmCraft reopens a link after the user closed EffectCraft without saving: a new process
    /// reads the sidecar `link.open` wrote and binds the same comp to the same link id.
    #[test]
    fn a_closed_effectcraft_reopens_the_link_from_the_sidecar() {
        let (reply, dir) = open_then_edit("reopen");
        let mut fresh = Session::default();
        assert!(fresh.project.craft_links.is_empty());
        let attach = json!({"link_id": LINK_ID, "project": reply["project"], "comp": reply["comp"]});
        let attached = crate::attach_link(&mut fresh, &attach).unwrap();

        let comp = effectcraft_project::ItemId(reply["comp"].as_str().unwrap().parse().unwrap());
        assert_eq!(fresh.active_comp_id(), Some(comp), "the reopened comp is not on screen");
        assert_eq!(attached["format"], reply["format"], "the reopen reports the format link.open reported");
        assert_eq!(fresh.project.craft_links[LINK_ID].result_path, format!("{dir}/{LINK_ID}.mov"));
        assert_eq!(fresh.project.item(comp).unwrap().name, "Main", "the sidecar holds the comp as link.open left it");
        assert!(!fresh.is_dirty(), "a reopen must not dirty the session");
        assert!(fresh.history.undo.is_empty(), "a reopen made an undo step");
        // Back must still be allowed to write the result next to the sidecar.
        let result_path = fresh.project.craft_links[LINK_ID].result_path.clone();
        crate::result::check_result_target(LINK_ID, &result_path).expect("the reopened link may write its result");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The sidecar is gone: the attach says `comp_missing`, FilmCraft sends `link.open` again with the
    /// same link id, and the session ends with one binding for it.
    #[test]
    fn a_missing_sidecar_is_comp_missing_and_a_second_open_rebinds_the_same_link() {
        let (reply, dir) = open_then_edit("missing");
        let sidecar = reply["project"].as_str().unwrap().to_string();
        std::fs::remove_file(&sidecar).unwrap();
        let mut fresh = Session::default();
        let before = Arc::clone(&fresh.project);
        let attach = json!({"link_id": LINK_ID, "project": sidecar, "comp": reply["comp"]});
        let err = crate::attach_link(&mut fresh, &attach).unwrap_err();
        assert_eq!(err.code, "comp_missing");
        assert!(err.message.contains("does not exist"), "{}", err.message);
        assert!(Arc::ptr_eq(&fresh.project, &before), "a failed attach changed the project");
        assert!(fresh.active_comp_id().is_none());

        let params = open_params(otio_json(), &format!("{dir}/{LINK_ID}.mov"), json!([]));
        let opened = open_link(&mut fresh, &params).unwrap();
        assert_eq!(fresh.project.craft_links.len(), 1);
        assert_eq!(fresh.project.craft_links[LINK_ID].comp.0.to_string(), opened.comp);
        assert!(Path::new(&sidecar).is_file(), "the second open writes the sidecar again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn link_open_rejects_payload_over_64_mib() {
        let mut session = Session::default();
        let dir = format!("/tmp/ec-open-{}-oversize", std::process::id());
        let result_path = format!("{dir}/{LINK_ID}.mov");
        let sidecar = format!("{dir}/{LINK_ID}.ecproj");
        let _ = std::fs::remove_dir_all(&dir);
        let data = "x".repeat(MAX_PAYLOAD_BYTES + 1);
        let params = json!({
            "link_id": LINK_ID,
            "payload": {"format": "otio-json", "data": data},
            "place": {"seconds": 0.0},
            "result_path": result_path,
            "source": source(),
        });
        let err = open_link(&mut session, &params).unwrap_err();
        assert_eq!(err.code, "payload_too_large");
        assert!(!Path::new(&sidecar).exists());
        assert!(!Path::new(&dir).exists());
        assert!(session.project.items.is_empty());
    }

    #[test]
    fn link_open_missing_payload_leaves_the_project() {
        let mut session = Session::default();
        let before = Arc::clone(&session.project);
        let revision = session.revision;
        let result_path = format!("/tmp/ec-open-{}-missing/{LINK_ID}.mov", std::process::id());
        let params = json!({
            "link_id": LINK_ID,
            "place": {"seconds": 0.0},
            "result_path": result_path,
            "source": source(),
        });
        let err = open_link(&mut session, &params).unwrap_err();
        assert_eq!(err.code, "bad_params");
        assert!(session.project.same_content(&before));
        assert!(Arc::ptr_eq(&session.project, &before));
        assert_eq!(session.revision, revision);
        assert!(!session.project.to_json().contains("\"craft_links\""));
        assert!(!Path::new(&result_path).exists());
    }

    /// A peer cannot name a result file other than `<link_id>.mov`, so the later render
    /// cannot be aimed at an existing file.
    #[test]
    fn link_open_refuses_a_result_path_that_is_not_the_link_movie() {
        let dir = format!("/tmp/ec-open-{}-name", std::process::id());
        for bad in [format!("{dir}/authorized_keys"), format!("{dir}/{LINK_ID}.txt"), format!("{dir}/other.mov"), format!("{LINK_ID}.mov")] {
            let mut session = Session::default();
            let params = json!({
                "link_id": LINK_ID,
                "payload": {"format": "otio-json", "data": otio_json()},
                "place": {"seconds": 0.0},
                "result_path": bad,
                "source": source(),
            });
            let err = open_link(&mut session, &params).unwrap_err();
            assert_eq!(err.code, "bad_params", "{bad}");
            assert!(session.project.craft_links.is_empty(), "{bad}");
            assert!(!Path::new(&dir).exists(), "{bad}");
        }
    }

    #[test]
    fn link_open_selection_miss_selects_nothing() {
        let data = otio_json();
        let mut session = Session::default();
        let dir = format!("/tmp/ec-open-{}-miss", std::process::id());
        let result_path = format!("{dir}/{LINK_ID}.mov");
        let params = json!({
            "link_id": LINK_ID,
            "payload": {"format": "otio-json", "data": data},
            "place": {
                "seconds": 1.0,
                "selection": [{"start_s": 10.0, "end_s": 11.0, "track_index": 0}],
                "view": {"zoom": 2.0}
            },
            "result_path": result_path,
            "source": source(),
        });
        let opened = open_link(&mut session, &params).unwrap();
        let reply = serde_json::to_value(&opened).unwrap();
        assert!(!opened.selection_applied);
        assert!(reply.get("selection_applied").is_none());
        assert!(session.state.selected_layers.is_empty());
        let comp_id = effectcraft_project::ItemId(reply["comp"].as_str().unwrap().parse().unwrap());
        assert_eq!(session.active_comp_id(), Some(comp_id));
        let comp = session.project.comp(comp_id).unwrap();
        assert!(comp.layers.iter().any(|layer| layer.name == "shot"));
    }

    #[test]
    fn link_open_without_selection_selects_nothing() {
        let data = otio_json();
        let mut session = Session::default();
        let dir = format!("/tmp/ec-open-{}-none", std::process::id());
        let result_path = format!("{dir}/{LINK_ID}.mov");
        let params = json!({
            "link_id": LINK_ID,
            "payload": {"format": "otio-json", "data": data},
            "place": {"seconds": 1.0, "view": {"zoom": 2.0}},
            "result_path": result_path,
            "source": source(),
        });
        let opened = open_link(&mut session, &params).unwrap();
        let reply = serde_json::to_value(&opened).unwrap();
        assert!(!opened.selection_applied);
        assert!(reply.get("selection_applied").is_none());
        assert!(session.state.selected_layers.is_empty());
        assert!(session.active_comp_id().is_some());
    }

    // ---- regression tests for the first live run (bugs 1 and 3) ----

    struct Placed {
        name: &'static str,
        track: usize,
        start_s: f64,
        dur_s: f64,
    }

    /// An OTIO payload with one media item per clip, all pointing at `media`, on `tracks` video tracks.
    fn otio_clips(media: &str, size: (u32, u32), tracks: usize, clips: &[Placed]) -> String {
        let rate = ft::FrameRate { num: 24, den: 1 };
        let mut doc = fp::Project::new("Link");
        let main = doc.new_sequence("Main", fp::SequenceSettings { width: size.0, height: size.1, frame_rate: rate, ..Default::default() }, tracks, 0, None);
        for clip in clips {
            let info = fm::MediaInfo {
                name: String::new(),
                kind: fm::MediaKind::Movie,
                duration: ft::Tick::from_seconds_f64(10.0),
                video: Some(fm::VideoStreamInfo {
                    width: size.0,
                    height: size.1,
                    frame_rate: rate,
                    par: (1, 1),
                    codec: "prores".into(),
                    pixel_format: String::new(),
                    color: Default::default(),
                    has_alpha: false,
                    bitrate: None,
                    hdr: None,
                }),
                audio_streams: Vec::new(),
                container: String::new(),
                start_timecode: None,
                file_size: None,
            };
            let media_clip = fp::MediaClip {
                media: fp::MediaRef::File { path: media.into() },
                info,
                interpret: Default::default(),
                mark_in: None,
                mark_out: None,
                markers: vec![],
                offline: false,
                proxy: None,
                identity: None,
            };
            let item = doc.add_item(clip.name, fp::Label::Iris, fp::ItemKind::Media(media_clip), None);
            let placed = doc
                .make_track_item(
                    item,
                    fp::TrackKind::Video,
                    ft::Tick::from_seconds_f64(clip.start_s),
                    ft::TimeRange::new(ft::Tick::from_seconds_f64(0.0), ft::Tick::from_seconds_f64(clip.dur_s)),
                    rate,
                )
                .unwrap();
            doc.sequence_mut(main).unwrap().video_tracks[clip.track].items.push(placed);
        }
        let (bytes, _) = effectcraft_interchange::fc::export(&doc, main, effectcraft_interchange::fc::Format::Otio, &Default::default()).unwrap();
        String::from_utf8(bytes).unwrap()
    }

    /// Render a comp with one opaque blue solid to a ProRes file with a fully wired session: a generated test clip.
    fn generated_clip(dir: &str) -> String {
        use effectcraft_engine::color::Label;
        use effectcraft_project::{Comp, ItemKind, LayerSource, Solid};
        use effectcraft_time::FrameRate;
        let rate = FrameRate::new(24, 1);
        let mut session = effectcraft_host::session();
        let project = session.project_mut();
        let solid = project.add_item("Blue", Label::Red, None, ItemKind::Solid(Solid { color: [0.1, 0.2, 0.8], width: 128, height: 72, pixel_aspect: 1.0 }));
        let mut comp = Comp::new(128, 72, rate, rate.tick_of(48));
        let layer = effectcraft_project::build::layer(project, &comp, "Blue", LayerSource::Solid { item: solid }, (128, 72), None);
        comp.layers.push(layer);
        let comp_id = project.add_item("Clip", Label::Sandstone, None, ItemKind::Comp(Arc::new(comp)));
        let clip = format!("{dir}/gen.mov");
        std::fs::create_dir_all(dir).unwrap();
        // `link.open` writes this sidecar. The render accepts only a directory that holds it.
        std::fs::write(format!("{dir}/gen.ecproj"), b"{}").unwrap();
        session
            .project_mut()
            .craft_links
            .insert("gen".into(), CraftLinkBinding { link_id: "gen".into(), comp: comp_id, source: source(), result_path: clip.clone(), epoch: 0 });
        crate::result::render_result(&mut session, "gen").unwrap();
        clip
    }

    fn open_params(data: String, result_path: &str, selection: serde_json::Value) -> serde_json::Value {
        json!({
            "link_id": LINK_ID,
            "payload": {"format": "otio-json", "data": data},
            "place": {"seconds": 0.5, "selection": selection},
            "result_path": result_path,
            "source": source(),
        })
    }

    /// Mean r, g, b of the frame at `tick_frame` of the movie at `path`.
    fn mean_rgb(path: &str, frame_index: i64) -> (f32, f32, f32) {
        let probed = effectcraft_media::probe(path).unwrap();
        let frame = effectcraft_media::MediaPool::new().frame_at(&probed, probed.frame_rate.tick_of(frame_index)).unwrap();
        let n = frame.data.len() as f32;
        let (r, g, b) = frame.data.iter().fold((0.0, 0.0, 0.0), |a, p| (a.0 + p[0], a.1 + p[1], a.2 + p[2]));
        (r / n, g / n, b / n)
    }

    #[test]
    fn link_open_probes_footage_so_the_result_movie_is_not_black() {
        let dir = format!("/tmp/ec-open-{}-probe", std::process::id());
        let _ = std::fs::remove_dir_all(&dir);
        let clip = generated_clip(&format!("{dir}/media"));
        let source_rgb = mean_rgb(&clip, 12);
        println!("SOURCE_MEAN_RGB:{source_rgb:?}");
        assert!(source_rgb.2 > 0.3, "the generated test clip is itself black: {source_rgb:?}");
        let data = otio_clips(&clip, (128, 72), 1, &[Placed { name: "shot", track: 0, start_s: 0.0, dur_s: 2.0 }]);
        let result_path = format!("{dir}/{LINK_ID}.mov");
        let mut session = effectcraft_host::session();
        open_link(&mut session, &open_params(data, &result_path, json!([]))).unwrap();

        let footage: Vec<_> = session
            .project
            .items
            .values()
            .filter_map(|item| match &item.kind {
                effectcraft_project::ItemKind::Footage(f) => Some(f.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(footage.len(), 1);
        assert!(!footage[0].missing, "the imported footage must not be a placeholder");
        assert_eq!((footage[0].width, footage[0].height), (128, 72));

        crate::result::render_result(&mut session, LINK_ID).unwrap();
        let (r, g, b) = mean_rgb(&result_path, 12);
        println!("RESULT_MEAN_RGB:{r} {g} {b}");
        assert!(b > 0.3 && g > 0.05, "the result frame is black: mean rgb {r} {g} {b}");
    }

    fn selected_names(session: &Session) -> Vec<String> {
        let comp = session.project.comp(session.active_comp_id().unwrap()).unwrap();
        let mut names: Vec<String> = comp.layers.iter().filter(|l| session.state.selected_layers.contains(&l.id)).map(|l| l.name.clone()).collect();
        names.sort();
        names
    }

    #[test]
    fn selection_of_two_clips_on_one_track_selects_both_layers() {
        let data = otio_clips(
            "/media/shot.mov",
            (320, 180),
            1,
            &[Placed { name: "first", track: 0, start_s: 0.0, dur_s: 2.0 }, Placed { name: "second", track: 0, start_s: 2.0, dur_s: 2.0 }],
        );
        let mut session = Session::default();
        let sel = json!([{"start_s": 0.0, "end_s": 2.0, "track_index": 0}, {"start_s": 2.0, "end_s": 4.0, "track_index": 0}]);
        let opened = open_link(&mut session, &open_params(data, &format!("/tmp/ec-open-sel1/{LINK_ID}.mov"), sel)).unwrap();
        assert!(opened.selection_applied);
        assert_eq!(selected_names(&session), vec!["first".to_string(), "second".to_string()]);
    }

    #[test]
    fn selection_on_the_second_track_selects_only_that_clip() {
        // Both clips span the same time. Only the track index tells them apart.
        let data = otio_clips(
            "/media/shot.mov",
            (320, 180),
            2,
            &[Placed { name: "lower", track: 0, start_s: 0.0, dur_s: 2.0 }, Placed { name: "upper", track: 1, start_s: 0.0, dur_s: 2.0 }],
        );
        let mut session = Session::default();
        let sel = json!([{"start_s": 0.0, "end_s": 2.0, "track_index": 1}]);
        open_link(&mut session, &open_params(data.clone(), &format!("/tmp/ec-open-sel2/{LINK_ID}.mov"), sel)).unwrap();
        assert_eq!(selected_names(&session), vec!["upper".to_string()]);

        let mut session = Session::default();
        let sel = json!([{"start_s": 0.0, "end_s": 2.0, "track_index": 0}]);
        open_link(&mut session, &open_params(data, &format!("/tmp/ec-open-sel2/{LINK_ID}.mov"), sel)).unwrap();
        assert_eq!(selected_names(&session), vec!["lower".to_string()]);
    }

    #[test]
    fn selection_picks_clips_on_different_tracks_and_times() {
        let data = otio_clips(
            "/media/shot.mov",
            (320, 180),
            2,
            &[
                Placed { name: "a", track: 0, start_s: 0.0, dur_s: 2.0 },
                Placed { name: "b", track: 0, start_s: 2.0, dur_s: 2.0 },
                Placed { name: "c", track: 1, start_s: 1.0, dur_s: 2.0 },
            ],
        );
        let mut session = Session::default();
        let sel = json!([{"start_s": 2.0, "end_s": 4.0, "track_index": 0}, {"start_s": 1.0, "end_s": 3.0, "track_index": 1}]);
        open_link(&mut session, &open_params(data, &format!("/tmp/ec-open-sel3/{LINK_ID}.mov"), sel)).unwrap();
        assert_eq!(selected_names(&session), vec!["b".to_string(), "c".to_string()]);
    }
}
