//! Image sequences: numbered stills (`shot_0001.png`, `shot_0002.png`, …) played as one footage
//! item ([`FootageKind::Sequence`]).
//!
//! A sequence's files are kept in frame-number order ([`Footage::sequence`]). How its frames map
//! to the files follows Interpret Footage: **Start Frame** ([`Footage::start_frame`]) is the
//! frame number shown at the footage's first frame. A gap in the numbering shows a placeholder
//! (colour bars), as in After Effects, except in a sequence imported with Force Alphabetical
//! Order ([`Footage::alphabetical`]), whose files play one after another.

use crate::{Footage, FootageKind};

/// The most frames an image sequence's numbering can span (a gap of millions of frames is a
/// naming accident, such as dates in file names, not footage).
pub const MAX_SEQUENCE_FRAMES: i64 = 10_000_000;

/// Split a file stem into (prefix, frame number digits): `shot_0012` → (`shot_`, `0012`).
/// `None` when it doesn't end in digits.
pub fn split_frame_number(stem: &str) -> Option<(&str, &str)> {
    let digits = stem.bytes().rev().take_while(u8::is_ascii_digit).count();
    // ASCII digits are one byte each, so the split is on a char boundary.
    (digits > 0).then(|| stem.split_at(stem.len() - digits))
}

