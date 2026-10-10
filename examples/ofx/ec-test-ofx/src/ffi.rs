//! The minimal slice of the OpenFX C API this plug-in needs (struct layouts and suite member
//! order from the BSD-3 headers ofxCore.h, ofxProperty.h, ofxParam.h and ofxImageEffect.h).

#![allow(dead_code)]

use std::ffi::{CStr, c_char, c_double, c_int, c_uint, c_void};

pub type Status = c_int;
pub type Props = *mut c_void; // OfxPropertySetHandle
pub type Effect = *mut c_void; // OfxImageEffectHandle
pub type ParamSet = *mut c_void;
pub type Param = *mut c_void;
pub type Clip = *mut c_void;

pub const OK: Status = 0;
pub const FAILED: Status = 1;
pub const ERR_UNSUPPORTED: Status = 5;
pub const REPLY_DEFAULT: Status = 14;

#[repr(C)]
pub struct OfxHost {
    pub host: Props,
    pub fetch_suite: unsafe extern "C" fn(Props, *const c_char, c_int) -> *const c_void,
}

pub type EntryPoint = unsafe extern "C" fn(*const c_char, *const c_void, Props, Props) -> Status;

#[repr(C)]
pub struct OfxPlugin {
    pub plugin_api: *const c_char,
    pub api_version: c_int,
    pub plugin_identifier: *const c_char,
    pub version_major: c_uint,
    pub version_minor: c_uint,
    pub set_host: unsafe extern "C" fn(*mut OfxHost),
    pub main_entry: EntryPoint,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RectD {
    pub x1: c_double,
    pub y1: c_double,
    pub x2: c_double,
    pub y2: c_double,
}

#[repr(C)]
pub struct PropertySuite {
    pub set_pointer: unsafe extern "C" fn(Props, *const c_char, c_int, *mut c_void) -> Status,
    pub set_string: unsafe extern "C" fn(Props, *const c_char, c_int, *const c_char) -> Status,
    pub set_double: unsafe extern "C" fn(Props, *const c_char, c_int, c_double) -> Status,
    pub set_int: unsafe extern "C" fn(Props, *const c_char, c_int, c_int) -> Status,
    _set_n: [*const c_void; 4],
    pub get_pointer: unsafe extern "C" fn(Props, *const c_char, c_int, *mut *mut c_void) -> Status,
    pub get_string: unsafe extern "C" fn(Props, *const c_char, c_int, *mut *mut c_char) -> Status,
    pub get_double: unsafe extern "C" fn(Props, *const c_char, c_int, *mut c_double) -> Status,
    pub get_int: unsafe extern "C" fn(Props, *const c_char, c_int, *mut c_int) -> Status,
    _get_n: [*const c_void; 4],
    _reset: *const c_void,
    _get_dimension: *const c_void,
}

#[repr(C)]
pub struct ParameterSuite {
    pub define: unsafe extern "C" fn(ParamSet, *const c_char, *const c_char, *mut Props) -> Status,
    pub get_handle: unsafe extern "C" fn(ParamSet, *const c_char, *mut Param, *mut Props) -> Status,
    _set_get_property_set: *const c_void,
    _get_property_set: *const c_void,
    _get_value: *const c_void,
    pub get_value_at_time: unsafe extern "C" fn(Param, c_double, ...) -> Status,
    _rest: [*const c_void; 12],
}

#[repr(C)]
pub struct ImageEffectSuite {
    pub get_property_set: unsafe extern "C" fn(Effect, *mut Props) -> Status,
    pub get_param_set: unsafe extern "C" fn(Effect, *mut ParamSet) -> Status,
    pub clip_define: unsafe extern "C" fn(Effect, *const c_char, *mut Props) -> Status,
    pub clip_get_handle: unsafe extern "C" fn(Effect, *const c_char, *mut Clip, *mut Props) -> Status,
    _clip_get_property_set: *const c_void,
    pub clip_get_image: unsafe extern "C" fn(Clip, c_double, *const RectD, *mut Props) -> Status,
    pub clip_release_image: unsafe extern "C" fn(Props) -> Status,
    pub clip_get_region_of_definition: unsafe extern "C" fn(Clip, c_double, *mut RectD) -> Status,
    _abort_and_memory: [*const c_void; 5],
}

/// The suites fetched from the host in the Load action.
pub struct Suites {
    pub prop: &'static PropertySuite,
    pub param: &'static ParameterSuite,
    pub fx: &'static ImageEffectSuite,
}

impl Suites {
    /// # Safety
    /// `host` must be the pointer the host passed to `setHost`.
    pub unsafe fn fetch(host: *const OfxHost) -> Option<Suites> {
        unsafe {
            let h = host.as_ref()?;
            let get = |name: &CStr| (h.fetch_suite)(h.host, name.as_ptr(), 1);
            Some(Suites {
                prop: (get(c"OfxPropertySuite") as *const PropertySuite).as_ref()?,
                param: (get(c"OfxParameterSuite") as *const ParameterSuite).as_ref()?,
                fx: (get(c"OfxImageEffectSuite") as *const ImageEffectSuite).as_ref()?,
            })
        }
    }

    // Property helpers (index 0 unless given). Errors are ignored: a missing optional host
    // property just leaves the default.
    pub unsafe fn set_str(&self, p: Props, key: &CStr, i: c_int, v: &CStr) {
        unsafe { (self.prop.set_string)(p, key.as_ptr(), i, v.as_ptr()) };
    }
    pub unsafe fn set_int(&self, p: Props, key: &CStr, i: c_int, v: c_int) {
        unsafe { (self.prop.set_int)(p, key.as_ptr(), i, v) };
    }
    pub unsafe fn set_dbl(&self, p: Props, key: &CStr, i: c_int, v: f64) {
        unsafe { (self.prop.set_double)(p, key.as_ptr(), i, v) };
    }
    pub unsafe fn get_int(&self, p: Props, key: &CStr, i: c_int) -> Option<c_int> {
        let mut v = 0;
        (unsafe { (self.prop.get_int)(p, key.as_ptr(), i, &mut v) } == OK).then_some(v)
    }
    pub unsafe fn get_dbl(&self, p: Props, key: &CStr, i: c_int) -> Option<f64> {
        let mut v = 0.0;
        (unsafe { (self.prop.get_double)(p, key.as_ptr(), i, &mut v) } == OK).then_some(v)
    }
    pub unsafe fn get_ptr(&self, p: Props, key: &CStr) -> Option<*mut c_void> {
        let mut v = std::ptr::null_mut();
        (unsafe { (self.prop.get_pointer)(p, key.as_ptr(), 0, &mut v) } == OK).then_some(v)
    }
    pub unsafe fn get_string(&self, p: Props, key: &CStr) -> Option<String> {
        let mut v: *mut c_char = std::ptr::null_mut();
        if unsafe { (self.prop.get_string)(p, key.as_ptr(), 0, &mut v) } != OK || v.is_null() {
            return None;
        }
        Some(unsafe { CStr::from_ptr(v) }.to_string_lossy().into_owned())
    }
}

// SAFETY: the suites are immutable tables of function pointers owned by the host for the whole
// process lifetime.
unsafe impl Send for Suites {}
unsafe impl Sync for Suites {}
