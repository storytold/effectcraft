//! The main window's size and position. The first launch fits the window to its monitor, opening
//! it maximized where the default size leaves no room for a taskbar or Dock. Later launches restore
//! the size, position and maximized state saved in the config directory (`window.json`), moved
//! onto a monitor that is still connected.

use effectcraft_engine::config::ConfigStore;
use serde::{Deserialize, Serialize};
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};

const FILE: &str = "window.json";
const MAX_BYTES: usize = 4096;
/// Seconds a window opened maximized gets to say so before its size is tracked.
const MAXIMIZE_WAIT: f64 = 2.0;

/// The window's geometry between launches.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    /// The outer top-left corner in physical pixels; `None` where the system doesn't say (Wayland).
    pub pos: Option<[i32; 2]>,
    /// The inner size in points; while maximized, the size to return to.
    pub size: [f32; 2],
    pub maximized: bool,
}

/// The first window's size, in points.
pub const DEFAULT_SIZE: [f32; 2] = [1680.0, 1020.0];
/// The smallest window, in points.
pub const MIN_SIZE: [f32; 2] = [960.0, 600.0];
/// Room the first window leaves around itself for a taskbar, Dock or menu bar (the system doesn't
/// say how much of the screen they take): a monitor with less room opens it maximized.
const SCREEN_ROOM: [f32; 2] = [64.0, 128.0];

/// A monitor: its top-left corner and size in physical pixels, and its scale factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    pos: [i64; 2],
    size: [i64; 2],
    scale: f32,
}

impl Monitor {
    pub fn new(pos: PhysicalPosition<i32>, size: PhysicalSize<u32>, scale: f64) -> Self {
        let scale = scale as f32;
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        Self { pos: [pos.x.into(), pos.y.into()], size: [size.width.into(), size.height.into()], scale }
    }

    /// Its size in points.
    fn points(&self) -> [f32; 2] {
        self.size.map(|s| s as f32 / self.scale)
    }

    /// Its size in points less room for a taskbar, Dock or menu bar.
    fn usable(&self) -> [f32; 2] {
        let points = self.points();
        std::array::from_fn(|i| points[i] - SCREEN_ROOM[i])
    }

    /// A window of `size` points on this monitor, in physical pixels.
    fn pixels(&self, size: [f32; 2]) -> [i64; 2] {
        size.map(|s| (s * self.scale) as i64)
    }

    /// How much of a window at `pos` of `size` points this monitor shows, in square pixels.
    fn overlap(&self, pos: [i32; 2], size: [f32; 2]) -> i64 {
        let px = self.pixels(size);
        let span = |i: usize| {
            let start = i64::from(pos[i]).max(self.pos[i]);
            let end = i64::from(pos[i]).saturating_add(px[i]).min(self.pos[i] + self.size[i]);
            (end - start).max(0)
        };
        span(0).saturating_mul(span(1))
    }

    /// The top-left corner nearest to `pos` (centred when none) that keeps a window of `size`
    /// points on this monitor.
    fn keep_on(&self, pos: Option<[i32; 2]>, size: [f32; 2]) -> [i32; 2] {
        let px = self.pixels(size);
        std::array::from_fn(|i| {
            let lo = self.pos[i];
            let hi = lo + self.size[i].saturating_sub(px[i]).max(0);
            let want = pos.map_or(lo + (hi - lo) / 2, |p| i64::from(p[i]));
            i32::try_from(want.max(lo).min(hi)).unwrap_or_default()
        })
    }
}

/// Where the window opens: `saved` (the last session's geometry) kept on a connected monitor, else
/// the default size on `home` (the monitor the system opens it on). Without monitors (the system
/// doesn't list them, so nothing to check a saved geometry against) the default size.
pub fn place(saved: Option<WindowGeometry>, monitors: &[Monitor], home: Option<usize>) -> WindowGeometry {
    let default = WindowGeometry { pos: None, size: DEFAULT_SIZE, maximized: false };
    let Some(home) = home.and_then(|i| monitors.get(i)).or(monitors.first()) else {
        return default;
    };
    let Some(saved) = saved else {
        let usable = home.usable();
        if (0..2).all(|i| DEFAULT_SIZE[i] <= usable[i]) {
            return default;
        }
        // A small screen: maximized, with a size that fits once un-maximized.
        let size = fit(DEFAULT_SIZE, usable);
        return WindowGeometry { pos: Some(home.keep_on(None, size)), size, maximized: true };
    };
    // The monitor showing most of the window; none when it was on a monitor that is gone.
    let shown = saved.pos.and_then(|p| {
        let (area, m) = monitors.iter().map(|m| (m.overlap(p, saved.size), m)).max_by_key(|(area, _)| *area)?;
        (area > 0).then_some(m)
    });
    let m = shown.unwrap_or(home);
    // Bigger than its monitor (a smaller screen than last time): maximized, like the first window.
    let room = m.points();
    let too_big = (0..2).any(|i| saved.size[i] > room[i]);
    let size = fit(saved.size, if too_big { m.usable() } else { room });
    WindowGeometry {
        // Moved fully onto its monitor, or centred on the home monitor.
        pos: saved.pos.map(|p| m.keep_on(shown.map(|_| p), size)),
        size,
        maximized: saved.maximized || too_big,
    }
}

