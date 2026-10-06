//! CPU-only responsiveness checks with original tiny pixels and an explicitly held render.

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use super::*;
use effectcraft_engine::project::{Footage, FootageKind, ItemId, ItemKind, LayerSource, Project, build};
use effectcraft_engine::render::{FootageSource, Image, LayerCache};
use effectcraft_engine::time::FrameRate;

struct HeldPixels {
    first: std::sync::atomic::AtomicBool,
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl FootageSource for HeldPixels {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        if !self.first.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.entered.send(()).ok()?;
            self.release.lock().ok()?.recv_timeout(Duration::from_secs(15)).ok()?;
        }
        Some(Arc::new(Image::filled(2, 2, [1.0, 0.0, 0.0, 1.0])))
    }
}

fn tiny_app(footage: Arc<dyn FootageSource>) -> (EffectcraftApp, ItemId) {
    let mut project = Project::default();
    project.settings.gpu_acceleration = false;
    let mut comp = Comp::new(2, 2, FrameRate::FPS_30, Tick::from_seconds_f64(10.0));
    let item = project.add_item(
        "Original pixels",
        Default::default(),
        None,
        ItemKind::Footage(Footage { width: 2, height: 2, kind: FootageKind::Video, has_video: true, duration: comp.duration, ..Default::default() }),
    );
    let layer = build::layer(&mut project, &comp, "Pixels", LayerSource::Footage { item }, (2, 2), None);
    comp.layers.push(layer);
    let cid = project.add_item("Tiny", Default::default(), None, ItemKind::Comp(comp.into()));
    let mut session = effectcraft_engine::Session { project: Arc::new(project), footage, layer_cache: Arc::new(LayerCache::new(0)), ..Default::default() };
    session.open_comp(cid);
    (EffectcraftApp::new(session), cid)
}

fn draw_secondary(app: &mut EffectcraftApp, ctx: &egui::Context, cid: ItemId, locked: bool) {
    let comp = app.session.project.comp_arc(cid).unwrap();
    app.frames.begin_secondary_paint();
    ctx.run_ui(Default::default(), |ui| {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
        if locked {
            locked_pane(app, ui, rect, Color32::BLACK);
        } else {
            aux_views(app, ui, &comp, cid, rect, Color32::BLACK);
        }
    })
    .drop_without_applying_deltas();
    app.frames.end_secondary_paint();
}

fn returns_while_pixels_are_held(locked: bool) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let footage = Arc::new(HeldPixels { first: Default::default(), entered: entered_tx, release: Mutex::new(release_rx) });
    let (mut app, cid) = tiny_app(footage);
    app.session.state.view_layout = 2;
    if locked {
        app.session.execute("view.splitLockedViewer", json!({})).unwrap();
    }
    let ctx = egui::Context::default();
    app.frames.set_context(&ctx);
    let (returned_tx, returned_rx) = mpsc::channel();
    let gate = std::thread::spawn(move || {
        let entered = entered_rx.recv_timeout(Duration::from_secs(5)).is_ok();
        let returned = entered && returned_rx.recv_timeout(Duration::from_secs(5)).is_ok();
        // Always release before returning a failure, so the old synchronous path cannot hang.
        let _ = release_tx.send(());
        (entered, returned)
    });
    draw_secondary(&mut app, &ctx, cid, locked);
    let _ = returned_tx.send(());
    let (entered, returned) = gate.join().unwrap();
    assert!(entered, "the test must actually reach the held footage");
    assert!(returned, "drawing the secondary pane waited for rendering to finish");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while app.frames.inflight() > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(app.frames.inflight(), 0);
    draw_secondary(&mut app, &ctx, cid, locked);
    let slot = if locked { crate::frames::SecondarySlot::LockedSplit } else { crate::frames::SecondarySlot::Top };
    let installed: Option<SecondaryTexture> = ctx.data(|d| d.get_temp(secondary_id(slot)));
    let installed = installed.expect("completed pixels must be installed on the next UI redraw");
    assert_eq!(installed.key.comp, cid.0);
    assert_eq!(installed.texture.size(), [2, 2]);
}

#[test]
fn auxiliary_pane_returns_before_a_held_render_finishes() {
    returns_while_pixels_are_held(false);
}

#[test]
fn locked_pane_returns_before_a_held_render_finishes() {
    returns_while_pixels_are_held(true);
}

#[derive(Default)]
struct CapturedFrames(Mutex<Vec<crate::frames::RemoteJob>>);

impl crate::frames::RemoteFrames for CapturedFrames {
    fn slots(&self) -> usize {
        4
    }
    fn start(&self, job: crate::frames::RemoteJob, done: crate::frames::RemoteDone) {
        self.0.lock().unwrap().push(job);
        done(Ok(crate::frames::RemoteFrame { width: 2, height: 2, rgba: [80, 120, 160, 255].repeat(4), ms: 0.0 }));
    }
}

