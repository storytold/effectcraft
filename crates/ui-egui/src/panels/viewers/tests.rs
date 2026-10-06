//! Passive panes exercise the same bounded queue using original tiny CPU pixels.

use std::sync::Mutex;

use super::*;
use crate::frames::{RemoteDone, RemoteFrame, RemoteFrames, RemoteJob};
use effectcraft_engine::project::{Comp, ItemKind, Project};
use effectcraft_engine::time::{FrameRate, Tick};

#[derive(Default)]
struct HeldFrames {
    pending: Mutex<Vec<(RemoteJob, RemoteDone)>>,
    started: std::sync::atomic::AtomicUsize,
}

impl RemoteFrames for HeldFrames {
    fn slots(&self) -> usize {
        4
    }

    fn start(&self, job: RemoteJob, done: RemoteDone) {
        self.started.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.pending.lock().unwrap().push((job, done));
    }
}

impl HeldFrames {
    fn complete(&self) -> RemoteJob {
        self.complete_with([80, 120, 160, 255])
    }

    fn complete_with(&self, pixel: [u8; 4]) -> RemoteJob {
        let (job, done) = self.pending.lock().unwrap().remove(0);
        done(Ok(RemoteFrame { width: 2, height: 2, rgba: pixel.repeat(4), ms: 0.0 }));
        job
    }
}

fn fixture() -> (EffectcraftApp, [ItemId; 2], Arc<HeldFrames>, egui::Context) {
    let mut project = Project::default();
    project.settings.gpu_acceleration = false;
    let ids = ["First", "Second"].map(|name| {
        let comp = Comp::new(2, 2, FrameRate::FPS_30, Tick::from_seconds_f64(10.0));
        project.add_item(name, Default::default(), None, ItemKind::Comp(comp.into()))
    });
    let mut session = effectcraft_engine::Session { project: Arc::new(project), ..Default::default() };
    session.open_comp(ids[0]);
    session.state.times.insert(ids[0], FrameRate::FPS_30.tick_of(3));
    session.state.times.insert(ids[1], FrameRate::FPS_30.tick_of(7));
    let mut app = EffectcraftApp::new(session);
    app.ui.viewers.insert(1, Some(ids[0].0));
    app.ui.viewers.insert(2, Some(ids[1].0));
    let remote = Arc::new(HeldFrames::default());
    app.frames.set_remote(Some(remote.clone()));
    let ctx = egui::Context::default();
    app.frames.set_context(&ctx);
    (app, ids, remote, ctx)
}

fn draw(app: &mut EffectcraftApp, ctx: &egui::Context, ids: &[u32]) {
    app.frames.begin_secondary_paint();
    ctx.run_ui(Default::default(), |ui| {
        for (i, id) in ids.iter().enumerate() {
            let rect = Rect::from_min_size(pos2(i as f32 * 240.0, 0.0), vec2(240.0, 200.0));
            show(app, ui, *id, rect);
        }
    })
    .drop_without_applying_deltas();
    app.frames.end_secondary_paint();
}

#[test]
fn passive_panes_are_bounded_and_installed_pixels_survive_raw_cache_eviction() {
    let (mut app, comps, remote, ctx) = fixture();
    app.frames.set_budget(16);
    draw(&mut app, &ctx, &[1, 2]);
    assert_eq!(app.frames.dispatch_remote(), 1, "four worker slots still permit only one passive render");
    assert_eq!(app.frames.inflight(), 2);
    let first = remote.complete();
    assert_eq!(first.comp, comps[0]);
    assert_eq!(first.t, FrameRate::FPS_30.tick_of(3));
    assert_eq!(format!("{:?}", first.opts), format!("{:?}", app.frame_opts(comps[0], 1.0)));
    draw(&mut app, &ctx, &[1, 2]);
    let (first_key, first_id) = app.passive_tex.get(&1).map(|(key, tex)| (*key, tex.id())).unwrap();
    assert_eq!(first_key, app.frame_key(comps[0], 3, 1.0));
    assert_eq!(app.frames.dispatch_remote(), 1);
    let second = remote.complete();
    assert_eq!(second.comp, comps[1]);
    assert_eq!(second.t, FrameRate::FPS_30.tick_of(7));
    assert!(!app.frames.is_cached(&first_key), "second pixels must really evict the first raw image");
    draw(&mut app, &ctx, &[1, 2]);
    assert_eq!(app.passive_tex.get(&1).unwrap().1.id(), first_id);
    assert_eq!(app.passive_tex.get(&2).unwrap().0, app.frame_key(comps[1], 7, 1.0));
    assert_eq!(app.frames.dispatch_remote(), 0, "installed pixels do not request raw-cache reconstruction");
    assert_eq!(remote.started.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert!(app.frames.viewer_timing().last.is_none());
}

#[test]
fn shared_passive_frame_survives_one_pane_closing() {
    let (mut app, comps, remote, ctx) = fixture();
    app.ui.viewers.insert(2, Some(comps[0].0));
    draw(&mut app, &ctx, &[1, 2]);
    assert_eq!(app.frames.inflight(), 1);
    on_close(&mut app, panel(1));
    draw(&mut app, &ctx, &[2]);
    assert_eq!(app.frames.dispatch_remote(), 1);
    remote.complete();
    draw(&mut app, &ctx, &[2]);
    assert!(!app.passive_tex.contains_key(&1));
    assert_eq!(app.passive_tex.get(&2).unwrap().0.comp, comps[0].0);
    assert_eq!(remote.started.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn memory_purge_replaces_a_passive_texture_even_when_the_key_stays_the_same() {
    let (mut app, _, remote, ctx) = fixture();
    draw(&mut app, &ctx, &[1]);
    assert_eq!(app.frames.dispatch_remote(), 1);
    remote.complete();
    draw(&mut app, &ctx, &[1]);
    let (key, old) = app.passive_tex.get(&1).map(|(key, tex)| (*key, tex.id())).unwrap();
    app.session.execute("edit.purge", json!({"what":"memory"})).unwrap();
    app.handle_events(&ctx);
    assert!(app.passive_tex.is_empty());
    assert!(!app.frames.is_cached(&key));
    draw(&mut app, &ctx, &[1]);
    assert_eq!(app.frames.dispatch_remote(), 1, "same-key demand must request pixels after an explicit purge");
    remote.complete_with([160, 120, 80, 255]);
    draw(&mut app, &ctx, &[1]);
    let (new_key, new) = app.passive_tex.get(&1).unwrap();
    assert_eq!(*new_key, key);
    assert_ne!(new.id(), old);
    let FrameImage::Cpu(fresh) = app.frames.get(&key).unwrap() else { panic!("CPU fixture") };
    assert!(fresh.pixels.iter().all(|p| p.to_array() == [160, 120, 80, 255]));
    assert_eq!(remote.started.load(std::sync::atomic::Ordering::Relaxed), 2);
}
