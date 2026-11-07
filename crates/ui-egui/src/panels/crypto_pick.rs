//! The Cryptomatte eyedropper in the Composition viewer (#473). The button next to the effect's
//! Selection arms it ([`super::effect_controls::toggle_pick`] with [`KIND`]); while it is armed
//! the viewer names the ID under the cursor, and a click picks it into the Selection: a plain
//! click replaces it, Shift adds, Alt or Ctrl (Option or Cmd on a Mac) removes. It stays armed
//! for the next click until Esc or the button again, and highlights the hovered ID's coverage.
//!
//! As in After Effects, the Selection tool picks the same way without arming anything while the
//! Cryptomatte effect is selected in Effect Controls (#2839, [`implicit_pick`]): over the
//! effect's layer (selected, so the viewer's press acts on it) a click picks and a drag moves nothing;
//! elsewhere the viewer keeps its clicks (selecting and moving layers). Esc has nothing to end.
//! "Over the layer" is its bounding quad, as the viewer's own hit test (not its alpha), less the
//! viewer's handles on it ([`super::viewer::grabs_at`]: bounding-box handles, mask vertices,
//! motion-path keys, guides), which keep scaling, reshaping and moving. A press in the very pass
//! the pointer leaves the layer is passed on to the viewer as a click; one in the pass it arrives
//! on the layer (a touch, a pen without hover) is claimed from the viewer and picks. While the effect is
//! selected (or the eyedropper armed), a double-click on the layer picks (one step: the second
//! click changes nothing) instead of opening its precomp or editing its text.
//!
//! The hover needs the frame's Cryptomatte probe ([`effectcraft_engine::Session::crypto_probe_job`]):
//! it is built off the UI thread, one at a time, not while playing or scrubbing; the chip says so
//! meanwhile, and clicks wait for it (the hover then builds nothing), except while playing, where
//! a click without its probe is not made. Once built, a hover is a few array reads.
//!
//! A click is the frame and the project shown at its press (and the resolution it read). It is
//! made at the end of a pass ([`upkeep`]), checked against the frame and the project right
//! before: if either changed since its press, it and every click waiting with it are dropped,
//! and the status says so. A press dragged further than [`SLOP`] picks nothing, and says so.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use effectcraft_engine::commands::CryptoProbeJob;
use effectcraft_engine::geom::Mat3;
use effectcraft_engine::project::{GroupKind, Layer};
use effectcraft_engine::render::EvalCtx;
use egui::{Color32, Rect, Sense, vec2};
use serde_json::{Value, json};

use super::fx_widgets as fw;
use crate::EffectcraftApp;
use crate::i18n::{tr, tr_args};
use crate::state::{FxPick, Tool};
use crate::theme::Tokens;

/// [`FxPick::kind`] of the Cryptomatte eyedropper.
pub const KIND: &str = "crypto";

/// The Cryptomatte effect's match name.
const EFFECT: &str = "ec.3d.cryptomatte";

/// How long the time must stay put before a probe is built for it (scrubbing).
const SETTLE: f64 = 0.1;

/// A press that moves further than this (points) before its release picks nothing.
const SLOP: f32 = 24.0;

/// The status when waiting clicks are dropped.
const NOT_MADE: &str = "Pick not made: the frame or the project changed before its Cryptomatte IDs were read";

/// The status when a click is not made because the preview plays.
const PLAYING: &str = "Pick not made: stop playback to pick";

/// egui temp-data id for the eyedropper's [`State`] between frames.
const VIEWER_CRYPTO_PICK: &str = "viewer-crypto-pick";

/// egui temp-data id for the project revision and frame at the start of a pass.
const VIEWER_CRYPTO_PICK_START: &str = "viewer-crypto-pick-start";

fn pick_id() -> egui::Id {
    egui::Id::unique(VIEWER_CRYPTO_PICK)
}

fn pick_start_id() -> egui::Id {
    egui::Id::unique(VIEWER_CRYPTO_PICK_START)
}

/// The press of a click in progress.
#[derive(Clone)]
struct Press {
    pos: egui::Pos2,
    /// Its modifiers: letting go of Shift a moment early still adds.
    keys: egui::Modifiers,
    /// A menu or popup list was open: the click only closes it.
    menu: bool,
    /// A text field had the focus: the click commits it (the project may change by that).
    typing: bool,
    /// The pick query at the press (frame, resolution, layer point) and the project revision.
    q: Option<Value>,
    rev: u64,
}

