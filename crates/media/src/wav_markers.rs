//! Markers a WAV file carries, read from its RIFF chunks.
//!
//! Two encodings are read and merged:
//! - **Cue points** (the standard): a `cue ` chunk with one point per marker, named by `labl` (or
//!   `note`) sub-chunks of a `LIST` `adtl` chunk, and given a length by `ltxt`.
//! - **XMP** (Adobe's tools): an `_PMX` chunk holding an XMP packet whose `xmpDM:markers` list has
//!   one entry per marker (`xmpDM:startTime`, `xmpDM:duration`, `xmpDM:name`), counted in frames
//!   of `xmpDM:frameRate` (`f44100` for samples at 44.1 kHz).
//!
//! RF64 files (WAVs over 4 GB) carry a `0xFFFFFFFF` size for a chunk too big for 32 bits and give its real size in a
//! `ds64` chunk; those sizes are resolved so the chunks after the audio are still found.
//!
//! The file is untrusted input: every read is bounds-checked, sizes use checked arithmetic, and
//! the number of markers, of `adtl` sub-chunks looked at and the size of the XMP packet are capped.

use std::collections::{HashMap, HashSet};

use effectcraft_color::Label;
use effectcraft_project::Marker;
use effectcraft_time::Tick;

/// Most markers read from one file.
pub const MAX_MARKERS: usize = 10_000;
/// Largest XMP packet read (bytes).
const MAX_XMP: usize = 4 << 20;
/// Most `adtl` sub-chunks (`labl`, `note`, `ltxt`) looked at in one file: a few per cue point is plenty.
const MAX_ADTL_RECORDS: usize = 4 * MAX_MARKERS;
/// Most entries read from an RF64 file's `ds64` size table.
const MAX_DS64_TABLE: usize = 1_024;

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(s.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(s.try_into().ok()?))
}

/// The text of a NUL-terminated label (the rest of the chunk), lossily decoded.
fn label(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(b.get(..end).unwrap_or_default()).trim().to_string()
}

fn secs(frames: f64, rate: f64) -> Tick {
    if rate > 0.0 && frames.is_finite() { Tick::from_seconds_f64((frames / rate).max(0.0)) } else { Tick::ZERO }
}

/// Whether `bytes` start a RIFF/WAVE file.
pub fn is_wav(bytes: &[u8]) -> bool {
    matches!(bytes.get(0..4), Some(b"RIFF" | b"RF64")) && bytes.get(8..12) == Some(b"WAVE")
}

/// The sizes an RF64 file's `ds64` chunk gives for chunks too big for the 32-bit size field.
struct Ds64 {
    /// The `data` chunk's size.
    data: u64,
    /// Other chunks: `(id, size)`.
    table: Vec<([u8; 4], u64)>,
}

impl Ds64 {
    /// The real size of chunk `id`, when the file says.
    fn size_of(&self, id: &[u8; 4]) -> Option<u64> {
        if id == b"data" { Some(self.data) } else { self.table.iter().find(|(t, _)| t == id).map(|(_, size)| *size) }
    }
}

/// The `ds64` chunk of an RF64 file (`RF64`, size, `WAVE`, then `ds64`); `None` for any other file or one whose
/// `ds64` doesn't fit.
fn ds64(bytes: &[u8]) -> Option<Ds64> {
    if bytes.get(0..4) != Some(b"RF64") || bytes.get(12..16) != Some(b"ds64") {
        return None;
    }
    let size = usize::try_from(u32_at(bytes, 16)?).ok()?;
    let body = bytes.get(20..20usize.checked_add(size)?)?;
    // riff size (8), data size (8), sample count (8), table length (4), then `(id, size)` entries of 12 bytes
    let data = u64_at(body, 8)?;
    let entries = usize::try_from(u32_at(body, 24)?).ok()?;
    let mut table = Vec::new();
    for i in 0..entries.min(MAX_DS64_TABLE) {
        let at = 28usize.checked_add(i.checked_mul(12)?)?;
        let id = <[u8; 4]>::try_from(body.get(at..at.checked_add(4)?)?).ok()?;
        table.push((id, u64_at(body, at + 4)?));
    }
    Some(Ds64 { data, table })
}

