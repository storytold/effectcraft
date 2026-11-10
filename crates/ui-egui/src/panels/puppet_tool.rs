//! Puppet tools in the Composition viewer: the deformed mesh and pins of the selected layers'
//! Puppet effect, click to add a pin (`puppet.addPin`), click / Shift-click a pin to select it
//! (`puppet.selectPins`), drag a pin to move it (`puppet.movePin`), and drag an Advanced or Bend
//! pin's ring to rotate it or its square to scale it (`puppet.setPin`); each drag is one undo step.

use std::sync::Arc;

use effectcraft_engine::commands::puppet::PuppetSource;
use effectcraft_engine::effects::puppet::{self, Mesh, Pin, PinKind};
use effectcraft_engine::geom::vec2 as gv2;
use effectcraft_engine::project::{ItemId, Layer, LayerId};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::time::Tick;
use egui::{Color32, Pos2, Stroke};

use super::viewer::{ViewerMap, fx2c};
use crate::EffectcraftApp;

/// Deformed meshes of a layer's Puppet effect: (mesh uid, mesh, deformed vertices, pins).
pub type Overlay = Arc<Vec<(u64, Arc<Mesh>, Vec<[f64; 2]>, Vec<Pin>)>>;

/// Rotation ring radius (screen points) of Advanced and Bend pins.
pub const RING: f32 = 9.0;

/// A pin drawn this frame.
#[derive(Clone, Copy, Debug)]
pub struct PinHit {
    pub layer: LayerId,
    pub pin: u64,
    pub kind: PinKind,
    pub pos: Pos2,
    /// Rotation (°) and scale (1 = 100 %) now, for Advanced and Bend pins.
    pub rotation: f64,
    pub scale: f64,
    /// The scale handle on the ring (Advanced and Bend pins).
    pub handle: Option<Pos2>,
}

/// The part of a pin under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinPart {
    /// The pin itself (drag moves Position, Advanced, Starch and Overlap pins).
    Center,
    /// The rotation ring of an Advanced or Bend pin.
    Ring,
    /// The scale handle on that ring.
    Scale,
}

/// The pin part under `p`, topmost pin first.
pub fn hit(hits: &[PinHit], p: Pos2) -> Option<(PinHit, PinPart)> {
    hits.iter().rev().find_map(|h| {
        let d = h.pos.distance(p);
        match h.handle {
            Some(s) if s.distance(p) < 5.0 => Some((*h, PinPart::Scale)),
            Some(_) if d < 6.0 => Some((*h, PinPart::Center)),
            Some(_) if (d - RING).abs() < 4.0 => Some((*h, PinPart::Ring)),
            None if d < 8.0 => Some((*h, PinPart::Center)),
            _ => None,
        }
    })
}

/// What an overlay is evaluated for: (project revision, comp, layer, comp time).
type OverlayKey = (u64, u64, u64, i64);

/// How long a frame waits for a layer's first overlay evaluated off the UI thread: about a frame
/// at 60 Hz, so a light precomp's pins show as soon as the tool is chosen.
#[cfg(not(test))]
const OVERLAY_GRACE: std::time::Duration = std::time::Duration::from_millis(16);
/// Tests see the evaluation in flight, however fast it is.
#[cfg(test)]
const OVERLAY_GRACE: std::time::Duration = std::time::Duration::ZERO;

/// A layer's Puppet overlay: the last one evaluated (`None` when the layer has no pixels) and
/// what for, and the one being evaluated off the UI thread.
#[derive(Default)]
struct OverlaySlot {
    shown: Option<(OverlayKey, Option<Overlay>)>,
    job: Option<(OverlayKey, std::sync::mpsc::Receiver<Option<Overlay>>)>,
}

impl OverlaySlot {
    /// Takes the overlay evaluated off the UI thread, if it is done; forgets an evaluation whose
    /// worker died, so the next frame starts another.
    fn land(&mut self) {
        let Some((key, rx)) = &self.job else { return };
        match rx.try_recv() {
            Ok(ov) => self.shown = Some((*key, ov)),
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
        }
        self.job = None;
    }
}

