//! `OfxMemorySuiteV1`: plain host allocation. Blocks carry a 16-byte header holding their size so
//! `memoryFree` needs only the pointer.

use std::alloc::{Layout, alloc, dealloc};
use std::ffi::c_void;

use crate::ffi::*;

const HEADER: usize = 16;
const MAX_BYTES: usize = 1 << 40;

/// Allocate `bytes` (16-byte aligned); `None` when the request is absurd or memory is short.
pub fn host_alloc(bytes: usize) -> Option<*mut u8> {
    if bytes > MAX_BYTES {
        return None;
    }
    let layout = Layout::from_size_align(bytes.checked_add(HEADER)?, HEADER).ok()?;
    // SAFETY: non-zero size (HEADER > 0), valid layout.
    let base = unsafe { alloc(layout) };
    if base.is_null() {
        return None;
    }
    // SAFETY: the block is at least HEADER bytes and 16-byte aligned.
    unsafe {
        (base as *mut usize).write(bytes);
        Some(base.add(HEADER))
    }
}

/// Free a block from [`host_alloc`] (null is ignored).
///
/// # Safety
/// `p` is null or a block from [`host_alloc`] that has not been freed yet.
pub unsafe fn host_free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    // SAFETY: `p` came from `host_alloc`, so the header sits right before it.
    unsafe {
        let base = p.sub(HEADER);
        let bytes = (base as *const usize).read();
        if let Ok(layout) = Layout::from_size_align(bytes + HEADER, HEADER) {
            dealloc(base, layout);
        }
    }
}

extern "C" fn memory_alloc(_handle: Handle, bytes: usize, out: *mut *mut c_void) -> OfxStatus {
    guard(|| {
        if out.is_null() {
            return K_STAT_ERR_BAD_HANDLE;
        }
        match host_alloc(bytes) {
            Some(p) => {
                // SAFETY: `out` is a valid out-pointer.
                unsafe { *out = p as *mut c_void };
                K_STAT_OK
            }
            None => {
                unsafe { *out = std::ptr::null_mut() };
                K_STAT_ERR_MEMORY
            }
        }
    })
}

extern "C" fn memory_free(p: *mut c_void) -> OfxStatus {
    guard(|| {
        // SAFETY: the plug-in frees a block it got from `memory_alloc`.
        unsafe { host_free(p as *mut u8) };
        K_STAT_OK
    })
}

/// The memory suite table.
pub static SUITE: OfxMemorySuiteV1 = OfxMemorySuiteV1 { memory_alloc, memory_free };