/// The file stem of a path (either separator, so Windows paths split on any host).
fn stem_of(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

/// The frame number at the end of a file name (`shot_0012.png` → 12); `None` without one.
pub fn frame_number(path: &str) -> Option<i64> {
    let (_, digits) = split_frame_number(stem_of(path))?;
    // Keep the last 15 digits: longer runs would overflow and aren't frame numbers anyway.
    digits.get(digits.len().saturating_sub(15)..)?.parse().ok()
}

/// The After Effects name of a numbered sequence: `shot_[0001-0250].png`. `None` when the first
/// file isn't numbered.
pub fn sequence_name(files: &[String]) -> Option<String> {
    let (first, last) = (files.first()?, files.last()?);
    let name = |p: &str| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string();
    let (prefix, a) = split_frame_number(stem_of(first))?;
    let (_, b) = split_frame_number(stem_of(last))?;
    let first_name = name(first);
    let ext = first_name.rsplit_once('.').map(|(_, e)| format!(".{e}")).unwrap_or_default();
    Some(format!("{prefix}[{a}-{b}]{ext}"))
}

/// Frame numbers missing from a run of numbered files, as `(from, to)` ranges.
pub fn missing_frame_ranges(files: &[String]) -> Vec<(i64, i64)> {
    let mut ns: Vec<i64> = files.iter().filter_map(|p| frame_number(p)).collect();
    ns.sort_unstable();
    ns.dedup();
    ns.windows(2).filter(|w| w[1].saturating_sub(w[0]) > 1).map(|w| (w[0].saturating_add(1), w[1].saturating_sub(1))).collect()
}

impl Footage {
    /// Whether this is an image sequence with files.
    fn is_file_sequence(&self) -> bool {
        self.kind == FootageKind::Sequence && !self.sequence.is_empty()
    }

    /// Frame number of the sequence's `i`th file.
    fn file_number(&self, i: usize) -> Option<i64> {
        self.sequence.get(i).and_then(|p| frame_number(p))
    }

    /// The numbering is used: a numbered first file, not in Force Alphabetical Order.
    fn numbered(&self) -> Option<i64> {
        (!self.alphabetical).then(|| self.file_number(0)).flatten()
    }

    /// Index of the first file numbered `n` or later.
    fn first_file_from(&self, n: i64) -> usize {
        self.sequence.partition_point(|p| frame_number(p).is_some_and(|x| x < n))
    }

    /// The frame number at the footage's first frame: Start Frame, or the first file's number
    /// (0 when unnumbered).
    pub fn first_frame_number(&self) -> i64 {
        self.start_frame.or_else(|| self.file_number(0)).unwrap_or(0)
    }

    /// How many frames an image sequence has: the span of its numbering from Start Frame, or its
    /// files from Start Frame on (Force Alphabetical Order, or unnumbered files). At least 1; 0
    /// for other footage.
    pub fn sequence_frames(&self) -> i64 {
        if !self.is_file_sequence() {
            return 0;
        }
        let n = match self.numbered() {
            Some(first) => {
                let last = self.sequence.len().checked_sub(1).and_then(|i| self.file_number(i)).unwrap_or(first);
                last.saturating_sub(self.start_frame.unwrap_or(first)).saturating_add(1)
            }
            None => {
                let skip = self.start_frame.map_or(0, |s| self.first_file_from(s));
                self.sequence.len().saturating_sub(skip) as i64
            }
        };
        n.clamp(1, MAX_SEQUENCE_FRAMES)
    }

    /// The file an image sequence shows at its frame `i` (0-based, after looping). `None` for a
    /// missing frame shown as a placeholder (or when there are no files).
    pub fn sequence_file(&self, i: i64) -> Option<&str> {
        if !self.is_file_sequence() {
            return None;
        }
        let Some(first) = self.numbered() else {
            let skip = self.start_frame.map_or(0, |s| self.first_file_from(s));
            let k = usize::try_from(i).ok()?.saturating_add(skip);
            // Past the last file (Start Frame after it): the last file.
            return self.sequence.get(k).or(self.sequence.last()).map(String::as_str);
        };
        let n = self.start_frame.unwrap_or(first).saturating_add(i);
        let k = self.first_file_from(n);
        self.sequence.get(k).filter(|p| frame_number(p) == Some(n)).map(String::as_str)
    }

    /// Set an image sequence's duration to its frames at its frame rate (after its files,
    /// frame rate or Start Frame changed).
    pub fn sync_sequence_duration(&mut self) {
        if self.is_file_sequence() {
            self.duration = self.frame_rate.tick_of(self.sequence_frames());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(numbers: &[i64]) -> Footage {
        Footage { kind: FootageKind::Sequence, sequence: numbers.iter().map(|n| format!("C:\\shots\\a_{n:04}.png")).collect(), ..Default::default() }
    }

    #[test]
    fn numbers_and_names() {
        assert_eq!(split_frame_number("shot_0012"), Some(("shot_", "0012")));
        assert_eq!(split_frame_number("shot"), None);
        assert_eq!(split_frame_number("v2_10"), Some(("v2_", "10")));
        assert_eq!(frame_number("/a/v2/shot_0012.exr"), Some(12));
        assert_eq!(frame_number("C:\\a.b\\shot.png"), None);
        assert_eq!(frame_number("/a/99999999999999999999999.png"), Some(999_999_999_999_999));
        let files: Vec<String> = ["/s/fx_0998.png", "/s/fx_1002.png"].map(String::from).to_vec();
        assert_eq!(sequence_name(&files).as_deref(), Some("fx_[0998-1002].png"));
        assert_eq!(sequence_name(&["/s/a.png".to_string()]), None);
    }

    #[test]
    fn missing_frames_map_frames_to_files() {
        // Files 1, 2, 5: frames 3 and 4 are missing and show placeholders.
        let f = seq(&[1, 2, 5]);
        assert_eq!(f.sequence_frames(), 5);
        let at = |f: &Footage, i| f.sequence_file(i).map(|p| frame_number(p).unwrap_or(-1));
        assert_eq!((0..5).map(|i| at(&f, i)).collect::<Vec<_>>(), [Some(1), Some(2), None, None, Some(5)]);
        // Force Alphabetical Order: the files play one after another.
        let s = Footage { alphabetical: true, ..seq(&[1, 2, 5]) };
        assert_eq!(s.sequence_frames(), 3);
        assert_eq!((0..3).map(|i| at(&s, i)).collect::<Vec<_>>(), [Some(1), Some(2), Some(5)]);
        assert_eq!(missing_frame_ranges(&f.sequence), [(3, 4)]);
    }

    #[test]
    fn projects_saved_without_the_flag_show_placeholders() {
        // Projects saved before #297 have no `alphabetical`: their gaps show placeholders, as in
        // After Effects; only Force Alphabetical Order is saved.
        let f = seq(&[1, 2, 5]);
        let json = serde_json::to_value(&f).unwrap();
        assert!(json.get("alphabetical").is_none(), "{json}");
        let back: Footage = serde_json::from_value(json).unwrap();
        assert_eq!((back.alphabetical, back.sequence_frames()), (false, 5));
        let alpha = serde_json::to_value(Footage { alphabetical: true, ..seq(&[1, 2, 5]) }).unwrap();
        assert_eq!(serde_json::from_value::<Footage>(alpha).unwrap().sequence_frames(), 3);
    }

    #[test]
    fn start_frame_trims_or_pads_the_head() {
        let at = |f: &Footage, i| f.sequence_file(i).map(|p| frame_number(p).unwrap_or(-1));
        let mut f = seq(&[1001, 1002, 1003]);
        assert_eq!(f.first_frame_number(), 1001);
        f.start_frame = Some(1002);
        assert_eq!((f.sequence_frames(), at(&f, 0), at(&f, 1)), (2, Some(1002), Some(1003)));
        f.start_frame = Some(999);
        assert_eq!((f.sequence_frames(), at(&f, 0), at(&f, 2)), (5, None, Some(1001)));
        f.alphabetical = true;
        f.start_frame = Some(1003);
        assert_eq!((f.sequence_frames(), at(&f, 0)), (1, Some(1003)));
        // Hostile values never panic and stay bounded.
        f.alphabetical = false;
        f.start_frame = Some(i64::MIN);
        assert_eq!(f.sequence_frames(), MAX_SEQUENCE_FRAMES);
        assert_eq!(f.sequence_file(i64::MAX), None);
        f.start_frame = Some(i64::MAX);
        assert_eq!(f.sequence_frames(), 1);
        f.alphabetical = true;
        assert_eq!(f.sequence_file(-1), None);
        assert!(f.sequence_file(5).is_some());
    }

    #[test]
    fn duration_follows_frames_and_rate() {
        let mut f = seq(&[1, 2, 5]);
        f.frame_rate = effectcraft_time::FrameRate::from_f64(10.0);
        f.sync_sequence_duration();
        assert!((f.duration.seconds() - 0.5).abs() < 1e-9);
        assert_eq!(Footage::default().sequence_frames(), 0);
    }
}