/// The deformed meshes of `layer`'s Puppet effect at comp time `t`, evaluated once per project
/// revision. A layer whose input pixels the layer cache keeps is evaluated here: after an edit
/// elsewhere that is a cache lookup. One whose input is rendered every time (a precomp's whole
/// nested composition, at full size) is evaluated off the UI thread: with the Puppet tool active,
/// every Position or Scale change anywhere re-rendered it on this thread and slowed the whole
/// window (#2516). Until it lands the frame draws the layer's last overlay, and repaints when it
/// does; with none yet (the tool just chosen), it waits up to [`OVERLAY_GRACE`] for it first.
pub fn overlay(app: &EffectcraftApp, ctx: &egui::Context, cid: ItemId, layer: &Layer, t: Tick) -> Option<Overlay> {
    let fx = layer.effects()?.groups().find(|g| puppet::is_puppet(g))?;
    let key = (app.session.revision, cid.0, layer.id.0, t.0);
    let id = egui::Id::unique(("puppet-overlay", layer.id.0));
    let slot = ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<std::sync::Mutex<OverlaySlot>>>(id).clone());
    let mut slot = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    slot.land();
    if let Some((k, ov)) = &slot.shown
        && *k == key
    {
        return ov.clone();
    }
    let s = &app.session;
    let src = PuppetSource { project: &s.project, footage: &*s.footage, expr: s.expr.as_deref(), cache: &s.layer_cache };
    if src.input_cacheable(cid, layer.id, fx.uid, t) {
        let ov = evaluate(src, cid, layer.id, fx.uid, t);
        slot.shown = Some((key, ov.clone()));
        return ov;
    }
    // One evaluation at a time: a newer revision waits for the one in flight to land.
    if slot.job.is_none() {
        let rx = evaluate_off_thread(app, ctx, cid, layer.id, fx.uid, t);
        // Waited for only while there is no overlay to draw: with one, every edit elsewhere would
        // block the frame for up to the grace. An inline result (the web) is taken first, without
        // the clock `recv_timeout` reads, which the web doesn't have.
        let waited = rx.try_recv().ok().or_else(|| if slot.shown.is_none() { rx.recv_timeout(OVERLAY_GRACE).ok() } else { None });
        match waited {
            Some(ov) => {
                slot.shown = Some((key, ov.clone()));
                return ov;
            }
            None => slot.job = Some((key, rx)),
        }
    }
    slot.shown.as_ref().and_then(|(_, ov)| ov.clone())
}

/// The overlay of the Puppet effect `fx` of layer `lid` (see [`overlay`]) from `src`.
fn evaluate(src: PuppetSource<'_>, cid: ItemId, lid: LayerId, fx: effectcraft_engine::project::Uid, t: Tick) -> Option<Overlay> {
    let (buf, params) = src.eval(cid, lid, fx, t)?;
    Some(Arc::new(puppet::overlay(&buf, &params)))
}

/// [`evaluate`] on a worker thread, from the session's shared project, footage, expressions and
/// layer cache (inline on the web, where there are none, or when a spawn fails); `ctx` repaints
/// when it is done.
fn evaluate_off_thread(
    app: &EffectcraftApp,
    ctx: &egui::Context,
    cid: ItemId,
    lid: LayerId,
    fx: effectcraft_engine::project::Uid,
    t: Tick,
) -> std::sync::mpsc::Receiver<Option<Overlay>> {
    let s = &app.session;
    let (project, footage, expr, cache) = (s.project.clone(), s.footage.clone(), s.expr.clone(), s.layer_cache.clone());
    let work = move || evaluate(PuppetSource { project: &project, footage: &*footage, expr: expr.as_deref(), cache: &cache }, cid, lid, fx, t);
    let (tx, rx) = std::sync::mpsc::channel();
    let (worker_tx, repaint) = (tx.clone(), ctx.clone());
    let worker = work.clone();
    let spawned = std::thread::Builder::new()
        .name("ec-puppet-overlay".into())
        .spawn(move || {
            // The viewer may have moved on meanwhile: nothing to deliver to.
            let _ = worker_tx.send(worker());
            repaint.request_repaint();
        })
        .is_ok();
    if !spawned {
        // Received at once by the caller.
        let _ = tx.send(work());
    }
    rx
}

