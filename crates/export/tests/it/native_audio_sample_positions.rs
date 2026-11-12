//! Native MediaPool decoding and native WAV export preserve selected sample positions.
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
fn chunk<'a>(bytes: &'a [u8], name: &[u8; 4]) -> &'a [u8] {
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let length = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        if &bytes[offset..offset + 4] == name {
            return &bytes[offset + 8..offset + 8 + length];
        }
        offset += 8 + length + length % 2;
    }
    panic!("native WAV chunk missing");
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
        let mut comp = Comp::new(4, 4, frame_rate, duration);
        comp.work_area = span;
        comp.layers.push(build::layer(&mut project, &comp, "Native WAV", LayerSource::Footage { item }, (4, 4), None));
        let comp = project.add_item("Comp", Default::default(), None, ItemKind::Comp(comp.into()));
        let settings = RenderSettings::default();
        let mut output = OutputModule::for_format(OutputFormat::Wav);
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
            path: "native-impulse.wav",
            sink: Some(&*sink),
            nested_switches: true,
            options: JobOptions::default(),
        };
        let report = export(&job, &mut |_| true).unwrap();
        assert!(report.audio);
        let files = files.lock().unwrap();
        let wav = &files.iter().find(|(name, _)| name.ends_with(".wav")).unwrap().1;
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        let format = chunk(wav, b"fmt ");
        assert_eq!(u16::from_le_bytes(format[..2].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(format[4..8].try_into().unwrap()), output_rate);
        let pcm = chunk(wav, b"data");
        assert_eq!(pcm.len(), frames * 8, "native work-area sample count");
        let actual: Vec<f32> = pcm.as_chunks::<4>().0.iter().map(|bytes| f32::from_le_bytes(*bytes)).collect();
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
    assert!(mismatches.is_empty(), "production native WAV export must preserve the native provider's selected sample positions: {mismatches:?}");
}
#[test]
fn native_media_wav_work_area_preserves_44100_positions() {
    check(44_100, 44_100);
}
#[test]
fn native_media_wav_work_area_preserves_48000_positions() {
    check(48_000, 48_000);
}
#[test]
fn native_media_wav_work_area_preserves_44117_positions() {
    check(44_117, 44_117);
}
