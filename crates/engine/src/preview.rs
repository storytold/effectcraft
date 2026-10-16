//! Preview panel settings (After Effects' Preview panel): five keyboard shortcuts (Spacebar,
//! Shift+Spacebar, Numpad 0, Shift+Numpad 0, Alt+Numpad 0), each with its own saved options —
//! what to include (video, audio, overlays, layer controls), Loop, Cache Before Playback, Range,
//! Play From, Frame Rate, Skip, Resolution, Full Screen, "If caching, play cached frames" and
//! "Move time to preview time".
//!
//! The settings live in [`crate::prefs::Prefs::preview`] (saved with the other settings).
//! [`plan`] turns one shortcut's options, a comp and the current time into the frames to play;
//! the frontend's playback loop follows the plan.
//!
//! Commands: `playback.settings.get {shortcut?}`, `playback.settings.set {shortcut?, values?,
//! <field>: value…, current?}`.

use effectcraft_project::Comp;
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{Result, Session, cmd, query};

/// A Preview panel shortcut.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewShortcut {
    #[default]
    Spacebar,
    ShiftSpacebar,
    Numpad0,
    ShiftNumpad0,
    AltNumpad0,
}

impl PreviewShortcut {
    pub const ALL: [PreviewShortcut; 5] = [Self::Spacebar, Self::ShiftSpacebar, Self::Numpad0, Self::ShiftNumpad0, Self::AltNumpad0];
    pub fn label(self) -> &'static str {
        match self {
            Self::Spacebar => "Spacebar",
            Self::ShiftSpacebar => "Shift+Spacebar",
            Self::Numpad0 => "Numpad 0",
            Self::ShiftNumpad0 => "Shift+Numpad 0",
            Self::AltNumpad0 => "Alt+Numpad 0",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::Spacebar => "spacebar",
            Self::ShiftSpacebar => "shiftSpacebar",
            Self::Numpad0 => "numpad0",
            Self::ShiftNumpad0 => "shiftNumpad0",
            Self::AltNumpad0 => "altNumpad0",
        }
    }
    /// From an id (`shiftSpacebar`) or a label (`Shift+Spacebar`).
    pub fn parse(s: &str) -> Option<Self> {
        let k = s.to_ascii_lowercase().replace([' ', '+', '-', '_'], "");
        Self::ALL.into_iter().find(|x| x.id().to_ascii_lowercase() == k || x.label().to_ascii_lowercase().replace([' ', '+'], "") == k)
    }
}

/// Range of a preview.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewRange {
    WorkArea,
    #[default]
    WorkAreaExtended,
    EntireDuration,
    /// The pre-roll before and post-roll after the current time.
    AroundCurrentTime,
}

impl PreviewRange {
    pub const ALL: [PreviewRange; 4] = [Self::WorkArea, Self::WorkAreaExtended, Self::EntireDuration, Self::AroundCurrentTime];
    pub fn label(self) -> &'static str {
        match self {
            Self::WorkArea => "Work Area",
            Self::WorkAreaExtended => "Work Area Extended By Current Time",
            Self::EntireDuration => "Entire Duration",
            Self::AroundCurrentTime => "Play Around Current Time",
        }
    }
}

/// Where playback starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayFrom {
    RangeStart,
    #[default]
    CurrentTime,
}

impl PlayFrom {
    pub const ALL: [PlayFrom; 2] = [Self::RangeStart, Self::CurrentTime];
    pub fn label(self) -> &'static str {
        match self {
            Self::RangeStart => "Range Start",
            Self::CurrentTime => "Current Time",
        }
    }
}

/// Resolution of a preview (Auto: the viewer's own).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewResolution {
    #[default]
    Auto,
    Full,
    Half,
    Third,
    Quarter,
    /// Every n-th pixel ([`PreviewPreset::custom_resolution`]).
    Custom,
}

