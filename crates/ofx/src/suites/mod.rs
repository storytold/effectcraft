//! The host side of the OFX suites, and the `OfxHost` block handed to every plug-in.
//!
//! Every callback is an `extern "C" fn` that wraps its body in `catch_unwind`
//! ([`guard`](crate::ffi::guard)), null-checks its pointers and validates handles by magic number;
//! none of them panics across the boundary.

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

use crate::ffi::*;
use crate::props::PropSet;

pub mod image_effect;
pub mod memory;
pub mod message;
pub mod misc;
pub mod multithread;
pub mod parameter;
pub mod property;

/// The `OfxHost` struct plus the property set it points at.
struct HostBlock {
    ofx: OfxHost,
    _props: Box<PropSet>,
}

// Built once; the contents are only read afterwards (the property set is internally locked).
unsafe impl Send for HostBlock {}
unsafe impl Sync for HostBlock {}

/// The name plug-ins see in `kOfxPropName`: "EffectCraft", or `EFFECTCRAFT_OFX_HOST_NAME`
/// (some plug-ins only enable themselves for hosts they know).
fn host_name() -> String {
    std::env::var("EFFECTCRAFT_OFX_HOST_NAME").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "EffectCraft".to_string())
}

fn build_host_props() -> PropSet {
    let p = PropSet::new();
    let name = host_name();
    p.put_str(P_TYPE, TYPE_IMAGE_EFFECT_HOST);
    p.put_str(P_NAME, &name);
    p.put_str(P_LABEL, &name);
    let mut ver = env!("CARGO_PKG_VERSION").split('.').map(|s| s.parse::<i32>().unwrap_or(0));
    p.put_ints(P_VERSION, &[ver.next().unwrap_or(0), ver.next().unwrap_or(0), ver.next().unwrap_or(0)]);
    p.put_str(P_VERSION_LABEL, env!("CARGO_PKG_VERSION"));
    p.put_ints(P_API_VERSION, &[1, 4]);
    p.put_int(IE_HOST_IS_BACKGROUND, 0);
    p.put_int(IE_SUPPORTS_OVERLAYS, 0);
    p.put_int(IE_SUPPORTS_MULTI_RESOLUTION, 0);
    p.put_int(IE_SUPPORTS_TILES, 0);
    p.put_int(IE_TEMPORAL_CLIP_ACCESS, 1);
    p.put_strs(IE_SUPPORTED_COMPONENTS, &[COMP_RGBA, COMP_RGB, COMP_ALPHA]);
    p.put_strs(IE_SUPPORTED_CONTEXTS, &[CTX_FILTER, CTX_GENERAL, CTX_TRANSITION, CTX_GENERATOR]);
    p.put_strs(IE_SUPPORTED_PIXEL_DEPTHS, &[DEPTH_FLOAT, DEPTH_SHORT, DEPTH_BYTE]);
    p.put_int(IE_MULTIPLE_CLIP_DEPTHS, 0);
    p.put_int(IE_MULTIPLE_CLIP_PARS, 0);
    p.put_int(IE_SETABLE_FRAME_RATE, 0);
    p.put_int(IE_SETABLE_FIELDING, 0);
    p.put_str(IE_NATIVE_ORIGIN, IE_NATIVE_ORIGIN_BOTTOM_LEFT);
    p.put_str(IE_OPENGL_RENDER_SUPPORTED, "false");
    p.put_int(PH_SUPPORTS_CUSTOM_ANIMATION, 0);
    p.put_int(PH_SUPPORTS_STRING_ANIMATION, 0);
    p.put_int(PH_SUPPORTS_BOOLEAN_ANIMATION, 1);
    p.put_int(PH_SUPPORTS_CHOICE_ANIMATION, 1);
    p.put_int(PH_SUPPORTS_STR_CHOICE_ANIMATION, 0);
    p.put_int(PH_SUPPORTS_STR_CHOICE, 1);
    p.put_int(PH_SUPPORTS_CUSTOM_INTERACT, 0);
    p.put_int(PH_MAX_PARAMETERS, -1);
    p.put_int(PH_MAX_PAGES, 0);
    p.put_ints(PH_PAGE_ROW_COLUMN_COUNT, &[0, 0]);
    p
}

fn host_block() -> &'static HostBlock {
    static HOST: OnceLock<HostBlock> = OnceLock::new();
    HOST.get_or_init(|| {
        let props = Box::new(build_host_props());
        let handle = props.as_handle();
        HostBlock { ofx: OfxHost { host: handle, fetch_suite }, _props: props }
    })
}

/// The `OfxHost*` to give plug-ins (valid for the whole process).
pub fn host_ptr() -> *mut OfxHost {
    &host_block().ofx as *const OfxHost as *mut OfxHost
}

extern "C" fn fetch_suite(_host: Handle, name: *const c_char, version: c_int) -> *const c_void {
    std::panic::catch_unwind(|| {
        // SAFETY: a C string from the plug-in.
        let Some(name) = (unsafe { str_arg(name) }) else { return std::ptr::null() };
        match (name, version) {
            (SUITE_PROPERTY, 1) => &property::SUITE as *const OfxPropertySuiteV1 as *const c_void,
            (SUITE_PARAMETER, 1) => &parameter::SUITE as *const OfxParameterSuiteV1 as *const c_void,
            (SUITE_IMAGE_EFFECT, 1) => &image_effect::SUITE as *const OfxImageEffectSuiteV1 as *const c_void,
            (SUITE_MEMORY, 1) => &memory::SUITE as *const OfxMemorySuiteV1 as *const c_void,
            (SUITE_MULTI_THREAD, 1) => &multithread::SUITE as *const OfxMultiThreadSuiteV1 as *const c_void,
            (SUITE_MESSAGE, 1) => &message::SUITE_V1 as *const OfxMessageSuiteV1 as *const c_void,
            (SUITE_MESSAGE, 2) => &message::SUITE_V2 as *const OfxMessageSuiteV2 as *const c_void,
            (SUITE_INTERACT, 1) => &misc::INTERACT as *const OfxInteractSuiteV1 as *const c_void,
            (SUITE_PROGRESS, 1 | 2) => &misc::PROGRESS as *const OfxProgressSuite as *const c_void,
            (SUITE_TIMELINE, 1) => &misc::TIMELINE as *const OfxTimeLineSuiteV1 as *const c_void,
            _ => {
                crate::props::note_unknown(&format!("suite {name} v{version}"));
                std::ptr::null()
            }
        }
    })
    .unwrap_or(std::ptr::null())
}
