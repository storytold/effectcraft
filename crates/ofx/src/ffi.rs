//! The OpenFX C ABI: status codes, structs, suite tables and property / action names.
//!
//! Transcribed by hand from the OpenFX 1.4/1.5 public headers (`ofxCore.h`, `ofxProperty.h`,
//! `ofxParam.h`, `ofxImageEffect.h`, `ofxMemory.h`, `ofxMultiThread.h`, `ofxMessage.h`,
//! `ofxInteract.h`, `ofxProgress.h`, `ofxTimeLine.h`, `ofxPixels.h`), which are
//! Copyright OpenFX and contributors to the OpenFX project, licensed BSD-3-Clause
//! (<https://github.com/AcademySoftwareFoundation/openfx>). Only the interface (names, layouts and
//! numbers the specification fixes) is reproduced; no host implementation code is copied from
//! any other host.
//!
//! The `ATTRIBUTION.md` entry for this file is: *OpenFX API headers, BSD-3-Clause, The Open
//! Effects Association / Academy Software Foundation.*
//!
//! ## Variadic suite functions
//!
//! `paramGetValue`, `paramSetValue`, `paramGetValueAtTime`, `paramSetValueAtTime`,
//! `paramGetDerivative`, `paramGetIntegral` and the message suite's printf-style functions are C
//! variadic, and stable Rust cannot *define* a variadic function. They are declared here as
//! ordinary functions that read the argument slots the C caller filled: on Windows x64 every
//! variadic argument occupies one 8-byte slot (doubles are duplicated into the integer
//! register), on the System V ABIs (Linux, macOS x86-64) integers and doubles are taken from
//! separate register files. Both are modelled by [`VarArgs`]. Apple Silicon passes variadic
//! arguments on the stack, which this does not model, so the parameter suite is not available
//! there. The message functions are declared with extra slots too ([`VarMessageFn`]) and the host
//! expands the common printf conversions itself; on Apple Silicon they only read their fixed
//! arguments (the format string is logged with its conversions unexpanded).

use std::ffi::{CString, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// `OfxStatus`.
pub type OfxStatus = c_int;
/// Opaque handles are all passed around as `void*`; the host validates them with a magic number.
pub type Handle = *mut c_void;

pub const K_STAT_OK: OfxStatus = 0;
pub const K_STAT_FAILED: OfxStatus = 1;
pub const K_STAT_ERR_FATAL: OfxStatus = 2;
pub const K_STAT_ERR_UNKNOWN: OfxStatus = 3;
pub const K_STAT_ERR_MISSING_HOST_FEATURE: OfxStatus = 4;
pub const K_STAT_ERR_UNSUPPORTED: OfxStatus = 5;
pub const K_STAT_ERR_EXISTS: OfxStatus = 6;
pub const K_STAT_ERR_FORMAT: OfxStatus = 7;
pub const K_STAT_ERR_MEMORY: OfxStatus = 8;
pub const K_STAT_ERR_BAD_HANDLE: OfxStatus = 9;
pub const K_STAT_ERR_BAD_INDEX: OfxStatus = 10;
pub const K_STAT_ERR_VALUE: OfxStatus = 11;
pub const K_STAT_REPLY_YES: OfxStatus = 12;
pub const K_STAT_REPLY_NO: OfxStatus = 13;
pub const K_STAT_REPLY_DEFAULT: OfxStatus = 14;
pub const K_STAT_UNLICENSED: OfxStatus = 15;
pub const K_STAT_ERR_IMAGE_FORMAT: OfxStatus = 1000;

/// `OfxRangeD`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct OfxRangeD {
    pub min: f64,
    pub max: f64,
}
/// `OfxRectI`: x1/y1 inclusive, x2/y2 exclusive, y up.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OfxRectI {
    pub x1: c_int,
    pub y1: c_int,
    pub x2: c_int,
    pub y2: c_int,
}
/// `OfxRectD` (canonical coordinates).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OfxRectD {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

/// `OfxHost`.
#[repr(C)]
pub struct OfxHost {
    pub host: Handle,
    pub fetch_suite: extern "C" fn(host: Handle, name: *const c_char, version: c_int) -> *const c_void,
}
// The table is built once and only read afterwards.
unsafe impl Sync for OfxHost {}
unsafe impl Send for OfxHost {}

/// `OfxPluginEntryPoint`.
pub type EntryPoint = unsafe extern "C" fn(action: *const c_char, handle: *const c_void, in_args: Handle, out_args: Handle) -> OfxStatus;

/// `OfxPlugin`.
#[repr(C)]
pub struct OfxPlugin {
    pub plugin_api: *const c_char,
    pub api_version: c_int,
    pub plugin_identifier: *const c_char,
    pub plugin_version_major: u32,
    pub plugin_version_minor: u32,
    pub set_host: Option<unsafe extern "C" fn(host: *mut OfxHost)>,
    pub main_entry: Option<EntryPoint>,
}