impl PreviewResolution {
    pub const ALL: [PreviewResolution; 6] = [Self::Auto, Self::Full, Self::Half, Self::Third, Self::Quarter, Self::Custom];
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Full => "Full",
            Self::Half => "Half",
            Self::Third => "Third",
            Self::Quarter => "Quarter",
            Self::Custom => "Custom",
        }
    }
    /// Pixel step (1 Full … 4 Quarter, `custom` for Custom); `None` for Auto.
    pub fn factor(self, custom: u32) -> Option<u32> {
        match self {
            Self::Auto => None,
            Self::Full => Some(1),
            Self::Half => Some(2),
            Self::Third => Some(3),
            Self::Quarter => Some(4),
            Self::Custom => Some(custom.clamp(1, 40)),
        }
    }
}

/// The options of one shortcut.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PreviewPreset {
    pub include_video: bool,
    pub include_audio: bool,
    /// Grids, guides and safe margins stay visible while playing.
    pub include_overlays: bool,
    /// Layer handles, paths and motion paths stay visible while playing.
    pub include_layer_controls: bool,
    #[serde(rename = "loop")]
    pub loop_: bool,
    /// Render the whole range before playing it.
    pub cache_before_playback: bool,
    pub range: PreviewRange,
    /// Play Around Current Time: seconds before and after the current time.
    pub pre_roll: f64,
    pub post_roll: f64,
    pub play_from: PlayFrom,
    /// Frames per second; `None` = Auto (the comp's rate).
    pub frame_rate: Option<f64>,
    /// Frames skipped between the frames played (0 = every frame).
    pub skip: u32,
    pub resolution: PreviewResolution,
    /// Custom resolution: render every n-th pixel (1–40).
    pub custom_resolution: u32,
    /// Play in a full-screen viewer.
    pub full_screen: bool,
    /// With Cache Before Playback, pressing the shortcut again while caching plays the frames
    /// already cached.
    pub play_cached_frames: bool,
    /// When the preview stops, the current time moves to the last frame shown (off: it returns
    /// to where the preview started).
    pub move_time_to_preview_time: bool,
}

impl Default for PreviewPreset {
    fn default() -> Self {
        PreviewPreset {
            include_video: true,
            include_audio: true,
            include_overlays: true,
            include_layer_controls: false,
            loop_: true,
            cache_before_playback: false,
            range: PreviewRange::WorkAreaExtended,
            pre_roll: 2.0,
            post_roll: 2.0,
            play_from: PlayFrom::CurrentTime,
            frame_rate: None,
            skip: 0,
            resolution: PreviewResolution::Auto,
            custom_resolution: 2,
            full_screen: false,
            play_cached_frames: true,
            move_time_to_preview_time: true,
        }
    }
}

impl PreviewPreset {
    /// The factory options of a shortcut.
    pub fn default_for(s: PreviewShortcut) -> Self {
        let d = PreviewPreset::default();
        match s {
            // Frame skipping is off by default; set Skip to play fewer frames.
            PreviewShortcut::Spacebar | PreviewShortcut::ShiftSpacebar => d,
            PreviewShortcut::Numpad0 => PreviewPreset { range: PreviewRange::WorkArea, play_from: PlayFrom::RangeStart, ..d },
            PreviewShortcut::ShiftNumpad0 => {
                PreviewPreset { range: PreviewRange::WorkArea, play_from: PlayFrom::RangeStart, skip: 1, resolution: PreviewResolution::Half, ..d }
            }
            // Audio only.
            PreviewShortcut::AltNumpad0 => PreviewPreset { include_video: false, range: PreviewRange::WorkArea, play_from: PlayFrom::RangeStart, ..d },
        }
    }

