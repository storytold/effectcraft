//! The frame server: `frames.open` and bulk `CFRQ` requests answered with `Renderer::comp_frame`.
//!
//! [`CompFrames`] is the app side of `craft_ipc::bulk::FrameSource`. It never touches the
//! [`Session`]. The UI thread publishes a [`FrameWorld`] (an `Arc<Project>` snapshot plus the
//! footage, expression and GPU hosts the session already owns) whenever the project or an epoch
//! changes. The bulk connection threads of the link server are the render workers: each call to
//! [`FrameSource::render`] reads the newest published world and renders on its own thread. Frames
//! are premultiplied RGBA8 from [`rgba8_premultiplied`], the same bytes the viewer shows.
//!
//! A frame is stamped with the epoch of the world it came from. The layer cache is the session's
//! own, so a frame the viewer rendered is not rendered twice.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use craft_ipc::LinkError;
use craft_ipc::bulk::{FrameFormat, FrameLayout, FrameRequest, FrameSource, FrameSpec, RenderedFrame};
use effectcraft_engine::Session;
use effectcraft_engine::remote::rgba8_premultiplied;
use effectcraft_project::{ItemId, Project};
use effectcraft_render::{Accelerator, ExprHost, FootageSource, LayerCache, RenderOpts, Renderer};
use effectcraft_time::Tick;

/// One bound link as the frame workers see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundLink {
    pub comp: ItemId,
    pub epoch: u64,
}

/// Everything a worker needs to render, frozen at one project revision.
pub struct FrameWorld {
    pub project: Arc<Project>,
    pub footage: Arc<dyn FootageSource>,
    pub expr: Option<Arc<dyn ExprHost>>,
    pub accel: Option<Arc<dyn Accelerator>>,
    pub cache: Arc<LayerCache>,
    pub nested_switches: bool,
    pub draft_shadows: bool,
    /// By link id.
    pub links: BTreeMap<String, BoundLink>,
}

impl FrameWorld {
    /// The world of `session`, with link epochs from `epochs` (a link not listed uses the epoch
    /// its binding stores).
    pub fn of(session: &Session, epochs: &BTreeMap<String, u64>) -> Self {
        let links = session
            .project
            .craft_links
            .iter()
            .map(|(link_id, binding)| (link_id.clone(), BoundLink { comp: binding.comp, epoch: epochs.get(link_id).copied().unwrap_or(binding.epoch) }))
            .collect();
        Self {
            project: Arc::clone(&session.project),
            footage: Arc::clone(&session.footage),
            expr: session.expr.clone(),
            accel: session.accel.clone(),
            cache: Arc::clone(&session.layer_cache),
            nested_switches: session.prefs.general.switches_affect_nested_comps,
            draft_shadows: session.prefs.three_d.realtime_shadows,
            links,
        }
    }

    fn link(&self, link_id: &str) -> Result<BoundLink, LinkError> {
        self.links.get(link_id).copied().ok_or_else(|| LinkError::Unknown(format!("no link `{link_id}` is bound in this project")))
    }

    /// Pixel size of the comp at `scale`, as `comp_frame` makes it.
    fn size(&self, comp: ItemId, scale: f64) -> Result<(u32, u32), LinkError> {
        let comp = self.project.comp(comp).ok_or_else(|| LinkError::Unknown("the composition of this link is not in the project".into()))?;
        Ok((((f64::from(comp.width) * scale).round() as u32).max(1), ((f64::from(comp.height) * scale).round() as u32).max(1)))
    }

    /// Render comp time `time_s` of `link_id`.
    pub fn render(&self, link_id: &str, scale: f64, time_s: f64, region: Option<[u32; 4]>) -> Result<RenderedFrame, LinkError> {
        let bound = self.link(link_id)?;
        let comp = self.project.comp(bound.comp).ok_or_else(|| LinkError::Unknown("the composition of this link is not in the project".into()))?;
        // Past the end the comp holds its last frame, like the result movie does.
        let last = (comp.duration.0 - comp.frame_rate.tick_of(1).0).max(0);
        let t = Tick::from_seconds_f64(time_s.max(0.0)).min(Tick(last));
        let started = Instant::now();
        let opts = RenderOpts { scale, nested_switches: self.nested_switches, draft_shadows: self.draft_shadows, ..RenderOpts::default() };
        let mut renderer = Renderer::new(&self.project, self.footage.as_ref(), opts);
        renderer.expr = self.expr.as_deref();
        renderer.cache = Some(&self.cache);
        renderer.accel = self.accel.as_deref();
        let image = renderer.comp_frame(bound.comp, t);
        let (w, h) = (image.width, image.height);
        let full = rgba8_premultiplied(&image);
        let render_ms = started.elapsed().as_secs_f64() * 1e3;
        let (w, h, pixels) = match region {
            None => (w, h, full),
            Some(rect) => crop(&full, w, h, rect)?,
        };
        Ok(RenderedFrame { epoch: bound.epoch, w, h, stride: w * 4, format: FrameFormat::Rgba8Premul, render_ms, pixels })
    }
}

