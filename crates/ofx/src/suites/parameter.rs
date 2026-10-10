//! `OfxParameterSuiteV1`: define parameters (descriptors), fetch their handles (instances) and
//! read / write values.
//!
//! The value functions are C variadic; see the module docs of [`crate::ffi`] for how the argument
//! slots are read. Reads during a render (`paramGetValueAtTime`) go to the host's animated values
//! through the render context; reads outside a render, and `paramGetValue`, return the
//! instance's stored value.
//!
//! `paramGetDerivative` and `paramGetIntegral` (retimers integrate a Speed curve to find the source
//! frame) sample the same animated values numerically: a central difference over one frame and a
//! composite Simpson sum with steps of at most one frame (see `integral`). They exist for double,
//! 2D / 3D double and colour parameters; outside a render the value is taken as constant.
//!
//! Keyframe access reports "no keys": the host owns animation, the plug-in only ever sees the
//! evaluated value at a time.

use std::ffi::{c_char, c_int, c_void};

use crate::ffi::*;
use crate::instance::{EffectObj, RenderCtx};
use crate::params::{PKind, ParamObj};
use crate::props::{PVal, as_double, as_int};

extern "C" fn param_define(set: Handle, ty: *const c_char, name: *const c_char, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: handles and strings come from the plug-in.
        let Some(eff) = (unsafe { EffectObj::from_handle(set) }) else { return K_STAT_ERR_BAD_HANDLE };
        let (Some(ty), Some(name)) = (unsafe { str_arg(ty) }, unsafe { str_arg(name) }) else { return K_STAT_ERR_UNKNOWN };
        let Some(kind) = PKind::from_type(ty) else { return K_STAT_ERR_UNKNOWN };
        match eff.params.define(eff as *const EffectObj, kind, name, eff.is_instance) {
            Ok(p) => {
                if !props.is_null() {
                    // SAFETY: `p` is a stable pointer into the set; `props` is an out-pointer.
                    unsafe { *props = (*p).props.as_handle() };
                }
                K_STAT_OK
            }
            Err(e) => e,
        }
    })
}

extern "C" fn param_get_handle(set: Handle, name: *const c_char, param: *mut Handle, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(eff) = (unsafe { EffectObj::from_handle(set) }) else { return K_STAT_ERR_BAD_HANDLE };
        let Some(name) = (unsafe { str_arg(name) }) else { return K_STAT_ERR_UNKNOWN };
        let Some(p) = eff.params.find(name) else { return K_STAT_ERR_UNKNOWN };
        // SAFETY: stable pointer into the set.
        let p = unsafe { &*p };
        if param.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        unsafe {
            *param = p.handle();
            if !props.is_null() {
                *props = p.props.as_handle();
            }
        }
        K_STAT_OK
    })
}

extern "C" fn param_set_get_property_set(set: Handle, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(eff) = (unsafe { EffectObj::from_handle(set) }) else { return K_STAT_ERR_BAD_HANDLE };
        if props.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        unsafe { *props = eff.props.as_handle() };
        K_STAT_OK
    })
}

extern "C" fn param_get_property_set(param: Handle, props: *mut Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: as above.
        let Some(p) = (unsafe { ParamObj::from_handle(param) }) else { return K_STAT_ERR_BAD_HANDLE };
        if props.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        unsafe { *props = p.props.as_handle() };
        K_STAT_OK
    })
}

/// Write `vals` to the pointers in `va`, as `paramGetValue` does for the parameter's type.
fn write_value(kind: PKind, vals: &[PVal], va: &mut VarArgs) -> OfxStatus {
    let dbl = |i: usize| vals.get(i).and_then(as_double).unwrap_or(0.0);
    let int = |i: usize| vals.get(i).and_then(as_int).unwrap_or(0);
    let n = match kind {
        PKind::Double | PKind::Int | PKind::Bool | PKind::Choice | PKind::Str | PKind::Custom | PKind::StrChoice => 1,
        PKind::Double2D | PKind::Int2D => 2,
        PKind::Double3D | PKind::Int3D | PKind::Rgb => 3,
        PKind::Rgba => 4,
        PKind::Bytes | PKind::Group | PKind::Page | PKind::Push => return K_STAT_ERR_UNSUPPORTED,
    };
    for i in 0..n {
        let p: *mut c_void = va.next_ptr();
        if p.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        // SAFETY: the plug-in passes one valid out-pointer of the documented type per component.
        unsafe {
            match kind {
                PKind::Int | PKind::Bool | PKind::Choice | PKind::Int2D | PKind::Int3D => *(p as *mut c_int) = int(i),
                PKind::Str | PKind::Custom | PKind::StrChoice => {
                    *(p as *mut *const c_char) = match vals.first() {
                        Some(PVal::Str(s)) => s.as_ptr(),
                        _ => c"".as_ptr(),
                    };
                }
                _ => *(p as *mut f64) = dbl(i),
            }
        }
    }
    K_STAT_OK
}