    fn normalize(&mut self) {
        self.pre_roll = if self.pre_roll.is_finite() { self.pre_roll.clamp(0.0, 3600.0) } else { 2.0 };
        self.post_roll = if self.post_roll.is_finite() { self.post_roll.clamp(0.0, 3600.0) } else { 2.0 };
        self.frame_rate = self.frame_rate.filter(|r| r.is_finite() && *r > 0.0).map(|r| r.clamp(0.01, 999.0));
        self.skip = self.skip.min(99);
        self.custom_resolution = self.custom_resolution.clamp(1, 40);
        if !self.include_video && !self.include_audio {
            self.include_video = true;
        }
    }
}

/// The Preview panel: the shortcut shown in the panel and every shortcut's options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PreviewSettings {
    /// The shortcut whose options the panel shows (and the play button uses).
    pub current: PreviewShortcut,
    pub spacebar: PreviewPreset,
    pub shift_spacebar: PreviewPreset,
    pub numpad0: PreviewPreset,
    pub shift_numpad0: PreviewPreset,
    pub alt_numpad0: PreviewPreset,
}

impl Default for PreviewSettings {
    fn default() -> Self {
        use PreviewShortcut as S;
        PreviewSettings {
            current: S::Spacebar,
            spacebar: PreviewPreset::default_for(S::Spacebar),
            shift_spacebar: PreviewPreset::default_for(S::ShiftSpacebar),
            numpad0: PreviewPreset::default_for(S::Numpad0),
            shift_numpad0: PreviewPreset::default_for(S::ShiftNumpad0),
            alt_numpad0: PreviewPreset::default_for(S::AltNumpad0),
        }
    }
}

impl PreviewSettings {
    pub fn get(&self, s: PreviewShortcut) -> &PreviewPreset {
        match s {
            PreviewShortcut::Spacebar => &self.spacebar,
            PreviewShortcut::ShiftSpacebar => &self.shift_spacebar,
            PreviewShortcut::Numpad0 => &self.numpad0,
            PreviewShortcut::ShiftNumpad0 => &self.shift_numpad0,
            PreviewShortcut::AltNumpad0 => &self.alt_numpad0,
        }
    }
    pub fn get_mut(&mut self, s: PreviewShortcut) -> &mut PreviewPreset {
        match s {
            PreviewShortcut::Spacebar => &mut self.spacebar,
            PreviewShortcut::ShiftSpacebar => &mut self.shift_spacebar,
            PreviewShortcut::Numpad0 => &mut self.numpad0,
            PreviewShortcut::ShiftNumpad0 => &mut self.shift_numpad0,
            PreviewShortcut::AltNumpad0 => &mut self.alt_numpad0,
        }
    }
    /// The options of the shortcut shown in the panel.
    pub fn active(&self) -> &PreviewPreset {
        self.get(self.current)
    }
}

/// What a preview plays: frames `start..=end` (comp frames), starting at `first`, every
/// `step`-th frame, `fps` frames shown per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewPlan {
    pub start: i64,
    pub end: i64,
    pub first: i64,
    pub step: i64,
    /// Comp frames advanced per second of playback (the frame rate; Skip doesn't change speed).
    pub fps: f64,
    pub looping: bool,
    pub cache_first: bool,
    pub video: bool,
    pub audio: bool,
    /// Where the current time goes when the preview stops without moving it.
    pub origin: Tick,
}

impl PreviewPlan {
    /// The frame shown `elapsed` seconds after the start; `None` once a non-looping preview has
    /// played to its end.
    pub fn frame_at(&self, elapsed: f64) -> Option<i64> {
        let n = ((elapsed.max(0.0) * self.fps) / self.step as f64).floor() as i64;
        self.frame_after(self.first, n)
    }

    /// The frame `n` played frames after `from` (wrapping when looping).
    pub fn frame_after(&self, from: i64, n: i64) -> Option<i64> {
        let f = from + n * self.step;
        if f <= self.end {
            return Some(f);
        }
        if !self.looping {
            return None;
        }
        // Loop back to the range start on the skip grid.
        let len = ((self.end - self.start) / self.step + 1).max(1);
        let past = (f - self.end - 1) / self.step;
        Some(self.start + past.rem_euclid(len) * self.step)
    }