/// The hovered ID's coverage highlight drawn over the viewer.
#[derive(Clone)]
struct HighlightMask {
    key: (u32, Value),
    texture: egui::TextureHandle,
    size: [f64; 2],
    uv: [f32; 2],
}

/// Hover and click state between frames.
#[derive(Clone, Default)]
struct State {
    /// The last hover query (with the project revision) and its answer (or why there is none).
    asked: Option<(Value, u64)>,
    reply: Option<Result<Value, String>>,
    /// Clicks waiting for their probe, in order, and the project revision they were made at.
    pending: Vec<Value>,
    pending_rev: u64,
    /// The waiting clicks took the focus from a text field (the Selection being typed): they
    /// are made once it has committed, on top of what it committed.
    after_typing: bool,
    /// A text field, a popup or a dialog had the keyboard the pass before: its Esc isn't the
    /// pick's (egui lets go of the focus and closes popups, and Esc closes the dialog, before
    /// the viewer runs).
    esc_taken: bool,
    /// The press of the click in progress.
    press: Option<Press>,
    /// Probes may be built for the waiting clicks (not while playing or scrubbing).
    start: bool,
    /// The comp time shown and since when (to wait for a scrub to settle).
    shown: Option<(f64, f64)>,
    /// The pick (layer, Selection) and the egui pass this state was last used for.
    seen: Option<((u64, u64), u64)>,
    /// The hovered ID's highlight: its hash and probe query, coverage texture, size and uv.
    mask: Option<HighlightMask>,
    /// Picking with the Selection tool while the effect is selected ([`implicit_pick`]), not with
    /// the armed eyedropper.
    implicit: bool,
    /// The last egui pass the overlay took clicks in (egui hands a press to what sensed it the
    /// pass before).
    sensed: Option<u64>,
    /// The query of the last click queued: the second click of a double-click on the same pixel
    /// with the same keys is not queued again.
    last_click: Option<Value>,
}

/// Clears the busy flag and repaints when a build ends, a panic included.
struct Done(Arc<AtomicBool>, egui::Context);
impl Drop for Done {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
        self.1.request_repaint();
    }
}

fn run(job: CryptoProbeJob, done: Done) {
    let _done = done;
    job();
}

/// Whether the probe `q` needs is built; if not and `start`, start building it unless one is
/// being built (one at a time for the app). Asking changes nothing: only a build started here
/// supersedes another.
fn ready(app: &EffectcraftApp, ctx: &egui::Context, q: &Value, start: bool) -> Result<bool, String> {
    let busy = &app.crypto_probe_busy;
    if app.session.crypto_probe_ready(q).map_err(|e| shown_error(&e.to_string()))? {
        return Ok(true);
    }
    // (Only this thread sets the flag, so it stays clear until the store.)
    if !start || busy.load(Ordering::Acquire) {
        return Ok(false);
    }
    let Some(job) = app.session.crypto_probe_job(q).map_err(|e| shown_error(&e.to_string()))? else { return Ok(true) };
    busy.store(true, Ordering::Release);
    let done = Done(busy.clone(), ctx.clone());
    if cfg!(target_arch = "wasm32") {
        run(job, done);
    } else if std::thread::Builder::new().name("crypto-probe".into()).spawn(move || run(job, done)).is_err() {
        return Err("could not start reading the Cryptomatte IDs".into());
    }
    Ok(false)
}

/// An engine error as the chip and the status line show it: without the command's "invalid
/// parameters for `…`:" prefix, capitalised.
fn shown_error(e: &str) -> String {
    let e = e.strip_prefix("invalid parameters for `").and_then(|r| r.split_once("`: ")).map_or(e, |(_, m)| m);
    let mut c = e.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// The keys that remove an ID (Ctrl removes on a Mac too, but means right-click there).
pub fn remove_keys(os: egui::os::OperatingSystem) -> &'static str {
    if os == egui::os::OperatingSystem::Mac { tr("Option or Cmd") } else { tr("Alt or Ctrl") }
}

/// At the start of every pass, before its keys: the project revision and frame on screen when
/// the pass's input happened. A press in the pass is taken against them, not against what the
/// pass's own keys made of them (a slow pass can hold a press, a PageDown and the release).
pub fn pass_start(app: &EffectcraftApp, ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(pick_start_id(), (app.session.revision, app.session.time().seconds())));
}

