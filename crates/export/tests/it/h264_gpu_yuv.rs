//! H.264 export with an accelerator that converts frames to 4:2:0 itself
//! (`Accelerator::comp_frame_yuv420`): the file is the same as the CPU conversion's, and frames
//! the accelerator can't take (odd sizes, crop) fall back to the RGBA path.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use effectcraft_color::Label;
use effectcraft_export::render_queue::{OutputFormat, OutputModule, RenderSettings, TimeSpan};
use effectcraft_export::{Job, export};
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_raster::Image;
use effectcraft_render::{Accelerator, NoFootage, Renderer};
use effectcraft_time::{FrameRate, Tick};

/// Renders on the CPU; converts to 4:2:0 with the export's own function when `yuv` is set.
/// `deferred`: claims deferred readbacks (a browser worker's GPU), whose frames render in passes.
/// `stop_after`: the device stops for good after that many planes (`available` turns false).
struct Fake {
    yuv: bool,
    yuv_frames: AtomicUsize,
    deferred: bool,
    stop_after: usize,
    /// Frames asking for planes right now, and the most at once after the stop.
    asking: AtomicUsize,
    most_after_stop: AtomicUsize,
}

impl Fake {
    fn new(yuv: bool, deferred: bool, stop_after: usize) -> Self {
        Fake { yuv, yuv_frames: AtomicUsize::new(0), deferred, stop_after, asking: AtomicUsize::new(0), most_after_stop: AtomicUsize::new(0) }
    }
}

impl Accelerator for Fake {
    fn name(&self) -> String {
        "fake".into()
    }
    fn comp_frame(&self, r: &Renderer, comp: ItemId, t: Tick) -> Option<Image> {
        Some(r.comp_frame_cpu(comp, t))
    }
    fn comp_frame_yuv420(&self, r: &Renderer, comp: ItemId, t: Tick, bg: [f32; 3]) -> Option<Vec<u8>> {
        if !self.yuv {
            return None;
        }
        if self.yuv_frames.fetch_add(1, Ordering::SeqCst) >= self.stop_after {
            self.yuv_frames.fetch_sub(1, Ordering::SeqCst);
            // Stopped: every frame still asks first, so asks that overlap are frames rendering at once.
            let now = self.asking.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_after_stop.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(30));
            self.asking.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(effectcraft_raster::yuv::yuv420_over(&r.comp_frame_cpu(comp, t), bg))
    }
    fn available(&self) -> bool {
        self.yuv_frames.load(Ordering::SeqCst) < self.stop_after
    }
    fn miss_gate(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        self.deferred.then(Default::default)
    }
    fn supports_effect(&self, _id: &str) -> bool {
        false
    }
    fn effects(&self, _chain: &[effectcraft_render::FxStep], _buf: &effectcraft_effects::Buf, _levels: Option<f32>) -> Option<effectcraft_effects::Buf> {
        None
    }
}

