//! `OfxMultiThreadSuiteV1`: SMP helper threads and recursive mutexes.
//!
//! `multiThread` runs the plug-in's function on `min(n, CPUs)` scoped threads and returns when
//! all are done. Threads it spawns are flagged so `multiThreadIsSpawnedThread` / `multiThreadIndex`
//! answer correctly. Mutexes are recursive (OFX allows relocking) and are created with an
//! initial lock count owned by the creating thread.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread::{self, ThreadId};

use crate::ffi::*;

thread_local! {
    static SPAWNED: Cell<Option<u32>> = const { Cell::new(None) };
    static IN_MULTI_THREAD: Cell<bool> = const { Cell::new(false) };
}

fn cpus() -> u32 {
    thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1).clamp(1, 256)
}

struct RawArg(usize);

extern "C" fn multi_thread(func: ThreadFn, n_threads: u32, arg: *mut c_void) -> OfxStatus {
    guard(|| {
        if IN_MULTI_THREAD.with(Cell::get) {
            return K_STAT_ERR_EXISTS;
        }
        let n = n_threads.clamp(1, cpus());
        let arg = RawArg(arg as usize);
        let run = |index: u32| {
            let a = &arg;
            SPAWNED.with(|s| s.set(Some(index)));
            // The plug-in's code must not unwind into Rust; a panic here would be a bug in it.
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // SAFETY: the plug-in's thread function, called as the specification describes.
                unsafe { func(index, n, a.0 as *mut c_void) }
            }));
            SPAWNED.with(|s| s.set(None));
            r.is_ok()
        };
        IN_MULTI_THREAD.with(|f| f.set(true));
        let ok = if n == 1 {
            run(0)
        } else {
            thread::scope(|s| {
                let handles: Vec<_> = (1..n).map(|i| s.spawn(move || run(i))).collect();
                let first = run(0);
                handles.into_iter().fold(first, |acc, h| h.join().unwrap_or(false) && acc)
            })
        };
        IN_MULTI_THREAD.with(|f| f.set(false));
        if ok { K_STAT_OK } else { K_STAT_FAILED }
    })
}

extern "C" fn multi_thread_num_cpus(out: *mut u32) -> OfxStatus {
    guard(|| {
        if out.is_null() {
            return K_STAT_FAILED;
        }
        // SAFETY: valid out-pointer.
        unsafe { *out = cpus() };
        K_STAT_OK
    })
}

extern "C" fn multi_thread_index(out: *mut u32) -> OfxStatus {
    guard(|| match (SPAWNED.with(Cell::get), out.is_null()) {
        (Some(i), false) => {
            // SAFETY: valid out-pointer.
            unsafe { *out = i };
            K_STAT_OK
        }
        _ => K_STAT_FAILED,
    })
}

extern "C" fn multi_thread_is_spawned_thread() -> std::ffi::c_int {
    guard_int(|| SPAWNED.with(Cell::get).is_some() as std::ffi::c_int, 0)
}

const MUTEX_MAGIC: u32 = 0x4F46_584D ^ 0x5555;

#[repr(C)]
struct OfxMutex {
    magic: u32,
    state: Mutex<(Option<ThreadId>, i64)>,
    cv: Condvar,
}

fn mutex_of<'a>(h: Handle) -> Option<&'a OfxMutex> {
    if h.is_null() {
        return None;
    }
    // SAFETY: readable handle from the plug-in; the magic rejects foreign pointers.
    if unsafe { std::ptr::read(h as *const u32) } != MUTEX_MAGIC {
        return None;
    }
    Some(unsafe { &*(h as *const OfxMutex) })
}

extern "C" fn mutex_create(out: *mut Handle, lock_count: std::ffi::c_int) -> OfxStatus {
    guard(|| {
        if out.is_null() {
            return K_STAT_FAILED;
        }
        let owner = if lock_count > 0 { Some(thread::current().id()) } else { None };
        let m = Box::new(OfxMutex { magic: MUTEX_MAGIC, state: Mutex::new((owner, lock_count.max(0) as i64)), cv: Condvar::new() });
        // SAFETY: valid out-pointer.
        unsafe { *out = Box::into_raw(m) as Handle };
        K_STAT_OK
    })
}

extern "C" fn mutex_destroy(h: Handle) -> OfxStatus {
    guard(|| {
        if mutex_of(h).is_none() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        // SAFETY: created by Box::into_raw in `mutex_create`.
        drop(unsafe { Box::from_raw(h as *mut OfxMutex) });
        K_STAT_OK
    })
}

extern "C" fn mutex_lock(h: Handle) -> OfxStatus {
    guard(|| {
        let Some(m) = mutex_of(h) else { return K_STAT_ERR_BAD_HANDLE };
        let me = thread::current().id();
        let mut st = m.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match st.0 {
                None => {
                    *st = (Some(me), 1);
                    return K_STAT_OK;
                }
                Some(o) if o == me => {
                    st.1 += 1;
                    return K_STAT_OK;
                }
                Some(_) => st = m.cv.wait(st).unwrap_or_else(PoisonError::into_inner),
            }
        }
    })
}

extern "C" fn mutex_unlock(h: Handle) -> OfxStatus {
    guard(|| {
        let Some(m) = mutex_of(h) else { return K_STAT_ERR_BAD_HANDLE };
        let mut st = m.state.lock().unwrap_or_else(PoisonError::into_inner);
        if st.1 > 0 {
            st.1 -= 1;
        }
        if st.1 == 0 {
            st.0 = None;
            m.cv.notify_one();
        }
        K_STAT_OK
    })
}

extern "C" fn mutex_try_lock(h: Handle) -> OfxStatus {
    guard(|| {
        let Some(m) = mutex_of(h) else { return K_STAT_ERR_BAD_HANDLE };
        let me = thread::current().id();
        let mut st = m.state.lock().unwrap_or_else(PoisonError::into_inner);
        match st.0 {
            None => {
                *st = (Some(me), 1);
                K_STAT_OK
            }
            Some(o) if o == me => {
                st.1 += 1;
                K_STAT_OK
            }
            Some(_) => K_STAT_FAILED,
        }
    })
}

/// The multi-thread suite table.
pub static SUITE: OfxMultiThreadSuiteV1 = OfxMultiThreadSuiteV1 {
    multi_thread,
    multi_thread_num_cpus,
    multi_thread_index,
    multi_thread_is_spawned_thread,
    mutex_create,
    mutex_destroy,
    mutex_lock,
    mutex_unlock,
    mutex_try_lock,
};
