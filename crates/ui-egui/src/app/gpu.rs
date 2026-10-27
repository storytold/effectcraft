use super::*;

impl EffectcraftApp {
    pub fn render_source(&self) -> RenderSource {
        RenderSource {
            project: self.session.project.clone(),
            footage: self.session.footage.clone(),
            expr: self.session.expr.clone(),
            layer_cache: self.session.layer_cache.clone(),
            gpu: self.gpu.clone(),
            gpu_display: self.gpu_display(),
            disk: self.session.disk_cache.clone(),
        }
    }

    /// Keep GPU-composited viewer frames on the GPU (drawn from their texture). The browser can't
    /// read frames back (its thread can't wait for the GPU), so there the viewer shows GPU frames
    /// directly whenever it draws them plainly (RGB, no exposure, no region of interest); the
    /// desktop reads them back (see [`frames::RenderSource::gpu_display`]).
    fn gpu_display(&self) -> bool {
        let v = &self.session.state.viewer;
        cfg!(target_arch = "wasm32")
            && v.channel == effectcraft_engine::viewer::Channel::Rgb
            && v.exposure == 0.0
            && self.session.state.region_of_interest.is_none()
            && self.ui.viewer.extended.is_none()
    }

    /// The UI draws its panels and handles input such as dropped files. Until the fonts are
    /// installed (the first frames) it draws nothing, and that input goes unhandled.
    pub fn is_ready(&self) -> bool {
        self.fonts_ready
    }

    /// Provide the executable/embedding host's device notifications. The host installs the
    /// handlers once; this frontend never replaces handlers on a borrowed device.
    pub fn set_gpu_failure_bridge(&mut self, bridge: gpu_failure::GpuFailureBridge) {
        if let Some(gpu) = &self.gpu {
            bridge.forward_to(gpu.context().device_failure_notifier());
        }
        self.gpu_failures = Some(bridge);
    }

    pub(super) fn apply_gpu_failure(&mut self, ctx: &egui::Context) {
        let Some(bridge) = &self.gpu_failures else { return };
        // Readback deadlines can retire the compositor without a host device notification.
        if let Some(gpu) = &self.gpu
            && let Err(reason) = gpu.context().check_health()
        {
            bridge.report(&reason, false);
        }
        let Some(failure) = bridge.failure() else { return };
        if self.gpu_failure_applied == Some(failure.presentation_lost) {
            return;
        }
        self.gpu_failure_applied = Some(failure.presentation_lost);
        self.gpu_checked = true;
        self.frames.retire_gpu();
        self.session.accel = None;
        self.session.accel_note = Some(format!("GPU preview retired: {}", failure.reason));
        self.session.layer_cache.clear();
        self.gpu = None;
        if let Some(rs) = &self.wgpu {
            let mut renderer = rs.renderer.write();
            if let Some((id, _, _)) = &self.viewer_native {
                renderer.free_texture(id);
            }
            for (_, texture) in self.passive_tex.values() {
                if let panels::viewers::PassiveTexture::Gpu(id, _) = texture {
                    renderer.free_texture(id);
                }
            }
        }
        self.viewer_native = None;
        self.passive_tex.retain(|_, (_, texture)| matches!(texture, panels::viewers::PassiveTexture::Cpu(_)));
        self.viewer_shown = self.viewer_tex.as_ref().map(|(texture, key, _)| (texture.id(), *key));
        self.viewer_image = None;
        // Old completion closures retain only their detached slot, not the replacement.
        self.viewer_readback = Default::default();
        self.wgpu = None;
        self.ui.status = if failure.presentation_lost {
            "Graphics device lost. The project is still open; presentation requires restarting the application.".into()
        } else {
            "Shared GPU preview disabled; CPU compositing remains available.".into()
        };
        log::warn!("GPU preview retired: {}", failure.reason);
        ctx.request_repaint();
    }

