//! Layer ▸ Scene Edit Detection…: find the cuts in a footage (or precomp) layer and Create
//! Markers, Split Layers, or Split and Precompose at them. The analysis
//! (`effectcraft_raster::cuts`: colour histograms + motion-compensated difference, adaptive peak
//! threshold) runs as a background job (Window ▸ Progress).

use std::sync::Arc;

use effectcraft_project::{ItemId, LayerId, Marker, Project};
use effectcraft_raster::cuts::{self, CutOpts};
use effectcraft_render::{ExprHost, FootageSource, LayerCache, RenderOpts, Renderer};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, f_p, has_layers, layer_p, str_p};
use crate::jobs::Apply;
use crate::{EngineError, Result, Session, cmd};

/// What a background job needs to render a layer's frames.
#[derive(Clone)]
pub(crate) struct LayerFrames {
    pub project: Arc<Project>,
    pub footage: Arc<dyn FootageSource>,
    pub expr: Option<Arc<dyn ExprHost>>,
    pub cache: Arc<LayerCache>,
    pub comp: ItemId,
    pub layer: LayerId,
}

impl LayerFrames {
    pub fn new(s: &Session, comp: ItemId, layer: LayerId) -> LayerFrames {
        LayerFrames { project: s.project.clone(), footage: s.footage.clone(), expr: s.expr.clone(), cache: s.layer_cache.clone(), comp, layer }
    }
    /// Run `f` with a frame getter: comp time → (layer source pixels, offset in layer space).
    pub fn with<R>(&self, f: impl FnOnce(&(dyn Fn(Tick) -> Option<(Arc<effectcraft_raster::Image>, [f64; 2])> + Sync)) -> R) -> Option<R> {
        let comp = self.project.comp(self.comp)?;
        let layer = comp.layer(self.layer)?;
        let mut r = Renderer::new(&self.project, self.footage.as_ref(), RenderOpts::default());
        r.expr = self.expr.as_deref();
        r.cache = Some(&self.cache);
        let r = &r;
        let get = |t: Tick| crate::tracking::source_frame(r, &self.project, self.comp, comp, layer, t, self.expr.as_deref());
        Some(f(&get))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Markers,
    Split,
    SplitPrecompose,
}

/// Cut times (comp time) of the layer's frames `times` (first frame never a cut).
pub(crate) fn analyse(
    lf: &LayerFrames,
    times: &[Tick],
    o: &CutOpts,
    progress: &dyn Fn(u64, u64) -> bool,
) -> std::result::Result<(Vec<Tick>, Vec<f32>), String> {
    lf.with(|get| {
        let mut sigs = Vec::with_capacity(times.len());
        // Signatures in batches (parallel), with progress and cancel between batches.
        for (k, chunk) in times.chunks(8).enumerate() {
            use rayon::prelude::*;
            let batch: Vec<cuts::Signature> = chunk.par_iter().map(|t| get(*t).map(|(img, _)| cuts::signature(&img)).unwrap_or_default()).collect();
            sigs.extend(batch);
            if !progress(((k + 1) * 8).min(times.len()) as u64, times.len() as u64) {
                return Err(crate::render_queue::CANCELLED.to_string());
            }
        }
        let mut scores = vec![0.0f32];
        for i in 1..sigs.len() {
            scores.push(cuts::dissimilarity(&sigs[i - 1], &sigs[i]));
        }
        let cuts = cuts::detect(&scores, o);
        Ok((cuts.into_iter().map(|i| times[i]).collect(), scores))
    })
    .ok_or_else(|| "layer missing".to_string())?
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "layer.sceneEditDetection";
    let (cid, lid) = layer_p(s, p, c)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    if layer.source.item().is_none() {
        return Err(bad(c, "scene edit detection needs a footage or precomp layer"));
    }
    let mode = match str_p(p, "mode").unwrap_or("markers") {
        "markers" | "createMarkers" => Mode::Markers,
        "split" | "splitLayers" => Mode::Split,
        "splitPrecompose" | "splitAndPrecompose" => Mode::SplitPrecompose,
        x => return Err(bad(c, format!("mode: markers|split|splitPrecompose, not `{x}`"))),
    };
    let fr = comp.frame_rate;
    let (a, b) = (fr.frame_at(fr.snap_nearest(layer.in_point.max(Tick::ZERO))), fr.frame_at(layer.out_point.min(comp.duration) - Tick(1)));
    let times: Vec<Tick> = (a..=b).map(|f| fr.tick_of(f)).collect();
    if times.len() < 2 {
        return Err(bad(c, "the layer is too short"));
    }
    let o = CutOpts { threshold: f_p(p, "threshold").map(|t| (t as f32).clamp(0.01, 1.0)).unwrap_or(CutOpts::default().threshold), ..Default::default() };
    let lf = LayerFrames::new(s, cid, lid);
    let name = layer.name.clone();
    let wait = b_p(p, "wait").unwrap_or(false);
    s.spawn_task("sceneDetect", format!("Scene Edit Detection: {name}"), wait, move |ctl| {
        let (cut_times, _) = analyse(&lf, &times, &o, &|d, t| ctl.progress(d, t))?;
        let apply: Apply = Box::new(move |s: &mut Session| apply(s, cid, lid, mode, &cut_times));
        Ok(apply)
    })
}

fn apply(s: &mut Session, cid: ItemId, lid: LayerId, mode: Mode, cuts: &[Tick]) -> Result<Value> {
    let secs: Vec<f64> = cuts.iter().map(|t| t.seconds()).collect();
    if cuts.is_empty() {
        return Ok(json!({"cuts": secs, "layers": [lid.0]}));
    }
    if mode == Mode::Markers {
        s.edit("Scene Edit Detection", None, |proj, _| {
            let l = super::layer_mut(proj, cid, lid)?;
            for (i, t) in cuts.iter().enumerate() {
                let lt = l.layer_time(*t);
                l.markers.retain(|m| m.time != lt);
                l.markers.push(Marker { time: lt, comment: format!("Scene {}", i + 2), ..Default::default() });
            }
            l.markers.sort_by_key(|m| m.time);
            Ok(())
        })?;
        return Ok(json!({"cuts": secs, "layers": [lid.0]}));
    }
    let snap = s.project.clone();
    // Split from the source layer: each piece ends at the next cut (pieces stacked like Split
    // Layer: later pieces above).
    let pieces = s.edit("Scene Edit Detection", None, |proj, st| {
        let mut next = proj.next_id;
        let cm = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let i = cm.layers.iter().position(|l| l.id == lid).ok_or(EngineError::NoComp)?;
        let mut ids = vec![lid];
        for t in cuts {
            let cur = ids.last().and_then(|last| cm.layers.iter().position(|l| l.id == *last)).unwrap_or(i);
            let l = &mut cm.layers[cur];
            if !(*t > l.in_point && *t < l.out_point) {
                continue;
            }
            let mut b = l.clone();
            super::edit::reid(&mut b, &mut next);
            b.in_point = *t;
            l.out_point = *t;
            ids.push(b.id);
            cm.layers.insert(cur, b);
        }
        proj.next_id = next;
        st.selected_layers = ids.clone();
        Ok(ids)
    })?;
    let mut out = pieces.clone();
    if mode == Mode::SplitPrecompose {
        let base = s.project.comp(cid).and_then(|c| c.layer(lid)).map(|l| l.name.clone()).unwrap_or_default();
        out.clear();
        for (k, id) in pieces.iter().enumerate() {
            let r = s.execute(
                "layer.precompose",
                json!({"comp": cid.0, "layers": [id.0], "mode": "move", "adjustDuration": true, "name": format!("{base} Scene {}", k + 1)}),
            )?;
            let new = r.get("layer").and_then(Value::as_u64).map(LayerId).unwrap_or(*id);
            out.push(new);
        }
        s.collapse_undo("Scene Edit Detection", &snap);
    }
    Ok(json!({"cuts": secs, "layers": out.iter().map(|l| l.0).collect::<Vec<_>>()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "layer.sceneEditDetection",
        "Scene Edit Detection...",
        ["Layer"],
        None,
        "{layer?, mode?: markers|split|splitPrecompose, threshold? (0…1, 0.25), wait?: bool}",
        has_layers,
        run
    )]
}