/// `OfxPropertySuiteV1`.
#[repr(C)]
pub struct OfxPropertySuiteV1 {
    pub prop_set_pointer: extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> OfxStatus,
    pub prop_set_string: extern "C" fn(Handle, *const c_char, c_int, *const c_char) -> OfxStatus,
    pub prop_set_double: extern "C" fn(Handle, *const c_char, c_int, f64) -> OfxStatus,
    pub prop_set_int: extern "C" fn(Handle, *const c_char, c_int, c_int) -> OfxStatus,
    pub prop_set_pointer_n: extern "C" fn(Handle, *const c_char, c_int, *const *mut c_void) -> OfxStatus,
    pub prop_set_string_n: extern "C" fn(Handle, *const c_char, c_int, *const *const c_char) -> OfxStatus,
    pub prop_set_double_n: extern "C" fn(Handle, *const c_char, c_int, *const f64) -> OfxStatus,
    pub prop_set_int_n: extern "C" fn(Handle, *const c_char, c_int, *const c_int) -> OfxStatus,
    pub prop_get_pointer: extern "C" fn(Handle, *const c_char, c_int, *mut *mut c_void) -> OfxStatus,
    pub prop_get_string: extern "C" fn(Handle, *const c_char, c_int, *mut *const c_char) -> OfxStatus,
    pub prop_get_double: extern "C" fn(Handle, *const c_char, c_int, *mut f64) -> OfxStatus,
    pub prop_get_int: extern "C" fn(Handle, *const c_char, c_int, *mut c_int) -> OfxStatus,
    pub prop_get_pointer_n: extern "C" fn(Handle, *const c_char, c_int, *mut *mut c_void) -> OfxStatus,
    pub prop_get_string_n: extern "C" fn(Handle, *const c_char, c_int, *mut *const c_char) -> OfxStatus,
    pub prop_get_double_n: extern "C" fn(Handle, *const c_char, c_int, *mut f64) -> OfxStatus,
    pub prop_get_int_n: extern "C" fn(Handle, *const c_char, c_int, *mut c_int) -> OfxStatus,
    pub prop_reset: extern "C" fn(Handle, *const c_char) -> OfxStatus,
    pub prop_get_dimension: extern "C" fn(Handle, *const c_char, *mut c_int) -> OfxStatus,
}

/// The shape every variadic parameter-suite function is declared with (see the module docs).
#[cfg(windows)]
pub type VarParamFn = extern "C" fn(Handle, u64, u64, u64, u64, u64, u64) -> OfxStatus;
/// The shape every variadic parameter-suite function is declared with (see the module docs).
#[cfg(not(windows))]
pub type VarParamFn = extern "C" fn(Handle, u64, u64, u64, u64, u64, f64, f64, f64, f64, f64) -> OfxStatus;
/// [`VarParamFn`] for the functions taking a fixed `OfxTime` first: on Windows x64 that double
/// arrives only in its XMM register, so it must be declared as one.
#[cfg(windows)]
pub type VarParamTimeFn = extern "C" fn(Handle, f64, u64, u64, u64, u64, u64) -> OfxStatus;
/// [`VarParamFn`] for the functions taking a fixed `OfxTime` first (System V reads it from the
/// first float register already).
#[cfg(not(windows))]
pub type VarParamTimeFn = VarParamFn;
/// [`VarParamFn`] for `paramGetIntegral`, which has two fixed `OfxTime`s (`time1`, `time2`) before
/// the variadic out-pointers. On Windows x64 argument positions are fixed: `time1` is the second
/// argument (XMM1), `time2` the third (XMM2) and the first out-pointer the fourth (R9), the rest on
/// the stack, so both doubles must be declared as `f64` and four integer slots remain (enough for
/// the four pointers of an RGBA parameter).
#[cfg(windows)]
pub type VarParamTime2Fn = extern "C" fn(Handle, f64, f64, u64, u64, u64, u64) -> OfxStatus;
/// [`VarParamFn`] for `paramGetIntegral` (System V passes the two fixed doubles in the first two
/// float registers, exactly where [`VarParamFn`]'s `f1`, `f2` are read, and the out-pointers in
/// the integer registers after the handle).
#[cfg(not(windows))]
pub type VarParamTime2Fn = VarParamFn;

/// `OfxParameterSuiteV1`.
#[repr(C)]
pub struct OfxParameterSuiteV1 {
    pub param_define: extern "C" fn(Handle, *const c_char, *const c_char, *mut Handle) -> OfxStatus,
    pub param_get_handle: extern "C" fn(Handle, *const c_char, *mut Handle, *mut Handle) -> OfxStatus,
    pub param_set_get_property_set: extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub param_get_property_set: extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub param_get_value: VarParamFn,
    pub param_get_value_at_time: VarParamTimeFn,
    pub param_get_derivative: VarParamTimeFn,
    pub param_get_integral: VarParamTime2Fn,
    pub param_set_value: VarParamFn,
    pub param_set_value_at_time: VarParamTimeFn,
    pub param_get_num_keys: extern "C" fn(Handle, *mut u32) -> OfxStatus,
    pub param_get_key_time: extern "C" fn(Handle, u32, *mut f64) -> OfxStatus,
    pub param_get_key_index: extern "C" fn(Handle, f64, c_int, *mut c_int) -> OfxStatus,
    pub param_delete_key: extern "C" fn(Handle, f64) -> OfxStatus,
    pub param_delete_all_keys: extern "C" fn(Handle) -> OfxStatus,
    pub param_copy: extern "C" fn(Handle, Handle, f64, *const OfxRangeD) -> OfxStatus,
    pub param_edit_begin: extern "C" fn(Handle, *const c_char) -> OfxStatus,
    pub param_edit_end: extern "C" fn(Handle) -> OfxStatus,
}