/// The chunks of a WAVE file as `(id, body)`, stopping at the first one that doesn't fit.
fn chunks(bytes: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let big = ds64(bytes);
    let mut out = Vec::new();
    let mut at = 12usize;
    while let Some(head) = bytes.get(at..at.saturating_add(8)) {
        let (Ok(id), Some(field)) = (<[u8; 4]>::try_from(&head[..4]), u32_at(head, 4)) else { break };
        // In an RF64 file a size field of 0xFFFFFFFF stands for the size `ds64` gives.
        let size = match &big {
            Some(d) if field == u32::MAX => d.size_of(&id).unwrap_or(u64::from(field)),
            _ => u64::from(field),
        };
        let start = at.saturating_add(8);
        let Some(end) = usize::try_from(size).ok().and_then(|n| start.checked_add(n)) else { break };
        let Some(body) = bytes.get(start..end) else { break };
        out.push((id, body));
        // Chunks are padded to an even size.
        at = end.saturating_add(end & 1);
    }
    out
}

/// The markers `bytes` carry, in time order (empty for anything that isn't a WAV with markers).
pub fn read(bytes: &[u8]) -> Vec<Marker> {
    if !is_wav(bytes) {
        return Vec::new();
    }
    let chunks = chunks(bytes);
    let rate = chunks.iter().find(|(id, _)| id == b"fmt ").and_then(|(_, b)| u32_at(b, 4)).map_or(0.0, f64::from);
    let mut out = cue_markers(&chunks, rate);
    if let Some((_, xmp)) = chunks.iter().find(|(id, _)| id == b"_PMX") {
        // Tools that write both encodings repeat each marker.
        let mut seen: HashSet<(String, Tick)> = out.iter().map(|o| (o.comment.clone(), o.time)).collect();
        for m in xmp_markers(xmp, rate) {
            if seen.insert((m.comment.clone(), m.time)) {
                out.push(m);
            }
        }
    }
    out.truncate(MAX_MARKERS);
    out.sort_by_key(|m| m.time);
    out
}

/// Cue points named and sized by the `adtl` list.
fn cue_markers(chunks: &[([u8; 4], &[u8])], rate: f64) -> Vec<Marker> {
    let Some((_, cue)) = chunks.iter().find(|(id, _)| id == b"cue ") else { return Vec::new() };
    let count = (u32_at(cue, 0).unwrap_or(0) as usize).min(MAX_MARKERS);
    // The cue points first: `(id, offset in samples)`.
    let mut points = Vec::new();
    for i in 0..count {
        let Some(point) = cue.get(4usize.saturating_add(i.saturating_mul(24))..) else { break };
        let (Some(id), Some(offset)) = (u32_at(point, 0), u32_at(point, 20)) else { break };
        points.push((id, offset));
    }
    // Only the names and lengths of those ids are kept, so a hostile `adtl` list cannot grow them past the cue count,
    // and each record is a lookup, not a scan.
    let wanted: HashSet<u32> = points.iter().map(|(id, _)| *id).collect();
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut lengths: HashMap<u32, u32> = HashMap::new();
    let mut records = 0usize;
    'lists: for (_, list) in chunks.iter().filter(|(id, b)| id == b"LIST" && b.get(0..4) == Some(b"adtl")) {
        let mut at = 4usize;
        while let Some(head) = list.get(at..at.saturating_add(8)) {
            let (Ok(id), Some(size)) = (<[u8; 4]>::try_from(&head[..4]), u32_at(head, 4)) else { break };
            let start = at.saturating_add(8);
            let Some(end) = start.checked_add(size as usize) else { break };
            let Some(body) = list.get(start..end) else { break };
            records += 1;
            if records > MAX_ADTL_RECORDS {
                break 'lists;
            }
            if let Some(cue_id) = u32_at(body, 0).filter(|c| wanted.contains(c)) {
                match &id {
                    // A `labl` is the name; a `note` only when there is none.
                    b"labl" => {
                        names.insert(cue_id, label(body.get(4..).unwrap_or_default()));
                    }
                    b"note" => {
                        names.entry(cue_id).or_insert_with(|| label(body.get(4..).unwrap_or_default()));
                    }
                    // The first `ltxt` of a cue point gives its length.
                    b"ltxt" => {
                        lengths.entry(cue_id).or_insert_with(|| u32_at(body, 4).unwrap_or(0));
                    }
                    _ => {}
                }
            }
            at = end.saturating_add(end & 1);
        }
    }
    points
        .into_iter()
        .map(|(id, offset)| {
            let comment = names.get(&id).cloned().unwrap_or_default();
            let length = lengths.get(&id).copied().unwrap_or(0);
            Marker { time: secs(f64::from(offset), rate), duration: secs(f64::from(length), rate), comment, label: Label::None, ..Default::default() }
        })
        .collect()
}