/// Every frame: a disarmed eyedropper lets go of its state and the session's probe.
pub fn upkeep(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let sid = pick_id();
    // Another project (New, Open, Revert) ends a pick: it is kept by layer id and property uid,
    // which the next project reuses.
    let eid = egui::Id::unique("viewer-fx-pick-project");
    let epoch = app.session.script_ui.project_epoch;
    let seen = ctx.data(|d| d.get_temp::<u64>(eid));
    if seen != Some(epoch) {
        if seen.is_some() && app.ui.fx_pick.is_some() {
            app.ui.fx_pick = None;
            app.ui.status.clear();
        }
        ctx.data_mut(|d| d.insert_temp(eid, epoch));
    }
    // Picking with the Selection tool (the effect selected) keeps its state while the viewer runs
    // it, every pass.
    let old = ctx.data(|d| d.get_temp::<State>(sid));
    let implicit = old.as_ref().is_some_and(|s| s.implicit && s.seen.is_some_and(|(_, n)| n == ctx.cumulative_pass_nr()));
    if app.ui.fx_pick.as_ref().is_none_or(|p| p.kind != KIND)
        && !implicit
        && let Some(mut old) = old
    {
        ctx.data_mut(|d| d.remove::<State>(sid));
        app.session.drop_crypto_probe();
        // The effect deselected (or the tool changed) while clicks waited: say they're not made.
        if old.implicit && !old.pending.is_empty() {
            drop_waiting(app, ctx, &mut old);
        }
    }
    // The clicks waiting in the viewer, after everything else this pass did (its keys, edits and
    // frame steps included).
    if let Some(mut st) = ctx.data(|d| d.get_temp::<State>(sid)).filter(|s| s.seen.is_some_and(|(_, n)| n == ctx.cumulative_pass_nr())) {
        make_waiting(app, ctx, &mut st);
        ctx.data_mut(|d| d.insert_temp(sid, st));
    }
}

/// Drop every waiting click, and say so.
fn drop_waiting(app: &mut EffectcraftApp, ctx: &egui::Context, st: &mut State) {
    (st.pending, st.after_typing) = (vec![], false);
    app.say(ctx, tr(if app.playback.playing { PLAYING } else { NOT_MADE }).to_string());
}

/// Make one click. Our own pick is not "the project changed" for a click still held.
fn make(app: &mut EffectcraftApp, ctx: &egui::Context, st: &mut State, q: Value) -> Result<Value, String> {
    let before = app.session.revision;
    let r = crate::menus::invoke(app, ctx, "effect.pickCryptomatte", q);
    if let Some(p) = st.press.as_mut()
        && p.rev == before
    {
        p.rev = app.session.revision;
    }
    r
}

/// Make the waiting clicks whose probe is built, in order. Each is checked against the frame and
/// the project right before it is made, with nothing in between: if either changed since its
/// press, it and the clicks waiting with it are not made.
fn make_waiting(app: &mut EffectcraftApp, ctx: &egui::Context, st: &mut State) {
    // A click that took the focus from a text field waits for the field to commit (it may do so
    // later in this pass or in the next), then is made on what it committed.
    let typing = st.after_typing && ctx.text_edit_focused();
    if st.after_typing && !typing {
        (st.after_typing, st.pending_rev) = (false, app.session.revision);
    }
    while let Some(q) = st.pending.first().cloned().filter(|_| !typing) {
        let comp = app.session.active_comp_id().map(|c| c.0);
        if st.pending_rev != app.session.revision || q["comp"].as_u64() != comp || q["time"].as_f64() != Some(app.session.time().seconds()) {
            return drop_waiting(app, ctx, st);
        }
        match ready(app, ctx, &q, st.start) {
            Ok(true) => match make(app, ctx, st, q) {
                Ok(r) if r["pending"] == true => break,
                r => {
                    st.pending.remove(0);
                    (st.asked, st.pending_rev) = (None, app.session.revision);
                    let msg = match r {
                        Ok(r) => tr_args("Cryptomatte Selection: {}", &[&r["selection"].as_str().unwrap_or_default()]),
                        Err(e) => shown_error(&e),
                    };
                    app.say(ctx, msg);
                }
            },
            Ok(false) => break,
            Err(e) => {
                (st.pending, st.after_typing) = (vec![], false);
                app.say(ctx, e);
            }
        }
    }
}

