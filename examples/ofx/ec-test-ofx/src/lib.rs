//! A tiny OpenFX plug-in bundle used to test EffectCraft's OFX host. It exports five effects:
//!
//! * `org.effectcraft.test.Invert`: filter, inverts the colours (double param `amount`);
//! * `org.effectcraft.test.Echo`: filter, averages Source at `t` and `t - 1` (temporal access);
//! * `org.effectcraft.test.Border`: filter, grows the region of definition by int param `size`
//!   pixels and paints the new area opaque red;
//! * `org.effectcraft.test.Mix`: general context, clips Source / Matte / Output, the Matte clip's
//!   alpha multiplies the Source;
//! * `org.effectcraft.test.AllParams`: declares every parameter type, passes the image through.
//!
//! Float RGBA only. Rows are addressed through `OfxImagePropRowBytes` (which may be negative).

mod ffi;

use std::ffi::{CStr, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{null, null_mut};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, Ordering};

use ffi::*;

const INVERT: usize = 0;
const ECHO: usize = 1;
const BORDER: usize = 2;
const MIX: usize = 3;
const ALL_PARAMS: usize = 4;

const LABELS: [&CStr; 5] = [c"EC Test Invert", c"EC Test Echo", c"EC Test Border", c"EC Test Mix", c"EC Test All Params"];

static HOST: AtomicPtr<OfxHost> = AtomicPtr::new(null_mut());
static SUITES: OnceLock<Suites> = OnceLock::new();

fn suites() -> &'static Suites {
    SUITES.get().expect("OfxActionLoad has not run")
}

unsafe extern "C" fn set_host(host: *mut OfxHost) {
    HOST.store(host, Ordering::SeqCst);
}

// ---- exports ---------------------------------------------------------------------------------

struct Plug(OfxPlugin);
// SAFETY: the table is immutable and only holds pointers to 'static data.
unsafe impl Sync for Plug {}

macro_rules! plug {
    ($id:expr, $k:expr) => {
        Plug(OfxPlugin {
            plugin_api: c"OfxImageEffectPluginAPI".as_ptr(),
            api_version: 1,
            plugin_identifier: $id.as_ptr(),
            version_major: 1,
            version_minor: 0,
            set_host,
            main_entry: entry::<{ $k }>,
        })
    };
}

static PLUGINS: [Plug; 5] = [
    plug!(c"org.effectcraft.test.Invert", INVERT),
    plug!(c"org.effectcraft.test.Echo", ECHO),
    plug!(c"org.effectcraft.test.Border", BORDER),
    plug!(c"org.effectcraft.test.Mix", MIX),
    plug!(c"org.effectcraft.test.AllParams", ALL_PARAMS),
];

#[unsafe(no_mangle)]
pub extern "C" fn OfxGetNumberOfPlugins() -> c_int {
    PLUGINS.len() as c_int
}

#[unsafe(no_mangle)]
pub extern "C" fn OfxGetPlugin(nth: c_int) -> *mut OfxPlugin {
    usize::try_from(nth).ok().and_then(|n| PLUGINS.get(n)).map_or(null_mut(), |p| &p.0 as *const OfxPlugin as *mut OfxPlugin)
}

// ---- actions ---------------------------------------------------------------------------------

unsafe extern "C" fn entry<const K: usize>(action: *const c_char, handle: *const c_void, in_args: Props, out_args: Props) -> Status {
    let action = unsafe { CStr::from_ptr(action) }.to_bytes();
    catch_unwind(AssertUnwindSafe(|| unsafe { dispatch(K, action, handle as Effect, in_args, out_args) })).unwrap_or(FAILED)
}

unsafe fn dispatch(k: usize, action: &[u8], effect: Effect, in_args: Props, out_args: Props) -> Status {
    unsafe {
        match action {
            b"OfxActionLoad" => match Suites::fetch(HOST.load(Ordering::SeqCst)) {
                Some(s) => {
                    let _ = SUITES.set(s);
                    OK
                }
                None => ERR_UNSUPPORTED,
            },
            b"OfxActionDescribe" => describe(k, effect),
            b"OfxImageEffectActionDescribeInContext" => describe_in_context(k, effect),
            b"OfxImageEffectActionGetRegionOfDefinition" if k == BORDER => border_rod(effect, in_args, out_args),
            b"OfxImageEffectActionRender" => render(k, effect, in_args),
            _ => REPLY_DEFAULT,
        }
    }
}

