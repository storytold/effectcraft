//! Premiere Pro interop through the public timeline interchange formats Premiere itself reads and
//! writes (After Effects' File ▸ Import / Export ▸ Adobe Premiere Pro Project…):
//!
//! - **Final Cut Pro 7 XML** (`xmeml`, `.xml`): Premiere's File ▸ Export ▸ Final Cut Pro XML and
//!   File ▸ Import; the default for export;
//! - **FCPXML** 1.9–1.11 (`.fcpxml`);
//! - **OpenTimelineIO** (`.otio`);
//! - **CMX 3600 EDL** (`.edl`);
//! - **AAF** (`.aaf`) and **OMF** (`.omf`) (embedded audio is not extracted).
//!
//! Parsing and writing is FilmCraft's `filmcraft-interchange` (our first-party sibling project);
//! this crate maps its timeline model to compositions and back:
//!
//! - [`import`]: sequences → compositions (size, rate, duration, pixel aspect, start timecode,
//!   markers); video tracks → layers stacked by track (top track = top layer); clips → footage
//!   layers with in/out/start, speed and reverse (time stretch), frame holds (time remapping),
//!   Motion (position/scale/rotation/anchor point) and Opacity with keyframes; cross dissolves and
//!   dips → overlapping layers with opacity keyframes; nested sequences → precomps; audio tracks →
//!   audio-only layers with Audio Levels; bins → Project panel folders; missing media →
//!   placeholders.
//! - [`export`]: a composition → one sequence with one track per layer (bottom layer = V1),
//!   footage clips with their timing, Motion and Opacity; precomps as nested sequences; solids as
//!   colour mattes; layers only EffectCraft can draw (text, shapes, effects, masks, 3D…) are
//!   either left out or pre-rendered by the caller (ProRes 4444 with alpha, see
//!   [`plan_prerender`]) and referenced as media.
//!
//! The native `.prproj` format has no public specification and is not read or written.
//!
//! This crate is L3 and does no file I/O: media files are probed through a callback, pre-rendered
//! media come in as paths, and documents are bytes.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod export;
mod import;
#[cfg(test)]
mod tests;

pub use export::{ExportOptions, ExportOutput, PrecompMode, PrerenderMode, Prerendered, export, plan_prerender, prerender_reason};
pub use import::{ImportOptions, ImportResult, import};

pub use filmcraft_geom as fc_geom;
/// FilmCraft's interchange crate (format readers/writers) and timeline model, re-exported so
/// tests and tools can build documents in code.
pub use filmcraft_interchange as fc;
pub use filmcraft_media as fc_media;
pub use filmcraft_project as fc_project;
pub use filmcraft_time as fc_time;

use serde::{Deserialize, Serialize};

/// A timeline interchange format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TimelineFormat {
    /// Final Cut Pro 7 XML (`xmeml`), the format Premiere Pro imports and exports.
    Fcp7Xml,
    /// FCPXML 1.9–1.11.
    Fcpxml,
    /// OpenTimelineIO JSON.
    Otio,
    /// CMX 3600 edit decision list.
    Edl,
    /// Advanced Authoring Format (AAF Edit Protocol), as Premiere Pro exports for audio post.
    Aaf,
    /// OMF Interchange 2.0.
    Omf,
}

impl TimelineFormat {
    pub const ALL: [TimelineFormat; 6] =
        [TimelineFormat::Fcp7Xml, TimelineFormat::Fcpxml, TimelineFormat::Otio, TimelineFormat::Edl, TimelineFormat::Aaf, TimelineFormat::Omf];