fn immediate_app() -> (EffectcraftApp, ItemId, Arc<CapturedFrames>, egui::Context) {
    let (mut app, cid) = tiny_app(Arc::new(effectcraft_engine::render::NoFootage));
    let remote = Arc::new(CapturedFrames::default());
    app.frames.set_remote(Some(remote.clone()));
    let ctx = egui::Context::default();
    app.frames.set_context(&ctx);
    (app, cid, remote, ctx)
}

#[test]
fn secondary_views_preserve_cpu_camera_draft_and_nested_preferences() {
    let (mut app, cid, remote, ctx) = immediate_app();
    app.session.state.view_layout = 4;
    app.session.prefs.general.switches_affect_nested_comps = false;
    app.session.prefs.three_d.realtime_shadows = false;
    // A real 3D layer makes the three auxiliary camera views distinct.
    Arc::make_mut(&mut app.session.project).comp_mut(cid).unwrap().layers[0].switches.three_d = true;
    draw_secondary(&mut app, &ctx, cid, false);
    assert_eq!(app.frames.dispatch_remote(), 3);
    let jobs = remote.0.lock().unwrap();
    assert_eq!(jobs.len(), 3);
    for job in jobs.iter() {
        assert!(job.opts.draft);
        assert!(job.opts.view.is_some());
        assert!(matches!(job.opts.backend, effectcraft_engine::render::Backend::Cpu));
        assert!(!job.opts.nested_switches && !job.opts.draft_shadows);
        assert_eq!(job.t, app.session.time());
    }
    assert_ne!(format!("{:?}", jobs[0].opts.view), format!("{:?}", jobs[1].opts.view));
    drop(jobs);
    draw_secondary(&mut app, &ctx, cid, false);
    for slot in [crate::frames::SecondarySlot::Top, crate::frames::SecondarySlot::Front, crate::frames::SecondarySlot::Right] {
        assert!(ctx.data(|d| d.get_temp::<SecondaryTexture>(secondary_id(slot))).is_some());
    }
    assert!(app.frames.viewer_timing().last.is_none(), "secondary renders must not acknowledge primary-viewer timing");
}

#[test]
fn secondary_cache_reuses_unrelated_edits_but_keeps_exact_subframe_time() {
    let (mut app, cid, remote, ctx) = immediate_app();
    let slot = crate::frames::SecondarySlot::Top;
    let opts = effectcraft_engine::render::RenderOpts::default();
    assert!(secondary_texture(&app, &ctx, slot, cid, Tick(1), opts, false).is_none());
    assert_eq!(app.frames.dispatch_remote(), 1);
    let first = secondary_texture(&app, &ctx, slot, cid, Tick(1), opts, false).unwrap();
    app.session.execute("comp.new", json!({"name":"Unrelated", "width":2, "height":2, "duration":1})).unwrap();
    let reused = secondary_texture(&app, &ctx, slot, cid, Tick(1), opts, false).unwrap();
    assert_eq!(reused.id(), first.id());
    assert_eq!(app.frames.dispatch_remote(), 0);
    secondary_texture(&app, &ctx, slot, cid, Tick(2), opts, false);
    assert_eq!(app.frames.dispatch_remote(), 1, "different ticks within a frame need separate pixels");
    let next = secondary_texture(&app, &ctx, slot, cid, Tick(2), opts, false).unwrap();
    assert_ne!(next.id(), first.id());
    assert_eq!(remote.0.lock().unwrap().len(), 2);
}

#[test]
fn locked_display_profile_changes_rebuild_texture_without_mutating_shared_pixels() {
    let (mut app, cid, remote, ctx) = immediate_app();
    Arc::make_mut(&mut app.session.project).settings.working_space = Some(effectcraft_engine::project::ColorSpace::Srgb);
    app.session.state.viewer.display_color_management = true;
    app.session.prefs.previews.display_profile = "Display P3".into();
    assert!(effectcraft_engine::viewer::DisplayColor::of(&app.session).is_some());
    let slot = crate::frames::SecondarySlot::LockedSplit;
    let opts = effectcraft_engine::render::RenderOpts::default();
    secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, true);
    app.frames.dispatch_remote();
    let first = secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, true).unwrap();
    let stored: SecondaryTexture = ctx.data(|d| d.get_temp(secondary_id(slot))).unwrap();
    let crate::frames::FrameImage::Cpu(raw) = app.frames.get(&stored.key).unwrap() else { panic!("CPU fixture") };
    let original = raw.pixels.clone();
    app.session.prefs.previews.display_profile = "Rec. 2020".into();
    app.session.prefs_revision += 1;
    assert!(effectcraft_engine::viewer::DisplayColor::of(&app.session).is_some());
    let second = secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, true).unwrap();
    assert_ne!(first.id(), second.id());
    assert_eq!(raw.pixels, original);
    assert_eq!(app.frames.dispatch_remote(), 0);
    assert_eq!(remote.0.lock().unwrap().len(), 1);
}

