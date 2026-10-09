//! `OfxImageEffectSuiteV1`: effect / clip handles, `clipGetImage` and friends, image memory.
//!
//! Images come from the render context installed by the host around a render: the plug-in's own
//! layer at the current time is already converted; other times and other layers are fetched
//! lazily from the [`PluginHost`](effectcraft_effects::plugin::PluginHost) and cached for the
//! duration of the render.

use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::clips::{ClipObj, new_image_handle, release_image};
use crate::ffi::*;
use crate::instance::{ClipRole, EffectObj, RenderCtx};
use crate::suites::memory::{host_alloc, host_free};

extern "C" fn get_property_set(effect: Handle, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: handles and out-pointers come from the plug-in.
        let Some(e) = (unsafe { EffectObj::from_handle(effect) }) else { return K_STAT_ERR_BAD_HANDLE };
        if props.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        unsafe { *props = e.props.as_handle() };
        K_STAT_OK
    })
}

extern "C" fn get_param_set(effect: Handle, set: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(e) = (unsafe { EffectObj::from_handle(effect) }) else { return K_STAT_ERR_BAD_HANDLE };
        if set.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        // The parameter set and the effect share one handle (and one property set).
        unsafe { *set = e.handle() };
        K_STAT_OK
    })
}

extern "C" fn clip_define(effect: Handle, name: *const c_char, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(e) = (unsafe { EffectObj::from_handle(effect) }) else { return K_STAT_ERR_BAD_HANDLE };
        let Some(name) = (unsafe { str_arg(name) }) else { return K_STAT_ERR_UNKNOWN };
        match e.define_clip(name) {
            Ok(c) => {
                if !props.is_null() {
                    // SAFETY: stable pointer into the effect's clip list.
                    unsafe { *props = (*c).props.as_handle() };
                }
                K_STAT_OK
            }
            Err(s) => s,
        }
    })
}

extern "C" fn clip_get_handle(effect: Handle, name: *const c_char, clip: *mut Handle, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(e) = (unsafe { EffectObj::from_handle(effect) }) else { return K_STAT_ERR_BAD_HANDLE };
        let Some(name) = (unsafe { str_arg(name) }) else { return K_STAT_ERR_UNKNOWN };
        let Some(c) = e.find_clip(name) else { return K_STAT_ERR_UNKNOWN };
        // SAFETY: stable pointer into the effect's clip list.
        let c = unsafe { &*c };
        if clip.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        unsafe {
            *clip = c.handle();
            if !props.is_null() {
                *props = c.props.as_handle();
            }
        }
        K_STAT_OK
    })
}

extern "C" fn clip_get_property_set(clip: Handle, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(c) = (unsafe { ClipObj::from_handle(clip) }) else { return K_STAT_ERR_BAD_HANDLE };
        if props.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        unsafe { *props = c.props.as_handle() };
        K_STAT_OK
    })
}

/// The render context of the effect a clip belongs to.
fn ctx_of(c: &ClipObj) -> Option<std::sync::Arc<RenderCtx>> {
    // SAFETY: an instance outlives its clips.
    unsafe { c.owner.as_ref() }.and_then(EffectObj::current_render)
}

extern "C" fn clip_get_image(clip: Handle, time: f64, _region: *const OfxRectD, out: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(c) = (unsafe { ClipObj::from_handle(clip) }) else { return K_STAT_ERR_BAD_HANDLE };
        if out.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        let Some(ctx) = ctx_of(c) else { return K_STAT_FAILED };
        let Some(img) = ctx.image(&c.name, time) else { return K_STAT_FAILED };
        let h = new_image_handle(&img, ctx.geom.scale, RenderCtx::next_image_id());
        unsafe { *out = h };
        K_STAT_OK
    })
}

extern "C" fn clip_release_image(img: Handle) -> OfxStatus {
    guard(|| release_image(img))
}