/// Middle-button pan and forwarded right-click for the armed eyedropper overlay.
fn viewer_pan_and_secondary(app: &mut EffectcraftApp, ctx: &egui::Context, resp: &egui::Response) {
    // The middle button pans the viewer, as the Hand tool (and as without the eyedropper).
    if resp.dragged_by(egui::PointerButton::Middle) {
        let pid = egui::Id::unique("viewer-fx-pick-pan");
        if resp.drag_started() {
            let start = app.ui.viewer.pan;
            ctx.data_mut(|d| d.insert_temp(pid, start));
        }
        if let (Some(start), Some(d)) = (ctx.data(|m| m.get_temp::<[f32; 2]>(pid)), resp.total_drag_delta()) {
            app.ui.viewer.pan = [start[0] + d.x, start[1] + d.y];
        }
    }
    // The right button opens the viewer's context menu (next pass).
    if resp.secondary_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique(super::viewer::FORWARDED_SECONDARY), p));
    }
}

/// Whether Space / Hand / Zoom (`tools`) or middle/right buttons (`nav`) own the pointer, and
/// whether a press at a point is the overlay's (`owned`: the Selection tool picking only over the
/// effect's layer, off the viewer's own handles there).
struct PointerNav<'a> {
    tools: bool,
    nav: bool,
    owned: &'a dyn Fn(egui::Pos2) -> bool,
    /// The overlay took the pass's presses from the viewer (it sensed clicks the pass before):
    /// one it does not own is passed on to the viewer ([`super::viewer::FORWARDED_PRIMARY`]).
    forward: bool,
    /// The overlay did not sense clicks the pass before, so egui hands the pass's presses to the
    /// viewer: one it owns (a touch, or a pen without hover, arriving on the layer and pressing in
    /// one pass) is claimed from the viewer ([`super::viewer::CLAIMED_PRIMARY`]).
    claim: bool,
}

/// Queues Cryptomatte picks from primary presses and releases on the armed eyedropper overlay.
///
/// A press is taken against the pass-start revision and frame, then queued for [`make_waiting`]
/// (or dropped if the drag, playback, frame or project make it invalid). `nav` suppresses a new
/// press when another pointer button or tool owns the click.
fn viewer_click_to_pick(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    map: &super::viewer::ViewerMap,
    ectx: &EvalCtx,
    st: &mut State,
    pointer: PointerNav<'_>,
    at: &dyn Fn(egui::Pos2) -> Option<Value>,
) {
    let ctx = ui.ctx().clone();
    // The click is what the press saw: its frame, resolution, point and project. The pass's
    // primary presses and releases are taken in order: a slow pass can hold a whole click, or one
    // click's release and the next one's press (egui then reports no press on the overlay).
    // `None`: the window lost the focus, so a held press may never see its release.
    let events: Vec<Option<(egui::Pos2, bool, egui::Modifiers)>> = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers } => Some(Some((*pos, *pressed, *modifiers))),
                egui::Event::WindowFocused(false) => Some(None),
                _ => None,
            })
            .collect()
    });
    // Space (or the Hand or Zoom tool) while the button is held cancels the press (a pan needs a
    // new press).
    if pointer.tools {
        st.press = None;
    }
    let (rev0, t0) = ctx.data(|d| d.get_temp::<(u64, f64)>(pick_start_id())).unwrap_or((app.session.revision, ectx.time.seconds()));
    for e in events {
        let Some((pos, pressed, keys)) = e else {
            st.press = None;
            continue;
        };
        if pressed {
            let in_viewer = map.area.contains(pos) && ctx.layer_id_at(pos) == Some(ui.layer_id());
            let ours = in_viewer && (pointer.owned)(pos);
            // Taken from the viewer but not ours (the pointer left the layer in this very pass):
            // the viewer clicks there next pass, so it isn't swallowed.
            if pointer.forward && in_viewer && !ours && !pointer.nav {
                let pass = ctx.cumulative_pass_nr();
                ctx.data_mut(|d| d.insert_temp(egui::Id::unique(super::viewer::FORWARDED_PRIMARY), (pos, keys, pass)));
            }
            // Handed to the viewer but ours: the viewer leaves it alone, so it only picks.
            if pointer.claim && ours && !pointer.nav {
                ctx.data_mut(|d| d.insert_temp(egui::Id::unique(super::viewer::CLAIMED_PRIMARY), ()));
            }
            st.press = (!pointer.nav && ours).then(|| Press {
                pos,
                keys,
                menu: crate::widgets::any_menu_open(&ctx),
                typing: ctx.text_edit_focused(),
                // The frame on screen at the press: a frame step earlier in this pass drops it.
                q: at(pos).map(|mut q| {
                    if t0 != ectx.time.seconds() {
                        q["time"] = json!(t0);
                    }
                    q
                }),
                rev: rev0,
            });
            continue;
        }
        // The release ends the press, whatever egui made of it.
        let Some(Press { pos: from, keys: k, menu: false, typing, q: Some(mut q), rev }) = st.press.take() else { continue };
        // Ctrl too on a Mac, where `command` is Cmd.
        q["mode"] = json!(if k.alt || k.command || k.ctrl {
            "remove"
        } else if k.shift {
            "add"
        } else {
            "replace"
        });
        let same_frame = q["time"].as_f64() == Some(ectx.time.seconds());
        let double = ctx.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));
        if double && st.last_click.as_ref() == Some(&q) {
            // The second click of a double-click, same pixel and keys: it would change nothing.
            continue;
        }
        if pos.distance(from) > SLOP {
            app.say(&ctx, tr("Drag cancelled the pick").to_string());
        } else if app.playback.playing && !(same_frame && app.session.crypto_probe_ready(&q).unwrap_or(false)) {
            // Playing, a frame without its IDs read is gone before they could be: not queued.
            app.say(&ctx, tr(PLAYING).to_string());
        } else if !same_frame || !typing && rev != app.session.revision {
            // A frame step or an edit came between the press and the release.
            drop_waiting(app, &ctx, st);
        } else {
            if st.pending.is_empty() {
                st.pending_rev = rev;
            }
            st.after_typing |= typing || ctx.text_edit_focused();
            st.last_click = Some(q.clone());
            st.pending.push(q);
        }
    }
}