/// `size` no bigger than `room`, and no smaller than the smallest window unless `room` is.
fn fit(size: [f32; 2], room: [f32; 2]) -> [f32; 2] {
    std::array::from_fn(|i| size[i].min(room[i]).max(MIN_SIZE[i].min(room[i])).max(1.0))
}

/// Size and place the new window (eframe shows it after drawing the first frame) and return its
/// geometry.
pub fn restore(window: &winit::window::Window, saved: Option<WindowGeometry>) -> WindowGeometry {
    let handles: Vec<_> = window.available_monitors().collect();
    let monitors: Vec<Monitor> = handles.iter().map(|m| Monitor::new(m.position(), m.size(), m.scale_factor())).collect();
    let home = window.current_monitor().or_else(|| window.primary_monitor()).and_then(|c| handles.iter().position(|m| *m == c));
    let g = place(saved, &monitors, home);
    if let Some([x, y]) = g.pos {
        window.set_outer_position(PhysicalPosition::new(x, y));
    }
    // Returns the new size when it applies at once; either way the window gets it.
    let _ = window.request_inner_size(LogicalSize::new(g.size[0], g.size[1]));
    if g.maximized {
        window.set_maximized(true);
    }
    g
}

/// Per frame: remember the window's geometry for the next launch. Minimized or full screen keeps
/// the last one; maximized only sets its flag, so un-maximizing next time restores the last size.
pub fn track(ctx: &egui::Context, geometry: &mut Option<WindowGeometry>) {
    let zoom = ctx.zoom_factor();
    ctx.input(|i| {
        let v = i.viewport();
        if v.minimized == Some(true) || v.fullscreen == Some(true) {
            return;
        }
        if v.maximized == Some(true) {
            if let Some(g) = geometry {
                g.maximized = true;
            }
            return;
        }
        let Some(inner) = v.inner_rect else { return };
        let pos = v.outer_rect.map(|r| [r.min.x, r.min.y].map(|c| (c * i.pixels_per_point).round() as i32));
        *geometry = Some(WindowGeometry { pos, size: [inner.width() * zoom, inner.height() * zoom], maximized: false });
    });
}

/// The geometry saved by the last session; `None` when there is none or it can't be read.
pub fn load(config: &dyn ConfigStore) -> Option<WindowGeometry> {
    let text = config.read_bounded(FILE, MAX_BYTES).ok()??;
    serde_json::from_str(&text).ok()
}

/// Follows the window and saves its geometry for the next launch: when it has changed (at most
/// once a second, so a session that ends badly still has it) and on exit.
pub struct Tracker {
    geometry: Option<WindowGeometry>,
    saved: Option<WindowGeometry>,
    last_attempt: f64,
    /// Opened maximized, and the system hasn't reported it so yet (when the wait ends): the
    /// window's first frames report the maximized size as an ordinary one, which would replace
    /// the size to return to.
    maximizing: Option<f64>,
    /// Agent-driven runs (`--control`) leave the user's saved window alone.
    save: bool,
}

impl Tracker {
    /// `geometry`: what the window opened with.
    pub fn new(geometry: Option<WindowGeometry>, saved: Option<WindowGeometry>, save: bool) -> Self {
        let maximizing = geometry.is_some_and(|g| g.maximized).then_some(f64::INFINITY);
        Self { geometry, saved, last_attempt: f64::NEG_INFINITY, maximizing, save }
    }

    /// Per frame.
    pub fn frame(&mut self, ctx: &egui::Context, config: Option<&dyn ConfigStore>) {
        let (now, maximized) = ctx.input(|i| (i.time, i.viewport().maximized == Some(true)));
        if let Some(until) = self.maximizing {
            // A system that never maximizes the window is believed after a moment.
            let until = until.min(now + MAXIMIZE_WAIT);
            self.maximizing = (!maximized && now < until).then_some(until);
        }
        if self.maximizing.is_none() {
            track(ctx, &mut self.geometry);
        }
        if now - self.last_attempt >= 1.0 {
            self.last_attempt = now;
            self.save(config);
        }
    }