    /// Id used by commands (`xml`, `fcpxml`, `otio`, `edl`).
    pub fn id(self) -> &'static str {
        match self {
            TimelineFormat::Fcp7Xml => "xml",
            TimelineFormat::Fcpxml => "fcpxml",
            TimelineFormat::Otio => "otio",
            TimelineFormat::Edl => "edl",
            TimelineFormat::Aaf => "aaf",
            TimelineFormat::Omf => "omf",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TimelineFormat::Fcp7Xml => "Final Cut Pro XML (Premiere Pro)",
            TimelineFormat::Fcpxml => "FCPXML",
            TimelineFormat::Otio => "OpenTimelineIO",
            TimelineFormat::Edl => "CMX 3600 EDL",
            TimelineFormat::Aaf => "AAF",
            TimelineFormat::Omf => "OMF",
        }
    }

    /// File extension (without the dot).
    pub fn extension(self) -> &'static str {
        self.fc().extension()
    }

    /// Parse a command parameter: a format id, a name or an extension.
    pub fn parse(s: &str) -> Option<TimelineFormat> {
        match s.trim().trim_start_matches('.').to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "xml" | "fcp7" | "fcp7xml" | "xmeml" | "finalcutproxml" | "premiere" | "premierepro" | "prproj" => Some(TimelineFormat::Fcp7Xml),
            "fcpxml" | "fcpxmld" | "fcpx" => Some(TimelineFormat::Fcpxml),
            "otio" | "opentimelineio" => Some(TimelineFormat::Otio),
            "edl" | "cmx3600" | "cmx" => Some(TimelineFormat::Edl),
            "aaf" => Some(TimelineFormat::Aaf),
            "omf" | "omfi" => Some(TimelineFormat::Omf),
            _ => None,
        }
    }

    /// Format of a file from its extension.
    pub fn from_path(path: &str) -> Option<TimelineFormat> {
        let ext = path.rsplit_once('.').map(|(_, e)| e)?;
        if ext.contains('/') || ext.contains('\\') {
            return None;
        }
        filmcraft_interchange::Format::from_extension(ext).map(TimelineFormat::from_fc)
    }

    /// Sniff a document (bytes first, the extension as a hint for ambiguous `.xml`).
    pub fn detect(bytes: &[u8], path: Option<&str>) -> Option<TimelineFormat> {
        let ext = path.and_then(|p| p.rsplit_once('.')).map(|(_, e)| e);
        filmcraft_interchange::detect(bytes, ext).map(TimelineFormat::from_fc)
    }

    pub(crate) fn fc(self) -> filmcraft_interchange::Format {
        match self {
            TimelineFormat::Fcp7Xml => filmcraft_interchange::Format::Fcp7Xml,
            TimelineFormat::Fcpxml => filmcraft_interchange::Format::Fcpxml,
            TimelineFormat::Otio => filmcraft_interchange::Format::Otio,
            TimelineFormat::Edl => filmcraft_interchange::Format::Edl,
            TimelineFormat::Aaf => filmcraft_interchange::Format::Aaf,
            TimelineFormat::Omf => filmcraft_interchange::Format::Omf,
        }
    }

    pub(crate) fn from_fc(f: filmcraft_interchange::Format) -> TimelineFormat {
        match f {
            filmcraft_interchange::Format::Fcp7Xml => TimelineFormat::Fcp7Xml,
            filmcraft_interchange::Format::Fcpxml => TimelineFormat::Fcpxml,
            filmcraft_interchange::Format::Otio => TimelineFormat::Otio,
            filmcraft_interchange::Format::Edl => TimelineFormat::Edl,
            filmcraft_interchange::Format::Aaf => TimelineFormat::Aaf,
            filmcraft_interchange::Format::Omf => TimelineFormat::Omf,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Interchange(#[from] filmcraft_interchange::Error),
    #[error("not a timeline document (expected Final Cut Pro XML, FCPXML, OpenTimelineIO or EDL)")]
    UnknownFormat,
    #[error("no composition {0}")]
    NoComp(u64),
    #[error("the timeline document nests more than {0} levels deep")]
    TooDeep(usize),
    #[error("the timeline document declares XML entities, which timeline files don't use and which can expand without bound")]
    XmlEntities,
}

pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------- shared conversions

pub(crate) fn rate_in(r: filmcraft_time::FrameRate) -> effectcraft_time::FrameRate {
    if r.num <= 0 || r.den <= 0 { effectcraft_time::FrameRate::new(30, 1) } else { effectcraft_time::FrameRate::new(r.num, r.den) }
}

pub(crate) fn rate_out(r: effectcraft_time::FrameRate) -> filmcraft_time::FrameRate {
    filmcraft_time::FrameRate { num: r.num, den: r.den }
}

// Both projects count time in the same ticks (254 016 000 000 per second).
const _: () = assert!(filmcraft_time::TICKS_PER_SECOND == effectcraft_time::TICKS_PER_SECOND);

pub(crate) fn tick_in(t: filmcraft_time::Tick) -> effectcraft_time::Tick {
    effectcraft_time::Tick(t.0)
}

pub(crate) fn tick_out(t: effectcraft_time::Tick) -> filmcraft_time::Tick {
    filmcraft_time::Tick(t.0)
}

/// A pixel aspect ratio as a small fraction (1.0 → 1:1, 0.9 → 9:10, 1.2121 → 40:33…).
pub(crate) fn par_fraction(par: f64) -> (u32, u32) {
    if !par.is_finite() || par <= 0.0 || (par - 1.0).abs() < 1e-6 {
        return (1, 1);
    }
    let mut best = (1u32, 1u32);
    let mut err = f64::MAX;
    for den in 1..=200u32 {
        let num = (par * den as f64).round() as u32;
        if num == 0 {
            continue;
        }
        let e = (num as f64 / den as f64 - par).abs();
        if e < err - 1e-9 {
            err = e;
            best = (num, den);
        }
        if e < 1e-6 {
            break;
        }
    }
    best
}