/// `OfxImageEffectSuiteV1`.
#[repr(C)]
pub struct OfxImageEffectSuiteV1 {
    pub get_property_set: extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub get_param_set: extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub clip_define: extern "C" fn(Handle, *const c_char, *mut Handle) -> OfxStatus,
    pub clip_get_handle: extern "C" fn(Handle, *const c_char, *mut Handle, *mut Handle) -> OfxStatus,
    pub clip_get_property_set: extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub clip_get_image: extern "C" fn(Handle, f64, *const OfxRectD, *mut Handle) -> OfxStatus,
    pub clip_release_image: extern "C" fn(Handle) -> OfxStatus,
    pub clip_get_region_of_definition: extern "C" fn(Handle, f64, *mut OfxRectD) -> OfxStatus,
    pub abort: extern "C" fn(Handle) -> c_int,
    pub image_memory_alloc: extern "C" fn(Handle, usize, *mut Handle) -> OfxStatus,
    pub image_memory_free: extern "C" fn(Handle) -> OfxStatus,
    pub image_memory_lock: extern "C" fn(Handle, *mut *mut c_void) -> OfxStatus,
    pub image_memory_unlock: extern "C" fn(Handle) -> OfxStatus,
}

/// `OfxMemorySuiteV1`.
#[repr(C)]
pub struct OfxMemorySuiteV1 {
    pub memory_alloc: extern "C" fn(Handle, usize, *mut *mut c_void) -> OfxStatus,
    pub memory_free: extern "C" fn(*mut c_void) -> OfxStatus,
}

/// `OfxThreadFunctionV1`.
pub type ThreadFn = unsafe extern "C" fn(thread_index: u32, thread_max: u32, custom_arg: *mut c_void);

/// `OfxMultiThreadSuiteV1`.
#[repr(C)]
pub struct OfxMultiThreadSuiteV1 {
    pub multi_thread: extern "C" fn(ThreadFn, u32, *mut c_void) -> OfxStatus,
    pub multi_thread_num_cpus: extern "C" fn(*mut u32) -> OfxStatus,
    pub multi_thread_index: extern "C" fn(*mut u32) -> OfxStatus,
    pub multi_thread_is_spawned_thread: extern "C" fn() -> c_int,
    pub mutex_create: extern "C" fn(*mut Handle, c_int) -> OfxStatus,
    pub mutex_destroy: extern "C" fn(Handle) -> OfxStatus,
    pub mutex_lock: extern "C" fn(Handle) -> OfxStatus,
    pub mutex_unlock: extern "C" fn(Handle) -> OfxStatus,
    pub mutex_try_lock: extern "C" fn(Handle) -> OfxStatus,
}

/// The shape of the printf-style message functions (`message`, `setPersistentMessage`): the four
/// fixed arguments (handle, message type, message id, format) followed by the slots the variadic
/// arguments are read from. Windows x64: eight 8-byte stack slots (doubles as their bit pattern).
#[cfg(windows)]
pub type VarMessageFn = extern "C" fn(Handle, *const c_char, *const c_char, *const c_char, u64, u64, u64, u64, u64, u64, u64, u64) -> OfxStatus;
/// The shape of the printf-style message functions. System V (and AAPCS64 outside Apple): after the
/// four fixed integer registers, integer arguments continue in the next two registers and then on
/// the stack, doubles come from the first eight float registers.
#[cfg(all(not(windows), not(all(target_vendor = "apple", target_arch = "aarch64"))))]
pub type VarMessageFn =
    extern "C" fn(Handle, *const c_char, *const c_char, *const c_char, u64, u64, u64, u64, u64, u64, f64, f64, f64, f64, f64, f64, f64, f64) -> OfxStatus;
/// The shape of the printf-style message functions. Apple Silicon passes variadic arguments on the
/// stack in a way the fixed-slot trick does not model, so only the fixed arguments are declared.
#[cfg(all(not(windows), target_vendor = "apple", target_arch = "aarch64"))]
pub type VarMessageFn = extern "C" fn(Handle, *const c_char, *const c_char, *const c_char) -> OfxStatus;

/// `OfxMessageSuiteV1`. The printf arguments are read from the declared slots (see
/// [`VarMessageFn`]).
#[repr(C)]
pub struct OfxMessageSuiteV1 {
    pub message: VarMessageFn,
}

/// `OfxMessageSuiteV2`.
#[repr(C)]
pub struct OfxMessageSuiteV2 {
    pub message: VarMessageFn,
    pub set_persistent_message: VarMessageFn,
    pub clear_persistent_message: extern "C" fn(Handle) -> OfxStatus,
}

