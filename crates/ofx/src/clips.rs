//! Clips and images: the clip objects plug-ins define and query, the OFX image handles returned
//! by `clipGetImage`, and the conversion between EffectCraft's premultiplied RGBA `f32` rows
//! (row 0 = top) and OFX pixel buffers (any depth, RGBA / RGB / alpha, rows bottom-up).
//!
//! OFX images are y-up. Instead of handing out negative row bytes (some plug-ins mishandle
//! them), the conversion flips the rows: OFX row 0 is EffectCraft's bottom row and `rowBytes`
//! is positive.

use std::cell::UnsafeCell;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::ffi::*;
use crate::instance::EffectObj;
use crate::props::PropSet;

/// The marker at the start of every [`ClipObj`].
pub const CLIP_MAGIC: u32 = 0x4F46_5843; // "OFXC"

/// A clip of an effect descriptor or instance.
#[repr(C)]
pub struct ClipObj {
    magic: u32,
    pub name: String,
    pub props: PropSet,
    pub owner: *const EffectObj,
}

unsafe impl Send for ClipObj {}
unsafe impl Sync for ClipObj {}

impl ClipObj {
    pub fn new(name: &str, owner: *const EffectObj) -> ClipObj {
        let props = PropSet::new();
        props.put_str(P_TYPE, TYPE_CLIP);
        props.put_str(P_NAME, name);
        props.put_str(P_LABEL, name);
        props.put_str(P_SHORT_LABEL, name);
        props.put_str(P_LONG_LABEL, name);
        props.put_int(CLIP_OPTIONAL, 0);
        props.put_int(CLIP_IS_MASK, 0);
        props.put_int(IE_TEMPORAL_CLIP_ACCESS, 0);
        props.put_int(IE_SUPPORTS_TILES, 1);
        props.put(IE_SUPPORTED_COMPONENTS, vec![]);
        props.put_str(CLIP_FIELD_EXTRACTION, "OfxImageFieldDoubled");
        ClipObj { magic: CLIP_MAGIC, name: name.to_string(), props, owner }
    }

    /// An instance's copy of a descriptor clip.
    pub fn instantiate(&self, owner: *const EffectObj) -> ClipObj {
        ClipObj { magic: CLIP_MAGIC, name: self.name.clone(), props: self.props.clone(), owner }
    }

    /// Validate a raw handle.
    ///
    /// # Safety
    /// `h` must be null or point to readable memory; the reference is only used during a callback.
    pub unsafe fn from_handle<'a>(h: *mut c_void) -> Option<&'a ClipObj> {
        if h.is_null() || unsafe { std::ptr::read(h as *const u32) } != CLIP_MAGIC {
            return None;
        }
        Some(unsafe { &*(h as *const ClipObj) })
    }

    pub fn handle(&self) -> *mut c_void {
        self as *const ClipObj as *mut c_void
    }
}

/// OFX pixel depths this host can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    Byte,
    Short,
    Float,
}

impl Depth {
    pub fn name(self) -> &'static str {
        match self {
            Depth::Byte => DEPTH_BYTE,
            Depth::Short => DEPTH_SHORT,
            Depth::Float => DEPTH_FLOAT,
        }
    }
    pub fn from_name(s: &str) -> Option<Depth> {
        match s {
            DEPTH_BYTE => Some(Depth::Byte),
            DEPTH_SHORT => Some(Depth::Short),
            DEPTH_FLOAT => Some(Depth::Float),
            _ => None,
        }
    }
    pub fn bytes(self) -> usize {
        match self {
            Depth::Byte => 1,
            Depth::Short => 2,
            Depth::Float => 4,
        }
    }
}

/// OFX pixel components this host can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comps {
    Rgba,
    Rgb,
    Alpha,
}

impl Comps {
    pub fn name(self) -> &'static str {
        match self {
            Comps::Rgba => COMP_RGBA,
            Comps::Rgb => COMP_RGB,
            Comps::Alpha => COMP_ALPHA,
        }
    }
    pub fn from_name(s: &str) -> Option<Comps> {
        match s {
            COMP_RGBA => Some(Comps::Rgba),
            COMP_RGB => Some(Comps::Rgb),
            COMP_ALPHA => Some(Comps::Alpha),
            _ => None,
        }
    }
    pub fn channels(self) -> usize {
        match self {
            Comps::Rgba => 4,
            Comps::Rgb => 3,
            Comps::Alpha => 1,
        }
    }
}

/// A 4-byte-aligned pixel buffer the plug-in reads (and, for the output image, writes) through
/// a raw pointer while the host keeps it alive.
pub struct PixBuf {
    cell: UnsafeCell<Vec<f32>>,
    bytes: usize,
}

