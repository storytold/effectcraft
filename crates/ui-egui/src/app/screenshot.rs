use super::*;

/// How long a screenshot request waits for a frame, from admission (app clock seconds).
pub(crate) const SCREENSHOT_TIMEOUT_S: f64 = 8.0;

/// Seconds between a screenshot's admission and the earliest capture: the UI settles first.
pub(crate) const SCREENSHOT_SETTLE_S: f64 = 0.35;

/// Final (non-discarded) UI passes a screenshot waits for after admission before it is issued.
pub(crate) const SCREENSHOT_SETTLE_PASSES: u64 = 3;

/// Geometry changes a queued screenshot may retry before it reports an error.
const SCREENSHOT_MAX_RETRIES: u32 = 4;

/// Outstanding screenshot requests (queued or waiting for their pixels).
pub(crate) const MAX_PENDING_SHOTS: usize = 4;

/// One capture's pixels: 128 MiB.
const MAX_SHOT_BYTES: usize = 128 * 1024 * 1024;

/// Every pending capture's reserved pixels together: 256 MiB.
const MAX_SHOT_TOTAL_BYTES: usize = 256 * 1024 * 1024;

/// Whether a captured image's dimensions match `rect` at `ppp`.
///
/// The surface may be smaller than requested when the adapter-limit fit (fork E2) applies, but a
/// different aspect ratio means the pixels came from another layout than the one issued.
fn shot_image_matches(image: &egui::ColorImage, rect: egui::Rect, ppp: f32) -> bool {
    let [w, h] = image.size;
    if w == 0 || h == 0 || !ppp.is_finite() || ppp <= 0.0 {
        return false;
    }
    let ew = (rect.width() as f64 * ppp as f64).max(1.0);
    let eh = (rect.height() as f64 * ppp as f64).max(1.0);
    let want = ew / eh;
    let got = w as f64 / h as f64;
    (got - want).abs() <= 0.02 * want.max(1.0)
}

impl EffectcraftApp {
    /// The geometry of the last completed UI pass, if one ran.
    pub fn draw_geometry(&self) -> Option<DrawGeometry> {
        self.draw_geom
    }

    // ---------------------------------------------------------------- screenshots

    /// The root surface geometry a screenshot is admitted against.
    ///
    /// The last completed pass's geometry is authoritative. Before any pass, only the viewport's
    /// `inner_rect` (the client surface a capture returns) is accepted: `outer_rect` also covers
    /// the title bar and window frame, so a capture issued against it would be cropped and
    /// validated against a different rectangle than the callback delivers. A request that arrives
    /// before the geometry is known is refused with an actionable error, and the pass that would
    /// establish it is requested.
    fn current_geometry(&self, ctx: &egui::Context) -> Option<DrawGeometry> {
        if let Some(g) = self.draw_geom {
            return Some(g);
        }
        let rect = ctx.input(|i| i.viewport().inner_rect)?;
        (rect.width() > 0.0 && rect.height() > 0.0).then_some(DrawGeometry { rect, ppp: ctx.pixels_per_point(), epoch: self.geometry_epoch })
    }

    /// The point rectangle a request captures: its crop, clamped to the window, or all of it.
    fn shot_target_rect(geom: &DrawGeometry, crop: Option<[f32; 4]>) -> Option<egui::Rect> {
        let r = match crop {
            Some([x, y, w, h]) if [x, y, w, h].iter().all(|v| v.is_finite()) => {
                egui::Rect::from_min_size(geom.rect.min + egui::vec2(x, y), egui::vec2(w.max(0.0), h.max(0.0))).intersect(geom.rect)
            }
            Some(_) => return None,
            None => geom.rect,
        };
        (r.width() > 0.0 && r.height() > 0.0).then_some(r)
    }