    /// Write the geometry when it differs from the saved one.
    pub fn save(&mut self, config: Option<&dyn ConfigStore>) {
        let (Some(config), Some(g)) = (config, self.geometry) else { return };
        if !self.save || self.saved == Some(g) {
            return;
        }
        match serde_json::to_string(&g).map_err(|e| e.to_string()).and_then(|text| config.write(FILE, &text).map_err(|e| e.to_string())) {
            Ok(()) => self.saved = Some(g),
            Err(e) => log::warn!("could not save the window geometry: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_engine::config::MemoryConfig;

    fn monitor(x: i32, y: i32, w: u32, h: u32, scale: f64) -> Monitor {
        Monitor::new(PhysicalPosition::new(x, y), PhysicalSize::new(w, h), scale)
    }

    fn saved(pos: Option<[i32; 2]>, size: [f32; 2], maximized: bool) -> Option<WindowGeometry> {
        Some(WindowGeometry { pos, size, maximized })
    }

    #[test]
    fn the_first_window_fits_its_screen() {
        // 2560 × 1440: the default size, where the system puts it.
        let big = [monitor(0, 0, 2560, 1440, 1.0)];
        assert_eq!(place(None, &big, Some(0)), WindowGeometry { pos: None, size: DEFAULT_SIZE, maximized: false });
        // 1920 × 1080, 1600 × 900, 1366 × 768 and 1920 × 1080 at 150% (1280 × 720 points):
        // maximized, and a size that fits the screen with room for the taskbar once un-maximized.
        for (m, size) in [
            (monitor(0, 0, 1920, 1080, 1.0), [1680.0, 952.0]),
            (monitor(0, 0, 1600, 900, 1.0), [1536.0, 772.0]),
            (monitor(0, 0, 1366, 768, 1.0), [1302.0, 640.0]),
            (monitor(0, 0, 1920, 1080, 1.5), [1216.0, 592.0]),
        ] {
            let g = place(None, &[m], Some(0));
            assert!(g.maximized, "{m:?}");
            assert_eq!(g.size, size, "{m:?}");
            let [x, y] = g.pos.unwrap();
            let px = m.pixels(g.size);
            assert!(x >= 0 && y >= 0 && i64::from(x) + px[0] <= m.size[0] && i64::from(y) + px[1] <= m.size[1], "{m:?}: {g:?}");
        }
        // No monitor information: the default size, whatever was saved.
        assert_eq!(place(None, &[], None), WindowGeometry { pos: None, size: DEFAULT_SIZE, maximized: false });
        assert_eq!(place(saved(Some([9000, 9000]), [1e9, 1e9], true), &[], None).size, DEFAULT_SIZE);
    }

    #[test]
    fn the_saved_window_comes_back_where_it_was() {
        let two = [monitor(0, 0, 2560, 1440, 1.0), monitor(2560, 0, 1920, 1080, 1.0)];
        for g in [saved(Some([100, 80]), [1200.0, 800.0], false), saved(Some([2700, 50]), [1000.0, 700.0], true), saved(None, [1000.0, 700.0], false)] {
            assert_eq!(Some(place(g, &two, Some(0))), g);
        }
    }

    #[test]
    fn a_saved_window_stays_on_a_connected_monitor() {
        let laptop = [monitor(0, 0, 1600, 900, 1.0)];
        // Last time on a second monitor that is gone: centred on the remaining one.
        let g = place(saved(Some([2700, 50]), [1000.0, 700.0], false), &laptop, Some(0));
        assert_eq!(g, WindowGeometry { pos: Some([300, 100]), size: [1000.0, 700.0], maximized: false });
        // Half off the monitor's right and bottom edges: moved fully onto it.
        let g = place(saved(Some([1200, 600]), [1000.0, 700.0], false), &laptop, Some(0));
        assert_eq!(g.pos, Some([600, 200]));
        // Bigger than the screen now (it was on a larger one): it fits, and opens maximized.
        let g = place(saved(Some([0, 0]), [2400.0, 1300.0], false), &laptop, Some(0));
        assert_eq!(g, WindowGeometry { pos: Some([0, 0]), size: [1536.0, 772.0], maximized: true });
        // A monitor left of the primary one (negative coordinates) still counts.
        let left = [monitor(0, 0, 1920, 1080, 1.0), monitor(-1920, 0, 1920, 1080, 1.0)];
        assert_eq!(place(saved(Some([-1800, 40]), [1000.0, 700.0], false), &left, Some(0)).pos, Some([-1800, 40]));
        // Nonsense sizes from a damaged preferences file are kept within the screen.
        let g = place(saved(Some([i32::MAX, i32::MIN]), [-5.0, f32::MAX], false), &laptop, Some(0));
        assert_eq!(g, WindowGeometry { pos: Some([320, 64]), size: [960.0, 772.0], maximized: true });
        let g = place(saved(Some([5, 5]), [f32::NEG_INFINITY, f32::INFINITY], false), &laptop, Some(0));
        assert_eq!(g, WindowGeometry { pos: Some([320, 64]), size: [960.0, 772.0], maximized: true });
    }

    #[test]
    fn the_window_geometry_is_tracked_for_the_next_launch() {
        let ctx = egui::Context::default();
        let mut geometry = None;
        let mut frame = |info: egui::ViewportInfo| {
            let mut input = egui::RawInput::default();
            input.viewports.insert(egui::ViewportId::ROOT, info);
            ctx.run_ui(input, |ui| track(ui.ctx(), &mut geometry)).textures_delta.clear();
            geometry
        };
        let rect = egui::Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(1000.0, 600.0));
        let normal = egui::ViewportInfo {
            native_pixels_per_point: Some(1.5),
            inner_rect: Some(rect),
            outer_rect: Some(rect),
            maximized: Some(false),
            ..Default::default()
        };
        // Position in physical pixels, size in points.
        let g = WindowGeometry { pos: Some([30, 45]), size: [1000.0, 600.0], maximized: false };
        assert_eq!(frame(normal.clone()), Some(g));
        // Maximized or minimized: the size to restore stays.
        let big = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 700.0));
        let maximized = egui::ViewportInfo { inner_rect: Some(big), outer_rect: Some(big), maximized: Some(true), ..normal.clone() };
        assert_eq!(frame(maximized), Some(WindowGeometry { maximized: true, ..g }));
        let minimized = egui::ViewportInfo { inner_rect: None, outer_rect: None, minimized: Some(true), ..normal.clone() };
        assert_eq!(frame(minimized), Some(WindowGeometry { maximized: true, ..g }));
        assert_eq!(frame(normal), Some(g));
    }