unsafe fn describe(k: usize, effect: Effect) -> Status {
    let s = suites();
    unsafe {
        let mut p = null_mut();
        (s.fx.get_property_set)(effect, &mut p);
        s.set_str(p, c"OfxPropLabel", 0, LABELS[k]);
        s.set_str(p, c"OfxImageEffectPluginPropGrouping", 0, c"EC Test");
        s.set_str(p, c"OfxImageEffectPropSupportedContexts", 0, if k == MIX { c"OfxImageEffectContextGeneral" } else { c"OfxImageEffectContextFilter" });
        s.set_str(p, c"OfxImageEffectPropSupportedPixelDepths", 0, c"OfxBitDepthFloat");
        s.set_str(p, c"OfxImageEffectPluginRenderThreadSafety", 0, c"OfxImageEffectRenderFullySafe");
        s.set_int(p, c"OfxImageEffectPropSupportsTiles", 0, 0);
        s.set_int(p, c"OfxImageEffectPropTemporalClipAccess", 0, (k == ECHO) as c_int);
    }
    OK
}

unsafe fn define_clip(effect: Effect, name: &CStr, optional: bool, temporal: bool) {
    let s = suites();
    unsafe {
        let mut p = null_mut();
        (s.fx.clip_define)(effect, name.as_ptr(), &mut p);
        s.set_str(p, c"OfxImageEffectPropSupportedComponents", 0, c"OfxImageComponentRGBA");
        s.set_int(p, c"OfxImageClipPropOptional", 0, optional as c_int);
        s.set_int(p, c"OfxImageEffectPropTemporalClipAccess", 0, temporal as c_int);
    }
}

/// Defines a parameter and returns its property set.
unsafe fn define_param(set: ParamSet, kind: &CStr, name: &CStr, label: &CStr) -> Props {
    let s = suites();
    unsafe {
        let mut p = null_mut();
        (s.param.define)(set, kind.as_ptr(), name.as_ptr(), &mut p);
        s.set_str(p, c"OfxPropLabel", 0, label);
        p
    }
}

unsafe fn describe_in_context(k: usize, effect: Effect) -> Status {
    let s = suites();
    unsafe {
        define_clip(effect, c"Output", false, false);
        define_clip(effect, c"Source", false, k == ECHO);
        if k == MIX {
            define_clip(effect, c"Matte", true, false);
        }
        let mut set = null_mut();
        (s.fx.get_param_set)(effect, &mut set);
        match k {
            INVERT => {
                let p = define_param(set, c"OfxParamTypeDouble", c"amount", c"Amount");
                s.set_dbl(p, c"OfxParamPropDefault", 0, 1.0);
                s.set_dbl(p, c"OfxParamPropMin", 0, 0.0);
                s.set_dbl(p, c"OfxParamPropMax", 0, 1.0);
                s.set_dbl(p, c"OfxParamPropDisplayMin", 0, 0.0);
                s.set_dbl(p, c"OfxParamPropDisplayMax", 0, 1.0);
            }
            BORDER => {
                let p = define_param(set, c"OfxParamTypeInteger", c"size", c"Size");
                s.set_int(p, c"OfxParamPropDefault", 0, 4);
                s.set_int(p, c"OfxParamPropMin", 0, 0);
                s.set_int(p, c"OfxParamPropMax", 0, 256);
            }
            ALL_PARAMS => describe_all_params(set),
            _ => {}
        }
    }
    OK
}