fn get_value(h: Handle, mut va: VarArgs) -> OfxStatus {
    // SAFETY: handle from the plug-in.
    let Some(p) = (unsafe { ParamObj::from_handle(h) }) else { return K_STAT_ERR_BAD_HANDLE };
    // The lock is held while pointers into a string value are handed out, so a concurrent
    // `set` cannot free it mid-copy of the pointer; the plug-in copies the string itself.
    let guard = p.value.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    write_value(p.kind, &guard, &mut va)
}

fn get_value_at_time(h: Handle, mut va: VarArgs) -> OfxStatus {
    // SAFETY: handle from the plug-in.
    let Some(p) = (unsafe { ParamObj::from_handle(h) }) else { return K_STAT_ERR_BAD_HANDLE };
    let time = va.next_f64();
    // SAFETY: the owner outlives its parameters.
    let ctx = unsafe { p.owner.as_ref() }.and_then(EffectObj::current_render);
    if let Some(ctx) = ctx
        && ctx.has_host()
        && let Some(map) = ctx.params_at(time)
        && let Some(vals) = map.get(&p.name)
    {
        return write_value(p.kind, vals, &mut va);
    }
    let guard = p.value.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    write_value(p.kind, &guard, &mut va)
}

/// A sampled parameter: up to four components (unused ones are 0).
type Sample = [f64; 4];

/// The most steps one integral takes (an even number, see [`integral`]).
const MAX_INTEGRAL_STEPS: usize = 4096;

/// How many `double` out-pointers `paramGetDerivative` / `paramGetIntegral` take for a parameter,
/// `None` for the kinds they do not support (only double-valued parameters and colours have a
/// derivative; integer, choice, string... parameters are step-like or not numeric).
fn components(kind: PKind) -> Option<usize> {
    match kind {
        PKind::Double => Some(1),
        PKind::Double2D => Some(2),
        PKind::Double3D | PKind::Rgb => Some(3),
        PKind::Rgba => Some(4),
        _ => None,
    }
}

fn sample_of(vals: &[PVal]) -> Sample {
    std::array::from_fn(|i| vals.get(i).and_then(as_double).filter(|v| v.is_finite()).unwrap_or(0.0))
}

/// Derivative per OFX time unit (frame) of the sampled function at `t`: a central difference over
/// one frame (half a frame either side).
fn derivative(f: &dyn Fn(f64) -> Option<Sample>, t: f64) -> Option<Sample> {
    const H: f64 = 0.5;
    if !t.is_finite() {
        return None;
    }
    let (lo, hi) = (f(t - H)?, f(t + H)?);
    Some(std::array::from_fn(|i| (hi[i] - lo[i]) / (2.0 * H)))
}

/// Integral of the sampled function from `t1` to `t2` (negative when `t2 < t1`) by composite
/// Simpson's rule with steps of at most one frame, and at most [`MAX_INTEGRAL_STEPS`] steps.
fn integral(f: &dyn Fn(f64) -> Option<Sample>, t1: f64, t2: f64) -> Option<Sample> {
    if !(t1.is_finite() && t2.is_finite()) {
        return None;
    }
    if t1 == t2 {
        return Some([0.0; 4]);
    }
    let (lo, hi, sign) = if t1 < t2 { (t1, t2, 1.0) } else { (t2, t1, -1.0) };
    let span = hi - lo;
    // Simpson needs an even number of steps.
    let mut n = span.ceil().clamp(2.0, MAX_INTEGRAL_STEPS as f64) as usize;
    if n % 2 == 1 {
        n += 1;
    }
    let h = span / n as f64;
    let mut sum = [0.0; 4];
    for k in 0..=n {
        let v = f(lo + h * k as f64)?;
        let w = if k == 0 || k == n {
            1.0
        } else if k % 2 == 1 {
            4.0
        } else {
            2.0
        };
        for (s, x) in sum.iter_mut().zip(v) {
            *s += w * x;
        }
    }
    Some(sum.map(|s| sign * s * h / 3.0))
}