    /// A window restored maximized keeps the size it returns to, though its first frames report
    /// the maximized size as an ordinary one.
    #[test]
    fn opening_maximized_keeps_the_size_to_return_to() {
        let ctx = egui::Context::default();
        let g = WindowGeometry { pos: Some([40, 30]), size: [1000.0, 600.0], maximized: true };
        let mut tracker = Tracker::new(Some(g), Some(g), true);
        let big = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1728.0, 1052.0));
        let small = egui::Rect::from_min_size(egui::pos2(40.0, 30.0), egui::vec2(900.0, 500.0));
        let frame = |tracker: &mut Tracker, time: f64, rect: egui::Rect, maximized: bool| {
            let info = egui::ViewportInfo { inner_rect: Some(rect), outer_rect: Some(rect), maximized: Some(maximized), ..Default::default() };
            let mut input = egui::RawInput { time: Some(time), ..Default::default() };
            input.viewports.insert(egui::ViewportId::ROOT, info);
            ctx.run_ui(input, |ui| tracker.frame(ui.ctx(), None)).textures_delta.clear();
            tracker.geometry
        };
        assert_eq!(frame(&mut tracker, 0.0, big, false), Some(g));
        assert_eq!(frame(&mut tracker, 0.5, big, true), Some(g));
        // Un-maximized by the user afterwards: tracked as usual.
        let restored = WindowGeometry { pos: Some([40, 30]), size: [900.0, 500.0], maximized: false };
        assert_eq!(frame(&mut tracker, 0.6, small, false), Some(restored));
        // A system that doesn't maximize: the window's real size is tracked after the wait.
        let mut tracker = Tracker::new(Some(g), Some(g), true);
        assert_eq!(frame(&mut tracker, 10.0, small, false), Some(g));
        assert_eq!(frame(&mut tracker, 10.0 + MAXIMIZE_WAIT, small, false), Some(restored));
    }

    /// The reported bug: the window opened at the default size and place on every launch.
    #[test]
    fn the_window_geometry_is_saved_and_loaded() {
        let config = MemoryConfig::default();
        assert_eq!(load(&config), None);
        let g = WindowGeometry { pos: Some([30, 45]), size: [1000.0, 600.0], maximized: true };
        let mut tracker = Tracker::new(Some(g), None, true);
        tracker.save(Some(&config));
        assert_eq!(load(&config), Some(g));
        // Positions the system doesn't report (Wayland) round-trip too.
        tracker.geometry = Some(WindowGeometry { pos: None, ..g });
        tracker.save(Some(&config));
        assert_eq!(load(&config), Some(WindowGeometry { pos: None, ..g }));
        // An agent-driven run leaves the saved window alone.
        Tracker::new(Some(g), None, false).save(Some(&config));
        assert_eq!(load(&config).and_then(|g| g.pos), None);
        // A damaged or oversized file is ignored: the window opens at its default.
        for text in ["{", "null", "{\"pos\":[1],\"size\":[1,2],\"maximized\":false}", &" ".repeat(MAX_BYTES + 1)] {
            config.write(FILE, text).unwrap();
            assert_eq!(load(&config), None, "{text:.20}");
        }
    }
}
