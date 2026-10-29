use super::*;

/// The application's monotonic clock, in seconds.
///
/// Neither `ctx.input(|i| i.time)` nor `RawInput::time` is valid in a logic-only tick:
/// `Context::run_logic` copies only the viewport information, so both are the previous full
/// pass's values. Playback, auto-save, screenshot deadlines and the control channel measure time
/// with this clock instead. Native and web use the same [`web_time::Instant`] source; tests
/// inject [`AppClock::manual`] and advance it.
#[derive(Clone, Debug)]
pub struct AppClock {
    origin: web_time::Instant,
    manual: Option<f64>,
}

impl AppClock {
    /// The host's monotonic clock.
    pub fn real() -> Self {
        Self { origin: web_time::Instant::now(), manual: None }
    }

    /// A clock tests advance explicitly with [`AppClock::advance`].
    pub fn manual() -> Self {
        Self { origin: web_time::Instant::now(), manual: Some(0.0) }
    }

    /// Seconds since the clock started (monotonic on native and in the browser).
    pub fn now(&self) -> f64 {
        match self.manual {
            Some(t) => t,
            None => self.origin.elapsed().as_secs_f64(),
        }
    }

    /// Move a manual clock forward; a real clock ignores this.
    pub fn advance(&mut self, seconds: f64) {
        if let Some(t) = &mut self.manual {
            *t = (*t + seconds.max(0.0)).max(0.0);
        }
    }

    /// Whether the clock is test-driven.
    pub fn is_manual(&self) -> bool {
        self.manual.is_some()
    }
}

impl Default for AppClock {
    /// The real monotonic clock (`Self::real()`).
    fn default() -> Self {
        Self::real()
    }
}

impl EffectcraftApp {
    /// The app clock's value at the last tick.
    pub fn now(&self) -> f64 {
        self.now
    }

    /// The clock itself (tests advance a manual clock).
    pub fn clock_mut(&mut self) -> &mut AppClock {
        &mut self.clock
    }

    /// Use a test-driven clock from now on.
    pub fn use_manual_clock(&mut self) {
        self.clock = AppClock::manual();
        self.now = 0.0;
    }
}