/// One parameter of every OFX type, plus a group (with a child) and a page listing them all.
unsafe fn describe_all_params(set: ParamSet) {
    let s = suites();
    unsafe {
        let p = define_param(set, c"OfxParamTypeDouble", c"double", c"Double");
        s.set_dbl(p, c"OfxParamPropDefault", 0, 0.5);
        s.set_dbl(p, c"OfxParamPropMin", 0, -10.0);
        s.set_dbl(p, c"OfxParamPropMax", 0, 10.0);
        let p = define_param(set, c"OfxParamTypeInteger", c"int", c"Int");
        s.set_int(p, c"OfxParamPropDefault", 0, 3);
        s.set_int(p, c"OfxParamPropMin", 0, 0);
        s.set_int(p, c"OfxParamPropMax", 0, 10);
        let p = define_param(set, c"OfxParamTypeBoolean", c"bool", c"Bool");
        s.set_int(p, c"OfxParamPropDefault", 0, 1);
        let p = define_param(set, c"OfxParamTypeChoice", c"choice", c"Choice");
        for (i, o) in [c"First", c"Second", c"Third"].iter().enumerate() {
            s.set_str(p, c"OfxParamPropChoiceOption", i as c_int, o);
        }
        s.set_int(p, c"OfxParamPropDefault", 0, 1);
        let p = define_param(set, c"OfxParamTypeRGBA", c"rgba", c"Colour");
        for (i, v) in [0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
            s.set_dbl(p, c"OfxParamPropDefault", i as c_int, v);
        }
        let p = define_param(set, c"OfxParamTypeDouble2D", c"double2d", c"Double 2D");
        // A position, so the host shows it as a viewer point (the 3D one stays plain: sliders).
        s.set_str(p, c"OfxParamPropDoubleType", 0, c"OfxParamDoubleTypeXYAbsolute");
        for i in 0..2 {
            s.set_dbl(p, c"OfxParamPropDefault", i, 10.0 * (i + 1) as f64);
        }
        let p = define_param(set, c"OfxParamTypeDouble3D", c"double3d", c"Double 3D");
        for i in 0..3 {
            s.set_dbl(p, c"OfxParamPropDefault", i, (i + 1) as f64);
        }
        let p = define_param(set, c"OfxParamTypeString", c"string", c"String");
        s.set_str(p, c"OfxParamPropDefault", 0, c"hello");
        s.set_str(p, c"OfxParamPropStringMode", 0, c"OfxParamStringIsSingleLine");
        let p = define_param(set, c"OfxParamTypeCustom", c"custom", c"Custom");
        s.set_str(p, c"OfxParamPropDefault", 0, c"opaque-data");
        define_param(set, c"OfxParamTypePushButton", c"button", c"Button");
        define_param(set, c"OfxParamTypeGroup", c"group", c"Group");
        let p = define_param(set, c"OfxParamTypeDouble", c"grouped", c"Grouped");
        s.set_dbl(p, c"OfxParamPropDefault", 0, 2.0);
        s.set_str(p, c"OfxParamPropParent", 0, c"group");
        let page = define_param(set, c"OfxParamTypePage", c"page", c"Main");
        let names = [c"double", c"int", c"bool", c"choice", c"rgba", c"double2d", c"double3d", c"string", c"custom", c"button", c"group"];
        for (i, n) in names.iter().enumerate() {
            s.set_str(page, c"OfxParamPropPageChild", i as c_int, n);
        }
    }
}

// ---- images ----------------------------------------------------------------------------------

/// A float RGBA image fetched from a clip; released on drop.
struct Img {
    props: Props,
    data: *mut u8,
    row_bytes: isize,
    /// Pixel bounds `[x1, y1, x2, y2)`, y up.
    bounds: [i32; 4],
}

impl Img {
    unsafe fn fetch(clip: Clip, time: f64) -> Option<Img> {
        let s = suites();
        unsafe {
            let mut props = null_mut();
            if (s.fx.clip_get_image)(clip, time, null(), &mut props) != OK || props.is_null() {
                return None;
            }
            let data = s.get_ptr(props, c"OfxImagePropData").unwrap_or(null_mut()) as *mut u8;
            let row_bytes = s.get_int(props, c"OfxImagePropRowBytes", 0).unwrap_or(0) as isize;
            let mut bounds = [0; 4];
            for (i, b) in bounds.iter_mut().enumerate() {
                *b = s.get_int(props, c"OfxImagePropBounds", i as c_int).unwrap_or(0);
            }
            // Dropping releases the image, also when there is no pixel data.
            let img = Img { props, data, row_bytes, bounds };
            if data.is_null() { None } else { Some(img) }
        }
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.bounds[0] && x < self.bounds[2] && y >= self.bounds[1] && y < self.bounds[3]
    }

    /// Pixel address: `data + (y - y1) * rowBytes + (x - x1) * 16`, valid for negative row bytes.
    fn ptr(&self, x: i32, y: i32) -> *mut [f32; 4] {
        let off = (y - self.bounds[1]) as isize * self.row_bytes + (x - self.bounds[0]) as isize * 16;
        unsafe { self.data.offset(off) as *mut [f32; 4] }
    }

    /// Transparent black outside the bounds.
    fn get(&self, x: i32, y: i32) -> [f32; 4] {
        if self.contains(x, y) { unsafe { self.ptr(x, y).read_unaligned() } } else { [0.0; 4] }
    }

    fn set(&self, x: i32, y: i32, v: [f32; 4]) {
        if self.contains(x, y) {
            unsafe { self.ptr(x, y).write_unaligned(v) }
        }
    }
}

impl Drop for Img {
    fn drop(&mut self) {
        unsafe { (suites().fx.clip_release_image)(self.props) };
    }
}

fn at(img: &Option<Img>, x: i32, y: i32) -> [f32; 4] {
    img.as_ref().map_or([0.0; 4], |i| i.get(x, y))
}

unsafe fn clip(effect: Effect, name: &CStr) -> Clip {
    let mut c = null_mut();
    unsafe { (suites().fx.clip_get_handle)(effect, name.as_ptr(), &mut c, null_mut()) };
    c
}