/// Drop stale hover/click state when the pick or pass gap means it no longer applies.
fn reset_stale_state(app: &mut EffectcraftApp, ctx: &egui::Context, st: &mut State, pick: &FxPick) {
    let pass = ctx.cumulative_pass_nr();
    // Disarmed (or the viewer hidden) since, or another pick: start afresh, so an old click
    // waiting for its probe is never made. (A probe still being built keeps the app's flag.)
    if st.seen.is_none_or(|(k, n)| k != (pick.layer, pick.prop) || n + 1 < pass) {
        if !st.pending.is_empty() {
            app.say(ctx, tr(NOT_MADE).to_string());
        }
        *st = State::default();
    }
    st.seen = Some(((pick.layer, pick.prop), pass));
}

/// Track the shown time for scrub settle; returns whether probe builds may start.
fn track_scrub_settle(app: &EffectcraftApp, ctx: &egui::Context, st: &mut State, time_secs: f64) -> bool {
    // No builds while playing, nor until a scrub has settled.
    let now = ctx.input(|i| i.time);
    if st.shown.is_none_or(|s| s.0 != time_secs) {
        st.shown = Some((time_secs, if st.shown.is_some() { now } else { f64::MIN }));
    }
    let settled = st.shown.is_some_and(|s| now - s.1 >= SETTLE);
    if !settled {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(SETTLE));
    }
    let start = settled && !app.playback.playing;
    st.start = start;
    start
}

/// Read the Cryptomatte ID under the cursor; returns whether a probe is still being read.
fn hover_read(
    app: &mut EffectcraftApp,
    ctx: &egui::Context,
    map: &super::viewer::ViewerMap,
    st: &mut State,
    resp: &egui::Response,
    start: bool,
    at: &dyn Fn(egui::Pos2) -> Option<Value>,
) -> bool {
    let mut reading = false;
    match resp.hover_pos() {
        Some(hp) => {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            let c = map.to_comp(hp);
            app.pointer_comp = Some([c[0] as f32, c[1] as f32]);
            let q = at(hp);
            let asked = q.clone().map(|q| (q, app.session.revision));
            if asked != st.asked {
                st.reply = None;
                // While clicks wait, they own the build: the hover only reads.
                match q.as_ref().map(|q| ready(app, ctx, q, start && st.pending.is_empty())) {
                    Some(Ok(true)) => {
                        let r = q.map(|q| app.session.execute("effect.cryptomatteAt", q).map_err(|e| shown_error(&e.to_string())));
                        if r.as_ref().is_some_and(|r| r.as_ref().is_ok_and(|v| v["pending"] == true)) {
                            reading = true;
                        } else {
                            st.reply = r.filter(|r| !matches!(r, Ok(Value::Null)));
                            st.asked = asked;
                        }
                    }
                    Some(Ok(false)) => reading = true,
                    Some(Err(e)) => (st.reply, st.asked) = (Some(Err(e)), asked),
                    None => st.asked = None,
                }
            }
        }
        None => (st.asked, st.reply) = (None, None),
    }
    reading
}

