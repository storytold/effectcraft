//! Markers a WAV file carries, read from its RIFF chunks.
//!
//! Two encodings are read and merged:
//! - **Cue points** (the standard): a `cue ` chunk with one point per marker, named by `labl` (or
//!   `note`) sub-chunks of a `LIST` `adtl` chunk, and given a length by `ltxt`.
//! - **XMP** (Adobe's tools): an `_PMX` chunk holding an XMP packet whose `xmpDM:markers` list has
//!   one entry per marker (`xmpDM:startTime`, `xmpDM:duration`, `xmpDM:name`), counted in frames
//!   of `xmpDM:frameRate` (`f44100` for samples at 44.1 kHz).
//!
//! The file is untrusted input: every read is bounds-checked, sizes use checked arithmetic, and
//! the number of markers and the size of the XMP packet are capped.

use effectcraft_color::Label;
use effectcraft_project::Marker;
use effectcraft_time::Tick;

/// Most markers read from one file.
pub const MAX_MARKERS: usize = 10_000;
/// Largest XMP packet read (bytes).
const MAX_XMP: usize = 4 << 20;

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(s.try_into().ok()?))
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

/// The chunks of a WAVE file as `(id, body)`, stopping at the first one that doesn't fit.
fn chunks(bytes: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 12usize;
    while let Some(head) = bytes.get(at..at.saturating_add(8)) {
        let (Ok(id), Some(size)) = (<[u8; 4]>::try_from(&head[..4]), u32_at(head, 4)) else { break };
        let start = at.saturating_add(8);
        let Some(end) = start.checked_add(size as usize) else { break };
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
        for m in xmp_markers(xmp, rate) {
            // Tools that write both encodings repeat each marker.
            if !out.iter().any(|o| o.comment == m.comment && o.time == m.time) {
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
    // Cue id -> (name, length in samples).
    let mut names: Vec<(u32, String)> = Vec::new();
    let mut lengths: Vec<(u32, u32)> = Vec::new();
    for (_, list) in chunks.iter().filter(|(id, b)| id == b"LIST" && b.get(0..4) == Some(b"adtl")) {
        let mut at = 4usize;
        while let Some(head) = list.get(at..at.saturating_add(8)) {
            let (Ok(id), Some(size)) = (<[u8; 4]>::try_from(&head[..4]), u32_at(head, 4)) else { break };
            let start = at.saturating_add(8);
            let Some(end) = start.checked_add(size as usize) else { break };
            let Some(body) = list.get(start..end) else { break };
            if let Some(cue_id) = u32_at(body, 0) {
                match &id {
                    b"labl" | b"note" => {
                        let text = label(body.get(4..).unwrap_or_default());
                        // A `labl` is the name; a `note` only when there is none.
                        let known = names.iter().any(|(c, _)| *c == cue_id);
                        if &id == b"labl" || !known {
                            names.retain(|(c, _)| *c != cue_id);
                            names.push((cue_id, text));
                        }
                    }
                    b"ltxt" => lengths.push((cue_id, u32_at(body, 4).unwrap_or(0))),
                    _ => {}
                }
            }
            at = end.saturating_add(end & 1);
        }
    }
    let mut out = Vec::new();
    for i in 0..count {
        let Some(point) = cue.get(4usize.saturating_add(i.saturating_mul(24))..) else { break };
        let (Some(id), Some(offset)) = (u32_at(point, 0), u32_at(point, 20)) else { break };
        let comment = names.iter().find(|(c, _)| *c == id).map(|(_, t)| t.clone()).unwrap_or_default();
        let length = lengths.iter().find(|(c, _)| *c == id).map_or(0, |(_, l)| *l);
        out.push(Marker { time: secs(f64::from(offset), rate), duration: secs(f64::from(length), rate), comment, label: Label::None, ..Default::default() });
    }
    out
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
}
