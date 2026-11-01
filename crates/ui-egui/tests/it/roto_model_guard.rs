//! The Roto Brush model is process-global (`effects::roto::set_model`), and every test in this binary shares the
//! process: a test that installs a scripted model holds this guard, so another cannot swap or clear the model
//! mid-run, and the model is cleared again even when the test panics.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use effectcraft_segment::MaskModel;

static LOCK: Mutex<()> = Mutex::new(());

pub struct InstalledModel {
    _exclusive: MutexGuard<'static, ()>,
}

/// Install `model` for the roto effect until the returned guard drops.
pub fn install(model: Arc<dyn MaskModel>) -> InstalledModel {
    let guard = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    effectcraft_engine::effects::roto::set_model(Some(model));
    InstalledModel { _exclusive: guard }
}

impl Drop for InstalledModel {
    fn drop(&mut self) {
        effectcraft_engine::effects::roto::set_model(None);
    }
}