/// Rebuilds the texture when the hovered ID, or the layer, effect, scale, frame or project
/// revision it was read at, changes; then draws it.
fn draw_coverage_highlight(app: &mut EffectcraftApp, ctx: &egui::Context, painter: &egui::Painter, map: &super::viewer::ViewerMap, m: &Mat3, st: &mut State) {
    let hovered = st.reply.as_ref().and_then(|r| u32::from_str_radix(r.as_ref().ok()?["hash"].as_str()?, 16).ok());
    let hovered = hovered.zip(st.asked.as_ref()).map(|(h, (q, rev))| (h, json!([q["layer"], q["effect"], q["scale"], q["time"], rev])));
    if let (Some(h), Some(q)) = (&hovered, st.asked.as_ref().map(|a| &a.0))
        && st.mask.as_ref().is_none_or(|mask| mask.key != *h)
    {
        st.mask = app.session.crypto_coverage(q, h.0, 1024).map(|c| {
            let a = app.tokens.accent;
            let px = c.coverage.iter().map(|v| Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), (v * 140.0) as u8)).collect();
            let img = egui::ColorImage::new([c.width, c.height], px);
            let size = [c.size[0], c.size[1]];
            HighlightMask {
                key: h.clone(),
                texture: ctx.load_texture("crypto-pick-highlight", img, egui::TextureOptions::NEAREST),
                size,
                uv: [c.uv[0] as f32, c.uv[1] as f32],
            }
        });
    }
    if let (Some(h), Some(mask)) = (&hovered, &st.mask)
        && h == &mask.key
    {
        let (tex, size, uv) = (&mask.texture, mask.size, mask.uv);
        let corners = [[0.0, 0.0], [size[0], 0.0], size, [0.0, size[1]]].map(|p| map.to_screen(fw::layer_to_comp(m, p)));
        let mut mesh = egui::Mesh::with_texture(tex.id());
        for (pos, t) in corners.iter().zip([[0.0, 0.0], [uv[0], 0.0], uv, [0.0, uv[1]]]) {
            mesh.vertices.push(egui::epaint::Vertex { pos: *pos, uv: egui::pos2(t[0], t[1]), color: Color32::WHITE });
        }
        mesh.indices = vec![0, 1, 2, 0, 2, 3];
        painter.add(mesh);
        // Its corners (screen points), for tests and agents.
        let label = corners.iter().map(|p| format!("{:.2},{:.2}", p.x, p.y)).collect::<Vec<_>>().join(" ");
        app.auto.add("viewer.cryptoPick.highlight", Rect::from_points(&corners), &label);
    }
}

/// Draw the status chip (hovered name, reading, or usage hint).
fn draw_status_chip(app: &mut EffectcraftApp, ctx: &egui::Context, painter: &egui::Painter, map: &super::viewer::ViewerMap, st: &State, reading: bool) {
    let err = !reading && matches!(st.reply, Some(Err(_)));
    let text = match &st.reply {
        _ if reading && app.playback.playing => tr("Stop playback to pick").to_string(),
        _ if reading => tr("Reading Cryptomatte IDs…").to_string(),
        Some(Ok(r)) => {
            let name = r["name"].as_str().map_or_else(|| format!("<{}>", r["hash"].as_str().unwrap_or_default()), str::to_string);
            if r["selected"] == true { tr_args("{} (selected)", &[&name]) } else { name }
        }
        Some(Err(e)) => e.clone(),
        None => tr_args("Click: replace · Shift: add · {}: remove", &[&remove_keys(ctx.os())]),
    };
    // One line, at most the viewer's width (long names end in an ellipsis); an error in red, on
    // as many lines as it needs (at least three, more in a narrow viewer), so its hint shows.
    let max = (map.area.width() - 44.0).clamp(40.0, 480.0);
    let rows = ((map.area.height() - 28.0) / 14.0).max(3.0) as usize;
    let colour = if err { app.tokens.danger } else { app.tokens.text };
    let mut job = egui::text::LayoutJob::simple_singleline(text.clone(), Tokens::ui(11.5), colour);
    job.wrap = egui::text::TextWrapping { max_width: max, max_rows: if err { rows } else { 1 }, break_anywhere: !err, overflow_character: Some('…') };
    let galley = painter.layout_job(job);
    let chip = Rect::from_min_size(map.area.min + vec2(10.0, 10.0), vec2(galley.size().x + 24.0, (galley.size().y + 8.0).max(22.0)));
    app.auto.add("viewer.cryptoPick", chip, &text);
    if err {
        app.auto.add("viewer.cryptoPick.error", chip, if galley.elided { "cut" } else { "whole" });
        painter.rect(chip, 6.0, Color32::from_rgba_unmultiplied(48, 10, 8, 220), egui::Stroke::new(1.0, colour), egui::StrokeKind::Inside);
    } else {
        painter.rect_filled(chip, 11.0, Color32::from_black_alpha(180));
    }
    painter.galley(chip.left_center() + vec2(12.0, -galley.size().y / 2.0), galley, colour);
}