    /// Every frame the preview shows, in range order.
    pub fn frames(&self) -> impl Iterator<Item = i64> + '_ {
        (self.start..=self.end).step_by(self.step.max(1) as usize)
    }
}

/// The plan of a preview of `comp` with options `p`, started at comp time `now`.
pub fn plan(p: &PreviewPreset, comp: &Comp, now: Tick) -> PreviewPlan {
    let fr = comp.frame_rate;
    let last = fr.frame_at(comp.duration - comp.frame_duration()).max(0);
    let cur = fr.frame_at(now).clamp(0, last);
    let (wa, wb) = (fr.frame_at(comp.work_area.0), fr.frame_at(comp.work_area.1 - comp.frame_duration()));
    let (wa, wb) = (wa.clamp(0, last), wb.clamp(0, last).max(wa.clamp(0, last)));
    let (start, end) = match p.range {
        PreviewRange::WorkArea => (wa, wb),
        PreviewRange::WorkAreaExtended => (wa.min(cur), wb.max(cur)),
        PreviewRange::EntireDuration => (0, last),
        PreviewRange::AroundCurrentTime => {
            let pre = (p.pre_roll * fr.as_f64()).round() as i64;
            let post = (p.post_roll * fr.as_f64()).round() as i64;
            ((cur - pre).max(0), (cur + post).min(last))
        }
    };
    let step = p.skip as i64 + 1;
    let first = match p.play_from {
        PlayFrom::RangeStart => start,
        // From the current time when it is inside the range (and not on its last frame).
        PlayFrom::CurrentTime if cur >= start && cur < end => start + (cur - start) / step * step,
        PlayFrom::CurrentTime => start,
    };
    PreviewPlan {
        start,
        end,
        first,
        step,
        fps: p.frame_rate.unwrap_or_else(|| fr.as_f64()),
        looping: p.loop_,
        cache_first: p.cache_before_playback,
        video: p.include_video,
        audio: p.include_audio,
        origin: now,
    }
}

fn shortcut_p(s: &Session, p: &Value, cmd: &str) -> Result<PreviewShortcut> {
    match str_p(p, "shortcut") {
        Some(k) => {
            PreviewShortcut::parse(k).ok_or_else(|| bad(cmd, format!("unknown shortcut `{k}` (spacebar|shiftSpacebar|numpad0|shiftNumpad0|altNumpad0)")))
        }
        None => Ok(s.prefs.preview.current),
    }
}