    /// After the graphics device is lost, eframe's renderer cannot draw again: any mesh makes
    /// egui-wgpu panic on the main thread when it cannot stage its index buffer (#591). Draw
    /// nothing from then on, auto-save a project with unsaved changes (keeping the crash
    /// recovery marker, so the next launch offers it) and close the window.
    pub(super) fn close_after_device_loss(&mut self, ctx: &egui::Context) {
        if self.device_lost_close.is_some() {
            return;
        }
        let keep_recovery = self.session.is_dirty();
        let saved = keep_recovery.then(|| match self.session.autosave_now().and_then(|path| self.session.autosave_wait().map(|()| path)) {
            Ok(path) => {
                log::warn!("graphics device lost: auto-saved {path}; the next launch offers to recover it");
                Ok(path)
            }
            Err(e) => {
                log::error!("graphics device lost: {e}");
                Err(e.to_string())
            }
        });
        self.device_lost_close = Some(keep_recovery);
        log::warn!("graphics device lost: closing the window");
        if let Some(tell) = &self.hooks.device_lost {
            tell(saved);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Run the commands whose non-blocking file dialogs have been answered.
    pub(crate) fn run_dialog_answers(&mut self) {
        while let Ok(answer) = self.dialog_answers.1.try_recv() {
            self.file_dialogs.answered();
            menus::dialog_answered(self, answer);
        }
    }

    /// Set up the GPU compositor on egui-wgpu's device (once). Without an adapter that runs
    /// compute shaders (WebGL2, software GL) everything stays on the CPU.
    pub(crate) fn init_gpu(&mut self, frame: &eframe::Frame) {
        if self.gpu_checked {
            return;
        }
        self.gpu_checked = true;
        if self.gpu_failures.as_ref().is_some_and(|bridge| bridge.failure().is_some()) {
            return;
        }
        let Some(rs) = frame.wgpu_render_state() else { return };
        // Help ▸ System Compatibility Report names the adapter and backend the window got.
        let info = rs.adapter.get_info();
        self.session.window_adapter = Some(format!(
            "{} ({:?}, {:?}{})",
            info.name,
            info.backend,
            info.device_type,
            if info.driver.is_empty() { String::new() } else { format!(", {} {}", info.driver, info.driver_info) }
        ));
        // Backend validation can panic on shader compilation even when device creation
        // succeeded. Keep the document and the egui renderer, and use CPU compositing.
        let gpu = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| effectcraft_gpu::Gpu::new(&rs.adapter, rs.device.clone(), rs.queue.clone())));
        match gpu {
            Ok(Ok(g)) => {
                if let Some(bridge) = &self.gpu_failures {
                    bridge.forward_to(g.context().device_failure_notifier());
                    if bridge.failure().is_some() {
                        return;
                    }
                }
                log::info!("GPU compositor: {}", effectcraft_engine::render::Accelerator::name(&g));
                self.session.accel = Some(Arc::new(g.clone()));
                self.gpu = Some(g);
                self.wgpu = Some(rs.clone());
            }
            Ok(Err(e)) => {
                log::info!("GPU compositor unavailable: {e}");
                self.session.accel_note = Some(format!("GPU compositor unavailable: {e}"));
            }
            Err(e) => {
                let reason = e.downcast_ref::<String>().map(String::as_str).or_else(|| e.downcast_ref::<&str>().copied()).unwrap_or("backend panicked");
                log::warn!("GPU compositor setup failed; using CPU compositing: {reason}");
                self.session.accel_note = Some(format!("GPU compositor setup failed: {reason}"));
            }
        }
    }

    /// The viewer shows a frame composited on the GPU (drawn from its wgpu texture).
    pub fn viewer_on_gpu(&self) -> bool {
        matches!((&self.viewer_shown, &self.viewer_native), (Some(s), Some(n)) if s.1 == n.1)
    }

    /// The GPU compositor's adapter, when the viewer has one.
    pub fn gpu_adapter(&self) -> Option<String> {
        self.gpu.as_ref().map(effectcraft_engine::render::Accelerator::name)
    }
}

#[cfg(test)]
mod gpu_failure_tests {
    use super::*;

    #[test]
    fn host_failure_preserves_document_undo_and_cpu_pixels_and_detaches_late_readbacks() {
        let mut session = Session::default();
        let comp = ItemId(session.execute("comp.new", json!({"name": "Recovery", "width": 4, "height": 4, "duration": 1})).unwrap()["comp"].as_u64().unwrap());
        session.execute("layer.newSolid", json!({"comp": comp.0, "color": "#ff0000"})).unwrap();
        let before = session.render(comp, Tick::ZERO, RenderOpts::default());
        let project = session.project.clone();
        let revision = session.revision;
        let prefs = serde_json::to_value(&session.prefs).unwrap();
        let mut app = EffectcraftApp::new(session);
        let ctx = egui::Context::default();
        let bridge = gpu_failure::GpuFailureBridge::new(&ctx);
        app.set_gpu_failure_bridge(bridge.clone());
        let late_slot = app.viewer_readback.1.clone();
        let key = FrameKey { revision, content: 1, comp: comp.0, frame: 0, scale: 1000, view: 0, opts: 0 };
        app.viewer_readback.0 = Some(key);
        bridge.report("synthetic host device failure", true);
        app.apply_gpu_failure(&ctx);
        assert!(Arc::ptr_eq(&project, &app.session.project));
        assert_eq!(app.session.revision, revision);
        assert_eq!(serde_json::to_value(&app.session.prefs).unwrap(), prefs);
        assert!(app.gpu_checked, "failed device cannot be initialized again next frame");
        assert!(app.session.accel.is_none());
        assert!(app.ui.status.contains("presentation requires restarting"));
        assert_eq!(app.session.render(comp, Tick::ZERO, RenderOpts::default()).data, before.data);
        *late_slot.lock().unwrap() = Some((key, Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::RED]))));
        assert!(app.viewer_readback.0.is_none());
        assert!(app.viewer_readback.1.lock().unwrap().is_none(), "a late completion cannot refill the replacement slot");
        app.session.execute("edit.undo", json!({})).unwrap();
        assert!(app.session.project.comp(comp).unwrap().layers.is_empty());
        app.session.execute("edit.redo", json!({})).unwrap();
        assert_eq!(app.session.render(comp, Tick::ZERO, RenderOpts::default()).data, before.data);
    }
}
