//! `OfxPropertySuiteV1`: get / set / reset / dimension on any property-set handle.
//!
//! Every property set is a [`PropSet`]; a handle that is not one (wrong magic, null) answers
//! `kOfxStatErrBadHandle`. Values are stored typed but read leniently (an `int` property can be
//! read as `double` and vice versa).

use std::ffi::{c_char, c_int, c_void};

use crate::ffi::*;
use crate::props::{PVal, PropSet, as_double, as_int};

/// Properties this host refuses so the plug-in falls back to its classic behaviour.
fn refused(name: &str) -> bool {
    name == PP_COLOUR_MANAGEMENT || name == IE_COLOUR_MANAGEMENT_STYLE
}

fn with_set(h: Handle, name: *const c_char, f: impl FnOnce(&PropSet, &str) -> OfxStatus) -> OfxStatus {
    guard(|| {
        // SAFETY: the plug-in passes a handle it got from this host and a C string.
        let Some(ps) = (unsafe { PropSet::from_handle(h) }) else { return K_STAT_ERR_BAD_HANDLE };
        let Some(n) = (unsafe { str_arg(name) }) else { return K_STAT_ERR_UNKNOWN };
        f(ps, n)
    })
}

fn set_one(h: Handle, name: *const c_char, index: c_int, v: PVal) -> OfxStatus {
    with_set(h, name, |ps, n| if refused(n) { K_STAT_ERR_UNSUPPORTED } else { ps.set_at(n, index, v) })
}

extern "C" fn prop_set_pointer(h: Handle, name: *const c_char, index: c_int, v: *mut c_void) -> OfxStatus {
    set_one(h, name, index, PVal::Ptr(v as usize))
}
extern "C" fn prop_set_string(h: Handle, name: *const c_char, index: c_int, v: *const c_char) -> OfxStatus {
    // SAFETY: a C string from the plug-in.
    let s = unsafe { str_arg(v) }.unwrap_or("");
    set_one(h, name, index, PVal::text(s))
}
extern "C" fn prop_set_double(h: Handle, name: *const c_char, index: c_int, v: f64) -> OfxStatus {
    set_one(h, name, index, PVal::Double(v))
}
extern "C" fn prop_set_int(h: Handle, name: *const c_char, index: c_int, v: c_int) -> OfxStatus {
    set_one(h, name, index, PVal::Int(v))
}

fn set_many(h: Handle, name: *const c_char, count: c_int, mut get: impl FnMut(usize) -> PVal) -> OfxStatus {
    if !(0..=65536).contains(&count) {
        return K_STAT_ERR_BAD_INDEX;
    }
    with_set(h, name, |ps, n| {
        if refused(n) {
            return K_STAT_ERR_UNSUPPORTED;
        }
        ps.put(n, (0..count as usize).map(&mut get).collect());
        K_STAT_OK
    })
}

extern "C" fn prop_set_pointer_n(h: Handle, name: *const c_char, count: c_int, v: *const *mut c_void) -> OfxStatus {
    if v.is_null() {
        return K_STAT_ERR_VALUE;
    }
    // SAFETY: the plug-in supplies `count` readable elements.
    set_many(h, name, count, |i| PVal::Ptr(unsafe { *v.add(i) } as usize))
}
extern "C" fn prop_set_string_n(h: Handle, name: *const c_char, count: c_int, v: *const *const c_char) -> OfxStatus {
    if v.is_null() {
        return K_STAT_ERR_VALUE;
    }
    // SAFETY: as above; each element is a C string.
    set_many(h, name, count, |i| PVal::text(unsafe { str_arg(*v.add(i)) }.unwrap_or("")))
}
extern "C" fn prop_set_double_n(h: Handle, name: *const c_char, count: c_int, v: *const f64) -> OfxStatus {
    if v.is_null() {
        return K_STAT_ERR_VALUE;
    }
    // SAFETY: as above.
    set_many(h, name, count, |i| PVal::Double(unsafe { *v.add(i) }))
}
extern "C" fn prop_set_int_n(h: Handle, name: *const c_char, count: c_int, v: *const c_int) -> OfxStatus {
    if v.is_null() {
        return K_STAT_ERR_VALUE;
    }
    // SAFETY: as above.
    set_many(h, name, count, |i| PVal::Int(unsafe { *v.add(i) }))
}

