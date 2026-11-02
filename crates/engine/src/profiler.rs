//! Explicit CPU reference captures. The owned job lets frontends keep rendering off their UI
//! thread; it neither changes the document nor reads/writes the session's processed-layer cache.

use std::sync::{Mutex, PoisonError};

use effectcraft_render::{Backend, LayerTiming, RenderOpts, Renderer};
use effectcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{EngineError, Result, Session};

pub type ProfileJob = Box<dyn FnOnce() -> Result<ProfileReport> + Send>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileReport {
    pub comp: u64,
    pub name: String,
    pub time: Tick,
    pub frame: i64,
    pub scale: f64,
    pub width: u32,
    pub height: u32,
    pub total_ms: f64,
    pub backend: String,
    pub layer_cache: bool,
    /// Execution records, not disjoint costs: nested frames and repeated reads may overlap.
    pub layers: Vec<LayerTiming>,
    /// Visible top-level layers with no individual timing record. Never display these as zero.
    pub unmeasured: Vec<UntimedLayer>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UntimedLayer {
    pub layer_id: u64,
    pub name: String,
}

impl Session {
    /// Prepare one CPU capture of a composition at `time` (seconds) or `frame`, default CTI.
    /// `scale` is 1/16…1 (default Full); captures above 16 million output pixels are refused.
    pub fn profile_job(&self, params: &Value) -> Result<ProfileJob> {
        let bad = |message: &str| EngineError::BadParams { cmd: "comp.profile".into(), msg: message.into() };
        if !params.is_object() && !params.is_null() {
            return Err(bad("expected an object"));
        }
        if let Some(comp) = params.get("comp")
            && !comp.is_string()
            && comp.as_u64().is_none()
        {
            return Err(bad("comp must be a composition name or id"));
        }
        let cid = crate::commands::comp_id(self, params)?;
        let comp = self.project.comp(cid).ok_or(EngineError::NoComp)?;
        if params.get("time").is_some() && params.get("frame").is_some() {
            return Err(bad("specify either time or frame"));
        }
        let time = if let Some(time) = params.get("time") {
            let seconds = time
                .as_f64()
                .filter(|x| x.is_finite() && x.abs() <= Tick::MAX.0 as f64 / TICKS_PER_SECOND as f64)
                .ok_or_else(|| bad("time must be finite seconds within the supported range"))?;
            Tick::from_seconds_f64(seconds)
        } else if let Some(frame) = params.get("frame") {
            let frame = frame.as_i64().ok_or_else(|| bad("frame must be an integer"))?;
            comp.frame_rate.tick_of(frame)
        } else {
            self.time_of(cid)
        };
        if !(Tick::MIN..=Tick::MAX).contains(&time) {
            return Err(bad("frame is outside the supported time range"));
        }
        let scale = match params.get("scale") {
            Some(value) => {
                value.as_f64().filter(|v| v.is_finite() && (1.0 / 16.0..=1.0).contains(v)).ok_or_else(|| bad("scale must be between 0.0625 and 1"))?
            }
            None => 1.0,
        };
        let (width, height) = effectcraft_render::frame_size(comp.width, comp.height, scale);
        if u64::from(width).saturating_mul(u64::from(height)) > 16_777_216 {
            return Err(bad("capture exceeds 16 million pixels; reduce scale"));
        }
        let (name, frame) = (self.project.item(cid).map(|i| i.name.clone()).unwrap_or_default(), comp.frame_rate.frame_at(time));
        let opts = self.render_opts(RenderOpts { scale, backend: Backend::Cpu, ..Default::default() });
        let (project, footage, expr) = (self.project.clone(), self.footage.clone(), self.expr.clone());
        Ok(Box::new(move || {
            crate::guard::guarded("Composition Profiler", || {
                let profile = Mutex::new(Vec::new());
                let mut renderer = Renderer::new(&project, footage.as_ref(), opts);
                renderer.expr = expr.as_deref();
                renderer.profile = Some(&profile);
                // No accelerator or shared LayerCache, including recursively evaluated precomps.
                let start = web_time::Instant::now();
                let image = renderer.comp_frame(cid, time);
                let total_ms = start.elapsed().as_secs_f64() * 1000.0;
                let comp = project.comp(cid).ok_or(EngineError::NoComp)?;
                let ctx = effectcraft_render::EvalCtx::new(&project, cid, comp, time);
                let visible = renderer.visible_layers(&ctx);
                let visible_truncated = visible.len() > effectcraft_render::MAX_PROFILE_RECORDS;
                let visible: Vec<_> = visible
                    .into_iter()
                    .take(effectcraft_render::MAX_PROFILE_RECORDS)
                    .map(|layer| UntimedLayer { layer_id: layer.id.0, name: layer.name.clone() })
                    .collect();
                let layers: Vec<LayerTiming> = profile.into_inner().unwrap_or_else(PoisonError::into_inner);
                let measured: std::collections::HashSet<_> = layers.iter().filter(|row| row.comp == cid.0).map(|row| row.layer_id).collect();
                let unmeasured = visible.into_iter().filter(|layer| !measured.contains(&layer.layer_id)).collect();
                let mut notes = vec![
                "CPU reference wall times; GPU preview performance is not measured.".into(),
                "Processed-layer cache bypassed; media and operating-system caches may be warm.".into(),
                "Records are inclusive and may overlap or repeat (precomps, mattes, effect reads, motion samples); do not sum them.".into(),
                "3D records measure source/effect preparation only; shared 3D composition, lighting and other frame overhead are included only in the frame total.".into(),
                "Untimed layers may be skipped, collapsed precomps or renderer paths without individual instrumentation; untimed is not zero cost.".into(),
            ];
                if visible_truncated || layers.len() >= effectcraft_render::MAX_PROFILE_RECORDS || layers.iter().any(|row| row.truncated) {
                    notes.push("Capture details truncated at the profiling record limit.".into());
                }
                // Keep the actual composition render alive until its timing has been recorded.
                drop(image);
                Ok(ProfileReport {
                    comp: cid.0,
                    name,
                    time,
                    frame,
                    scale,
                    width,
                    height,
                    total_ms,
                    backend: "cpu".into(),
                    layer_cache: false,
                    layers,
                    unmeasured,
                    notes,
                })
            })
        }))
    }
}
