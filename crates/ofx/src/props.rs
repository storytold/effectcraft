//! Property sets: the generic string-keyed, typed, multi-dimensional bags every OFX object carries.
//!
//! A [`PropSet`] is what an `OfxPropertySetHandle` points at. It is internally synchronised
//! (plug-ins may touch an effect's properties from several render threads) and lenient: setting
//! an unknown property creates it, `int` and `double` properties convert into each other on read,
//! and reading a property that was never set reports `kOfxStatErrUnknown` and logs the name once
//! so missing host features show up in the log without flooding it.

use std::collections::{HashMap, HashSet};
use std::ffi::{CString, c_void};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::ffi::{K_STAT_ERR_BAD_HANDLE, K_STAT_ERR_BAD_INDEX, K_STAT_ERR_UNKNOWN, K_STAT_OK, OfxStatus, cstring};

/// The marker at the start of every [`PropSet`], used to reject handles that are not ours.
pub const PROPSET_MAGIC: u32 = 0x4F46_5850; // "OFXP"

/// One value of a property.
#[derive(Clone, Debug)]
pub enum PVal {
    Int(i32),
    Double(f64),
    Str(CString),
    Ptr(usize),
}

impl PVal {
    pub fn text(s: &str) -> PVal {
        PVal::Str(cstring(s))
    }
}

/// A property set. `#[repr(C)]` with the magic first so a handle can be validated by reading it.
#[repr(C)]
pub struct PropSet {
    magic: u32,
    map: Mutex<HashMap<String, Vec<PVal>>>,
}

impl Default for PropSet {
    fn default() -> Self {
        PropSet::new()
    }
}

impl Clone for PropSet {
    fn clone(&self) -> Self {
        PropSet { magic: PROPSET_MAGIC, map: Mutex::new(self.lock().clone()) }
    }
}

impl std::fmt::Debug for PropSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PropSet").field("props", &self.lock().len()).finish()
    }
}

fn warned() -> &'static Mutex<HashSet<String>> {
    static W: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Log a property the plug-in asked for that this host does not provide (once per name).
pub fn note_unknown(name: &str) {
    let first = warned().lock().unwrap_or_else(PoisonError::into_inner).insert(name.to_string());
    if first {
        log::debug!("openfx: plug-in read property `{name}`, which this host does not provide");
    }
}