#[test]
fn zoom_quality_changes_refresh_installed_textures_without_purging_raw_pixels() {
    let (mut app, cid, remote, ctx) = immediate_app();
    app.apply_prefs(&ctx);
    let slot = crate::frames::SecondarySlot::Top;
    let opts = effectcraft_engine::render::RenderOpts::default();
    secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, false);
    assert_eq!(app.frames.dispatch_remote(), 1);
    let old = secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, false).unwrap();
    let installed: SecondaryTexture = ctx.data(|d| d.get_temp(secondary_id(slot))).unwrap();
    let key = installed.key;
    let raw = app.frames.get(&key).unwrap();
    show_frame(&mut app, &ctx, key, raw.clone());
    app.passive_tex.insert(1, (key, crate::panels::viewers::PassiveTexture::Cpu(old.clone())));
    assert_eq!(ctx.tex_manager().read().meta(old.id()).unwrap().options, egui::TextureOptions::LINEAR);
    app.session.prefs.previews.zoom_quality = "faster".into();
    app.session.prefs_revision += 1;
    app.apply_prefs(&ctx);
    assert!(app.viewer_tex.is_none() && app.passive_tex.is_empty(), "quality changes must refresh installed samplers");
    assert!(ctx.data(|d| d.get_temp::<SecondaryTexture>(secondary_id(slot))).is_none());
    assert!(app.frames.is_cached(&key), "a sampling preference does not invalidate rendered pixels");
    let fresh = secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, false).unwrap();
    assert_eq!(ctx.tex_manager().read().meta(fresh.id()).unwrap().options, egui::TextureOptions::NEAREST);
    show_frame(&mut app, &ctx, key, raw);
    let id = app.viewer_tex.as_ref().unwrap().0.id();
    assert_eq!(ctx.tex_manager().read().meta(id).unwrap().options, egui::TextureOptions::NEAREST);
    assert_eq!(app.frames.dispatch_remote(), 0);
    assert_eq!(remote.0.lock().unwrap().len(), 1, "fresh samplers reuse the original rendered image");
    // Theme-only preferences do not discard otherwise valid installed textures.
    app.session.prefs.appearance.theme = "light".into();
    app.session.prefs_revision += 1;
    app.apply_prefs(&ctx);
    assert_eq!(app.viewer_tex.as_ref().unwrap().0.id(), id);
    assert_eq!(secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, false).unwrap().id(), fresh.id());
}

#[test]
fn installed_secondary_texture_survives_raw_cache_eviction_without_rerendering() {
    let (app, cid, remote, ctx) = immediate_app();
    app.frames.set_budget(16);
    let slot = crate::frames::SecondarySlot::Top;
    let opts = effectcraft_engine::render::RenderOpts::default();
    secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, false);
    assert_eq!(app.frames.dispatch_remote(), 1);
    let first = secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, false).unwrap();
    let installed: SecondaryTexture = ctx.data(|d| d.get_temp(secondary_id(slot))).unwrap();
    // A second actual frame evicts only the first raw image, leaving its pane texture intact.
    secondary_texture(&app, &ctx, crate::frames::SecondarySlot::Front, cid, Tick(1), opts, false);
    assert_eq!(app.frames.dispatch_remote(), 1);
    assert!(!app.frames.is_cached(&installed.key));
    app.frames.begin_secondary_paint();
    let reused = secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, false).unwrap();
    app.frames.end_secondary_paint();
    assert_eq!(reused.id(), first.id());
    assert_eq!(app.frames.dispatch_remote(), 0, "installed pixels do not need raw-cache reconstruction");
    assert_eq!(remote.0.lock().unwrap().len(), 2);
}

struct MutableFrames {
    pixel: Mutex<[u8; 4]>,
    started: std::sync::atomic::AtomicUsize,
}

impl crate::frames::RemoteFrames for MutableFrames {
    fn slots(&self) -> usize {
        4
    }
    fn start(&self, _: crate::frames::RemoteJob, done: crate::frames::RemoteDone) {
        self.started.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let rgba = self.pixel.lock().unwrap().repeat(4);
        done(Ok(crate::frames::RemoteFrame { width: 2, height: 2, rgba, ms: 0.0 }));
    }
}

struct SameMetadata(Footage);