fn get(s: &mut Session, p: &Value) -> Result<Value> {
    let sc = shortcut_p(s, p, "playback.settings.get")?;
    let mut out = serde_json::to_value(s.prefs.preview.get(sc)).map_err(|e| bad("playback.settings.get", e.to_string()))?;
    out["shortcut"] = json!(sc.id());
    out["current"] = json!(s.prefs.preview.current.id());
    if let Some(c) = s.active_comp() {
        let pl = plan(s.prefs.preview.get(sc), c, s.time());
        out["plan"] = json!({"start": pl.start, "end": pl.end, "first": pl.first, "step": pl.step, "fps": pl.fps});
    }
    Ok(out)
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "playback.settings.set";
    let sc = shortcut_p(s, p, c)?;
    let mut fields = serde_json::Map::new();
    if let Some(Value::Object(m)) = p.get("values") {
        fields.extend(m.clone());
    }
    if let Value::Object(m) = p {
        for (k, v) in m {
            if !matches!(k.as_str(), "shortcut" | "values" | "current" | "reset") {
                fields.insert(k.clone(), v.clone());
            }
        }
    }
    let mut next = s.prefs.preview.clone();
    if p.get("reset").and_then(Value::as_bool) == Some(true) {
        *next.get_mut(sc) = PreviewPreset::default_for(sc);
    }
    let mut cur = serde_json::to_value(next.get(sc)).map_err(|e| bad(c, e.to_string()))?;
    for (k, v) in fields {
        if cur.get(&k).is_none() {
            return Err(bad(c, format!("unknown preview setting `{k}`")));
        }
        // Frame rate: a number, or "auto"/null.
        let v = if k == "frameRate" && (v.is_null() || v.as_str().is_some_and(|x| x.eq_ignore_ascii_case("auto"))) { Value::Null } else { v };
        cur[k.as_str()] = v;
    }
    let mut pr: PreviewPreset = serde_json::from_value(cur).map_err(|e| bad(c, e.to_string()))?;
    pr.normalize();
    *next.get_mut(sc) = pr;
    if let Some(k) = str_p(p, "current") {
        next.current = PreviewShortcut::parse(k).ok_or_else(|| bad(c, format!("unknown shortcut `{k}`")))?;
    } else if p.get("shortcut").is_some() && p.as_object().is_some_and(|m| m.len() == 1) {
        // `{shortcut}` alone shows that shortcut in the panel.
        next.current = sc;
    }
    s.prefs.preview = next;
    s.save_prefs();
    get(s, &json!({"shortcut": sc.id()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        query!(
            "playback.settings.get",
            "Preview Settings",
            "{shortcut?: spacebar|shiftSpacebar|numpad0|shiftNumpad0|altNumpad0 (default: the one shown in the Preview panel)} → options + plan {start, end, first, step, fps}",
            get
        ),
        cmd!(
            "playback.settings.set",
            "Change Preview Settings",
            [],
            None,
            "{shortcut?, current?: shortcut shown in the panel, reset?: bool, values?: {…}, includeVideo?, includeAudio?, includeOverlays?, includeLayerControls?, loop?, cacheBeforePlayback?, range?: workArea|workAreaExtended|entireDuration|aroundCurrentTime, preRoll?, postRoll?: seconds, playFrom?: rangeStart|currentTime, frameRate?: fps|\"auto\", skip?, resolution?: auto|full|half|third|quarter|custom, customResolution?, fullScreen?, playCachedFrames?, moveTimeToPreviewTime?}",
            always,
            set
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;

    fn comp_session() -> Session {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "C", "width": 320, "height": 180, "frameRate": 24, "duration": 10})).unwrap();
        s
    }

    #[test]
    fn ranges_and_play_from() {
        let mut s = comp_session();
        let mut c = s.active_comp().unwrap().clone();
        c.work_area = (Tick::from_seconds_f64(2.0), Tick::from_seconds_f64(4.0));
        let at = |sec: f64| Tick::from_seconds_f64(sec);
        let mut p = PreviewPreset { range: PreviewRange::WorkArea, play_from: PlayFrom::RangeStart, ..Default::default() };
        let pl = plan(&p, &c, at(1.0));
        assert_eq!((pl.start, pl.end, pl.first), (48, 95, 48));
        // Extended by the current time (before the work area).
        p.range = PreviewRange::WorkAreaExtended;
        let pl = plan(&p, &c, at(1.0));
        assert_eq!((pl.start, pl.end), (24, 95));
        // ... and after it.
        let pl = plan(&p, &c, at(6.0));
        assert_eq!((pl.start, pl.end), (48, 144));
        p.range = PreviewRange::EntireDuration;
        p.play_from = PlayFrom::CurrentTime;
        let pl = plan(&p, &c, at(5.0));
        assert_eq!((pl.start, pl.end, pl.first), (0, 239, 120));
        p.range = PreviewRange::AroundCurrentTime;
        p.pre_roll = 1.0;
        p.post_roll = 0.5;
        let pl = plan(&p, &c, at(5.0));
        assert_eq!((pl.start, pl.end, pl.first), (96, 132, 120));
        // Pre-roll clamps at the comp start.
        let pl = plan(&p, &c, at(0.25));
        assert_eq!(pl.start, 0);
        s.execute("playback.settings.set", json!({"range": "workArea"})).unwrap();
    }

    #[test]
    fn skip_rate_and_loop() {
        let s = comp_session();
        let c = s.active_comp().unwrap().clone();
        let p = PreviewPreset { range: PreviewRange::EntireDuration, play_from: PlayFrom::RangeStart, skip: 2, frame_rate: Some(12.0), ..Default::default() };
        let pl = plan(&p, &c, Tick::ZERO);
        assert_eq!(pl.step, 3);
        assert_eq!(pl.fps, 12.0);
        // 12 comp frames per second, every third shown.
        assert_eq!(pl.frame_at(0.0), Some(0));
        assert_eq!(pl.frame_at(0.26), Some(3));
        assert_eq!(pl.frame_at(1.0), Some(12));
        assert!(pl.frames().all(|f| f % 3 == 0));
        // Loop wraps onto the skip grid; no loop ends.
        assert_eq!(pl.frame_after(237, 1), Some(0));
        let pl = PreviewPlan { looping: false, ..pl };
        assert_eq!(pl.frame_after(237, 1), None);
    }

    /// #551: Shift+Spacebar used to skip every other frame out of the box.
    #[test]
    fn shift_spacebar_plays_every_frame_by_default() {
        assert_eq!(PreviewPreset::default_for(PreviewShortcut::ShiftSpacebar).skip, 0);
        assert_eq!(PreviewPreset::default_for(PreviewShortcut::ShiftSpacebar), PreviewPreset::default_for(PreviewShortcut::Spacebar));
    }

    #[test]
    fn settings_commands_round_trip() {
        let mut s = comp_session();
        let r = s.execute("playback.settings.get", json!({})).unwrap();
        assert_eq!(r["shortcut"], "spacebar");
        assert_eq!(r["range"], "workAreaExtended");
        // Each shortcut keeps its own options.
        let r = s
            .execute("playback.settings.set", json!({"shortcut": "shiftNumpad0", "skip": 3, "frameRate": 15, "resolution": "quarter", "loop": false}))
            .unwrap();
        assert_eq!(r["skip"], 3);
        assert_eq!(r["frameRate"], 15.0);
        assert_eq!(r["plan"]["step"], 4);
        let sp = s.execute("playback.settings.get", json!({"shortcut": "Spacebar"})).unwrap();
        assert_eq!(sp["skip"], 0);
        assert_eq!(sp["frameRate"], Value::Null);
        // Auto rate, panel shortcut, reset.
        s.execute("playback.settings.set", json!({"shortcut": "shiftNumpad0", "frameRate": "auto", "current": "shiftNumpad0"})).unwrap();
        assert_eq!(s.prefs.preview.current, PreviewShortcut::ShiftNumpad0);
        assert_eq!(s.prefs.preview.shift_numpad0.frame_rate, None);
        assert!(s.execute("playback.settings.set", json!({"bogus": 1})).is_err());
        assert!(s.execute("playback.settings.set", json!({"shortcut": "f13"})).is_err());
        s.execute("playback.settings.set", json!({"shortcut": "shiftNumpad0", "reset": true})).unwrap();
        assert_eq!(s.prefs.preview.shift_numpad0, PreviewPreset::default_for(PreviewShortcut::ShiftNumpad0));
        // Persisted with the settings.
        let v: Value = serde_json::from_str(&s.prefs.to_json()).unwrap();
        assert_eq!(v["preview"]["current"], "shiftNumpad0");
        let back: crate::prefs::Prefs = serde_json::from_value(v).unwrap();
        assert_eq!(back.preview, s.prefs.preview);
    }

    #[test]
    fn shortcut_names() {
        for s in PreviewShortcut::ALL {
            assert_eq!(PreviewShortcut::parse(s.id()), Some(s));
            assert_eq!(PreviewShortcut::parse(s.label()), Some(s));
        }
    }
}
