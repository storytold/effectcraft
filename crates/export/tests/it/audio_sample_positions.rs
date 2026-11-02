//! Sample positions through the public native WAV exporter, using a tiny recorded impulse.
use effectcraft_export::render_queue::{AudioFormat, OutputFormat, OutputModule, RenderSettings};
use effectcraft_export::{Job, JobOptions, Sink, export};
use effectcraft_project::{Comp, Footage, FootageKind, ItemId, ItemKind, LayerSource, Project, build};
use effectcraft_raster::Image;
use effectcraft_render::FootageSource;
use effectcraft_time::{FrameRate, Tick};
use std::sync::{Arc, Mutex};

struct Impulse {
    first: i64,
    last: i64,
    calls: Mutex<Vec<(Tick, i64, usize, u32)>>,
}
impl FootageSource for Impulse {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
    fn audio(&self, _: ItemId, _: &Footage, start: Tick, frames: usize, rate: u32) -> Option<Vec<f32>> {
        let index = start.to_units_floor(i64::from(rate));
        self.calls.lock().unwrap().push((start, index, frames, rate));
        let mut samples = Vec::with_capacity(frames * 2);
        for offset in 0..frames {
            let sample = index + offset as i64;
            let value = if sample == self.first {
                0.75
            } else if sample == self.last {
                0.25
            } else {
                0.0
            };
            samples.extend([value, -value]);
        }
        Some(samples)
    }
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
fn check(rate: u32) {
    let frame_rate = FrameRate::FPS_24;
    let span = (frame_rate.tick_of(1), frame_rate.tick_of(2));
    // Match the exporter's existing floor-to-sample span convention; this proof tests whether
    // its selected samples survive subsequent Tick conversions, without changing that convention.
    let first = span.0.to_units_floor(i64::from(rate));
    let end = span.1.to_units_floor(i64::from(rate));
    let expected_frames = usize::try_from(end - first).unwrap();
    let mut project = Project::default();
    let duration = Tick::from_seconds_f64(1.0);
    let item = project.add_item(
        "Impulse",
        Default::default(),
        None,
        ItemKind::Footage(Footage { path: "recorded-sample-impulse.wav".into(), kind: FootageKind::Audio, duration, has_audio: true, ..Default::default() }),
    );
    let mut comp = Comp::new(4, 4, frame_rate, duration);
    comp.work_area = span;
    comp.layers.push(build::layer(&mut project, &comp, "Impulse", LayerSource::Footage { item }, (4, 4), None));
    let comp = project.add_item("Comp", Default::default(), None, ItemKind::Comp(comp.into()));
    let settings = RenderSettings::default();
    let mut output = OutputModule::for_format(OutputFormat::Wav);
    output.audio_sample_rate = rate;
    output.audio_channels = 2;
    output.audio_format = AudioFormat::F32;
    let provider = Impulse { first, last: end - 1, calls: Mutex::default() };
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
        path: "impulse.wav",
        sink: Some(&*sink),
        nested_switches: true,
        options: JobOptions::default(),
    };
    let report = export(&job, &mut |_| true).unwrap();
    assert!(report.audio, "the production native audio exporter ran");
    let files = files.lock().unwrap();
    let bytes = &files.iter().find(|(name, _)| name.ends_with(".wav")).unwrap().1;
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let format = chunk(bytes, b"fmt ");
    assert_eq!(u16::from_le_bytes(format[..2].try_into().unwrap()), 3, "float PCM");
    assert_eq!(u32::from_le_bytes(format[4..8].try_into().unwrap()), rate);
    let pcm = chunk(bytes, b"data");
    assert_eq!(pcm.len(), expected_frames * 8, "the selected work-area sample count is unchanged");
    let samples: Vec<f32> = pcm.as_chunks::<4>().0.iter().map(|bytes| f32::from_le_bytes(*bytes)).collect();
    let impulses: Vec<usize> = samples.as_chunks::<2>().0.iter().enumerate().filter_map(|(i, pair)| (pair[0] != 0.0).then_some(i)).collect();
    let calls = provider.calls.lock().unwrap();
    eprintln!("rate={rate} first={first} end={end} frames={expected_frames} calls={calls:?} exported_impulses={impulses:?}");
    assert!(!calls.is_empty(), "native export actually requested provider samples");
    assert_eq!(
        samples[..2],
        [0.75, -0.75],
        "the first selected source sample must be the first exported sample at {rate} Hz; impulses={impulses:?}, calls={calls:?}"
    );
    assert_eq!(samples[samples.len() - 2..], [0.25, -0.25], "the final selected source sample must be preserved at {rate} Hz");
    assert_eq!(impulses, [0, expected_frames - 1], "no shifted or duplicated impulse");
}
#[test]
fn native_wav_work_area_preserves_44100_sample_positions() {
    check(44_100);
}
#[test]
fn native_wav_work_area_preserves_48000_sample_positions() {
    check(48_000);
}
#[test]
fn native_wav_work_area_preserves_44117_sample_positions() {
    check(44_117);
}