unsafe fn param(effect: Effect, name: &CStr) -> Param {
    let s = suites();
    let (mut set, mut p) = (null_mut(), null_mut());
    unsafe {
        (s.fx.get_param_set)(effect, &mut set);
        (s.param.get_handle)(set, name.as_ptr(), &mut p, null_mut());
    }
    p
}

unsafe fn double_param(effect: Effect, name: &CStr, time: f64) -> f64 {
    let (p, mut v) = (unsafe { param(effect, name) }, 0.0f64);
    unsafe { (suites().param.get_value_at_time)(p, time, &mut v as *mut f64) };
    v
}

unsafe fn int_param(effect: Effect, name: &CStr, time: f64) -> i32 {
    let (p, mut v) = (unsafe { param(effect, name) }, 0 as c_int);
    unsafe { (suites().param.get_value_at_time)(p, time, &mut v as *mut c_int) };
    v
}

// ---- region of definition and render ---------------------------------------------------------

unsafe fn border_rod(effect: Effect, in_args: Props, out_args: Props) -> Status {
    let s = suites();
    unsafe {
        let time = s.get_dbl(in_args, c"OfxPropTime", 0).unwrap_or(0.0);
        let mut r = RectD::default();
        if (s.fx.clip_get_region_of_definition)(clip(effect, c"Source"), time, &mut r) != OK {
            return REPLY_DEFAULT;
        }
        let n = int_param(effect, c"size", time) as f64;
        for (i, v) in [r.x1 - n, r.y1 - n, r.x2 + n, r.y2 + n].into_iter().enumerate() {
            s.set_dbl(out_args, c"OfxImageEffectPropRegionOfDefinition", i as c_int, v);
        }
    }
    OK
}

unsafe fn render(k: usize, effect: Effect, in_args: Props) -> Status {
    let s = suites();
    unsafe {
        let time = s.get_dbl(in_args, c"OfxPropTime", 0).unwrap_or(0.0);
        let mut win = [0; 4];
        for (i, w) in win.iter_mut().enumerate() {
            *w = s.get_int(in_args, c"OfxImageEffectPropRenderWindow", i as c_int).unwrap_or(0);
        }
        let scale = [0, 1].map(|i| s.get_dbl(in_args, c"OfxImageEffectPropRenderScale", i).unwrap_or(1.0));
        let Some(out) = Img::fetch(clip(effect, c"Output"), time) else { return FAILED };
        let src_clip = clip(effect, c"Source");
        let src = Img::fetch(src_clip, time);

        // The per-pixel function of each effect, in output pixel coordinates (y up).
        let pixel: Box<dyn Fn(i32, i32) -> [f32; 4]> = match k {
            INVERT => {
                let amount = double_param(effect, c"amount", time) as f32;
                Box::new(move |x, y| {
                    let c = at(&src, x, y);
                    // Premultiplied inversion (alpha - colour), mixed in by `amount`.
                    let inv = [c[3] - c[0], c[3] - c[1], c[3] - c[2], c[3]];
                    std::array::from_fn(|i| c[i] + amount * (inv[i] - c[i]))
                })
            }
            ECHO => {
                // The same clip one frame earlier (needs temporal clip access).
                let prev = Img::fetch(src_clip, time - 1.0);
                Box::new(move |x, y| {
                    let a = at(&src, x, y);
                    let b = if prev.is_some() { at(&prev, x, y) } else { a };
                    std::array::from_fn(|i| 0.5 * (a[i] + b[i]))
                })
            }
            BORDER => {
                // The source's region of definition in pixels; everything outside it is red.
                let mut r = RectD::default();
                (s.fx.clip_get_region_of_definition)(src_clip, time, &mut r);
                let rect = [(r.x1 * scale[0]).floor() as i32, (r.y1 * scale[1]).floor() as i32, (r.x2 * scale[0]).ceil() as i32, (r.y2 * scale[1]).ceil() as i32];
                Box::new(move |x, y| {
                    if x >= rect[0] && x < rect[2] && y >= rect[1] && y < rect[3] { at(&src, x, y) } else { [1.0, 0.0, 0.0, 1.0] }
                })
            }
            MIX => {
                let matte_clip = clip(effect, c"Matte");
                let matte = if matte_clip.is_null() { None } else { Img::fetch(matte_clip, time) };
                Box::new(move |x, y| {
                    let a = if matte.is_some() { at(&matte, x, y)[3] } else { 1.0 };
                    at(&src, x, y).map(|v| v * a)
                })
            }
            _ => Box::new(move |x, y| at(&src, x, y)),
        };

        for y in win[1].max(out.bounds[1])..win[3].min(out.bounds[3]) {
            for x in win[0].max(out.bounds[0])..win[2].min(out.bounds[2]) {
                out.set(x, y, pixel(x, y));
            }
        }
    }
    OK
}