/// Esc disarms the eyedropper only when no text field, popup, or dialog had the keyboard this
/// pass or the pass before (egui lets go of the focus and closes popups, and Esc closes the
/// dialog, before the viewer runs).
fn handle_escape(app: &mut EffectcraftApp, ui: &mut egui::Ui, st: &mut State) {
    let ctx = ui.ctx();
    // (Esc in a text field, the Selection being typed, only ends the typing; in a menu or dialog, it.)
    let esc_taken = std::mem::replace(&mut st.esc_taken, ctx.text_edit_focused() || egui::Popup::is_any_open(ctx) || app.dialog.is_some());
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !esc_taken && !st.esc_taken {
        app.ui.fx_pick = None;
        app.ui.status.clear();
        st.pending.clear();
    }
}

/// The armed eyedropper's part of the viewer, or (`implicit`) the Selection tool's while the
/// effect is selected ([`implicit_pick`]). `m` maps the layer's effect space to the comp.
#[allow(clippy::too_many_arguments)]
pub fn viewer_hook(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    map: &super::viewer::ViewerMap,
    ectx: &EvalCtx,
    layer: &Layer,
    m: &Mat3,
    pick: &FxPick,
    implicit: bool,
) {
    let ctx = ui.ctx().clone();
    let sid = pick_id();
    let mut st: State = ctx.data(|d| d.get_temp(sid)).unwrap_or_default();
    reset_stale_state(app, &ctx, &mut st, pick);
    // The Cryptomatte effect that owns the Selection (gone: the pick ends).
    let Some(fx) = layer.effects().and_then(|f| f.groups().find(|g| g.find(pick.prop).is_some())).map(|g| g.uid) else {
        if !implicit {
            app.ui.fx_pick = None;
        }
        return;
    };
    let start = track_scrub_settle(app, &ctx, &mut st, ectx.time.seconds());
    // The resolution of the frame on screen: the pick reads the pixel it shows.
    let scale = app
        .viewer_shown
        .as_ref()
        .filter(|(_, k)| k.comp == ectx.comp_id.0)
        .map_or(1.0, |(_, k)| crate::frames::render_scale(k.scale, ectx.comp.resolution.factor()));
    // The centre of the shown frame's pixel under the cursor: the layer may be resampled into
    // the frame (a proxy, a scaled layer), and that pixel is what the user sees.
    let region = super::viewer_tools::shown_region(&ctx, map.comp);
    let at = |pos: egui::Pos2| {
        let c = map.to_comp(pos);
        let snap = |v: f64, o: f64| o + (((v - o) * scale).floor() + 0.5) / scale;
        let p = fw::comp_to_layer(m, [snap(c[0], region[0]), snap(c[1], region[1])])?;
        let (x, y, time) = (p[0], p[1], ectx.time.seconds());
        Some(
            json!({"comp": ectx.comp_id.0, "layer": layer.id.0, "effect": fx, "x": x, "y": y, "scale": scale.clamp(1e-3, 1.0), "time": time, "probeOnly": true}),
        )
    };
    // The Hand and Zoom tools (Space held too) and other buttons keep their own clicks and drags.
    let tools = super::space_hand(&ctx) || matches!(app.ui.tool, Tool::Hand | Tool::Zoom);
    let nav = tools || ctx.input(|i| i.pointer.middle_down() || i.pointer.secondary_down());
    let oid = egui::Id::unique("viewer-fx-pick");
    // The Selection tool picks only over the effect's layer (#2839): the viewer's press there acts
    // on it, a selected layer, even where layers in front cover it ([`super::viewer::pick`]).
    // Elsewhere, and on the viewer's own handles there, the viewer keeps its clicks. The armed
    // eyedropper picks anywhere.
    let ui_layer = ui.layer_id();
    let on_layer = |p: egui::Pos2| {
        !implicit
            || (map.area.contains(p)
                && ctx.layer_id_at(p) == Some(ui_layer)
                && super::viewer::layer_hit(ectx, layer.id, map.to_comp(p))
                && !super::viewer::grabs_at(&ctx, p))
    };
    // A press the overlay holds (or its middle-button pan) stays its own wherever the pointer goes.
    let panning = ctx.is_being_dragged(oid) && ctx.input(|i| i.pointer.middle_down());
    let over = st.press.is_some() || panning || ctx.pointer_hover_pos().is_some_and(on_layer);
    let pass = ctx.cumulative_pass_nr();
    // Whether the overlay sensed clicks the pass before: egui handed it the pass's presses, not the
    // viewer underneath (which would select or move the layer as well). If it didn't, a press of
    // ours is claimed from the viewer; if it did, one not ours is passed on to the viewer.
    let owned_before = !implicit || st.sensed.is_some_and(|n| n.saturating_add(1) == pass);
    st.implicit = implicit;
    let mut reading = false;
    if !implicit || over {
        // Drags too: a click that drifts must not move the layer under it. (egui hands a press to
        // what sensed it the pass before, so middle and right presses land here: passed on below.)
        let resp = ui.interact(map.area, oid, if tools { Sense::hover() } else { Sense::click_and_drag() });
        if !tools {
            st.sensed = Some(pass);
        }
        viewer_pan_and_secondary(app, &ctx, &resp);
        app.auto.add(if implicit { "viewer.cryptoSelect" } else { "viewer.fxPick" }, map.area, &pick.name);
        reading = hover_read(app, &ctx, map, &mut st, &resp, start, &at);
    } else {
        (st.asked, st.reply) = (None, None);
    }
    let pointer = PointerNav { tools, nav, owned: &on_layer, forward: implicit && owned_before, claim: implicit && !owned_before };
    viewer_click_to_pick(app, ui, map, ectx, &mut st, pointer, &at);
    // Clicks waiting for their probe ([`make_waiting`], at the end of the pass).
    reading |= !st.pending.is_empty();
    draw_coverage_highlight(app, &ctx, painter, map, m, &mut st);
    if !implicit || over || reading {
        draw_status_chip(app, &ctx, painter, map, &st, reading);
    }
    // (Selected, the effect has nothing for Esc to end.)
    if !implicit {
        handle_escape(app, ui, &mut st);
    }
    ctx.data_mut(|d| d.insert_temp(sid, st));
}

