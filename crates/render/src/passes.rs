//! Rendering with deferred readbacks from loops that are not frame servers (the browser's
//! job workers: Render Queue renders and analyses).
//!
//! A frame worker renders one frame per request and resumes it pass by pass
//! (`FrameServer::resume`). Jobs render many frames from a loop; written as `async` functions,
//! the loop awaits [`in_passes`] for each frame: with an accelerator whose readbacks are
//! deferred ([`Accelerator::miss_gate`]), the render runs again after every pass that missed,
//! awaiting the device in between ([`Accelerator::settle`]), until a pass completes; with any
//! other accelerator (or none) it runs once and the future is ready at once. Desktop threads
//! drive the same futures with [`block_on`].

use std::future::Future;
use std::task::{Context, Poll, Waker};

use effectcraft_project::ItemId;
use effectcraft_raster::Image;
use effectcraft_time::Tick;

use crate::{Accelerator, AutoKey, Backend, Renderer};

/// Passes before a frame gives up on deferred readbacks and renders on the CPU.
pub const MAX_PASSES: u32 = 24;

/// What [`in_passes`] returns.
#[derive(Clone, Debug)]
pub struct Passed<T> {
    pub value: T,
    /// Render passes (1 without deferred readbacks).
    pub passes: u32,
    /// Rendered with the accelerator (not the CPU fallback after [`MAX_PASSES`]).
    pub accelerated: bool,
}

/// Whether `accel` defers its readbacks (renders take passes).
pub fn is_deferred(accel: Option<&dyn Accelerator>) -> bool {
    accel.is_some_and(|a| a.miss_gate().is_some())
}

/// Run `render` (one render with the accelerator it is given) until a pass doesn't miss: see
/// the module docs. After [`MAX_PASSES`] it renders once without the accelerator (the CPU).
/// Caches the render writes to must hold the accelerator's miss gate
/// ([`crate::LayerCache::set_gate`]).
pub async fn in_passes<'a, T>(accel: Option<&'a dyn Accelerator>, mut render: impl FnMut(Option<&'a dyn Accelerator>) -> T) -> Passed<T> {
    let Some(a) = accel.filter(|a| a.miss_gate().is_some()) else {
        return Passed { value: render(accel), passes: 1, accelerated: accel.is_some() };
    };
    a.frame_begin();
    for pass in 1..=MAX_PASSES {
        a.pass_begin();
        let value = render(Some(a));
        if !a.pass_missed() {
            return Passed { value, passes: pass, accelerated: true };
        }
        match a.settle() {
            Some(s) => s.await,
            None => break,
        }
    }
    Passed { value: render(None), passes: MAX_PASSES + 1, accelerated: false }
}

/// A top-level comp frame through [`in_passes`] ([`Renderer::comp_frame`] in passes). Backend
/// Auto picks CPU or GPU for the whole frame (all its passes) and is timed as a whole, as the
/// frame workers do. Ends the frame for the accelerator ([`Accelerator::frame_end`]).
pub async fn comp_frame(r: &Renderer<'_>, comp: ItemId, t: Tick) -> Image {
    let Some(a) = r.accel else { return r.comp_frame(comp, t) };
    if a.miss_gate().is_none() {
        let img = r.comp_frame(comp, t);
        a.frame_end();
        return img;
    }
    let mut opts = r.opts;
    let mut auto = None;
    if opts.backend == Backend::Auto {
        if !r.project.settings.gpu_acceleration {
            opts.backend = Backend::Cpu;
        } else if opts.roi.is_none()
            && let Some(pick) = a.auto_pick()
        {
            let key = AutoKey::new(comp, opts.scale, false);
            let gpu = pick.choose(key);
            opts.backend = if gpu { Backend::Gpu } else { Backend::Cpu };
            auto = Some((key, gpu));
        }
    }
    let t0 = web_time::Instant::now();
    let img = if opts.backend == Backend::Cpu {
        Renderer { opts, accel: None, ..*r }.comp_frame(comp, t)
    } else {
        in_passes(Some(a), |acc| Renderer { opts, accel: acc, ..*r }.comp_frame(comp, t)).await.value
    };
    a.frame_end();
    if let (Some((key, chose)), Some(pick)) = (auto, a.auto_pick()) {
        pick.record(key, chose, t0.elapsed().as_secs_f64() * 1e3);
    }
    img
}

/// Drive `f` to completion on this thread. For futures that only wait on deferred readbacks,
/// whose [`Accelerator::settle`] blocks natively: they never stay pending there.
pub fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}
