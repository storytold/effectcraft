//! Screen-pixel sampling for the EffectCraft desktop color picker.
//!
//! The eyedropper must read the pixel the user can actually see, on any panel of the window,
//! not a frame kept inside the compositor. On Windows this is a `GetDC(NULL)` / `GetPixel`
//! pair. The calls are wrapped here, in a tiny dependency-free crate outside the main workspace
//! (which forbids `unsafe`), and exposed as safe functions to the application.
//!
//! Coordinates are **physical screen pixels**. winit makes the process per-monitor DPI aware,
//! so `Window::inner_position()` and the pointer's physical position share this space; egui's
//! points are converted with `pixels_per_point` before they reach this crate.
//!
//! The hardware cursor is not part of a GDI screen DC, so the crosshair never contaminates
//! the sampled pixel.

/// An opaque screen pixel in sRGB 8-bit components.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenPixel {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ScreenPixel {
    /// The pixel as sRGB components in `0.0..=1.0`, the format EffectCraft stores.
    pub fn srgb(self) -> [f32; 3] {
        [self.r as f32 / 255.0, self.g as f32 / 255.0, self.b as f32 / 255.0]
    }
}

/// Decode a Win32 `COLORREF` (`0x00BBGGRR`) into sRGB components.
fn decode_colorref(value: u32) -> ScreenPixel {
    ScreenPixel { r: (value & 0xff) as u8, g: ((value >> 8) & 0xff) as u8, b: ((value >> 16) & 0xff) as u8 }
}

/// The whole virtual desktop's bounds `(x, y, width, height)` in physical pixels, or `None`
/// when the platform cannot report it. Using the virtual desktop (not just the primary monitor)
/// keeps points on a secondary monitor valid.
pub fn screen_bounds() -> Option<(i32, i32, i32, i32)> {
    #[cfg(windows)]
    {
        const SM_XVIRTUALSCREEN: i32 = 76;
        const SM_YVIRTUALSCREEN: i32 = 77;
        const SM_CXVIRTUALSCREEN: i32 = 78;
        const SM_CYVIRTUALSCREEN: i32 = 79;
        let (x, y, w, h) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        (w > 0 && h > 0).then_some((x, y, w, h))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Read the visible pixel at physical screen coordinates `(x, y)`.
///
/// `None` means the platform has no screen sampler, the point is off screen, or Windows
/// reported `CLR_INVALID` (for instance while the desktop is locked).
pub fn sample(x: i32, y: i32) -> Option<ScreenPixel> {
    let Some((left, top, width, height)) = screen_bounds() else { return None };
    let inside = (x as i64) >= left as i64 && (y as i64) >= top as i64 && (x as i64) < left as i64 + width as i64 && (y as i64) < top as i64 + height as i64;
    if !inside {
        return None;
    }
    #[cfg(windows)]
    {
        // SAFETY: `GetDC(NULL)` returns a screen DC or null; `ReleaseDC` is always paired with
        // a successful `GetDC`, and `GetPixel` only reads the single pixel at `(x, y)`, which
        // was checked to be on screen. No pointers or out parameters are passed to Windows.
        unsafe {
            let hdc = GetDC(0);
            if hdc == 0 {
                return None;
            }
            let color = GetPixel(hdc, x, y);
            ReleaseDC(0, hdc);
            // CLR_INVALID is returned for a point outside the DC's clipping region.
            (color != u32::MAX).then(|| decode_colorref(color))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = decode_colorref(0);
        None
    }
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn GetDC(hwnd: isize) -> isize;
    fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
}

#[cfg(windows)]
#[link(name = "gdi32")]
extern "system" {
    fn GetPixel(hdc: isize, x: i32, y: i32) -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_is_bgr() {
        assert_eq!(decode_colorref(0x00ff_8000), ScreenPixel { r: 0x00, g: 0x80, b: 0xff });
        assert_eq!(decode_colorref(0x0000_00ff).srgb(), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn off_screen_points_are_rejected() {
        assert!(sample(-1, -1).is_none());
        assert!(sample(i32::MAX, i32::MAX).is_none());
    }
}
