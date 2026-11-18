//! WAV cue/adtl/RF64/XMP metadata is parsed by craft-audio; timeline policy remains here.
use craft_audio_markers::wav_markers as shared;
use effectcraft_color::Label;
use effectcraft_project::Marker;
use effectcraft_time::Tick;
use std::collections::HashSet;

pub use shared::MAX_MARKERS;
#[cfg(test)]
const MAX_ADTL_RECORDS: usize = 4 * MAX_MARKERS;

fn secs(frames: f64, rate: f64) -> Tick {
    if rate > 0.0 && frames.is_finite() { Tick::from_seconds_f64((frames / rate).max(0.0)) } else { Tick::ZERO }
}

/// File markers in timeline order, deduplicated after the app's existing Tick quantization.
pub fn read(bytes: &[u8]) -> Vec<Marker> {
    let metadata = shared::read(bytes);
    let map = |m: shared::Marker| Marker {
        time: secs(m.start, m.rate),
        duration: secs(m.duration, m.rate),
        comment: m.name,
        label: Label::None,
        ..Default::default()
    };
    let mut out: Vec<_> = metadata.cue.into_iter().map(map).collect();
    let mut seen: HashSet<(String, Tick)> = out.iter().map(|m| (m.comment.clone(), m.time)).collect();
    for marker in metadata.xmp.into_iter().map(map) {
        if seen.insert((marker.comment.clone(), marker.time)) {
            out.push(marker);
        }
    }
    out.truncate(MAX_MARKERS);
    out.sort_by_key(|marker| marker.time);
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
    fn shared_sources_deduplicate_after_app_tick_quantization() {
        let xmp = br#"<rdf:li xmpDM:startTime="1e-100" xmpDM:name="Same"/>"#;
        let file = wav(&[fmt(44100), cue(&[(1, 0)]), adtl(&[labl(b"labl", 1, "Same")]), chunk(b"_PMX", xmp)]);
        let markers = read(&file);
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].time, Tick::ZERO);
        assert_eq!(markers[0].comment, "Same");
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