/// The Cryptomatte effect selected in Effect Controls with the Selection tool active (After
/// Effects: clicks in the Composition panel pick its IDs then, no eyedropper armed; #2839): the
/// pick for its Selection. The last selected item on a selected layer counts, and only the
/// effect itself (its header in Effect Controls or the Timeline): a press on one of its rows (the
/// eyedropper's button, the Selection field) selects that property instead. An armed eyedropper
/// or crosshair (of any kind) comes first.
pub fn implicit_pick(app: &EffectcraftApp, ctx: &egui::Context) -> Option<FxPick> {
    if app.ui.tool != Tool::Selection || app.ui.fx_pick.is_some() || app.ui.text_color_pick.is_some() || crate::color_picker::eyedropper_armed(ctx) {
        return None;
    }
    // Drawing a region of interest or a Free Transform box: the viewer keeps every press.
    if app.ui.viewer.roi_draw || super::viewer_overlays::free_transform_target(ctx).is_some() {
        return None;
    }
    let comp = app.session.active_comp()?;
    let time = app.session.time();
    let st = &app.session.state;
    let (lid, uid) = st.selected_props.iter().rev().find(|(l, _)| st.selected_layers.contains(l))?;
    let layer = comp.layer(*lid).filter(|l| l.is_active_at(time))?;
    let fx = layer.effects()?.groups().find(|g| g.uid == *uid)?;
    if !fx.enabled || !matches!(&fx.kind, GroupKind::Effect { effect } if effect == EFFECT) {
        return None;
    }
    let prop = fx.get("selection")?;
    Some(FxPick { kind: KIND.into(), layer: lid.0, prop: prop.uid, name: prop.name.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_show_without_the_command_prefix() {
        let e = "invalid parameters for `effect.cryptomatteAt`: this layer has no pixels";
        assert_eq!(shown_error(e), "This layer has no pixels");
        assert_eq!(shown_error("Reading the Cryptomatte IDs failed: x"), "Reading the Cryptomatte IDs failed: x");
    }

    /// A build that panics still clears the busy flag, so the next hover can build again.
    #[test]
    fn a_panicking_build_clears_busy() {
        let busy = Arc::new(AtomicBool::new(true));
        let done = Done(busy.clone(), egui::Context::default());
        let r = std::thread::spawn(move || run(Box::new(|| panic!("a broken build")), done)).join();
        assert!(r.is_err() && !busy.load(Ordering::Acquire));
    }
}
