//! The small suites: interact (overlays are unsupported), progress and timeline.

use std::ffi::c_char;

use crate::ffi::*;
use crate::instance::EffectObj;

extern "C" fn interact_swap_buffers(_h: Handle) -> OfxStatus {
    K_STAT_ERR_MISSING_HOST_FEATURE
}
extern "C" fn interact_redraw(_h: Handle) -> OfxStatus {
    K_STAT_ERR_MISSING_HOST_FEATURE
}
extern "C" fn interact_get_property_set(_h: Handle, props: *mut Handle) -> OfxStatus {
    if !props.is_null() {
        // SAFETY: valid out-pointer.
        unsafe { *props = std::ptr::null_mut() };
    }
    K_STAT_ERR_MISSING_HOST_FEATURE
}

/// Interact suite: every call reports a missing host feature (the host has no overlays).
pub static INTERACT: OfxInteractSuiteV1 = OfxInteractSuiteV1 { interact_swap_buffers, interact_redraw, interact_get_property_set };

extern "C" fn progress_start(_h: Handle, _message: *const c_char, _id: *const c_char) -> OfxStatus {
    K_STAT_OK
}
extern "C" fn progress_update(_h: Handle, _progress: f64) -> OfxStatus {
    // `kOfxStatReplyNo` would tell the plug-in to abort; renders are never cancelled this way.
    K_STAT_OK
}
extern "C" fn progress_end(_h: Handle) -> OfxStatus {
    K_STAT_OK
}

/// Progress suite (V1 and V2 share a layout): progress is accepted and ignored.
pub static PROGRESS: OfxProgressSuite = OfxProgressSuite { progress_start, progress_update, progress_end };

extern "C" fn get_time(h: Handle, out: *mut f64) -> OfxStatus {
    guard(|| {
        if out.is_null() {
            return K_STAT_FAILED;
        }
        // SAFETY: effect handle from the plug-in.
        let t = unsafe { EffectObj::from_handle(h) }.and_then(EffectObj::current_render).map(|c| c.time_sec * c.fps).unwrap_or(0.0);
        unsafe { *out = t };
        K_STAT_OK
    })
}
extern "C" fn goto_time(_h: Handle, _t: f64) -> OfxStatus {
    K_STAT_OK
}
extern "C" fn get_time_bounds(h: Handle, first: *mut f64, last: *mut f64) -> OfxStatus {
    guard(|| {
        // The source's real frame range during a render, else a very long timeline.
        // SAFETY: effect handle from the plug-in.
        let (a, b) = unsafe { EffectObj::from_handle(h) }.and_then(EffectObj::current_render).and_then(|c| c.frame_range()).unwrap_or((0.0, 99999.0));
        // SAFETY: valid out-pointers when non-null.
        unsafe {
            if !first.is_null() {
                *first = a;
            }
            if !last.is_null() {
                *last = b;
            }
        }
        K_STAT_OK
    })
}

/// Timeline suite: the source's frame range during a render (else a fixed, very long timeline);
/// seeking is ignored.
pub static TIMELINE: OfxTimeLineSuiteV1 = OfxTimeLineSuiteV1 { get_time, goto_time, get_time_bounds };