extern "C" fn prop_get_pointer(h: Handle, name: *const c_char, index: c_int, out: *mut *mut c_void) -> OfxStatus {
    with_set(h, name, |ps, n| match ps.get_at(n, index) {
        Ok(PVal::Ptr(p)) if !out.is_null() => {
            // SAFETY: `out` is a valid out-pointer.
            unsafe { *out = p as *mut c_void };
            K_STAT_OK
        }
        Ok(_) => K_STAT_ERR_UNKNOWN,
        Err(e) => e,
    })
}
extern "C" fn prop_get_string(h: Handle, name: *const c_char, index: c_int, out: *mut *const c_char) -> OfxStatus {
    with_set(h, name, |ps, n| match ps.get_str_ptr(n, index) {
        Ok(p) if !out.is_null() => {
            // SAFETY: `out` is a valid out-pointer.
            unsafe { *out = p };
            K_STAT_OK
        }
        Ok(_) => K_STAT_ERR_VALUE,
        Err(e) => e,
    })
}
extern "C" fn prop_get_double(h: Handle, name: *const c_char, index: c_int, out: *mut f64) -> OfxStatus {
    with_set(h, name, |ps, n| match ps.get_at(n, index) {
        Ok(v) if !out.is_null() => match as_double(&v) {
            Some(d) => {
                // SAFETY: `out` is a valid out-pointer.
                unsafe { *out = d };
                K_STAT_OK
            }
            None => K_STAT_ERR_UNKNOWN,
        },
        Ok(_) => K_STAT_ERR_VALUE,
        Err(e) => e,
    })
}
extern "C" fn prop_get_int(h: Handle, name: *const c_char, index: c_int, out: *mut c_int) -> OfxStatus {
    with_set(h, name, |ps, n| match ps.get_at(n, index) {
        Ok(v) if !out.is_null() => match as_int(&v) {
            Some(d) => {
                // SAFETY: `out` is a valid out-pointer.
                unsafe { *out = d };
                K_STAT_OK
            }
            None => K_STAT_ERR_UNKNOWN,
        },
        Ok(_) => K_STAT_ERR_VALUE,
        Err(e) => e,
    })
}

fn get_many<T>(h: Handle, name: *const c_char, count: c_int, out: *mut T, mut conv: impl FnMut(&PropSet, &str, usize) -> Result<T, OfxStatus>) -> OfxStatus {
    if !(0..=65536).contains(&count) || out.is_null() {
        return K_STAT_ERR_BAD_INDEX;
    }
    with_set(h, name, |ps, n| {
        for i in 0..count as usize {
            match conv(ps, n, i) {
                // SAFETY: the plug-in supplies room for `count` elements.
                Ok(v) => unsafe { out.add(i).write(v) },
                Err(e) => return e,
            }
        }
        K_STAT_OK
    })
}

extern "C" fn prop_get_pointer_n(h: Handle, name: *const c_char, count: c_int, out: *mut *mut c_void) -> OfxStatus {
    get_many(h, name, count, out, |ps, n, i| match ps.get_at(n, i as i32)? {
        PVal::Ptr(p) => Ok(p as *mut c_void),
        _ => Err(K_STAT_ERR_UNKNOWN),
    })
}
extern "C" fn prop_get_string_n(h: Handle, name: *const c_char, count: c_int, out: *mut *const c_char) -> OfxStatus {
    get_many(h, name, count, out, |ps, n, i| ps.get_str_ptr(n, i as i32))
}
extern "C" fn prop_get_double_n(h: Handle, name: *const c_char, count: c_int, out: *mut f64) -> OfxStatus {
    get_many(h, name, count, out, |ps, n, i| as_double(&ps.get_at(n, i as i32)?).ok_or(K_STAT_ERR_UNKNOWN))
}
extern "C" fn prop_get_int_n(h: Handle, name: *const c_char, count: c_int, out: *mut c_int) -> OfxStatus {
    get_many(h, name, count, out, |ps, n, i| as_int(&ps.get_at(n, i as i32)?).ok_or(K_STAT_ERR_UNKNOWN))
}

extern "C" fn prop_reset(h: Handle, name: *const c_char) -> OfxStatus {
    with_set(h, name, |ps, n| ps.reset(n))
}

extern "C" fn prop_get_dimension(h: Handle, name: *const c_char, count: *mut c_int) -> OfxStatus {
    with_set(h, name, |ps, n| match ps.dimension(n) {
        Ok(d) if !count.is_null() => {
            // SAFETY: `count` is a valid out-pointer.
            unsafe { *count = d };
            K_STAT_OK
        }
        Ok(_) => K_STAT_ERR_VALUE,
        Err(e) => e,
    })
}

/// The property suite table.
pub static SUITE: OfxPropertySuiteV1 = OfxPropertySuiteV1 {
    prop_set_pointer,
    prop_set_string,
    prop_set_double,
    prop_set_int,
    prop_set_pointer_n,
    prop_set_string_n,
    prop_set_double_n,
    prop_set_int_n,
    prop_get_pointer,
    prop_get_string,
    prop_get_double,
    prop_get_int,
    prop_get_pointer_n,
    prop_get_string_n,
    prop_get_double_n,
    prop_get_int_n,
    prop_reset,
    prop_get_dimension,
};
