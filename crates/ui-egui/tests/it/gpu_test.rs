//! Shared device-creation gate for the integration-test executable.

static GPU_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    static HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) struct Guard {
    held: Option<std::sync::MutexGuard<'static, ()>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.held.is_some() {
            HELD.with(|held| held.set(false));
        }
    }
}

/// GPU-backed harnesses hold this for their whole test; screenshots may enter it again.
pub(crate) fn lock() -> Guard {
    let held = if HELD.with(std::cell::Cell::get) {
        None
    } else {
        let guard = GPU_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        HELD.with(|held| held.set(true));
        Some(guard)
    };
    Guard { held }
}

/// Plain harnesses create their GPU renderer lazily on the first screenshot.
pub(crate) trait RenderSerial {
    fn render_serialized(&mut self) -> Result<image::RgbaImage, String>;
}

impl<State> RenderSerial for egui_kittest::Harness<'_, State> {
    fn render_serialized(&mut self) -> Result<image::RgbaImage, String> {
        let _device = lock();
        self.render().map_err(|error| format!("{error:?}"))
    }
}