// The buffer is only written by the plug-in's render (one effect instance at a time owns the
// output image) or by the host before / after the plug-in runs.
unsafe impl Sync for PixBuf {}
unsafe impl Send for PixBuf {}

impl PixBuf {
    pub fn zeroed(bytes: usize) -> PixBuf {
        PixBuf { cell: UnsafeCell::new(vec![0.0; bytes.div_ceil(4).max(1)]), bytes }
    }
    pub fn ptr(&self) -> *mut u8 {
        unsafe { (*self.cell.get()).as_mut_ptr() as *mut u8 }
    }
    /// # Safety
    /// No plug-in may be running that holds this buffer.
    // The bytes live in an `UnsafeCell`; exclusivity is the caller's contract above.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn bytes_mut(&self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr(), self.bytes) }
    }
    /// # Safety
    /// No plug-in may be writing this buffer.
    pub unsafe fn bytes_ref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr(), self.bytes) }
    }
}

/// One image in the plug-in's pixel format, plus its geometry in OFX pixel coordinates.
pub struct ClipImage {
    pub buf: PixBuf,
    /// The addressable pixels (y up, x2/y2 exclusive).
    pub bounds: OfxRectI,
    /// The region of definition.
    pub rod: OfxRectI,
    pub row_bytes: i32,
    pub depth: Depth,
    pub comps: Comps,
    pub premult: &'static str,
}

impl ClipImage {
    pub fn width(&self) -> usize {
        (self.bounds.x2 - self.bounds.x1).max(0) as usize
    }
    pub fn height(&self) -> usize {
        (self.bounds.y2 - self.bounds.y1).max(0) as usize
    }
    pub fn pixel_bytes(&self) -> usize {
        self.depth.bytes() * self.comps.channels()
    }

    /// A transparent image covering `bounds`.
    pub fn blank(bounds: OfxRectI, rod: OfxRectI, depth: Depth, comps: Comps) -> ClipImage {
        let w = (bounds.x2 - bounds.x1).max(0) as usize;
        let h = (bounds.y2 - bounds.y1).max(0) as usize;
        let px = depth.bytes() * comps.channels();
        ClipImage { buf: PixBuf::zeroed(w * h * px), bounds, rod, row_bytes: (w * px) as i32, depth, comps, premult: IMAGE_PREMULTIPLIED }
    }

    /// Convert the sub-rectangle `[x0, x1) x [y0, y1)` of an EffectCraft buffer (`sw` pixels per
    /// row, row 0 = top) into an OFX image with the given geometry.
    pub fn from_rows(
        src: &[[f32; 4]],
        sw: usize,
        crop: (usize, usize, usize, usize),
        bounds: OfxRectI,
        rod: OfxRectI,
        depth: Depth,
        comps: Comps,
    ) -> ClipImage {
        let (x0, y0, x1, y1) = crop;
        let w = x1.saturating_sub(x0);
        let h = y1.saturating_sub(y0);
        let img = ClipImage::blank(bounds, rod, depth, comps);
        if img.width() != w || img.height() != h {
            return img;
        }
        let px = img.pixel_bytes();
        let row_bytes = img.row_bytes as usize;
        // SAFETY: freshly created, not shared yet.
        let out = unsafe { img.buf.bytes_mut() };
        for j in 0..h {
            // OFX row j (from the bottom) is EffectCraft row y1 - 1 - j.
            let sy = y1 - 1 - j;
            let Some(row) = src.get(sy * sw + x0..sy * sw + x0 + w) else { continue };
            let Some(dst) = out.get_mut(j * row_bytes..j * row_bytes + w * px) else { continue };
            for (p, d) in row.iter().zip(dst.chunks_exact_mut(px)) {
                encode_pixel(*p, depth, comps, d);
            }
        }
        img
    }

    /// Write the image back into an EffectCraft buffer of exactly the image's size. Non-finite
    /// values become 0; `premultiply` converts straight-alpha output to premultiplied.
    pub fn write_rows(&self, dst: &mut [[f32; 4]], premultiply: bool) {
        let (w, h) = (self.width(), self.height());
        if dst.len() != w * h || w == 0 {
            return;
        }
        let px = self.pixel_bytes();
        let row_bytes = self.row_bytes as usize;
        // SAFETY: the plug-in's render has returned.
        let src = unsafe { self.buf.bytes_ref() };
        for j in 0..h {
            let dy = h - 1 - j;
            let Some(row) = src.get(j * row_bytes..j * row_bytes + w * px) else { continue };
            let Some(out) = dst.get_mut(dy * w..dy * w + w) else { continue };
            for (s, d) in row.chunks_exact(px).zip(out.iter_mut()) {
                let mut p = decode_pixel(s, self.depth, self.comps);
                if premultiply {
                    p[0] *= p[3];
                    p[1] *= p[3];
                    p[2] *= p[3];
                }
                for c in &mut p {
                    if !c.is_finite() {
                        *c = 0.0;
                    }
                }
                *d = p;
            }
        }
    }
}