/// Write `n` components to the `double*` out-pointers in `va`.
fn write_doubles(vals: &Sample, n: usize, va: &mut VarArgs) -> OfxStatus {
    for v in vals.iter().take(n) {
        let p: *mut f64 = va.next_ptr();
        if p.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        // SAFETY: the plug-in passes one valid `double*` per component.
        unsafe { *p = *v };
    }
    K_STAT_OK
}

/// The render with a host behind it that parameter `p` can be sampled through, if any.
fn sampling_ctx(p: &ParamObj) -> Option<std::sync::Arc<RenderCtx>> {
    // SAFETY: the owner outlives its parameters.
    unsafe { p.owner.as_ref() }.and_then(EffectObj::current_render).filter(|c| c.has_host())
}

/// `paramGetDerivative`: the animated value's rate of change per frame. Without a render (or when
/// the host cannot be asked) the value is constant, so the derivative is 0.
fn get_derivative(h: Handle, mut va: VarArgs) -> OfxStatus {
    // SAFETY: handle from the plug-in.
    let Some(p) = (unsafe { ParamObj::from_handle(h) }) else { return K_STAT_ERR_BAD_HANDLE };
    let Some(n) = components(p.kind) else { return K_STAT_ERR_UNSUPPORTED };
    let time = va.next_f64();
    let d = sampling_ctx(p).and_then(|ctx| derivative(&|t| ctx.param_at(&p.name, t).map(|v| sample_of(&v)), time)).unwrap_or([0.0; 4]);
    write_doubles(&d, n, &mut va)
}

/// `paramGetIntegral`: the animated value integrated over OFX time (frames) from `time1` to
/// `time2`. Without a render (or when the host cannot be asked) the value is constant:
/// `value * (time2 - time1)`.
fn get_integral(h: Handle, mut va: VarArgs) -> OfxStatus {
    // SAFETY: handle from the plug-in.
    let Some(p) = (unsafe { ParamObj::from_handle(h) }) else { return K_STAT_ERR_BAD_HANDLE };
    let Some(n) = components(p.kind) else { return K_STAT_ERR_UNSUPPORTED };
    let (t1, t2) = (va.next_f64(), va.next_f64());
    if !(t1.is_finite() && t2.is_finite()) {
        return K_STAT_FAILED;
    }
    let sampled = sampling_ctx(p).and_then(|ctx| integral(&|t| ctx.param_at(&p.name, t).map(|v| sample_of(&v)), t1, t2));
    let total = sampled.unwrap_or_else(|| sample_of(&p.values()).map(|v| v * (t2 - t1)));
    if total.iter().any(|v| !v.is_finite()) {
        return K_STAT_FAILED;
    }
    write_doubles(&total, n, &mut va)
}

/// Read the new value from the argument slots and store it.
fn read_new_value(kind: PKind, va: &mut VarArgs) -> Option<Vec<PVal>> {
    Some(match kind {
        PKind::Double => vec![PVal::Double(va.next_f64())],
        PKind::Int | PKind::Bool | PKind::Choice => vec![PVal::Int(va.next_i32())],
        PKind::Double2D => (0..2).map(|_| PVal::Double(va.next_f64())).collect(),
        PKind::Double3D | PKind::Rgb => (0..3).map(|_| PVal::Double(va.next_f64())).collect(),
        PKind::Rgba => (0..4).map(|_| PVal::Double(va.next_f64())).collect(),
        PKind::Int2D => (0..2).map(|_| PVal::Int(va.next_i32())).collect(),
        PKind::Int3D => (0..3).map(|_| PVal::Int(va.next_i32())).collect(),
        PKind::Str | PKind::Custom | PKind::StrChoice => {
            let p: *const c_char = va.next_ptr();
            // SAFETY: a C string from the plug-in.
            vec![PVal::text(unsafe { str_arg(p) }.unwrap_or(""))]
        }
        PKind::Bytes | PKind::Group | PKind::Page | PKind::Push => return None,
    })
}

fn set_value(h: Handle, mut va: VarArgs) -> OfxStatus {
    // SAFETY: handle from the plug-in.
    let Some(p) = (unsafe { ParamObj::from_handle(h) }) else { return K_STAT_ERR_BAD_HANDLE };
    match read_new_value(p.kind, &mut va) {
        Some(v) => {
            p.set_values(v);
            K_STAT_OK
        }
        None => K_STAT_ERR_UNSUPPORTED,
    }
}