extern "C" fn clip_get_region_of_definition(clip: Handle, _time: f64, bounds: *mut OfxRectD) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(c) = (unsafe { ClipObj::from_handle(clip) }) else { return K_STAT_ERR_BAD_HANDLE };
        if bounds.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        let Some(ctx) = ctx_of(c) else { return K_STAT_FAILED };
        let rod = match ctx.cfg.clip(&c.name).map(|k| &k.role) {
            Some(ClipRole::Output) => *ctx.rod.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
            Some(ClipRole::Input) => ctx.input_rod(),
            Some(ClipRole::Layer(_)) if ctx.connected(&c.name) => ctx.input_rod(),
            _ => return K_STAT_FAILED,
        };
        unsafe { *bounds = rod };
        K_STAT_OK
    })
}

extern "C" fn abort(_effect: Handle) -> c_int {
    0
}

const IMAGE_MEM_MAGIC: u32 = 0x4F46_584D; // "OFXM"

#[repr(C)]
struct ImageMem {
    magic: u32,
    ptr: *mut u8,
    locks: AtomicU32,
}

extern "C" fn image_memory_alloc(_effect: Handle, bytes: usize, out: *mut Handle) -> OfxStatus {
    guard(|| {
        if out.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        unsafe { *out = std::ptr::null_mut() };
        let Some(ptr) = host_alloc(bytes) else { return K_STAT_ERR_MEMORY };
        let m = Box::new(ImageMem { magic: IMAGE_MEM_MAGIC, ptr, locks: AtomicU32::new(0) });
        unsafe { *out = Box::into_raw(m) as Handle };
        K_STAT_OK
    })
}

fn image_mem<'a>(h: Handle) -> Option<&'a ImageMem> {
    if h.is_null() {
        return None;
    }
    // SAFETY: readable handle from the plug-in; the magic rejects foreign pointers.
    if unsafe { std::ptr::read(h as *const u32) } != IMAGE_MEM_MAGIC {
        return None;
    }
    Some(unsafe { &*(h as *const ImageMem) })
}

extern "C" fn image_memory_free(h: Handle) -> OfxStatus {
    guard(|| {
        let Some(m) = image_mem(h) else { return K_STAT_ERR_BAD_HANDLE };
        // SAFETY: `m.ptr` came from `host_alloc` in `image_memory_alloc`; the handle is freed below.
        unsafe { host_free(m.ptr) };
        // SAFETY: allocated with Box::into_raw above; the magic is cleared so a double free is
        // rejected as long as the allocator does not reuse the address.
        let b = unsafe { Box::from_raw(h as *mut ImageMem) };
        drop(b);
        K_STAT_OK
    })
}

extern "C" fn image_memory_lock(h: Handle, out: *mut *mut c_void) -> OfxStatus {
    guard(|| {
        let Some(m) = image_mem(h) else { return K_STAT_ERR_BAD_HANDLE };
        if out.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        m.locks.fetch_add(1, Ordering::Relaxed);
        unsafe { *out = m.ptr as *mut c_void };
        K_STAT_OK
    })
}

extern "C" fn image_memory_unlock(h: Handle) -> OfxStatus {
    guard(|| {
        let Some(m) = image_mem(h) else { return K_STAT_ERR_BAD_HANDLE };
        let v = m.locks.load(Ordering::Relaxed);
        if v > 0 {
            m.locks.store(v - 1, Ordering::Relaxed);
        }
        K_STAT_OK
    })
}

/// The image-effect suite table.
pub static SUITE: OfxImageEffectSuiteV1 = OfxImageEffectSuiteV1 {
    get_property_set,
    get_param_set,
    clip_define,
    clip_get_handle,
    clip_get_property_set,
    clip_get_image,
    clip_release_image,
    clip_get_region_of_definition,
    abort,
    image_memory_alloc,
    image_memory_free,
    image_memory_lock,
    image_memory_unlock,
};