/// The layer a Puppet tool press at comp point `cpt` works on, of the selected `layers` with a
/// Puppet effect (stack order): the one whose mesh, as deformed now, is under it (pins go on the
/// deformed shape, also where it reaches past the layer's own bounds), else the one `on_art` takes
/// it (see `takes_pin`). Never a layer without a Puppet effect: the tools stay on the layers being
/// rigged, as in After Effects (#284).
pub fn target(ectx: &EvalCtx, layers: &[(&Layer, Overlay)], cpt: [f64; 2], on_art: impl Fn(&Layer) -> bool) -> Option<LayerId> {
    let on_mesh = |l: &Layer, ov: &Overlay| {
        let Some(inv) = fx2c(ectx, l).inverse() else { return false };
        let p = inv.apply(gv2(cpt[0], cpt[1]));
        ov.iter().any(|(_, mesh, def, _)| puppet::contains(mesh, def, [p.x, p.y]))
    };
    layers.iter().find(|(l, ov)| on_mesh(l, ov)).or_else(|| layers.iter().find(|(l, _)| on_art(l))).map(|(l, _)| l.id)
}

/// Whether a press at comp point `cpt` places a pin on `layer`. Where it would not (the deformed
/// layer is empty there) the press is on nothing: a click does nothing, a drag is a marquee (#2865).
pub fn takes_pin(app: &EffectcraftApp, ectx: &EvalCtx, layer: &Layer, cpt: [f64; 2]) -> bool {
    let Some(inv) = fx2c(ectx, layer).inverse() else { return false };
    let p = inv.apply(gv2(cpt[0], cpt[1]));
    effectcraft_engine::commands::puppet::places_pin(&app.session, ectx.comp_id, layer.id, [p.x, p.y], ectx.time)
}

/// Keep a pin's keyframed Position in view in the Timeline after placing, moving or recording the
/// pin in the viewer (#273).
pub fn reveal_pin(app: &mut EffectcraftApp, layer: LayerId, pin: u64) {
    let comp = app.session.active_comp();
    let pos = comp.and_then(|c| c.layer(layer)?.props.find_group(pin)?.get("position")).filter(|p| p.is_animated()).map(|p| p.uid);
    if let Some(pos) = pos {
        super::timeline::keep_in_view(app, layer, pos);
    }
}

/// Where a pin sits now (effect space): its Position, or (Bend / Starch / Overlap pins) its rest
/// point carried along by the deformation.
pub fn pin_position(mesh: &Mesh, def: &[[f64; 2]], p: &Pin) -> [f64; 2] {
    if p.kind.moves() {
        return p.position;
    }
    let v = (0..mesh.verts.len()).min_by(|a, b| {
        let da = (mesh.verts[*a][0] - p.rest[0]).hypot(mesh.verts[*a][1] - p.rest[1]);
        let db = (mesh.verts[*b][0] - p.rest[0]).hypot(mesh.verts[*b][1] - p.rest[1]);
        da.total_cmp(&db)
    });
    match v {
        Some(v) => [def[v][0] + p.rest[0] - mesh.verts[v][0], def[v][1] + p.rest[1] - mesh.verts[v][1]],
        None => p.rest,
    }
}

/// Pin colours by kind: Position yellow, Starch red, Overlap blue, Advanced blue-green, Bend
/// orange-brown.
pub fn pin_color(kind: PinKind) -> Color32 {
    match kind {
        PinKind::Position => Color32::from_rgb(0xf0, 0xd0, 0x40),
        PinKind::Starch => Color32::from_rgb(0xe0, 0x50, 0x50),
        PinKind::Overlap => Color32::from_rgb(0x40, 0x90, 0xf0),
        PinKind::Advanced => Color32::from_rgb(0x30, 0xc8, 0xb4),
        PinKind::Bend => Color32::from_rgb(0xc8, 0x82, 0x3c),
    }
}