    /// The screen-point crop `target` names right now (a panel's or element's current rectangle),
    /// or an actionable error when it is not on screen. Resolved at issuance, never stored from
    /// admission: the UI may move while it settles.
    fn shot_crop(&self, target: &ShotTarget, geom: &DrawGeometry) -> Result<Option<egui::Rect>, String> {
        let crop = match target {
            ShotTarget::Window => None,
            ShotTarget::Rect(r) => Some(*r),
            ShotTarget::Panel(p) => {
                let id = format!("panel.{}", p.id());
                let found = self.auto.find(&id).ok_or_else(|| format!("the {} panel is not on screen", p.title()))?;
                Some(found.rect)
            }
            ShotTarget::Id(id) => Some(self.auto.find(id).ok_or_else(|| format!("no element `{id}` is on screen"))?.rect),
        };
        Self::shot_target_rect(geom, crop).map(Some).ok_or_else(|| "the screenshot area is empty or not a finite rectangle".to_string())
    }

    /// `target` as a crop relative to `geom`'s origin: what the image's own pixels are cut with.
    fn shot_relative_crop(&self, target: &ShotTarget, geom: &DrawGeometry) -> Result<Option<[f32; 4]>, String> {
        let Some(r) = self.shot_crop(target, geom)? else { return Ok(None) };
        Ok(Some([r.min.x - geom.rect.min.x, r.min.y - geom.rect.min.y, r.width(), r.height()]))
    }

    /// Pixel bytes `rect` takes at `ppp`, checked (`width * height * 4`).
    fn shot_pixel_bytes(rect: egui::Rect, ppp: f32) -> Option<usize> {
        if !ppp.is_finite() || ppp <= 0.0 {
            return None;
        }
        let w = (rect.width() as f64 * ppp as f64).ceil().max(1.0) as usize;
        let h = (rect.height() as f64 * ppp as f64).ceil().max(1.0) as usize;
        w.checked_mul(h)?.checked_mul(4)
    }

    /// Reserve and queue one `ui.screenshot`, or reply with an actionable error.
    pub(crate) fn admit_screenshot(&mut self, ctx: &egui::Context, path: Option<String>, target: ShotTarget, reply: Sender<serde_json::Value>) {
        let Some(geom) = self.current_geometry(ctx) else {
            let _ = reply.send(
                json!({"ok": false, "error": "the window geometry is not known yet (it is recorded by a completed UI pass; retry after the next frame)"}),
            );
            // Ask for the pass that records it.
            self.want_paint = true;
            ctx.request_repaint();
            return;
        };
        if self.pending_screenshots.len() >= MAX_PENDING_SHOTS {
            let _ = reply.send(json!({"ok": false, "error": format!("too many screenshots are pending (at most {MAX_PENDING_SHOTS})")}));
            return;
        }
        if let Err(e) = self.shot_crop(&target, &geom) {
            let _ = reply.send(json!({"ok": false, "error": e}));
            return;
        }
        // The capture is always the whole surface (the crop is applied when the PNG is written),
        // so the reservation and the per-image bound follow the surface's actual dimensions.
        let Some(bytes) = Self::shot_pixel_bytes(geom.rect, geom.ppp) else {
            let _ = reply.send(json!({"ok": false, "error": "the screenshot area is too large to reserve"}));
            return;
        };
        if bytes > MAX_SHOT_BYTES {
            let _ = reply
                .send(json!({"ok": false, "error": format!("the screenshot would need about {bytes} bytes, over the {MAX_SHOT_BYTES}-byte per-image limit")}));
            return;
        }
        if self.shot_bytes.checked_add(bytes).is_none_or(|n| n > MAX_SHOT_TOTAL_BYTES) {
            let _ = reply.send(json!({"ok": false, "error": format!("the screenshot would exceed the {MAX_SHOT_TOTAL_BYTES}-byte pending-image budget")}));
            return;
        }
        self.shot_bytes += bytes;
        self.pending_screenshots.push(PendingShot {
            path,
            target,
            crop: None,
            reply,
            deadline: self.now + SCREENSHOT_TIMEOUT_S,
            settle_at: self.now + SCREENSHOT_SETTLE_S,
            admitted: geom,
            admitted_pass: self.ui_pass_seq,
            issued: None,
            slot: None,
            reserved_bytes: bytes,
            attempts: 0,
        });
        // A queued capture needs a settled UI pass before it can be issued.
        self.want_paint = true;
    }

