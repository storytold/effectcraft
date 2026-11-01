//! Tiny native WAV to ProRes PCM MOV sample-position proof.
use effectcraft_export::render_queue::{AudioFormat, OutputFormat, OutputModule, RenderSettings};
use effectcraft_export::{Job, JobOptions, Sink, export};
use effectcraft_media::{MediaPool, probe_bytes};
use effectcraft_project::{Comp, Footage, ItemId, ItemKind, LayerSource, Project, build};
use effectcraft_raster::Image;
use effectcraft_render::FootageSource;
use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use std::sync::{Arc, Mutex};

struct RecordingNative {
    pool: MediaPool,
    calls: Mutex<Vec<(Tick, i64, usize, u32)>>,
}
impl FootageSource for RecordingNative {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
    fn audio(&self, _: ItemId, footage: &Footage, start: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
        self.calls.lock().unwrap().push((start, start.to_units_floor(i64::from(rate)), frames, rate));
        Some(self.pool.audio_samples(footage, start, frames, rate))
    }
}
fn native_wav(native_rate: u32, first: i64, last: i64, impulses: bool) -> Arc<[u8]> {
    let mut pcm = Vec::with_capacity(native_rate as usize * 8);
    for index in 0..i64::from(native_rate) {
        let pair = if impulses {
            let value: f32 = if index == first {
                0.75
            } else if index == last {
                0.25
            } else {
                0.0
            };
            [value, -value]
        } else {
            [(index % 97) as f32 / 128.0, -((index % 53) as f32) / 128.0]
        };
        pcm.extend(pair.into_iter().flat_map(f32::to_le_bytes));
    }
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + pcm.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(3u16.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(native_rate.to_le_bytes());
    wav.extend((native_rate * 8).to_le_bytes());
    wav.extend(8u16.to_le_bytes());
    wav.extend(32u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((pcm.len() as u32).to_le_bytes());
    wav.extend(pcm);
    wav.into()
}
fn boxes(bytes: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let name = bytes[offset + 4..offset + 8].try_into().unwrap();
        let (size, header) = if size == 1 {
            (usize::try_from(u64::from_be_bytes(bytes[offset + 8..offset + 16].try_into().unwrap())).unwrap(), 16)
        } else if size == 0 {
            (bytes.len() - offset, 8)
        } else {
            (size as usize, 8)
        };
        assert!(size >= header && offset + size <= bytes.len(), "valid returned MOV boxes");
        out.push((name, &bytes[offset + header..offset + size]));
        offset += size;
    }
    assert_eq!(offset, bytes.len());
    out
}
fn child<'a>(bytes: &'a [u8], name: &[u8; 4]) -> &'a [u8] {
    boxes(bytes).into_iter().find(|(kind, _)| kind == name).unwrap().1
}
fn movie_pcm_count(bytes: &[u8], rate: u32, frames: usize) {
    let moov = child(bytes, b"moov");
    let mdia = boxes(moov)
        .into_iter()
        .filter(|(kind, _)| kind == b"trak")
        .map(|(_, track)| child(track, b"mdia"))
        .find(|media| &child(media, b"hdlr")[8..12] == b"soun")
        .expect("the returned MOV has a sound track");
    let header = child(mdia, b"mdhd");
    let (timescale, duration) = if header[0] == 1 {
        (u32::from_be_bytes(header[20..24].try_into().unwrap()), u64::from_be_bytes(header[24..32].try_into().unwrap()))
    } else {
        (u32::from_be_bytes(header[12..16].try_into().unwrap()), u64::from(u32::from_be_bytes(header[16..20].try_into().unwrap())))
    };
    assert_eq!(timescale, rate, "PCM MOV audio timescale");
    assert_eq!(duration, frames as u64, "PCM MOV selected sample count");
    let table = child(child(mdia, b"minf"), b"stbl");
    let timing = child(table, b"stts");
    let count = u32::from_be_bytes(timing[4..8].try_into().unwrap()) as usize;
    let duration: u64 = timing[8..]
        .as_chunks::<8>()
        .0
        .iter()
        .take(count)
        .map(|entry| u64::from(u32::from_be_bytes(entry[..4].try_into().unwrap())) * u64::from(u32::from_be_bytes(entry[4..].try_into().unwrap())))
        .sum();
    assert_eq!(duration, frames as u64, "all encoded PCM samples are counted");
    let sizes = child(table, b"stsz");
    let fixed = u32::from_be_bytes(sizes[4..8].try_into().unwrap()) as u64;
    let count = u32::from_be_bytes(sizes[8..12].try_into().unwrap()) as usize;
    let bytes = if fixed != 0 {
        fixed * count as u64
    } else {
        sizes[12..].as_chunks::<4>().0.iter().take(count).map(|entry| u64::from(u32::from_be_bytes(*entry))).sum()
    };
    assert_eq!(bytes, (frames * 8) as u64, "native stereo float PCM bytes");
}
fn first_sample_tick(index: i64, rate: u32) -> Tick {
    let numerator = i128::from(index) * i128::from(TICKS_PER_SECOND);
    let denominator = i128::from(rate);
    Tick((numerator.div_euclid(denominator) + i128::from(numerator.rem_euclid(denominator) != 0)) as i64)
}
fn check(native_rate: u32, output_rate: u32) {
    let frame_rate = FrameRate::FPS_24;
    let span = (frame_rate.tick_of(1), frame_rate.tick_of(2));
    // Keep the production exporter's floor-to-sample span selection. The oracle below requests
    // that same first selected integer sample directly from the actual native decoder.
    let first = span.0.to_units_floor(i64::from(output_rate));
    let end = span.1.to_units_floor(i64::from(output_rate));
    let frames = usize::try_from(end - first).unwrap();
    let impulses = native_rate == output_rate;
    let bytes = native_wav(native_rate, first, end - 1, impulses);
    let mut mismatches = Vec::new();
    for loops in [1, 2] {
        let path = format!("indexed-native-{native_rate}-{output_rate}-{loops}.wav");
        let mut footage = probe_bytes(&path, bytes.clone()).unwrap();
        assert!(footage.has_audio && !footage.missing);
        footage.loop_count = loops;
        let pool = MediaPool::new();
        pool.add_bytes(&footage.path, bytes.clone());
        let oracle_tick = first_sample_tick(first, output_rate);
        assert_eq!(oracle_tick.to_units_floor(i64::from(output_rate)), first);
        let expected = pool.audio_samples(&footage, oracle_tick, frames, output_rate);
        assert_eq!(expected.len(), frames * 2);
        assert!(expected.iter().any(|sample| *sample != 0.0), "the real native decoding/resampling control must not be silent");
        if impulses {
            assert_eq!(expected[..2], [0.75, -0.75], "the actual native provider decodes the first indexed source impulse");
            assert_eq!(expected[expected.len() - 2..], [0.25, -0.25], "the actual native provider decodes the last indexed source impulse");
        }
        let provider = RecordingNative { pool, calls: Mutex::default() };
        let mut project = Project::default();
        let duration = Tick::from_seconds_f64(1.0);
        let item = project.add_item("Native WAV", Default::default(), None, ItemKind::Footage(footage));
        let mut comp = Comp::new(16, 16, frame_rate, duration);
        comp.work_area = span;
        comp.layers.push(build::layer(&mut project, &comp, "Native WAV", LayerSource::Footage { item }, (16, 16), None));
        let comp = project.add_item("Comp", Default::default(), None, ItemKind::Comp(comp.into()));
        let settings = RenderSettings::default();
        let mut output = OutputModule::for_format(OutputFormat::ProRes);
        output.audio_sample_rate = output_rate;
        output.audio_channels = 2;
        output.audio_format = AudioFormat::F32;
        let files: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Default::default();
        let received = files.clone();
        let sink: Box<Sink> = Box::new(move |name: &str, bytes: Vec<u8>| received.lock().unwrap().push((name.into(), bytes)));
        let job = Job {
            project: &project,
            footage: &provider,
            expr: None,
            accel: None,
            comp,
            settings: &settings,
            output: &output,
            path: "native-impulse.mov",
            sink: Some(&*sink),
            nested_switches: true,
            options: JobOptions::default(),
        };
        let report = export(&job, &mut |_| true).unwrap();
        assert!(report.audio);
        let files = files.lock().unwrap();
        let movie = &files.iter().find(|(name, _)| name.ends_with(".mov")).unwrap().1;
        assert_eq!(report.frames, 1, "the one-frame ProRes movie was actually encoded");
        movie_pcm_count(movie, output_rate, frames);
        let decoded = probe_bytes("returned-native-impulse.mov", Arc::from(movie.as_slice())).unwrap();
        assert!(decoded.has_audio && decoded.has_video, "the native movie decoder recognized both tracks");
        let decoder = MediaPool::new();
        decoder.add_bytes(&decoded.path, Arc::from(movie.as_slice()));
        let decoded_pcm = decoder.audio_samples(&decoded, Tick::ZERO, frames + 8, output_rate);
        assert_eq!(decoded_pcm.len(), (frames + 8) * 2);
        assert!(decoded_pcm[frames * 2..].iter().all(|value| *value == 0.0), "native PCM ends at the selected sample count");
        let actual = decoded_pcm[..frames * 2].to_vec();
        let calls = provider.calls.lock().unwrap();
        assert!(!calls.is_empty(), "production export actually called the real native provider");
        eprintln!(
            "native_rate={native_rate} output_rate={output_rate} loops={loops} first={first} end={end} count={frames} provider_calls={calls:?} first_actual={:?} first_expected={:?} last_actual={:?} last_expected={:?}",
            &actual[..2],
            &expected[..2],
            &actual[actual.len() - 2..],
            &expected[expected.len() - 2..]
        );
        if actual != expected {
            let first_mismatch = actual.iter().zip(&expected).position(|(a, b)| a != b);
            mismatches.push((loops, first_mismatch, actual[..2].to_vec(), expected[..2].to_vec()));
        }
    }
    assert!(mismatches.is_empty(), "production native MOV PCM export must preserve the native provider's selected sample positions: {mismatches:?}");
}
#[test]
fn native_media_mov_pcm_work_area_preserves_44100_positions() {
    check(44_100, 44_100);
}
#[test]
fn native_media_mov_pcm_work_area_preserves_48000_positions() {
    check(48_000, 48_000);
}
#[test]
fn native_media_mov_pcm_work_area_preserves_44117_positions() {
    check(44_117, 44_117);
}