/// Draw the mesh (when `show_mesh`) and pins of `layer` (the pin under `hover` is drawn larger);
/// returns pin hit targets.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    painter: &egui::Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    layer: &Layer,
    ov: &Overlay,
    show_mesh: bool,
    selected: &[u64],
    hover: Option<Pos2>,
) -> Vec<PinHit> {
    let m = fx2c(ectx, layer);
    let scr = |q: [f64; 2]| {
        let c = m.apply(gv2(q[0], q[1]));
        map.to_screen([c.x, c.y])
    };
    let mut hits = vec![];
    for (_, mesh, def, pins) in ov.iter() {
        if show_mesh {
            let st = Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xf0, 0xd0, 0x40, 110));
            for t in &mesh.tris {
                let (a, b, c) = (scr(def[t[0]]), scr(def[t[1]]), scr(def[t[2]]));
                painter.line_segment([a, b], st);
                painter.line_segment([b, c], st);
                painter.line_segment([c, a], st);
            }
        }
        for pn in pins {
            let at = pin_position(mesh, def, pn);
            let pos = scr(at);
            let sel = selected.contains(&pn.uid);
            let col = pin_color(pn.kind);
            if matches!(pn.kind, PinKind::Starch | PinKind::Overlap) {
                // Extent ring (layer pixels → screen).
                let r = (scr([pn.rest[0] + pn.extent, pn.rest[1]]) - scr(pn.rest)).length();
                painter.circle_stroke(pos, r.max(2.0), Stroke::new(1.0, col.gamma_multiply(0.6)));
            }
            // Advanced and Bend pins: the rotation ring with a square scale handle at the pin's
            // rotation (the layer's own orientation on screen).
            let mut handle = None;
            if matches!(pn.kind, PinKind::Bend | PinKind::Advanced) {
                painter.circle_stroke(pos, RING, Stroke::new(1.5, col));
                let (s, c) = pn.rotation.to_radians().sin_cos();
                let dir = (scr([at[0] + c, at[1] + s]) - pos).normalized();
                if dir.x.is_finite() && dir.y.is_finite() {
                    let hp = pos + dir * RING;
                    painter.rect_filled(egui::Rect::from_center_size(hp, egui::vec2(5.0, 5.0)), 0.0, col);
                    painter.rect_stroke(
                        egui::Rect::from_center_size(hp, egui::vec2(5.0, 5.0)),
                        0.0,
                        Stroke::new(1.0, Color32::BLACK),
                        egui::StrokeKind::Outside,
                    );
                    handle = Some(hp);
                }
            }
            // Pins grow under the pointer.
            // Selected pins are filled with their colour and ringed in white; unselected pins are
            // hollow (dark centre, coloured outline), like selected and unselected mask vertices.
            // Drawing only: hit-testing uses `pos` alone.
            let r = if sel { 5.5 } else { 4.5 } + if hover.is_some_and(|h| h.distance(pos) < 8.0) { 1.5 } else { 0.0 };
            if sel {
                painter.circle_filled(pos, r, col);
                painter.circle_stroke(pos, r, Stroke::new(1.0, Color32::BLACK));
                painter.circle_stroke(pos, r + 2.5, Stroke::new(1.5, Color32::WHITE));
            } else {
                painter.circle_filled(pos, r, Color32::from_black_alpha(160));
                painter.circle_stroke(pos, r, Stroke::new(1.5, col));
            }
            hits.push(PinHit { layer: layer.id, pin: pn.uid, kind: pn.kind, pos, rotation: pn.rotation, scale: pn.scale, handle });
        }
    }
    hits
}

#[cfg(test)]
mod overlay_tests {
    use effectcraft_engine::Session;
    use serde_json::json;

    use super::*;