impl PropSet {
    pub fn new() -> PropSet {
        PropSet { magic: PROPSET_MAGIC, map: Mutex::new(HashMap::new()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<PVal>>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Check a raw handle and borrow the set.
    ///
    /// # Safety
    /// `h` must be null or point to readable memory at least four bytes long; the returned
    /// reference must not outlive the object (callers keep it for one callback).
    pub unsafe fn from_handle<'a>(h: *mut c_void) -> Option<&'a PropSet> {
        if h.is_null() {
            return None;
        }
        let p = h as *const PropSet;
        if unsafe { std::ptr::read(p as *const u32) } != PROPSET_MAGIC {
            return None;
        }
        Some(unsafe { &*p })
    }

    pub fn as_handle(&self) -> *mut c_void {
        self as *const PropSet as *mut c_void
    }

    // ---- Rust-side convenience ----

    /// Set a whole property.
    pub fn put(&self, name: &str, vals: Vec<PVal>) {
        self.lock().insert(name.to_string(), vals);
    }
    pub fn put_int(&self, name: &str, v: i32) {
        self.put(name, vec![PVal::Int(v)]);
    }
    pub fn put_ints(&self, name: &str, v: &[i32]) {
        self.put(name, v.iter().map(|x| PVal::Int(*x)).collect());
    }
    pub fn put_double(&self, name: &str, v: f64) {
        self.put(name, vec![PVal::Double(v)]);
    }
    pub fn put_doubles(&self, name: &str, v: &[f64]) {
        self.put(name, v.iter().map(|x| PVal::Double(*x)).collect());
    }
    pub fn put_str(&self, name: &str, v: &str) {
        self.put(name, vec![PVal::text(v)]);
    }
    pub fn put_strs(&self, name: &str, v: &[&str]) {
        self.put(name, v.iter().map(|x| PVal::text(x)).collect());
    }
    pub fn put_ptr(&self, name: &str, v: usize) {
        self.put(name, vec![PVal::Ptr(v)]);
    }
    pub fn remove(&self, name: &str) {
        self.lock().remove(name);
    }
    pub fn has(&self, name: &str) -> bool {
        self.lock().contains_key(name)
    }
    pub fn all(&self, name: &str) -> Vec<PVal> {
        self.lock().get(name).cloned().unwrap_or_default()
    }

    /// First value as an integer (doubles are truncated).
    pub fn int(&self, name: &str) -> Option<i32> {
        self.lock().get(name).and_then(|v| v.first()).and_then(as_int)
    }
    pub fn double(&self, name: &str) -> Option<f64> {
        self.lock().get(name).and_then(|v| v.first()).and_then(as_double)
    }
    pub fn str(&self, name: &str) -> Option<String> {
        self.lock().get(name).and_then(|v| v.first()).and_then(as_string)
    }
    pub fn ptr(&self, name: &str) -> Option<usize> {
        self.lock().get(name).and_then(|v| v.first()).and_then(|v| match v {
            PVal::Ptr(p) => Some(*p),
            _ => None,
        })
    }
    pub fn flag(&self, name: &str, default: bool) -> bool {
        self.int(name).map(|v| v != 0).unwrap_or(default)
    }
    pub fn strs(&self, name: &str) -> Vec<String> {
        self.lock().get(name).map(|v| v.iter().filter_map(as_string).collect()).unwrap_or_default()
    }
    pub fn doubles(&self, name: &str) -> Vec<f64> {
        self.lock().get(name).map(|v| v.iter().filter_map(as_double).collect()).unwrap_or_default()
    }
    pub fn ints(&self, name: &str) -> Vec<i32> {
        self.lock().get(name).map(|v| v.iter().filter_map(as_int).collect()).unwrap_or_default()
    }

    // ---- suite-side accessors (return OFX statuses) ----

    pub fn set_at(&self, name: &str, index: i32, v: PVal) -> OfxStatus {
        if index < 0 {
            return K_STAT_ERR_BAD_INDEX;
        }
        let mut map = self.lock();
        let slot = map.entry(name.to_string()).or_default();
        let index = index as usize;
        if index > slot.len() || index > 4096 {
            // Setting beyond the end by more than one is how a dimension would be skipped.
            if index > 4096 {
                return K_STAT_ERR_BAD_INDEX;
            }
            while slot.len() < index {
                slot.push(match v {
                    PVal::Int(_) => PVal::Int(0),
                    PVal::Double(_) => PVal::Double(0.0),
                    PVal::Str(_) => PVal::text(""),
                    PVal::Ptr(_) => PVal::Ptr(0),
                });
            }
        }
        if index == slot.len() {
            slot.push(v);
        } else if let Some(s) = slot.get_mut(index) {
            *s = v;
        }
        K_STAT_OK
    }

    /// Fetch value `index` of `name`, logging unknown names.
    pub fn get_at(&self, name: &str, index: i32) -> Result<PVal, OfxStatus> {
        let map = self.lock();
        let Some(slot) = map.get(name) else {
            drop(map);
            note_unknown(name);
            return Err(K_STAT_ERR_UNKNOWN);
        };
        usize::try_from(index).ok().and_then(|i| slot.get(i)).cloned().ok_or(K_STAT_ERR_BAD_INDEX)
    }

    /// The raw pointer of a string value, stable while the property is not changed.
    pub fn get_str_ptr(&self, name: &str, index: i32) -> Result<*const std::ffi::c_char, OfxStatus> {
        let map = self.lock();
        let Some(slot) = map.get(name) else {
            drop(map);
            note_unknown(name);
            return Err(K_STAT_ERR_UNKNOWN);
        };
        match usize::try_from(index).ok().and_then(|i| slot.get(i)) {
            Some(PVal::Str(s)) => Ok(s.as_ptr()),
            Some(_) => Err(K_STAT_ERR_BAD_HANDLE),
            None => Err(K_STAT_ERR_BAD_INDEX),
        }
    }

    pub fn dimension(&self, name: &str) -> Result<i32, OfxStatus> {
        match self.lock().get(name) {
            Some(v) => Ok(v.len() as i32),
            None => {
                note_unknown(name);
                Err(K_STAT_ERR_UNKNOWN)
            }
        }
    }

    pub fn reset(&self, name: &str) -> OfxStatus {
        if self.lock().remove(name).is_some() { K_STAT_OK } else { K_STAT_ERR_UNKNOWN }
    }
}

pub fn as_int(v: &PVal) -> Option<i32> {
    match v {
        PVal::Int(i) => Some(*i),
        PVal::Double(d) if d.is_finite() => Some(*d as i32),
        _ => None,
    }
}

pub fn as_double(v: &PVal) -> Option<f64> {
    match v {
        PVal::Int(i) => Some(*i as f64),
        PVal::Double(d) => Some(*d),
        _ => None,
    }
}

pub fn as_string(v: &PVal) -> Option<String> {
    match v {
        PVal::Str(s) => Some(s.to_string_lossy().into_owned()),
        _ => None,
    }
}