/// `OfxInteractSuiteV1` (overlays are not supported; every call reports that).
#[repr(C)]
pub struct OfxInteractSuiteV1 {
    pub interact_swap_buffers: extern "C" fn(Handle) -> OfxStatus,
    pub interact_redraw: extern "C" fn(Handle) -> OfxStatus,
    pub interact_get_property_set: extern "C" fn(Handle, *mut Handle) -> OfxStatus,
}

/// `OfxProgressSuiteV1` / `V2` (identical layout).
#[repr(C)]
pub struct OfxProgressSuite {
    pub progress_start: extern "C" fn(Handle, *const c_char, *const c_char) -> OfxStatus,
    pub progress_update: extern "C" fn(Handle, f64) -> OfxStatus,
    pub progress_end: extern "C" fn(Handle) -> OfxStatus,
}

/// `OfxTimeLineSuiteV1`.
#[repr(C)]
pub struct OfxTimeLineSuiteV1 {
    pub get_time: extern "C" fn(Handle, *mut f64) -> OfxStatus,
    pub goto_time: extern "C" fn(Handle, f64) -> OfxStatus,
    pub get_time_bounds: extern "C" fn(Handle, *mut f64, *mut f64) -> OfxStatus,
}

/// Run a callback that is about to return into plug-in code: a panic must never unwind across
/// the C boundary, so it becomes `kOfxStatErrFatal` (and is logged).
pub fn guard(f: impl FnOnce() -> OfxStatus) -> OfxStatus {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(s) => s,
        Err(_) => {
            log::error!("openfx: internal panic inside a host callback (reported to the plug-in as kOfxStatErrFatal)");
            K_STAT_ERR_FATAL
        }
    }
}

/// [`guard`] for callbacks that return a plain `int`.
pub fn guard_int(f: impl FnOnce() -> c_int, fallback: c_int) -> c_int {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

/// A `CString` from any text (interior NULs are dropped).
pub fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// Read a C string argument; `None` for null or invalid UTF-8.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated string.
pub unsafe fn str_arg<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    unsafe { std::ffi::CStr::from_ptr(p) }.to_str().ok()
}

/// The variadic arguments of a suite function, as captured from the declared slots.
pub struct VarArgs {
    #[cfg(windows)]
    slots: [u64; 6],
    #[cfg(windows)]
    next: usize,
    #[cfg(not(windows))]
    ints: [u64; 5],
    #[cfg(not(windows))]
    floats: [f64; 5],
    #[cfg(not(windows))]
    ni: usize,
    #[cfg(not(windows))]
    nf: usize,
}