    /// A comp holding `layer` (a solid, or precomposed when `precomp`), selected and pinned with
    /// two Position pins, and the app around it.
    fn pinned(precomp: bool) -> (EffectcraftApp, ItemId, LayerId) {
        let mut s = Session::default();
        // Large enough that rendering the precomp takes longer than looking for the result.
        s.execute("comp.new", json!({"name": "Outer", "width": 2400, "height": 1600, "duration": 2})).unwrap();
        let l = s.execute("layer.newSolid", json!({"name": "Art", "width": 2000, "height": 1400, "color": "#e04020"})).unwrap()["layer"].clone();
        if precomp {
            s.execute("layer.precompose", json!({"layers": [l], "name": "Inner"})).unwrap();
        }
        let id = s.active_comp().unwrap().layers[0].id;
        s.execute("layer.select", json!({"layers": [id.0]})).unwrap();
        s.execute("puppet.addPin", json!({"layer": id.0, "position": [600, 700]})).unwrap();
        s.execute("puppet.addPin", json!({"layer": id.0, "position": [1800, 700]})).unwrap();
        let cid = s.active_comp_id().unwrap();
        (EffectcraftApp::new(s), cid, id)
    }

    fn layer(app: &EffectcraftApp, cid: ItemId, id: LayerId) -> Layer {
        app.session.project.comp(cid).unwrap().layer(id).unwrap().clone()
    }

    /// The overlay, once evaluated for the project as it is (at most 30 s).
    fn landed(app: &EffectcraftApp, ctx: &egui::Context, cid: ItemId, id: LayerId, not: Option<&Overlay>) -> Overlay {
        let t0 = std::time::Instant::now();
        loop {
            if let Some(ov) = overlay(app, ctx, cid, &layer(app, cid, id), Tick(0)).filter(|ov| not.is_none_or(|n| !Arc::ptr_eq(ov, n))) {
                return ov;
            }
            assert!(t0.elapsed() < std::time::Duration::from_secs(30), "the overlay never landed");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// A layer whose input the layer cache keeps (a solid) is evaluated at once, as before.
    #[test]
    fn a_cached_layers_overlay_is_evaluated_at_once() {
        let (app, cid, id) = pinned(false);
        let ctx = egui::Context::default();
        let ov = overlay(&app, &ctx, cid, &layer(&app, cid, id), Tick(0)).expect("evaluated in the frame");
        assert_eq!(ov.len(), 1, "one mesh");
        assert_eq!(ov[0].3.len(), 2, "with its two pins");
    }

    /// #2516: a precomp layer's input (its whole nested composition, at full size) is rendered
    /// off the UI thread. Before its first overlay lands nothing is drawn; after an edit
    /// elsewhere, the last overlay is drawn until the new one lands.
    #[test]
    fn a_precomps_overlay_is_evaluated_off_the_ui_thread_and_the_last_one_drawn_meanwhile() {
        let (mut app, cid, id) = pinned(true);
        let ctx = egui::Context::default();
        assert!(overlay(&app, &ctx, cid, &layer(&app, cid, id), Tick(0)).is_none(), "evaluated on the UI thread");
        let first = landed(&app, &ctx, cid, id, None);
        assert_eq!(first[0].3.len(), 2);

        // An edit elsewhere: a new revision, the precomp rendered again.
        app.session.execute("layer.newSolid", json!({"name": "Other", "width": 10, "height": 10})).unwrap();
        let meanwhile = overlay(&app, &ctx, cid, &layer(&app, cid, id), Tick(0)).expect("the last overlay");
        assert!(Arc::ptr_eq(&meanwhile, &first), "the last overlay is drawn while the new one renders");

        let second = landed(&app, &ctx, cid, id, Some(&first));
        let s = &app.session;
        let src = PuppetSource { project: &s.project, footage: &*s.footage, expr: s.expr.as_deref(), cache: &s.layer_cache };
        let fx = layer(&app, cid, id).effects().unwrap().groups().find(|g| puppet::is_puppet(g)).unwrap().uid;
        assert!(!src.input_cacheable(cid, id, fx, Tick(0)), "a precomp's input is rendered every time");
        let direct = evaluate(src, cid, id, fx, Tick(0)).unwrap();
        assert_eq!(second[0].2, direct[0].2, "the same deformed mesh as evaluating on the spot");
    }
}
