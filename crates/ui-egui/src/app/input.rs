use super::*;

impl EffectcraftApp {
    /// Feed this frame's share of the queued synthetic input (control channel `ui.click`,
    /// `ui.drag`…) into `raw_input`. Pointer events go one per frame so egui sees press → moves
    /// → release as a real gesture.
    ///
    /// Only one batch is ever moved out at a time, even when the host prepares several hidden
    /// logic-only iterations before the forced UI pass: eframe keeps the last prepared raw input
    /// and would otherwise apply a whole drag or click as one burst. A batch is released by the
    /// first UI pass that applies it ([`Self::mark_synthetic_applied`]). Callers must not feed a
    /// pass that cannot draw (fonts still installing): egui applies what it receives, so putting
    /// an applied batch back would apply its pointer/key state twice.
    pub fn feed_synthetic(&mut self, raw_input: &mut egui::RawInput) {
        if self.synthetic_in_flight || self.synthetic.is_empty() {
            return;
        }
        // Pointer events go one per frame so egui sees press → moves → release as a real drag.
        let pointer = |e: &egui::Event| matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::MouseWheel { .. });
        let n = if pointer(&self.synthetic[0]) {
            1
        } else {
            self.synthetic
                .iter()
                .position(|e| pointer(e) || matches!(e, egui::Event::Key { pressed: false, .. }))
                .map_or(self.synthetic.len(), |i| if pointer(&self.synthetic[i]) { i.max(1) } else { i + 1 })
        };
        let batch: Vec<egui::Event> = self.synthetic.drain(..n).collect();
        raw_input.events.extend(batch.iter().cloned());
        self.synthetic_in_flight_events.extend(batch);
        self.synthetic_in_flight = true;
    }

    /// Clear the single-pending-batch guard: a UI pass has applied the injected batch.
    ///
    /// [`EffectcraftApp::draw`] calls this itself; harnesses that feed synthetic input
    /// themselves (the headless kittest tests) call it after each pass they ran.
    pub fn mark_synthetic_applied(&mut self) {
        self.synthetic_in_flight_events.clear();
        self.synthetic_in_flight = false;
    }

    /// The events of the injected batch that no UI pass has applied yet.
    pub fn pending_synthetic_events(&self) -> &[egui::Event] {
        &self.synthetic_in_flight_events
    }

    /// Whether a synthetic batch is waiting for its UI pass.
    pub fn synthetic_in_flight(&self) -> bool {
        self.synthetic_in_flight
    }
}

impl EffectcraftApp {
    /// Take the pending synthetic input (from `ui.click`, `ui.key`, …). Hosts that don't call
    /// [`eframe::App::raw_input_hook`] (the headless test harness) feed these in themselves.
    pub fn take_synthetic_input(&mut self) -> Vec<egui::Event> {
        std::mem::take(&mut self.synthetic)
    }
}