impl VarArgs {
    /// Windows x64: one slot per argument, integers and doubles alike.
    #[cfg(windows)]
    pub fn new(slots: [u64; 6]) -> VarArgs {
        VarArgs { slots, next: 0 }
    }
    /// System V: integer-class and double arguments come from separate register files.
    #[cfg(not(windows))]
    pub fn new(ints: [u64; 5], floats: [f64; 5]) -> VarArgs {
        VarArgs { ints, floats, ni: 0, nf: 0 }
    }
    /// The next integer-class argument (pointers, `int`s) as raw bits.
    pub fn next_u64(&mut self) -> u64 {
        #[cfg(windows)]
        {
            let v = self.slots.get(self.next).copied().unwrap_or(0);
            self.next += 1;
            v
        }
        #[cfg(not(windows))]
        {
            let v = self.ints.get(self.ni).copied().unwrap_or(0);
            self.ni += 1;
            v
        }
    }
    /// The next `int` argument.
    pub fn next_i32(&mut self) -> i32 {
        self.next_u64() as u32 as i32
    }
    /// The next pointer argument.
    pub fn next_ptr<T>(&mut self) -> *mut T {
        self.next_u64() as usize as *mut T
    }
    /// The next `double` argument.
    pub fn next_f64(&mut self) -> f64 {
        #[cfg(windows)]
        {
            f64::from_bits(self.next_u64())
        }
        #[cfg(not(windows))]
        {
            let v = self.floats.get(self.nf).copied().unwrap_or(0.0);
            self.nf += 1;
            v
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Names. Actions.
// ---------------------------------------------------------------------------------------------

pub const ACT_LOAD: &str = "OfxActionLoad";
pub const ACT_DESCRIBE: &str = "OfxActionDescribe";
pub const ACT_UNLOAD: &str = "OfxActionUnload";
pub const ACT_CREATE_INSTANCE: &str = "OfxActionCreateInstance";
pub const ACT_DESTROY_INSTANCE: &str = "OfxActionDestroyInstance";
pub const ACT_INSTANCE_CHANGED: &str = "OfxActionInstanceChanged";
pub const ACT_BEGIN_INSTANCE_CHANGED: &str = "OfxActionBeginInstanceChanged";
pub const ACT_END_INSTANCE_CHANGED: &str = "OfxActionEndInstanceChanged";
pub const ACT_PURGE_CACHES: &str = "OfxActionPurgeCaches";
pub const ACT_GET_ROD: &str = "OfxImageEffectActionGetRegionOfDefinition";
pub const ACT_GET_ROI: &str = "OfxImageEffectActionGetRegionsOfInterest";
pub const ACT_GET_FRAMES_NEEDED: &str = "OfxImageEffectActionGetFramesNeeded";
pub const ACT_GET_CLIP_PREFS: &str = "OfxImageEffectActionGetClipPreferences";
pub const ACT_IS_IDENTITY: &str = "OfxImageEffectActionIsIdentity";
pub const ACT_RENDER: &str = "OfxImageEffectActionRender";
pub const ACT_BEGIN_SEQ: &str = "OfxImageEffectActionBeginSequenceRender";
pub const ACT_END_SEQ: &str = "OfxImageEffectActionEndSequenceRender";
pub const ACT_DESCRIBE_IN_CONTEXT: &str = "OfxImageEffectActionDescribeInContext";

// Suites.
pub const SUITE_PROPERTY: &str = "OfxPropertySuite";
pub const SUITE_PARAMETER: &str = "OfxParameterSuite";
pub const SUITE_IMAGE_EFFECT: &str = "OfxImageEffectSuite";
pub const SUITE_MEMORY: &str = "OfxMemorySuite";
pub const SUITE_MULTI_THREAD: &str = "OfxMultiThreadSuite";
pub const SUITE_MESSAGE: &str = "OfxMessageSuite";
pub const SUITE_INTERACT: &str = "OfxInteractSuite";
pub const SUITE_PROGRESS: &str = "OfxProgressSuite";
pub const SUITE_TIMELINE: &str = "OfxTimeLineSuite";

// Core properties.
pub const P_API_VERSION: &str = "OfxPropAPIVersion";
pub const P_TIME: &str = "OfxPropTime";
pub const P_IS_INTERACTIVE: &str = "OfxPropIsInteractive";
pub const P_INSTANCE_DATA: &str = "OfxPropInstanceData";
pub const P_TYPE: &str = "OfxPropType";
pub const P_NAME: &str = "OfxPropName";
pub const P_VERSION: &str = "OfxPropVersion";
pub const P_VERSION_LABEL: &str = "OfxPropVersionLabel";
pub const P_PLUGIN_DESCRIPTION: &str = "OfxPropPluginDescription";
pub const P_LABEL: &str = "OfxPropLabel";
pub const P_SHORT_LABEL: &str = "OfxPropShortLabel";
pub const P_LONG_LABEL: &str = "OfxPropLongLabel";
pub const P_CHANGE_REASON: &str = "OfxPropChangeReason";
pub const P_EFFECT_INSTANCE: &str = "OfxPropEffectInstance";
pub const P_PLUGIN_FILE_PATH: &str = "OfxPluginPropFilePath";
pub const CHANGE_USER_EDITED: &str = "OfxChangeUserEdited";
pub const CHANGE_PLUGIN_EDITED: &str = "OfxChangePluginEdited";
pub const CHANGE_TIME: &str = "OfxChangeTime";

// Bit depths and components.
pub const DEPTH_NONE: &str = "OfxBitDepthNone";
pub const DEPTH_BYTE: &str = "OfxBitDepthByte";
pub const DEPTH_SHORT: &str = "OfxBitDepthShort";
pub const DEPTH_HALF: &str = "OfxBitDepthHalf";
pub const DEPTH_FLOAT: &str = "OfxBitDepthFloat";
pub const COMP_NONE: &str = "OfxImageComponentNone";
pub const COMP_RGBA: &str = "OfxImageComponentRGBA";
pub const COMP_RGB: &str = "OfxImageComponentRGB";
pub const COMP_ALPHA: &str = "OfxImageComponentAlpha";

// Image-effect contexts, types, names.
pub const IMAGE_EFFECT_API: &str = "OfxImageEffectPluginAPI";
pub const CTX_GENERATOR: &str = "OfxImageEffectContextGenerator";
pub const CTX_FILTER: &str = "OfxImageEffectContextFilter";
pub const CTX_TRANSITION: &str = "OfxImageEffectContextTransition";
pub const CTX_PAINT: &str = "OfxImageEffectContextPaint";
pub const CTX_GENERAL: &str = "OfxImageEffectContextGeneral";
pub const CTX_RETIMER: &str = "OfxImageEffectContextRetimer";
pub const TYPE_IMAGE_EFFECT_HOST: &str = "OfxTypeImageEffectHost";
pub const TYPE_IMAGE_EFFECT: &str = "OfxTypeImageEffect";
pub const TYPE_IMAGE_EFFECT_INSTANCE: &str = "OfxTypeImageEffectInstance";
pub const TYPE_CLIP: &str = "OfxTypeClip";
pub const TYPE_IMAGE: &str = "OfxTypeImage";
pub const TYPE_PARAMETER: &str = "OfxTypeParameter";
pub const TYPE_PARAMETER_INSTANCE: &str = "OfxTypeParameterInstance";
pub const CLIP_OUTPUT: &str = "Output";
pub const CLIP_SOURCE: &str = "Source";
pub const CLIP_SOURCE_FROM: &str = "SourceFrom";
pub const CLIP_SOURCE_TO: &str = "SourceTo";

// Image-effect properties.
pub const IE_SUPPORTED_CONTEXTS: &str = "OfxImageEffectPropSupportedContexts";
pub const IE_PLUGIN_HANDLE: &str = "OfxImageEffectPropPluginHandle";
pub const IE_HOST_IS_BACKGROUND: &str = "OfxImageEffectHostPropIsBackground";
pub const IE_SINGLE_INSTANCE: &str = "OfxImageEffectPluginPropSingleInstance";
pub const IE_RENDER_THREAD_SAFETY: &str = "OfxImageEffectPluginRenderThreadSafety";
pub const IE_RENDER_UNSAFE: &str = "OfxImageEffectRenderUnsafe";
pub const IE_RENDER_INSTANCE_SAFE: &str = "OfxImageEffectRenderInstanceSafe";
pub const IE_RENDER_FULLY_SAFE: &str = "OfxImageEffectRenderFullySafe";
pub const IE_HOST_FRAME_THREADING: &str = "OfxImageEffectPluginPropHostFrameThreading";
pub const IE_MULTIPLE_CLIP_DEPTHS: &str = "OfxImageEffectPropMultipleClipDepths";
pub const IE_MULTIPLE_CLIP_PARS: &str = "OfxImageEffectPropSupportsMultipleClipPARs";
pub const IE_CLIP_PREFS_SLAVE_PARAM: &str = "OfxImageEffectPropClipPreferencesSlaveParam";
pub const IE_SETABLE_FRAME_RATE: &str = "OfxImageEffectPropSetableFrameRate";
pub const IE_SETABLE_FIELDING: &str = "OfxImageEffectPropSetableFielding";
pub const IE_SEQUENTIAL_RENDER: &str = "OfxImageEffectInstancePropSequentialRender";
pub const IE_SEQUENTIAL_RENDER_STATUS: &str = "OfxImageEffectPropSequentialRenderStatus";
pub const IE_INTERACTIVE_RENDER_STATUS: &str = "OfxImageEffectPropInteractiveRenderStatus";
pub const IE_NATIVE_ORIGIN: &str = "OfxImageEffectHostPropNativeOrigin";
pub const IE_NATIVE_ORIGIN_BOTTOM_LEFT: &str = "kOfxImageEffectHostPropNativeOriginBottomLeft";
pub const IE_GROUPING: &str = "OfxImageEffectPluginPropGrouping";
pub const IE_OBSOLETE: &str = "OfxImageEffectPluginPropObsolete";
pub const IE_SUPPORTS_OVERLAYS: &str = "OfxImageEffectPropSupportsOverlays";
pub const IE_OVERLAY_INTERACT_V1: &str = "OfxImageEffectPluginPropOverlayInteractV1";
pub const IE_SUPPORTS_MULTI_RESOLUTION: &str = "OfxImageEffectPropSupportsMultiResolution";
pub const IE_SUPPORTS_TILES: &str = "OfxImageEffectPropSupportsTiles";
pub const IE_TEMPORAL_CLIP_ACCESS: &str = "OfxImageEffectPropTemporalClipAccess";
pub const IE_CONTEXT: &str = "OfxImageEffectPropContext";
pub const IE_PIXEL_DEPTH: &str = "OfxImageEffectPropPixelDepth";
pub const IE_COMPONENTS: &str = "OfxImageEffectPropComponents";
pub const IE_PREMULTIPLICATION: &str = "OfxImageEffectPropPreMultiplication";
pub const IMAGE_OPAQUE: &str = "OfxImageOpaque";
pub const IMAGE_PREMULTIPLIED: &str = "OfxImageAlphaPremultiplied";
pub const IMAGE_UNPREMULTIPLIED: &str = "OfxImageAlphaUnPremultiplied";
pub const IE_SUPPORTED_PIXEL_DEPTHS: &str = "OfxImageEffectPropSupportedPixelDepths";
pub const IE_SUPPORTED_COMPONENTS: &str = "OfxImageEffectPropSupportedComponents";
pub const IE_FRAME_RATE: &str = "OfxImageEffectPropFrameRate";
pub const IE_UNMAPPED_FRAME_RATE: &str = "OfxImageEffectPropUnmappedFrameRate";
pub const IE_FRAME_STEP: &str = "OfxImageEffectPropFrameStep";
pub const IE_FRAME_RANGE: &str = "OfxImageEffectPropFrameRange";
pub const IE_UNMAPPED_FRAME_RANGE: &str = "OfxImageEffectPropUnmappedFrameRange";
pub const IE_FRAME_VARYING: &str = "OfxImageEffectFrameVarying";
pub const IE_RENDER_SCALE: &str = "OfxImageEffectPropRenderScale";
pub const IE_RENDER_QUALITY_DRAFT: &str = "OfxImageEffectPropRenderQualityDraft";
pub const IE_PROJECT_EXTENT: &str = "OfxImageEffectPropProjectExtent";
pub const IE_PROJECT_SIZE: &str = "OfxImageEffectPropProjectSize";
pub const IE_PROJECT_OFFSET: &str = "OfxImageEffectPropProjectOffset";
pub const IE_PROJECT_PAR: &str = "OfxImageEffectPropPixelAspectRatio";
pub const IE_EFFECT_DURATION: &str = "OfxImageEffectInstancePropEffectDuration";
pub const IE_FIELD_TO_RENDER: &str = "OfxImageEffectPropFieldToRender";
pub const IE_ROD: &str = "OfxImageEffectPropRegionOfDefinition";
pub const IE_ROI: &str = "OfxImageEffectPropRegionOfInterest";
pub const IE_RENDER_WINDOW: &str = "OfxImageEffectPropRenderWindow";
pub const IE_OPENGL_RENDER_SUPPORTED: &str = "OfxImageEffectPropOpenGLRenderSupported";
pub const IE_OPENGL_ENABLED: &str = "OfxImageEffectPropOpenGLEnabled";
pub const IE_BEHAVIOUR_WHEN_UNLICENSED: &str = "OfxImageEffectPropBehaviourWhenUnlicensed";
pub const IE_COLOUR_MANAGEMENT_STYLE: &str = "OfxImageEffectPropColourManagementStyle";
pub const IE_THUMBNAIL_RENDER: &str = "OfxImageEffectPropThumbnailRender";
pub const IE_NO_SPATIAL_AWARENESS: &str = "OfxImageEffectPropNoSpatialAwareness";
pub const IE_PARAM_PAGE_ORDER: &str = "OfxPluginPropParamPageOrder";

// Clip / image properties.
pub const CLIP_CONNECTED: &str = "OfxImageClipPropConnected";
pub const CLIP_OPTIONAL: &str = "OfxImageClipPropOptional";
pub const CLIP_IS_MASK: &str = "OfxImageClipPropIsMask";
pub const CLIP_CONTINUOUS_SAMPLES: &str = "OfxImageClipPropContinuousSamples";
pub const CLIP_UNMAPPED_DEPTH: &str = "OfxImageClipPropUnmappedPixelDepth";
pub const CLIP_UNMAPPED_COMPONENTS: &str = "OfxImageClipPropUnmappedComponents";
pub const CLIP_FIELD_ORDER: &str = "OfxImageClipPropFieldOrder";
pub const CLIP_FIELD_EXTRACTION: &str = "OfxImageClipPropFieldExtraction";
pub const CLIP_PREF_COMPONENTS_PREFIX: &str = "OfxImageClipPropComponents_";
pub const CLIP_PREF_DEPTH_PREFIX: &str = "OfxImageClipPropDepth_";
pub const CLIP_PREF_PAR_PREFIX: &str = "OfxImageClipPropPAR_";
pub const CLIP_ROI_PREFIX: &str = "OfxImageClipPropRoI_";
pub const CLIP_FRAMES_NEEDED_PREFIX: &str = "OfxImageClipPropFrameRange_";
pub const IMG_PIXEL_ASPECT_RATIO: &str = "OfxImagePropPixelAspectRatio";
pub const IMG_DATA: &str = "OfxImagePropData";
pub const IMG_BOUNDS: &str = "OfxImagePropBounds";
pub const IMG_ROD: &str = "OfxImagePropRegionOfDefinition";
pub const IMG_ROW_BYTES: &str = "OfxImagePropRowBytes";
pub const IMG_FIELD: &str = "OfxImagePropField";
pub const IMG_UNIQUE_ID: &str = "OfxImagePropUniqueIdentifier";
pub const FIELD_NONE: &str = "OfxFieldNone";

// Parameter types.
pub const PT_INTEGER: &str = "OfxParamTypeInteger";
pub const PT_DOUBLE: &str = "OfxParamTypeDouble";
pub const PT_BOOLEAN: &str = "OfxParamTypeBoolean";
pub const PT_CHOICE: &str = "OfxParamTypeChoice";
pub const PT_STR_CHOICE: &str = "OfxParamTypeStrChoice";
pub const PT_RGBA: &str = "OfxParamTypeRGBA";
pub const PT_RGB: &str = "OfxParamTypeRGB";
pub const PT_DOUBLE_2D: &str = "OfxParamTypeDouble2D";
pub const PT_INTEGER_2D: &str = "OfxParamTypeInteger2D";
pub const PT_DOUBLE_3D: &str = "OfxParamTypeDouble3D";
pub const PT_INTEGER_3D: &str = "OfxParamTypeInteger3D";
pub const PT_STRING: &str = "OfxParamTypeString";
pub const PT_CUSTOM: &str = "OfxParamTypeCustom";
pub const PT_BYTES: &str = "OfxParamTypeBytes";
pub const PT_GROUP: &str = "OfxParamTypeGroup";
pub const PT_PAGE: &str = "OfxParamTypePage";
pub const PT_PUSH_BUTTON: &str = "OfxParamTypePushButton";

// Parameter properties.
pub const PP_TYPE: &str = "OfxParamPropType";
pub const PP_ANIMATES: &str = "OfxParamPropAnimates";
pub const PP_CAN_UNDO: &str = "OfxParamPropCanUndo";
pub const PP_IS_ANIMATING: &str = "OfxParamPropIsAnimating";
pub const PP_PLUGIN_MAY_WRITE: &str = "OfxParamPropPluginMayWrite";
pub const PP_PERSISTANT: &str = "OfxParamPropPersistant";
pub const PP_EVALUATE_ON_CHANGE: &str = "OfxParamPropEvaluateOnChange";
pub const PP_SECRET: &str = "OfxParamPropSecret";
pub const PP_SCRIPT_NAME: &str = "OfxParamPropScriptName";
pub const PP_CACHE_INVALIDATION: &str = "OfxParamPropCacheInvalidation";
pub const PP_HINT: &str = "OfxParamPropHint";
pub const PP_DEFAULT: &str = "OfxParamPropDefault";
pub const PP_COLOUR_MANAGEMENT: &str = "OfxParamPropColourManagement";
pub const PP_DOUBLE_TYPE: &str = "OfxParamPropDoubleType";
pub const PP_DEFAULT_COORDINATE_SYSTEM: &str = "OfxParamPropDefaultCoordinateSystem";
pub const PP_PAGE_CHILD: &str = "OfxParamPropPageChild";
pub const PP_PARENT: &str = "OfxParamPropParent";
pub const PP_GROUP_OPEN: &str = "OfxParamPropGroupOpen";
pub const PP_ENABLED: &str = "OfxParamPropEnabled";
pub const PP_DATA_PTR: &str = "OfxParamPropDataPtr";
pub const PP_CHOICE_OPTION: &str = "OfxParamPropChoiceOption";
pub const PP_CHOICE_ENUM: &str = "OfxParamPropChoiceEnum";
pub const PP_MIN: &str = "OfxParamPropMin";
pub const PP_MAX: &str = "OfxParamPropMax";
pub const PP_DISPLAY_MIN: &str = "OfxParamPropDisplayMin";
pub const PP_DISPLAY_MAX: &str = "OfxParamPropDisplayMax";
pub const PP_INCREMENT: &str = "OfxParamPropIncrement";
pub const PP_DIGITS: &str = "OfxParamPropDigits";
pub const PP_DIMENSION_LABEL: &str = "OfxParamPropDimensionLabel";
pub const PP_STRING_MODE: &str = "OfxParamPropStringMode";
pub const PP_STRING_FILE_PATH_EXISTS: &str = "OfxParamPropStringFilePathExists";
pub const DOUBLE_TYPE_PLAIN: &str = "OfxParamDoubleTypePlain";
pub const DOUBLE_TYPE_ANGLE: &str = "OfxParamDoubleTypeAngle";
pub const DOUBLE_TYPE_XY: &str = "OfxParamDoubleTypeXY";
pub const DOUBLE_TYPE_XY_ABSOLUTE: &str = "OfxParamDoubleTypeXYAbsolute";
pub const COORDS_CANONICAL: &str = "OfxParamCoordinatesCanonical";
pub const COORDS_NORMALISED: &str = "OfxParamCoordinatesNormalised";
pub const STRING_SINGLE_LINE: &str = "OfxParamStringIsSingleLine";
pub const PAGE_SKIP_ROW: &str = "OfxParamPageSkipRow";
pub const PAGE_SKIP_COLUMN: &str = "OfxParamPageSkipColumn";

// Parameter host properties.
pub const PH_SUPPORTS_CUSTOM_ANIMATION: &str = "OfxParamHostPropSupportsCustomAnimation";
pub const PH_SUPPORTS_STRING_ANIMATION: &str = "OfxParamHostPropSupportsStringAnimation";
pub const PH_SUPPORTS_BOOLEAN_ANIMATION: &str = "OfxParamHostPropSupportsBooleanAnimation";
pub const PH_SUPPORTS_CHOICE_ANIMATION: &str = "OfxParamHostPropSupportsChoiceAnimation";
pub const PH_SUPPORTS_STR_CHOICE_ANIMATION: &str = "OfxParamHostPropSupportsStrChoiceAnimation";
pub const PH_SUPPORTS_STR_CHOICE: &str = "OfxParamHostPropSupportsStrChoice";
pub const PH_SUPPORTS_CUSTOM_INTERACT: &str = "OfxParamHostPropSupportsCustomInteract";
pub const PH_MAX_PARAMETERS: &str = "OfxParamHostPropMaxParameters";
pub const PH_MAX_PAGES: &str = "OfxParamHostPropMaxPages";
pub const PH_PAGE_ROW_COLUMN_COUNT: &str = "OfxParamHostPropPageRowColumnCount";

// Messages.
pub const MSG_FATAL: &str = "OfxMessageFatal";
pub const MSG_ERROR: &str = "OfxMessageError";
pub const MSG_WARNING: &str = "OfxMessageWarning";
pub const MSG_MESSAGE: &str = "OfxMessageMessage";
pub const MSG_LOG: &str = "OfxMessageLog";
pub const MSG_QUESTION: &str = "OfxMessageQuestion";