/// The text of `name` in an XMP entry, as an attribute (`name="v"`) or an element
/// (`<name>v</name>`).
fn xmp_value(entry: &str, name: &str) -> Option<String> {
    let attr = format!("{name}=\"");
    if let Some(i) = entry.find(&attr) {
        let rest = entry.get(i + attr.len()..)?;
        return Some(unescape(rest.get(..rest.find('"')?)?));
    }
    let open = format!("<{name}>");
    let i = entry.find(&open)?;
    let rest = entry.get(i + open.len()..)?;
    Some(unescape(rest.get(..rest.find('<')?)?))
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or_default());
        let tail = rest.get(i..).unwrap_or_default();
        let Some(semi) = tail.find(';').filter(|&n| n <= 10) else {
            out.push('&');
            rest = tail.get(1..).unwrap_or_default();
            continue;
        };
        let ent = tail.get(1..semi).unwrap_or_default();
        match ent {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let code = ent.strip_prefix("#x").and_then(|h| u32::from_str_radix(h, 16).ok()).or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()));
                match code.and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => out.push_str(tail.get(..=semi).unwrap_or_default()),
                }
            }
        }
        rest = tail.get(semi + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// A frame rate as XMP writes it: `f44100`, or `f30000s1001` (rate over scale).
fn xmp_rate(text: &str) -> Option<f64> {
    let t = text.trim().strip_prefix('f')?;
    let (num, den) = t.split_once('s').unwrap_or((t, "1"));
    let (n, d): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
    (n > 0.0 && d > 0.0).then_some(n / d)
}

/// The markers of an XMP packet's `xmpDM:markers` entries.
fn xmp_markers(packet: &[u8], file_rate: f64) -> Vec<Marker> {
    let text = String::from_utf8_lossy(packet.get(..packet.len().min(MAX_XMP)).unwrap_or_default());
    let mut out = Vec::new();
    // The frame rate belongs to the track an entry follows.
    let mut rate = file_rate;
    for entry in text.split("<rdf:li").skip(1) {
        if let Some(r) = xmp_value(entry, "xmpDM:frameRate").as_deref().and_then(xmp_rate) {
            rate = r;
        }
        let Some(start) = xmp_value(entry, "xmpDM:startTime").and_then(|v| v.trim().parse::<f64>().ok()) else { continue };
        let length = xmp_value(entry, "xmpDM:duration").and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0);
        let comment = xmp_value(entry, "xmpDM:name").unwrap_or_default();
        out.push(Marker { time: secs(start, rate), duration: secs(length, rate), comment, label: Label::None, ..Default::default() });
        if out.len() >= MAX_MARKERS {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((body.len() as u32).to_le_bytes());
        v.extend(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn fmt(rate: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(1u16.to_le_bytes()); // PCM
        b.extend(1u16.to_le_bytes()); // mono
        b.extend(rate.to_le_bytes());
        b.extend((rate * 2).to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        chunk(b"fmt ", &b)
    }

    fn wav(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.iter().flatten().copied().collect();
        let mut v = b"RIFF".to_vec();
        v.extend((body.len() as u32 + 4).to_le_bytes());
        v.extend(b"WAVE");
        v.extend(body);
        v
    }

    fn cue(points: &[(u32, u32)]) -> Vec<u8> {
        let mut b = (points.len() as u32).to_le_bytes().to_vec();
        for (id, offset) in points {
            b.extend(id.to_le_bytes());
            b.extend(0u32.to_le_bytes());
            b.extend(b"data");
            b.extend(0u32.to_le_bytes());
            b.extend(0u32.to_le_bytes());
            b.extend(offset.to_le_bytes());
        }
        chunk(b"cue ", &b)
    }

    fn labl(id: &[u8; 4], cue_id: u32, text: &str) -> Vec<u8> {
        let mut b = cue_id.to_le_bytes().to_vec();
        b.extend(text.as_bytes());
        b.push(0);
        chunk(id, &b)
    }

    fn adtl(subs: &[Vec<u8>]) -> Vec<u8> {
        let mut b = b"adtl".to_vec();
        b.extend(subs.iter().flatten());
        chunk(b"LIST", &b)
    }

    #[test]
    fn cue_points_are_named_by_the_adtl_list_and_timed_in_samples() {
        let file = wav(&[
            fmt(44100),
            chunk(b"data", &[0; 8]),
            cue(&[(2, 88200), (1, 22050)]),
            adtl(&[labl(b"labl", 1, "SEQ01_Prologue"), labl(b"labl", 2, "SEQ02_Vallee_P1_Agriculture")]),
        ]);
        let m = read(&file);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].comment.as_str(), m[0].time), ("SEQ01_Prologue", Tick::from_seconds_f64(0.5)));
        assert_eq!((m[1].comment.as_str(), m[1].time), ("SEQ02_Vallee_P1_Agriculture", Tick::from_seconds_f64(2.0)));
    }

    #[test]
    fn a_note_names_a_cue_only_when_it_has_no_label_and_ltxt_gives_a_length() {
        let mut ltxt = 1u32.to_le_bytes().to_vec();
        ltxt.extend(44100u32.to_le_bytes());
        ltxt.extend([0; 16]);
        let file = wav(&[
            fmt(44100),
            cue(&[(1, 0), (2, 44100)]),
            adtl(&[labl(b"note", 1, "from note"), chunk(b"ltxt", &ltxt), labl(b"labl", 2, "named"), labl(b"note", 2, "ignored")]),
        ]);
        let m = read(&file);
        assert_eq!((m[0].comment.as_str(), m[0].duration), ("from note", Tick::from_seconds_f64(1.0)));
        assert_eq!(m[1].comment, "named");
    }

    const XMP_ELEMENTS: &str = r#"<x:xmpmeta><rdf:RDF><rdf:Description><xmpDM:Tracks><rdf:Bag><rdf:li rdf:parseType="Resource"><xmpDM:trackName>Markers</xmpDM:trackName><xmpDM:trackType>Cue</xmpDM:trackType><xmpDM:frameRate>f44100</xmpDM:frameRate><xmpDM:markers><rdf:Seq><rdf:li rdf:parseType="Resource"><xmpDM:startTime>44100</xmpDM:startTime><xmpDM:duration>0</xmpDM:duration><xmpDM:name>One &amp; Two</xmpDM:name></rdf:li><rdf:li rdf:parseType="Resource"><xmpDM:startTime>132300</xmpDM:startTime><xmpDM:duration>22050</xmpDM:duration><xmpDM:name>Three</xmpDM:name></rdf:li></rdf:Seq></xmpDM:markers></rdf:li></rdf:Bag></xmpDM:Tracks></rdf:Description></rdf:RDF></x:xmpmeta>"#;

    #[test]
    fn xmp_markers_are_read_as_elements_in_frames_of_the_track_rate() {
        let file = wav(&[fmt(48000), chunk(b"_PMX", XMP_ELEMENTS.as_bytes())]);
        let m = read(&file);
        assert_eq!(m.len(), 2);
        // 44100 frames at f44100 is one second, whatever the file's own rate.
        assert_eq!((m[0].comment.as_str(), m[0].time), ("One & Two", Tick::from_seconds_f64(1.0)));
        assert_eq!((m[1].comment.as_str(), m[1].time, m[1].duration), ("Three", Tick::from_seconds_f64(3.0), Tick::from_seconds_f64(0.5)));
    }

    #[test]
    fn xmp_markers_are_read_as_attributes_and_use_the_file_rate_without_a_track_rate() {
        let xmp = r#"<rdf:Seq><rdf:li xmpDM:startTime="24000" xmpDM:duration="0" xmpDM:name="Attr &#233;"/></rdf:Seq>"#;
        let m = read(&wav(&[fmt(48000), chunk(b"_PMX", xmp.as_bytes())]));
        assert_eq!((m[0].comment.as_str(), m[0].time), ("Attr é", Tick::from_seconds_f64(0.5)));
    }

    #[test]
    fn a_marker_in_both_encodings_is_listed_once() {
        let xmp = r#"<rdf:li xmpDM:startTime="44100" xmpDM:name="Same"/><rdf:li xmpDM:startTime="88200" xmpDM:name="Only XMP"/>"#;
        let file = wav(&[fmt(44100), cue(&[(1, 44100)]), adtl(&[labl(b"labl", 1, "Same")]), chunk(b"_PMX", xmp.as_bytes())]);
        let names: Vec<String> = read(&file).into_iter().map(|m| m.comment).collect();
        assert_eq!(names, ["Same", "Only XMP"]);
    }

    #[test]
    fn importing_a_wav_puts_its_markers_on_the_footage() {
        let file = wav(&[
            fmt(44100),
            chunk(b"data", &[0; 4 * 44100]),
            cue(&[(1, 0), (2, 44100)]),
            adtl(&[labl(b"labl", 1, "SEQ01_Prologue"), labl(b"labl", 2, "SEQ02_Vallee_P1_Agriculture")]),
        ]);
        let f = crate::probe::probe_bytes("markers.wav", file.into()).expect("probe");
        assert!(f.has_audio);
        let names: Vec<&str> = f.markers.iter().map(|m| m.comment.as_str()).collect();
        assert_eq!(names, ["SEQ01_Prologue", "SEQ02_Vallee_P1_Agriculture"]);
        assert_eq!(f.markers[1].time, Tick::from_seconds_f64(1.0));
    }

    #[test]
    fn damaged_or_hostile_files_read_as_far_as_they_go_without_panicking() {
        assert!(read(b"").is_empty());
        assert!(read(b"RIFF").is_empty());
        assert!(read(b"not a wav file at all").is_empty());
        let good = wav(&[fmt(44100), cue(&[(1, 44100)]), adtl(&[labl(b"labl", 1, "Kept")])]);
        // Every truncation is safe.
        for n in 0..good.len() {
            let _ = read(&good[..n]);
        }
        // A cue count far beyond the chunk, and chunk sizes past the end of the file.
        let mut huge = 0xFFFF_FFFFu32.to_le_bytes().to_vec();
        huge.extend([0; 24]);
        let m = read(&wav(&[fmt(44100), chunk(b"cue ", &huge)]));
        assert!(m.len() <= 1);
        let mut lying = good.clone();
        lying.extend(b"LIST\xff\xff\xff\xffadtl");
        assert_eq!(read(&lying).len(), 1);
        // No sample rate: times are zero, not a division by zero.
        let m = read(&wav(&[cue(&[(1, 44100)])]));
        assert_eq!(m[0].time, Tick::ZERO);
    }

    /// A chunk whose size field says `field` (an RF64 file's `0xFFFFFFFF`) while its body is `body`.
    fn chunk_with_size_field(id: &[u8; 4], field: u32, body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend(field.to_le_bytes());
        v.extend(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    /// An RF64 `ds64` chunk: the `data` chunk's size and a table of other chunks' sizes.
    fn ds64_chunk(data_size: u64, table: &[([u8; 4], u64)]) -> Vec<u8> {
        let mut b = 0u64.to_le_bytes().to_vec(); // riff size (not used)
        b.extend(data_size.to_le_bytes());
        b.extend(0u64.to_le_bytes()); // sample count
        b.extend((table.len() as u32).to_le_bytes());
        for (id, size) in table {
            b.extend(id);
            b.extend(size.to_le_bytes());
        }
        chunk(b"ds64", &b)
    }

    fn rf64(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.iter().flatten().copied().collect();
        let mut v = b"RF64".to_vec();
        v.extend(u32::MAX.to_le_bytes());
        v.extend(b"WAVE");
        v.extend(body);
        v
    }

    #[test]
    fn rf64_markers_after_a_data_chunk_of_sentinel_size_are_read() {
        let audio = [0u8; 8];
        let file = rf64(&[
            ds64_chunk(audio.len() as u64, &[]),
            fmt(44100),
            chunk_with_size_field(b"data", u32::MAX, &audio),
            cue(&[(1, 44100)]),
            adtl(&[labl(b"labl", 1, "After the audio")]),
        ]);
        let m = read(&file);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].comment.as_str(), m[0].time), ("After the audio", Tick::from_seconds_f64(1.0)));
    }

    #[test]
    fn rf64_sizes_of_other_chunks_come_from_the_ds64_table() {
        let junk = [7u8; 6];
        let tail = || [cue(&[(1, 8000)]), adtl(&[labl(b"labl", 1, "Found")])].concat();
        let with_entry = rf64(&[ds64_chunk(0, &[(*b"junk", junk.len() as u64)]), fmt(8000), chunk_with_size_field(b"junk", u32::MAX, &junk), tail()]);
        assert_eq!(read(&with_entry)[0].comment, "Found");
        // Without the table entry the sentinel is all there is, and what follows is not reached.
        let without = rf64(&[ds64_chunk(0, &[]), fmt(8000), chunk_with_size_field(b"junk", u32::MAX, &junk), tail()]);
        assert!(read(&without).is_empty());
    }

    #[test]
    fn hostile_rf64_size_tables_read_as_far_as_they_go_without_panicking() {
        // sizes that overflow when added to an offset
        for data in [u64::MAX, u64::MAX - 7, 1 << 63] {
            let file = rf64(&[ds64_chunk(data, &[(*b"junk", u64::MAX)]), fmt(8000), chunk_with_size_field(b"data", u32::MAX, &[0; 4]), cue(&[(1, 0)])]);
            assert!(read(&file).is_empty());
        }
        // a table that claims far more entries than its chunk holds
        let mut liar = ds64_chunk(0, &[]);
        liar[8 + 24..8 + 28].copy_from_slice(&u32::MAX.to_le_bytes());
        let _ = read(&rf64(&[liar, fmt(8000), cue(&[(1, 0)])]));
        // every truncation of a good RF64 file is safe
        let good =
            rf64(&[ds64_chunk(4, &[]), fmt(44100), chunk_with_size_field(b"data", u32::MAX, &[0; 4]), cue(&[(1, 44100)]), adtl(&[labl(b"labl", 1, "Kept")])]);
        assert_eq!(read(&good).len(), 1);
        for n in 0..good.len() {
            let _ = read(&good[..n]);
        }
    }

    #[test]
    fn a_huge_adtl_list_is_read_in_linear_time_and_only_for_the_cue_points_that_exist() {
        // Records for ids that have no cue point are skipped, not stored, and the number looked at is capped.
        let junk = |n: u32| (1000..1000 + n).map(|id| labl(b"labl", id, "n")).collect::<Vec<_>>();
        let started = std::time::Instant::now();
        // the real label comes first: read
        let mut subs = vec![labl(b"labl", 1, "Real")];
        subs.extend(junk(100_000));
        let m = read(&wav(&[fmt(44100), cue(&[(1, 44100)]), adtl(&subs)]));
        assert_eq!(m[0].comment, "Real");
        // it comes after more records than are looked at: not read, and the marker is still there
        let mut subs = junk(MAX_ADTL_RECORDS as u32 + 10);
        subs.push(labl(b"labl", 1, "Too late"));
        let m = read(&wav(&[fmt(44100), cue(&[(1, 44100)]), adtl(&subs)]));
        assert_eq!((m.len(), m[0].comment.as_str()), (1, ""));
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
    }
}