/// The `[x, y, w, h]` rectangle of an RGBA8 picture. A rectangle outside the picture is an error.
fn crop(pixels: &[u8], w: u32, h: u32, rect: [u32; 4]) -> Result<(u32, u32, Vec<u8>), LinkError> {
    let [x, y, rw, rh] = rect;
    let inside = rw > 0 && rh > 0 && x.checked_add(rw).is_some_and(|end| end <= w) && y.checked_add(rh).is_some_and(|end| end <= h);
    if !inside {
        return Err(LinkError::BadParams(format!("region {rect:?} is outside the {w}x{h} frame")));
    }
    let row = |n: u32| -> usize { n as usize * 4 };
    let mut out = Vec::with_capacity(row(rw) * rh as usize);
    for line in y..y + rh {
        let start = row(line * w + x);
        let end = start + row(rw);
        out.extend_from_slice(pixels.get(start..end).ok_or_else(|| LinkError::Invalid("frame is shorter than its size".into()))?);
    }
    Ok((rw, rh, out))
}

/// `FrameSource` over the newest published [`FrameWorld`].
#[derive(Default)]
pub struct CompFrames {
    world: RwLock<Option<Arc<FrameWorld>>>,
}

impl CompFrames {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the world. Renders that began on the old one finish on it.
    pub fn publish(&self, world: FrameWorld) {
        *self.world.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(world));
    }

    /// Forget the world: every ask then fails with `not_ready`.
    pub fn clear(&self) {
        *self.world.write().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn world(&self) -> Result<Arc<FrameWorld>, LinkError> {
        self.world.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone().ok_or_else(|| LinkError::NotReady("no project is open yet".into()))
    }
}

impl FrameSource for CompFrames {
    fn open(&self, spec: &FrameSpec) -> Result<FrameLayout, LinkError> {
        if spec.format != FrameFormat::Rgba8Premul {
            return Err(LinkError::BadParams(format!("{} frames are not served yet; ask for rgba8_premul", spec.format.as_str())));
        }
        let world = self.world()?;
        let bound = world.link(&spec.link_id)?;
        let (w, h) = world.size(bound.comp, spec.scale)?;
        Ok(FrameLayout { w, h, stride: w * 4, len: u64::from(w) * u64::from(h) * 4 })
    }

    fn render(&self, spec: &FrameSpec, request: &FrameRequest) -> Result<RenderedFrame, LinkError> {
        let world = self.world()?;
        let mut frame = catch_unwind(AssertUnwindSafe(|| world.render(&spec.link_id, spec.scale, request.time_s, request.region)))
            .map_err(|_| LinkError::NotReady("the renderer failed on this frame".into()))??;
        // The contract: the epoch is at least the one asked for.
        frame.epoch = frame.epoch.max(request.epoch);
        Ok(frame)
    }

    fn close(&self, _spec: &FrameSpec) {}
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::*;

    const LINK: &str = "33333333-3333-4333-8333-333333333333";

