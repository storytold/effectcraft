//! A controlled probe for testing edits and cancellation while an import worker is running.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use effectcraft_project::Footage;

use crate::Importer;

#[derive(Default)]
struct State {
    started: bool,
    released: bool,
}

#[derive(Default)]
struct Signal {
    state: Mutex<State>,
    changed: Condvar,
}

/// Hold a worker at its probe until released; dropping the control also releases it on panic.
#[derive(Default)]
pub(super) struct ProbeGate {
    signal: Arc<Signal>,
}

impl ProbeGate {
    /// Wrap a synthetic importer, blocking before its first probe can return.
    pub(super) fn wrap(&self, importer: Arc<dyn Importer>) -> Arc<dyn Importer> {
        Arc::new(Gated { signal: self.signal.clone(), importer })
    }

    /// Wait for the worker to enter its probe, with a bounded failure if it never starts.
    pub(super) fn wait_started(&self) {
        let state = self.signal.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, _) = self.signal.changed.wait_timeout_while(state, Duration::from_secs(10), |s| !s.started).unwrap_or_else(PoisonError::into_inner);
        let started = state.started;
        drop(state);
        assert!(started, "the import worker did not enter its probe");
    }

    /// Let the blocked probe and all subsequent probes complete.
    pub(super) fn release(&self) {
        self.signal.state.lock().unwrap_or_else(PoisonError::into_inner).released = true;
        self.signal.changed.notify_all();
    }
}

impl Drop for ProbeGate {
    /// Unblock a worker even if an assertion fails before the explicit release.
    fn drop(&mut self) {
        self.release();
    }
}

struct Gated {
    signal: Arc<Signal>,
    importer: Arc<dyn Importer>,
}

impl Importer for Gated {
    /// Announce the running probe and wait for the test before delegating to the fixture importer.
    fn probe(&self, path: &str) -> Result<Footage, String> {
        let mut state = self.signal.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.started = true;
        self.signal.changed.notify_all();
        let state = self.signal.changed.wait_while(state, |s| !s.released).unwrap_or_else(PoisonError::into_inner);
        drop(state);
        self.importer.probe(path)
    }
}