fn set_value_at_time(h: Handle, mut va: VarArgs) -> OfxStatus {
    // SAFETY: handle from the plug-in.
    let Some(p) = (unsafe { ParamObj::from_handle(h) }) else { return K_STAT_ERR_BAD_HANDLE };
    let _time = va.next_f64();
    match read_new_value(p.kind, &mut va) {
        Some(v) => {
            p.set_values(v);
            K_STAT_OK
        }
        None => K_STAT_ERR_UNSUPPORTED,
    }
}

/// Declare a variadic suite function as the fixed-slot function the C caller's arguments land
/// in (see the `ffi` module docs).
macro_rules! var_fn {
    ($name:ident, $body:path) => {
        #[cfg(windows)]
        extern "C" fn $name(h: Handle, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> OfxStatus {
            let va = VarArgs::new([a1, a2, a3, a4, a5, a6]);
            guard(move || $body(h, va))
        }
        #[cfg(not(windows))]
        extern "C" fn $name(h: Handle, i1: u64, i2: u64, i3: u64, i4: u64, i5: u64, f1: f64, f2: f64, f3: f64, f4: f64, f5: f64) -> OfxStatus {
            let va = VarArgs::new([i1, i2, i3, i4, i5], [f1, f2, f3, f4, f5]);
            guard(move || $body(h, va))
        }
    };
}

/// [`var_fn`] for the suite functions whose first argument after the handle is a fixed
/// `OfxTime`. On Windows x64 a fixed double travels only in its XMM register (the integer slot
/// holds garbage), so it has to be declared as `f64`; System V already reads it from the first
/// float register.
macro_rules! var_fn_t {
    ($name:ident, $body:path) => {
        #[cfg(windows)]
        extern "C" fn $name(h: Handle, t: f64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> OfxStatus {
            let va = VarArgs::new([t.to_bits(), a2, a3, a4, a5, a6]);
            guard(move || $body(h, va))
        }
        #[cfg(not(windows))]
        var_fn!($name, $body);
    };
}

/// [`var_fn_t`] for `paramGetIntegral`, whose two fixed `OfxTime`s both arrive in XMM registers on
/// Windows x64 (positions 2 and 3); System V reads them from the first two float registers.
macro_rules! var_fn_t2 {
    ($name:ident, $body:path) => {
        #[cfg(windows)]
        extern "C" fn $name(h: Handle, t1: f64, t2: f64, a3: u64, a4: u64, a5: u64, a6: u64) -> OfxStatus {
            let va = VarArgs::new([t1.to_bits(), t2.to_bits(), a3, a4, a5, a6]);
            guard(move || $body(h, va))
        }
        #[cfg(not(windows))]
        var_fn!($name, $body);
    };
}

var_fn!(param_get_value, get_value);
var_fn_t!(param_get_value_at_time, get_value_at_time);
var_fn_t!(param_get_derivative, get_derivative);
var_fn_t2!(param_get_integral, get_integral);
var_fn!(param_set_value, set_value);
var_fn_t!(param_set_value_at_time, set_value_at_time);

extern "C" fn param_get_num_keys(h: Handle, n: *mut u32) -> OfxStatus {
    guard(|| {
        // SAFETY: handle from the plug-in.
        if unsafe { ParamObj::from_handle(h) }.is_none() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        if !n.is_null() {
            unsafe { *n = 0 };
        }
        K_STAT_OK
    })
}
extern "C" fn param_get_key_time(_h: Handle, _n: u32, _t: *mut f64) -> OfxStatus {
    K_STAT_ERR_BAD_INDEX
}
extern "C" fn param_get_key_index(_h: Handle, _t: f64, _dir: c_int, _out: *mut c_int) -> OfxStatus {
    K_STAT_FAILED
}
extern "C" fn param_delete_key(_h: Handle, _t: f64) -> OfxStatus {
    K_STAT_OK
}
extern "C" fn param_delete_all_keys(_h: Handle) -> OfxStatus {
    K_STAT_OK
}
extern "C" fn param_copy(to: Handle, from: Handle, _offset: f64, _range: *const OfxRangeD) -> OfxStatus {
    guard(|| {
        // SAFETY: handles from the plug-in.
        let (Some(a), Some(b)) = (unsafe { ParamObj::from_handle(to) }, unsafe { ParamObj::from_handle(from) }) else { return K_STAT_ERR_BAD_HANDLE };
        if a.kind != b.kind {
            return K_STAT_ERR_BAD_HANDLE;
        }
        a.set_values(b.values());
        K_STAT_OK
    })
}
extern "C" fn param_edit_begin(_set: Handle, _name: *const c_char) -> OfxStatus {
    K_STAT_OK
}
extern "C" fn param_edit_end(_set: Handle) -> OfxStatus {
    K_STAT_OK
}

/// The parameter suite table.
pub static SUITE: OfxParameterSuiteV1 = OfxParameterSuiteV1 {
    param_define,
    param_get_handle,
    param_set_get_property_set,
    param_get_property_set,
    param_get_value,
    param_get_value_at_time,
    param_get_derivative,
    param_get_integral,
    param_set_value,
    param_set_value_at_time,
    param_get_num_keys,
    param_get_key_time,
    param_get_key_index,
    param_delete_key,
    param_delete_all_keys,
    param_copy,
    param_edit_begin,
    param_edit_end,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// A sample function whose second component is twice the first.
    fn scalar(f: impl Fn(f64) -> f64) -> impl Fn(f64) -> Option<Sample> {
        move |t| Some([f(t), 2.0 * f(t), 0.0, 0.0])
    }

    #[test]
    fn integral_of_a_constant_is_value_times_duration() {
        let f = scalar(|_| 3.0);
        let v = integral(&f, 10.0, 14.5).unwrap();
        assert!((v[0] - 13.5).abs() < 1e-9 && (v[1] - 27.0).abs() < 1e-9);
    }

    #[test]
    fn integral_of_a_ramp_and_a_curve_is_accurate() {
        // Speed ramping from 0 to 200 over 100 frames: the integral is the distance travelled.
        let ramp = scalar(|t| 2.0 * t);
        assert!((integral(&ramp, 0.0, 100.0).unwrap()[0] - 10000.0).abs() < 1e-6);
        // A quadratic is integrated exactly by Simpson's rule.
        let quad = scalar(|t| t * t);
        assert!((integral(&quad, 0.0, 30.0).unwrap()[0] - 9000.0).abs() < 1e-6);
        // A sine over a few periods, with steps of at most a frame.
        let sine = scalar(|t| (t * 0.2).sin());
        let exact = (1.0 - (0.2f64 * 60.0).cos()) / 0.2;
        assert!((integral(&sine, 0.0, 60.0).unwrap()[0] - exact).abs() < 1e-3);
    }

    #[test]
    fn integral_runs_backwards_and_handles_short_and_empty_ranges() {
        let ramp = scalar(|t| 2.0 * t);
        assert!((integral(&ramp, 100.0, 0.0).unwrap()[0] + 10000.0).abs() < 1e-6);
        assert!((integral(&ramp, 0.0, 0.25).unwrap()[0] - 0.0625).abs() < 1e-9);
        assert_eq!(integral(&ramp, 5.0, 5.0).unwrap(), [0.0; 4]);
        assert!(integral(&ramp, 0.0, f64::NAN).is_none());
    }

    #[test]
    fn integral_caps_its_samples_and_fails_when_the_source_does() {
        let calls = std::cell::Cell::new(0usize);
        let counted = |_t: f64| {
            calls.set(calls.get() + 1);
            Some([1.0, 0.0, 0.0, 0.0])
        };
        let v = integral(&counted, 0.0, 1.0e7).unwrap();
        assert_eq!(calls.get(), MAX_INTEGRAL_STEPS + 1);
        assert!((v[0] - 1.0e7).abs() < 1e-3);
        assert!(integral(&|_| None, 0.0, 10.0).is_none());
    }

    #[test]
    fn derivative_is_the_slope_per_frame() {
        let lin = scalar(|t| 3.0 * t + 1.0);
        let d = derivative(&lin, 12.0).unwrap();
        assert!((d[0] - 3.0).abs() < 1e-9 && (d[1] - 6.0).abs() < 1e-9);
        let flat = scalar(|_| 5.0);
        assert!(derivative(&flat, 0.0).unwrap()[0].abs() < 1e-12);
        assert!(derivative(&lin, f64::INFINITY).is_none());
    }

    #[test]
    fn only_double_valued_kinds_have_components() {
        assert_eq!(components(PKind::Double), Some(1));
        assert_eq!(components(PKind::Double2D), Some(2));
        assert_eq!(components(PKind::Rgb), Some(3));
        assert_eq!(components(PKind::Rgba), Some(4));
        assert_eq!(components(PKind::Int), None);
        assert_eq!(components(PKind::Choice), None);
    }
}