    /// The demo project with its comp bound to a link.
    fn session_with_link() -> (Session, ItemId) {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).unwrap();
        let comp = session.active_comp_id().unwrap();
        session.project_mut().craft_links.insert(
            LINK.to_string(),
            effectcraft_project::CraftLinkBinding {
                link_id: LINK.to_string(),
                comp,
                source: json!({"app": "filmcraft", "doc_id": "0", "item": "1"}),
                result_path: "/tmp/never-written.mov".to_string(),
                epoch: 4,
            },
        );
        (session, comp)
    }

    fn spec(scale: f64) -> FrameSpec {
        FrameSpec { link_id: LINK.to_string(), format: FrameFormat::Rgba8Premul, scale }
    }

    fn request(time_s: f64, epoch: u64, region: Option<[u32; 4]>) -> FrameRequest {
        FrameRequest { req: 1, time_s, epoch, region }
    }

    fn served(session: &Session) -> CompFrames {
        let frames = CompFrames::new();
        frames.publish(FrameWorld::of(session, &BTreeMap::new()));
        frames
    }

    #[test]
    fn a_frame_has_the_bytes_the_viewer_renders() {
        let (session, comp) = session_with_link();
        let frames = served(&session);
        let layout = frames.open(&spec(0.25)).unwrap();
        let t = Tick::from_seconds_f64(1.0);
        let local = rgba8_premultiplied(&session.render(comp, t, RenderOpts { scale: 0.25, ..RenderOpts::default() }));
        let frame = frames.render(&spec(0.25), &request(1.0, 0, None)).unwrap();
        assert_eq!((frame.w, frame.h, frame.stride), (layout.w, layout.h, layout.stride));
        assert_eq!(u64::try_from(frame.pixels.len()).unwrap(), layout.len);
        assert_eq!(frame.format, FrameFormat::Rgba8Premul);
        assert_eq!(frame.pixels, local, "the served frame equals the viewer's frame");
        assert!(frame.pixels.iter().any(|b| *b != 0), "the demo comp is not empty");
        assert!(frame.render_ms > 0.0);
    }

    #[test]
    fn the_frame_carries_the_epoch_it_was_published_at() {
        let (session, _) = session_with_link();
        let frames = CompFrames::new();
        frames.publish(FrameWorld::of(&session, &BTreeMap::from([(LINK.to_string(), 9)])));
        assert_eq!(frames.render(&spec(0.25), &request(0.0, 3, None)).unwrap().epoch, 9);
        // A binding's stored epoch is used when the tracker has none.
        frames.publish(FrameWorld::of(&session, &BTreeMap::new()));
        assert_eq!(frames.render(&spec(0.25), &request(0.0, 0, None)).unwrap().epoch, 4);
        // The reply is never older than the ask.
        assert_eq!(frames.render(&spec(0.25), &request(0.0, 12, None)).unwrap().epoch, 12);
    }

    #[test]
    fn a_region_is_a_crop_of_the_whole_frame() {
        let (session, _) = session_with_link();
        let frames = served(&session);
        let whole = frames.render(&spec(0.25), &request(0.5, 0, None)).unwrap();
        let part = frames.render(&spec(0.25), &request(0.5, 0, Some([3, 2, 5, 4]))).unwrap();
        assert_eq!((part.w, part.h, part.stride), (5, 4, 20));
        for row in 0..4usize {
            let from = ((row + 2) * whole.w as usize + 3) * 4;
            assert_eq!(part.pixels[row * 20..row * 20 + 20], whole.pixels[from..from + 20], "row {row}");
        }
        let outside = frames.render(&spec(0.25), &request(0.5, 0, Some([whole.w - 1, 0, 2, 2])));
        assert!(matches!(outside, Err(LinkError::BadParams(_))));
    }

    #[test]
    fn time_past_the_end_holds_the_last_frame_and_before_zero_the_first() {
        let (session, comp) = session_with_link();
        let frames = served(&session);
        let duration = session.project.comp(comp).unwrap().duration.seconds();
        let last = frames.render(&spec(0.25), &request(duration + 50.0, 0, None)).unwrap();
        let held = frames.render(&spec(0.25), &request(duration + 1.0, 0, None)).unwrap();
        assert_eq!(last.pixels, held.pixels);
        let first = frames.render(&spec(0.25), &request(0.0, 0, None)).unwrap();
        let before = frames.render(&spec(0.25), &request(-3.0, 0, None)).unwrap();
        assert_eq!(first.pixels, before.pixels);
    }

    #[test]
    fn errors_name_what_is_wrong() {
        let (session, _) = session_with_link();
        let frames = CompFrames::new();
        assert!(matches!(frames.open(&spec(1.0)), Err(LinkError::NotReady(_))), "no world yet");
        frames.publish(FrameWorld::of(&session, &BTreeMap::new()));
        let unknown = FrameSpec { link_id: "nope".into(), ..spec(1.0) };
        assert!(matches!(frames.open(&unknown), Err(LinkError::Unknown(_))));
        let half_float = FrameSpec { format: FrameFormat::Rgba16fPremul, ..spec(1.0) };
        assert!(matches!(frames.open(&half_float), Err(LinkError::BadParams(_))));
        frames.clear();
        assert!(matches!(frames.render(&spec(1.0), &request(0.0, 0, None)), Err(LinkError::NotReady(_))));
    }

    #[test]
    fn a_new_world_changes_the_next_frame_and_not_a_frame_in_flight() {
        let (mut session, comp) = session_with_link();
        let frames = served(&session);
        let before = frames.render(&spec(0.25), &request(0.5, 0, None)).unwrap();
        let layers: Vec<u64> = session.project.comp(comp).unwrap().layers.iter().map(|layer| layer.id.0).collect();
        assert!(!layers.is_empty());
        for layer in layers {
            session.execute("effect.apply", json!({"layer": layer, "effect": "Invert"})).unwrap();
        }
        // Not published yet: the old world still answers.
        assert_eq!(frames.render(&spec(0.25), &request(0.5, 0, None)).unwrap().pixels, before.pixels);
        frames.publish(FrameWorld::of(&session, &BTreeMap::new()));
        assert_ne!(frames.render(&spec(0.25), &request(0.5, 0, None)).unwrap().pixels, before.pixels);
    }

    /// The whole path: a craft-ipc server with this frame source, and the fetcher a peer uses.
    #[test]
    fn frames_travel_over_the_socket_to_a_fetcher() {
        use craft_ipc::fetch::{FetchOptions, Fetched, FrameFetcher, FrameKey};
        use craft_ipc::server::{Activate, Activation, ControlSink, Frames, Server, ServerConfig};

        struct NoSink;
        impl ControlSink for NoSink {
            fn submit(&self, _m: String, _p: serde_json::Value) -> Option<std::sync::mpsc::Receiver<serde_json::Value>> {
                None
            }
        }
        struct NoActivate;
        impl Activate for NoActivate {
            fn activate(&self, _t: Option<String>, _p: Option<u32>, _o: Option<serde_json::Value>) -> Activation {
                Activation::No
            }
        }

        let runtime = crate::test_support::TempRuntime::new("frames");
        let (mut session, comp) = session_with_link();
        let frames = Arc::new(CompFrames::new());
        frames.publish(FrameWorld::of(&session, &BTreeMap::from([(LINK.to_string(), 6)])));
        let tickets = Arc::new(craft_ipc::bulk::FrameTickets::new(Arc::clone(&frames) as Arc<dyn FrameSource>));
        let server = craft_ipc::server::Server::start(ServerConfig {
            app: "effectcraft".into(),
            app_version: "0.0.0-test".into(),
            instance: None,
            runtime: Some(runtime.dir.clone()),
            tcp_port: None,
            sink: Arc::new(NoSink),
            activate: Arc::new(NoActivate),
            frames: tickets as Arc<dyn Frames>,
        })
        .unwrap();
        let _: &Server = &server;
        let fetcher = FrameFetcher::connect(server.record(), LINK, FrameFormat::Rgba8Premul, 0.25, FetchOptions::default()).unwrap();
        let key = FrameKey { frame: 24, scale_milli: 250 };
        let Fetched::Frame(first) = fetcher.fetch(key, 1.0, 1, std::time::Duration::from_secs(20)) else { panic!("no frame") };
        let local = rgba8_premultiplied(&session.render(comp, Tick::from_seconds_f64(1.0), RenderOpts { scale: 0.25, ..RenderOpts::default() }));
        assert_eq!(first.pixels, local);
        assert_eq!(first.epoch, 6);
        // A new world with a new epoch and a different picture reaches the next ask.
        let layers: Vec<u64> = session.project.comp(comp).unwrap().layers.iter().map(|layer| layer.id.0).collect();
        for layer in layers {
            session.execute("effect.apply", json!({"layer": layer, "effect": "Invert"})).unwrap();
        }
        frames.publish(FrameWorld::of(&session, &BTreeMap::from([(LINK.to_string(), 7)])));
        let Fetched::Frame(second) = fetcher.fetch(key, 1.0, 7, std::time::Duration::from_secs(20)) else { panic!("no frame") };
        assert_eq!(second.epoch, 7);
        assert_ne!(second.pixels, first.pixels);
    }
}