fn encode_pixel(p: [f32; 4], depth: Depth, comps: Comps, out: &mut [u8]) {
    let vals: [f32; 4] = match comps {
        Comps::Rgba => p,
        Comps::Rgb => [p[0], p[1], p[2], 0.0],
        Comps::Alpha => [p[3], 0.0, 0.0, 0.0],
    };
    let n = comps.channels();
    for (i, v) in vals.iter().take(n).enumerate() {
        match depth {
            Depth::Float => {
                if let Some(o) = out.get_mut(i * 4..i * 4 + 4) {
                    o.copy_from_slice(&v.to_ne_bytes());
                }
            }
            Depth::Short => {
                let q = (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
                if let Some(o) = out.get_mut(i * 2..i * 2 + 2) {
                    o.copy_from_slice(&q.to_ne_bytes());
                }
            }
            Depth::Byte => {
                if let Some(o) = out.get_mut(i) {
                    *o = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        }
    }
}

fn decode_pixel(s: &[u8], depth: Depth, comps: Comps) -> [f32; 4] {
    let n = comps.channels();
    let mut v = [0.0f32; 4];
    for (i, slot) in v.iter_mut().take(n).enumerate() {
        *slot = match depth {
            Depth::Float => s.get(i * 4..i * 4 + 4).map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0.0),
            Depth::Short => s.get(i * 2..i * 2 + 2).map(|b| u16::from_ne_bytes([b[0], b[1]]) as f32 / 65535.0).unwrap_or(0.0),
            Depth::Byte => s.get(i).map(|b| *b as f32 / 255.0).unwrap_or(0.0),
        };
    }
    match comps {
        Comps::Rgba => v,
        Comps::Rgb => [v[0], v[1], v[2], 1.0],
        Comps::Alpha => [0.0, 0.0, 0.0, v[0]],
    }
}

// ---------------------------------------------------------------------------------------------
// Image handles
// ---------------------------------------------------------------------------------------------

/// The object behind the property-set handle `clipGetImage` returns (props first, so the
/// handle is also a valid `PropSet` pointer).
#[repr(C)]
pub struct ImageObj {
    props: PropSet,
    _img: Arc<ClipImage>,
}

fn live_images() -> &'static Mutex<HashSet<usize>> {
    static L: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Hand an image to the plug-in; the handle stays valid until [`release_image`].
pub fn new_image_handle(img: &Arc<ClipImage>, render_scale: f64, unique: u64) -> *mut c_void {
    let props = PropSet::new();
    props.put_str(P_TYPE, TYPE_IMAGE);
    props.put_ptr(IMG_DATA, img.buf.ptr() as usize);
    props.put_ints(IMG_BOUNDS, &[img.bounds.x1, img.bounds.y1, img.bounds.x2, img.bounds.y2]);
    props.put_ints(IMG_ROD, &[img.rod.x1, img.rod.y1, img.rod.x2, img.rod.y2]);
    props.put_int(IMG_ROW_BYTES, img.row_bytes);
    props.put_str(IMG_FIELD, FIELD_NONE);
    props.put_str(IMG_UNIQUE_ID, &format!("effectcraft-{unique}"));
    props.put_double(IMG_PIXEL_ASPECT_RATIO, 1.0);
    props.put_str(IE_PIXEL_DEPTH, img.depth.name());
    props.put_str(IE_COMPONENTS, img.comps.name());
    props.put_str(IE_PREMULTIPLICATION, img.premult);
    props.put_doubles(IE_RENDER_SCALE, &[render_scale, render_scale]);
    let obj = Box::new(ImageObj { props, _img: img.clone() });
    let ptr = Box::into_raw(obj) as usize;
    live_images().lock().unwrap_or_else(PoisonError::into_inner).insert(ptr);
    ptr as *mut c_void
}

/// Release a handle from [`new_image_handle`].
pub fn release_image(h: *mut c_void) -> OfxStatus {
    let ptr = h as usize;
    if ptr == 0 || !live_images().lock().unwrap_or_else(PoisonError::into_inner).remove(&ptr) {
        return K_STAT_ERR_BAD_HANDLE;
    }
    // SAFETY: the pointer was produced by Box::into_raw in `new_image_handle` and removed from
    // the live set exactly once.
    drop(unsafe { Box::from_raw(ptr as *mut ImageObj) });
    K_STAT_OK
}