    /// Re-reserve request `i` against `geom` after a surface change; false when it no longer fits.
    fn refresh_shot_reservation(&mut self, i: usize, geom: &DrawGeometry) -> bool {
        let Some(bytes) = Self::shot_pixel_bytes(geom.rect, geom.ppp) else { return false };
        if bytes > MAX_SHOT_BYTES {
            return false;
        }
        let released = self.shot_bytes.saturating_sub(self.pending_screenshots[i].reserved_bytes);
        if released.checked_add(bytes).is_none_or(|n| n > MAX_SHOT_TOTAL_BYTES) {
            return false;
        }
        self.shot_bytes = released + bytes;
        self.pending_screenshots[i].reserved_bytes = bytes;
        true
    }

    /// Reply with an error to request `i` and release its reservation.
    fn fail_shot(&mut self, i: usize, message: impl Into<String>) {
        let shot = self.pending_screenshots.remove(i);
        self.shot_bytes = self.shot_bytes.saturating_sub(shot.reserved_bytes);
        let _ = shot.reply.send(json!({"ok": false, "error": message.into()}));
    }

    /// Collect completed captures, enforce timeouts and issue every settled request.
    ///
    /// Runs from [`Self::tick`] only: the callback stores pixels in its slot and wakes the
    /// viewport, and everything that can write a file or reply happens here, on the app thread.
    pub(super) fn tick_screenshots(&mut self, ctx: &egui::Context) {
        let Some(geom) = self.current_geometry(ctx) else { return };
        self.drain_completed_screenshots(geom);
        self.timeout_screenshots();
        self.issue_screenshots(ctx, geom);
        // 4. Wake up for the earliest deadline even when no further pass is otherwise needed: an
        //    issued capture whose callback never arrives (hidden window, display asleep, renderer
        //    failure) would otherwise never reach its timeout. This is a timer, not a paint
        //    request: the issued screenshot command already forces the frame it needs.
        if let Some(next) = self.pending_screenshots.iter().map(|s| s.deadline).reduce(f64::min) {
            let wait = (next - self.now).max(0.001);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
    }

    /// 1. Drain completed slots. A completion whose surface epoch changed since issuance is
    ///    discarded even if the geometry changed back: it was cropped with another frame's
    ///    pixels-per-point.
    fn drain_completed_screenshots(&mut self, geom: DrawGeometry) {
        let mut i = 0;
        while i < self.pending_screenshots.len() {
            let taken = self.pending_screenshots.get(i).and_then(|s| s.slot.as_ref()).and_then(craft_ui::capture::CaptureSlot::take);
            let Some(result) = taken else {
                i += 1;
                continue;
            };
            let issued = self.pending_screenshots[i].issued;
            match result {
                Ok(image) => {
                    if issued.is_none_or(|g| g.epoch != geom.epoch) {
                        self.pending_screenshots[i].slot = None;
                        self.pending_screenshots[i].issued = None;
                        if self.pending_screenshots[i].attempts >= SCREENSHOT_MAX_RETRIES {
                            self.fail_shot(i, "the window resized while the screenshot was being taken");
                        } else if !self.refresh_shot_reservation(i, &geom) {
                            self.fail_shot(i, "the resized window is too large to capture within the screenshot budget");
                        } else {
                            let settle_at = self.now + SCREENSHOT_SETTLE_S;
                            let passes = self.ui_pass_seq;
                            let s = &mut self.pending_screenshots[i];
                            s.attempts += 1;
                            s.admitted = geom;
                            s.admitted_pass = passes;
                            s.settle_at = settle_at;
                            i += 1;
                        }
                        continue;
                    }
                    let Some(issued) = issued else {
                        i += 1;
                        continue;
                    };
                    if !shot_image_matches(&image, issued.rect, issued.ppp) {
                        self.fail_shot(i, "the screenshot's pixels do not match the window it was taken from");
                        continue;
                    }
                    // The returned image can be smaller than requested (the E2 adapter-limit fit),
                    // so the crop uses the image's own scale, never the issued ppp.
                    let [iw, ih] = image.size;
                    let (sx, sy) = (iw as f64 / issued.rect.width() as f64, ih as f64 / issued.rect.height() as f64);
                    if !(sx.is_finite() && sx > 0.0 && sy.is_finite() && sy > 0.0) {
                        self.fail_shot(i, "the screenshot's size does not describe the window it was taken from");
                        continue;
                    }
                    let shot = self.pending_screenshots.remove(i);
                    self.shot_bytes = self.shot_bytes.saturating_sub(shot.reserved_bytes);
                    let r = control::save_screenshot(&image, shot.path.as_deref(), shot.crop, [sx, sy]);
                    let _ = shot.reply.send(r);
                }
                Err(e) => {
                    self.fail_shot(i, e.to_string());
                }
            }
        }
    }

    /// 2. Timeouts: a late callback finds its slot cancelled and writes nothing.
    fn timeout_screenshots(&mut self) {
        let mut i = 0;
        while i < self.pending_screenshots.len() {
            if self.now > self.pending_screenshots[i].deadline {
                self.fail_shot(i, "no frame was presented (window hidden or display asleep)");
            } else {
                i += 1;
            }
        }
    }

    /// 3. Issue settled requests. The command itself requests the paint; a request still
    ///    waiting for its passes needs `want_paint` so a hidden window keeps painting.
    fn issue_screenshots(&mut self, ctx: &egui::Context, geom: DrawGeometry) {
        let mut i = 0;
        while i < self.pending_screenshots.len() {
            if self.pending_screenshots[i].slot.is_some() {
                i += 1;
                continue;
            }
            if geom.epoch != self.pending_screenshots[i].admitted.epoch {
                if self.pending_screenshots[i].attempts >= SCREENSHOT_MAX_RETRIES {
                    self.fail_shot(i, "the window kept resizing before the screenshot could be taken");
                    continue;
                }
                if !self.refresh_shot_reservation(i, &geom) {
                    self.fail_shot(i, "the resized window is too large to capture within the screenshot budget");
                    continue;
                }
                let (now, passes) = (self.now, self.ui_pass_seq);
                let s = &mut self.pending_screenshots[i];
                s.attempts += 1;
                s.admitted = geom;
                s.admitted_pass = passes;
                s.settle_at = now + SCREENSHOT_SETTLE_S;
                self.want_paint = true;
                i += 1;
                continue;
            }
            let settled = self.ui_pass_seq >= self.pending_screenshots[i].admitted_pass.saturating_add(SCREENSHOT_SETTLE_PASSES)
                && self.now >= self.pending_screenshots[i].settle_at;
            if !settled {
                self.want_paint = true;
                i += 1;
                continue;
            }
            // Resolve the crop now, against the settled UI: an element that moved since admission
            // is captured where it is, and its rectangle is relative to this issued surface.
            let target = self.pending_screenshots[i].target.clone();
            let crop = match self.shot_relative_crop(&target, &geom) {
                Ok(c) => c,
                Err(e) => {
                    self.fail_shot(i, e);
                    continue;
                }
            };
            let (slot, callback) = craft_ui::capture::CaptureSlot::new(ctx, egui::ViewportId::ROOT, MAX_SHOT_BYTES);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(callback));
            let s = &mut self.pending_screenshots[i];
            s.slot = Some(slot);
            s.issued = Some(geom);
            s.crop = crop;
            i += 1;
        }
    }
}