impl effectcraft_engine::Importer for SameMetadata {
    fn probe(&self, _: &str) -> Result<Footage, String> {
        Ok(self.0.clone())
    }
}

#[test]
fn purge_and_same_metadata_reload_replace_installed_pixels_without_changing_frame_identity() {
    use crate::frames::{FrameImage, SecondarySlot};
    for reload in [false, true] {
        let (mut app, cid) = tiny_app(Arc::new(effectcraft_engine::render::NoFootage));
        let (footage_id, footage) = app
            .session
            .project
            .items
            .iter()
            .find_map(|(id, item)| match &item.kind {
                ItemKind::Footage(f) => Some((*id, f.clone())),
                _ => None,
            })
            .unwrap();
        app.session.importer = Some(Arc::new(SameMetadata(footage)));
        let remote = Arc::new(MutableFrames { pixel: Mutex::new([200, 0, 0, 255]), started: Default::default() });
        app.frames.set_remote(Some(remote.clone()));
        let ctx = egui::Context::default();
        let opts = effectcraft_engine::render::RenderOpts::default();
        let slots = [SecondarySlot::Top, SecondarySlot::Front, SecondarySlot::Right, SecondarySlot::LockedSplit];
        secondary_texture(&app, &ctx, slots[0], cid, Tick::ZERO, opts, false);
        assert_eq!(app.frames.dispatch_remote(), 1);
        let old_textures = slots.map(|slot| secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, false).unwrap());
        let installed: SecondaryTexture = ctx.data(|d| d.get_temp(secondary_id(slots[0]))).unwrap();
        let key = installed.key;
        let FrameImage::Cpu(old_pixels) = app.frames.get(&key).unwrap() else { panic!("CPU fixture") };
        show_frame(&mut app, &ctx, key, FrameImage::Cpu(old_pixels.clone()));
        let old_primary = app.viewer_tex.as_ref().unwrap().0.id();
        let late_readback = app.viewer_readback.1.clone();
        app.viewer_readback.0 = Some(key);
        let snapshot = Arc::new(Image::filled(2, 2, [0.0, 0.0, 1.0, 1.0]));
        app.session.snapshot = Some(effectcraft_engine::viewer::Snapshot { comp: cid, time: Tick::ZERO, scale: 1.0, image: snapshot.clone() });
        *remote.pixel.lock().unwrap() = [0, 200, 0, 255];
        if reload {
            app.session.execute("file.reloadFootage", json!({"items":[footage_id.0]})).unwrap();
        } else {
            app.session.execute("edit.purge", json!({"what":"memory"})).unwrap();
        }
        app.handle_events(&ctx);
        assert!(!app.frames.is_cached(&key));
        for slot in slots {
            assert!(ctx.data(|d| d.get_temp::<SecondaryTexture>(secondary_id(slot))).is_none());
        }
        assert!(app.viewer_tex.is_none() && app.viewer_shown.is_none() && app.viewer_image.is_none());
        assert!(app.viewer_readback.0.is_none());
        assert!(!Arc::ptr_eq(&late_readback, &app.viewer_readback.1));
        // Simulate a late browser callback without creating a GPU device.
        *late_readback.lock().unwrap() = Some((key, old_pixels));
        assert!(app.viewer_readback.1.lock().unwrap().is_none());
        assert!(Arc::ptr_eq(&app.session.snapshot.as_ref().unwrap().image, &snapshot), "memory purge preserves the deliberate comparison snapshot");
        assert!(secondary_texture(&app, &ctx, slots[0], cid, Tick::ZERO, opts, false).is_none());
        assert_eq!(app.frames.dispatch_remote(), 1);
        for (slot, old) in slots.into_iter().zip(old_textures) {
            let replacement = secondary_texture(&app, &ctx, slot, cid, Tick::ZERO, opts, false).unwrap();
            assert_ne!(replacement.id(), old.id());
            let now: SecondaryTexture = ctx.data(|d| d.get_temp(secondary_id(slot))).unwrap();
            assert_eq!(now.key, key, "the regression must preserve content identity despite changed source pixels");
        }
        let FrameImage::Cpu(fresh) = app.frames.get(&key).unwrap() else { panic!("CPU fixture") };
        assert!(fresh.pixels.iter().all(|p| p.to_array() == [0, 200, 0, 255]));
        show_frame(&mut app, &ctx, key, FrameImage::Cpu(fresh.clone()));
        assert_ne!(app.viewer_tex.as_ref().unwrap().0.id(), old_primary);
        assert!(Arc::ptr_eq(app.viewer_image.as_ref().unwrap(), &fresh));
        assert_eq!(remote.started.load(std::sync::atomic::Ordering::Relaxed), 2);
    }
}