fn out_dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn project() -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.gpu_acceleration = true;
    let mut comp = Comp::new(64, 48, FrameRate::new(10, 1), Tick::from_seconds_f64(0.6));
    comp.background = [0.1, 0.1, 0.4];
    let sid = p.add_item("Red", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.2, 0.0], width: 30, height: 20, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "Red", LayerSource::Solid { item: sid }, (30, 20), None);
    l.in_point = comp.frame_rate.tick_of(2);
    comp.layers.push(l);
    let cid = p.add_item("Comp 1", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

fn run(p: &Project, cid: ItemId, accel: Option<&dyn Accelerator>, s: &RenderSettings, path: &Path) -> Vec<u8> {
    let om = OutputModule { bitrate_kbps: 4000, ..OutputModule::for_format(OutputFormat::H264) };
    let path = path.to_string_lossy().to_string();
    let job = Job {
        project: p,
        footage: &NoFootage,
        expr: None,
        accel,
        comp: cid,
        settings: s,
        output: &om,
        path: &path,
        sink: None,
        options: Default::default(),
        nested_switches: true,
    };
    export(&job, &mut |_| true).expect("export");
    std::fs::read(&path).expect("read")
}

#[test]
fn accelerator_planes_encode_like_the_cpu_conversion() {
    let d = out_dir("h264-gpu-yuv");
    let (mut p, cid) = project();
    // Across more than one 16-frame batch so the second batch renders beside the encoder.
    p.comp_mut(cid).unwrap().duration = Tick::from_seconds_f64(2.0);
    let s = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let yuv = Fake::new(true, false, usize::MAX);
    let rgba = Fake::new(false, false, usize::MAX);
    let yuv_mp4 = d.join("yuv.mp4");
    let a = run(&p, cid, Some(&yuv), &s, &yuv_mp4);
    let b = run(&p, cid, Some(&rgba), &s, &d.join("rgba.mp4"));
    let c = run(&p, cid, None, &s, &d.join("cpu.mp4"));
    assert_eq!(yuv.yuv_frames.load(Ordering::SeqCst), 20, "every frame came from the accelerator");
    assert!(a == b && b == c, "the three files differ");
    let f = effectcraft_media::probe(&yuv_mp4).expect("probe yuv.mp4");
    assert_eq!((f.width, f.height), (64, 48));
    assert_eq!(f.frame_rate.frame_at(f.duration - Tick(1)) + 1, 20);
    let pool = effectcraft_media::MediaPool::new();
    for k in 0..20 {
        let img = pool.frame_at(&f, f.frame_rate.tick_of(k)).expect("decode frame");
        assert_eq!((img.width, img.height), (64, 48));
    }
}

#[test]
fn frames_the_accelerator_cannot_take_fall_back_to_rgba() {
    let d = out_dir("h264-gpu-yuv-fallback");
    let (p, cid) = project();
    let base = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    // Renders at 62×47, encodes at 62×46 (the codec size): planes of the render would not fit.
    let odd = RenderSettings { resolution: 31.0 / 32.0, ..base.clone() };
    let yuv = Fake::new(true, false, usize::MAX);
    let a = run(&p, cid, Some(&yuv), &odd, &d.join("yuv.mp4"));
    let c = run(&p, cid, None, &odd, &d.join("cpu.mp4"));
    assert_eq!(yuv.yuv_frames.load(Ordering::SeqCst), 0);
    assert!(a == c);
}

/// With deferred readbacks the export keeps rendering frames in passes (`Cx::frames`), never
/// through the blocking per-frame path the planes use.
#[test]
fn deferred_accelerators_keep_the_pass_based_path() {
    let d = out_dir("h264-gpu-yuv-deferred");
    let (p, cid) = project();
    let s = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let deferred = Fake::new(true, true, usize::MAX);
    let a = run(&p, cid, Some(&deferred), &s, &d.join("deferred.mp4"));
    let c = run(&p, cid, None, &s, &d.join("cpu.mp4"));
    assert_eq!(deferred.yuv_frames.load(Ordering::SeqCst), 0, "no planes asked for");
    assert!(a == c);
}

/// An accelerator that stops for good mid-export (`available` false): the frames left render
/// on the CPU side by side, not one at a time under the GPU's frames-in-flight cap (as
/// `Cx::frames` does), and the file is still the CPU's.
#[test]
fn frames_after_the_accelerator_stops_render_on_the_cpu() {
    let d = out_dir("h264-gpu-yuv-stops");
    let (p, cid) = project();
    let s = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let stops = Fake::new(true, false, 1);
    // Enough threads for frames to overlap wherever the machine runs it.
    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    let a = pool.install(|| run(&p, cid, Some(&stops), &s, &d.join("stops.mp4")));
    let c = run(&p, cid, None, &s, &d.join("cpu.mp4"));
    assert_eq!(stops.yuv_frames.load(Ordering::SeqCst), 1, "planes only until the stop");
    assert!(stops.most_after_stop.load(Ordering::SeqCst) > 1, "frames after the stop rendered one at a time");
    assert!(a == c);
}
