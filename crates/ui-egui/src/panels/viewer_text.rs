//! On-canvas text editing in the Composition viewer (Type tool, double-click a text layer).
//!
//! - **Type tool**: click empty space for point text, drag for paragraph (box) text, click a text
//!   layer to put the caret there. Cmd+T toggles horizontal / vertical type.
//! - **Editing**: caret at glyph boundaries, mouse click / drag selection, double-click selects a
//!   word and triple-click a line; arrows, Home / End and word / paragraph jumps (Cmd), Shift
//!   extends; typing, Backspace / Delete (Cmd or Alt: by word), Enter (new paragraph), Tab, Cmd+A,
//!   copy / cut / paste, IME composition; Alt+←/→ kerning (caret) or tracking (selection) ±20
//!   (Cmd: ±100); Cmd+Shift+< / > font size ±2; Cmd+Z / Cmd+Shift+Z undo / redo. Escape or
//!   Cmd+Enter commits. Paragraph text shows its box with handles that resize it.
//! - Selection highlight, caret and box are drawn in comp space through the layer's transform
//!   (and parenting).
//!
//! Every change runs an engine command (`text.*`, `layer.setText`, `edit.*`), so it's undoable and
//! agents can do the same over the control channel.

use effectcraft_engine::commands::text_edit::{ImeSurrounding, MAX_IME_TEXT_BYTES, ime_stamp_revision, ime_surrounding, layer_doc};
use effectcraft_engine::geom::{Mat3, vec2 as gv2};
use effectcraft_engine::keyframe::{Kerning, TextDoc};
use effectcraft_engine::project::{Layer, LayerId, LayerSource};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::text::kurbo::Point;
use effectcraft_engine::text::{TextLayout, layout_doc};
use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, vec2};
use serde_json::{Value, json};

use super::viewer::ViewerMap;
use crate::EffectcraftApp;
use crate::dock::PanelKind;
use crate::state::Tool;

/// Pointer interaction owned by text editing (kept across frames while the button is down).
#[derive(Clone, Debug)]
enum Drag {
    /// Extending the selection from the press.
    Select { layer: LayerId },
    /// Type tool: a click (point text) or a drag (paragraph box) from `start` (comp space).
    Create { start: [f64; 2], vertical: bool },
    /// Resizing a paragraph box by one of its 8 handles.
    Resize { layer: LayerId, handle: usize, start_box: [f64; 4] },
}

/// The `egui::Id` under which the in-progress text drag (selection, creation or box resize) is kept between frames.
fn drag_id() -> egui::Id {
    egui::Id::unique("viewer-text-drag")
}
/// The `egui::Id` of the invisible focusable widget that owns keyboard and IME focus while on-canvas text editing is
/// active.
fn focus_id() -> egui::Id {
    egui::Id::unique("viewer-text-focus")
}

/// The immutable identity of one text-edit IME target.
///
/// The layer and the edit-session token are the whole owner. Caret, selection, text, revision
/// and geometry are deliberately absent: ordinary IME edits keep this identity. Application caret
/// changes interrupt the composition separately, without replacing this immutable target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ImeOwner {
    layer: LayerId,
    session: u64,
}

/// Last accepted selection for this viewport, separate from immutable target identity.
/// IME deletes/commits advance it; application changes invalidate a live composition first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ImeSelection {
    owner: ImeOwner,
    anchor: usize,
    caret: usize,
}

/// One viewport's visible IME pre-edit.
///
/// The text is only a drawing preview; it never enters the document. Its owner identifies the
/// composition that produced it, so a preview can never survive a target switch.
#[derive(Clone, Debug)]
struct Preedit {
    owner: ImeOwner,
    text: String,
}

/// The input-method composition currently in flight for one viewport.
///
/// `TerminalPending` is an observed empty pre-edit: the input method cleared the visible
/// composition, but its terminal commit/delete is a separate event and may still arrive. Keeping
/// the owner means a target switch turns that state into `Stale`, so the pending old terminal event
/// is rejected instead of being applied to the replacement.
///
/// `Stale` rejects an interrupted composition until its exact request-bound completion. Old
/// terminals, empty/nonempty preedit and idle frames never acknowledge cancellation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImeComposition {
    Active(ImeOwner),
    TerminalPending(ImeOwner),
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OutputContext(egui::Id, u64, egui::IMEPurpose);

impl From<&egui::output::IMEOutput> for OutputContext {
    /// Copy the owner, context generation and purpose out of egui's IME output so they can be cached and compared
    /// cheaply.
    fn from(output: &egui::output::IMEOutput) -> Self {
        Self(output.owner, output.context_generation, output.purpose)
    }
}

#[derive(Clone, Debug)]
struct ResetGuard {
    request: Option<egui::ImeResetId>,
    destination: Option<ImeSelection>,
    committed: egui::ImeMutationStamp,
    output: Option<OutputContext>,
    suspended: bool,
}

/// The `egui::Id` under which this viewport's `ResetGuard` is stored.
fn reset_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("effectcraft.ime.reset", viewport))
}
/// The `egui::Id` holding whether this viewport is currently inside a full UI pass (set by the pass hooks).
fn mode_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("effectcraft.ime.ui-pass", viewport))
}
/// The `egui::Id` holding the last committed IME mutation stamp accepted for this viewport.
fn committed_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("effectcraft.ime.committed", viewport))
}
/// The `egui::Id` holding the frame number whose text-editor keyboard events were already consumed.
fn input_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("effectcraft.ime.input-frame", viewport))
}
/// The `egui::Id` holding whether this viewport may request an IME reset in the current pass.
fn admission_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("effectcraft.ime.can-request", viewport))
}

/// Allow or forbid IME reset requests for the current viewport's pass by storing `allowed` in context data.
fn set_admission(ctx: &egui::Context, allowed: bool) {
    let id = admission_id(ctx.viewport_id());
    ctx.data_mut(|data| data.insert_temp(id, allowed));
}

/// Whether the window can receive input: focused, not minimized and not occluded. Unknown viewport flags count as
/// available.
fn window_available(ctx: &egui::Context) -> bool {
    ctx.input(|input| {
        input.focused && input.viewport().focused.unwrap_or(true) && input.viewport().minimized != Some(true) && input.viewport().occluded != Some(true)
    })
}

/// Install the per-pass IME hooks on `ctx` once: the begin hook marks a full UI pass and the end hook clears it.
/// Later calls do nothing.
pub(crate) fn install_hooks(ctx: &egui::Context) {
    let id = egui::Id::unique("effectcraft.ime.hooks-installed");
    let first = ctx.data_mut(|data| {
        if data.get_temp::<bool>(id).unwrap_or(false) {
            false
        } else {
            data.insert_temp(id, true);
            true
        }
    });
    if !first {
        return;
    }
    ctx.on_begin_pass(
        "EffectCraft IME pass begin",
        std::sync::Arc::new(|ui| {
            let id = mode_id(ui.ctx().viewport_id());
            ui.ctx().data_mut(|data| data.insert_temp(id, true));
        }),
    );
    ctx.on_end_pass(
        "EffectCraft IME pass end",
        std::sync::Arc::new(|ui| {
            let id = mode_id(ui.ctx().viewport_id());
            ui.ctx().data_mut(|data| data.insert_temp(id, false));
        }),
    );
}

/// Read the current viewport's pending reset guard, if any.
fn read_reset(ctx: &egui::Context) -> Option<ResetGuard> {
    let id = reset_id(ctx.viewport_id());
    ctx.data(|data| data.get_temp(id))
}

/// Record a reset for the current viewport: request a fresh IME fence when this pass is admitted, then store the
/// destination selection, committed stamp and output identity and mark the composition stale. An unadmitted pass
/// stores a suspended guard instead.
fn request_reset_with_output(app: &EffectcraftApp, ctx: &egui::Context, output: Option<OutputContext>) {
    let viewport = ctx.viewport_id();
    let allowed = ctx.data(|data| data.get_temp::<bool>(admission_id(viewport)).unwrap_or(false));
    let request = if allowed { ctx.request_ime_reset(viewport) } else { None };
    let guard = ResetGuard { request, destination: ime_selection(app), committed: ime_stamp(app, ctx), output, suspended: !allowed };
    ctx.data_mut(|data| {
        data.insert_temp(reset_id(viewport), guard);
        data.insert_temp(ime_composition_id(viewport), ImeComposition::Stale);
    });
}

/// Re-request a reset for the current viewport, keeping the output identity published earlier in the pass.
fn request_reset(app: &EffectcraftApp, ctx: &egui::Context) {
    let output = read_reset(ctx).and_then(|guard| guard.output);
    request_reset_with_output(app, ctx, output);
}

/// Re-request the reset when an unsuspended guard's destination selection or committed stamp no longer matches the
/// app.
fn sync_reset(app: &EffectcraftApp, ctx: &egui::Context) {
    if let Some(guard) = read_reset(ctx)
        && !guard.suspended
        && (guard.destination != ime_selection(app) || guard.committed != ime_stamp(app, ctx))
    {
        request_reset(app, ctx);
    }
}

/// Retire the guard's request id locally and suspend it, so only a later eligible pass can request a new fence.
fn suspend_reset(ctx: &egui::Context) {
    if let Some(mut guard) = read_reset(ctx) {
        guard.request = None;
        guard.suspended = true;
        let id = reset_id(ctx.viewport_id());
        ctx.data_mut(|data| data.insert_temp(id, guard));
    }
}

/// Accept `request` only when it is this viewport's current, unsuspended request and the destination and stamp still
/// match; then sync target and selection, clear the composition and preview and remember the selection.
fn complete_reset(app: &EffectcraftApp, ui: &egui::Ui, request: egui::ImeResetId) {
    if !read_reset(ui.ctx()).is_some_and(|guard| !guard.suspended && guard.request == Some(request)) {
        return;
    }
    sync_ime_target(ui, app);
    sync_ime_selection(ui, app);
    let ctx = ui.ctx();
    let viewport = ctx.viewport_id();
    if read_reset(ctx).is_some_and(|guard| {
        !guard.suspended
            && guard.request == Some(request)
            && request.viewport_id() == viewport
            && guard.destination == ime_selection(app)
            && guard.committed == ime_stamp(app, ctx)
    }) {
        ctx.data_mut(|data| data.remove::<ResetGuard>(reset_id(viewport)));
        set_ime_composition(ui, None);
        clear_preedit(ui);
        remember_ime_selection(ui, app);
    }
}

/// Withhold custom IME controls for this pass: suspend the reset guard and drop every `ImeResetCompleted` event.
/// No custom mutation route can be waiting for completion here; stock text widgets keep their keys and text events.
fn unavailable_controls(ui: &egui::Ui) {
    suspend_reset(ui.ctx());
    // No custom mutation route runs here, so controls can only be obsolete. Stock text/keys
    // remain in place for their actual TextEdit owner.
    ui.ctx().input_mut(|input| input.events.retain(|event| !matches!(event, egui::Event::ImeResetCompleted { .. })));
}

/// Start a UI pass: revoke IME reset admission and clear the geometry published by the previous pass.
pub(crate) fn begin_ui(ctx: &egui::Context) {
    set_admission(ctx, false);
    let id = ime_geometry_id(ctx.viewport_id());
    ctx.data_mut(|data| data.remove::<ImeGeometry>(id));
}

/// Whether the text editor recorded its IME geometry in this pass of the current viewport.
pub(crate) fn ime_recorded(ctx: &egui::Context) -> bool {
    let viewport = ctx.viewport_id();
    let pass = ctx.cumulative_pass_nr_for(viewport);
    ctx.data(|d| d.get_temp::<ImeGeometry>(ime_geometry_id(viewport))).is_some_and(|(p, _, _)| p == pass)
}

/// Close a logic-only tick: when no full UI pass ran, revoke admission and resync and suspend the reset guard so a
/// hidden tick never publishes IME state.
pub(crate) fn finish_logic(app: &EffectcraftApp, ctx: &egui::Context) {
    let id = mode_id(ctx.viewport_id());
    if !ctx.data(|data| data.get_temp::<bool>(id).unwrap_or(false)) {
        set_admission(ctx, false);
        sync_reset(app, ctx);
        suspend_reset(ctx);
    }
}

type ImeGeometry = (u64, egui::Rect, egui::Rect);
type SurroundingKey = (u64, u64, u64, u64, usize, usize, i64);

#[derive(Clone, Debug)]
struct SurroundingCache {
    key: SurroundingKey,
    value: Option<ImeSurrounding>,
}

/// The `egui::Id` under which this viewport's visible IME pre-edit preview is stored.
fn ime_preedit_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-preedit", viewport))
}
/// The `egui::Id` under which this viewport's in-flight IME composition state is stored.
fn ime_composition_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-composition", viewport))
}
/// The `egui::Id` under which the current pass number, text region and caret rectangle are stored for the
/// viewport IME publisher.
fn ime_geometry_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-geometry", viewport))
}
/// The `egui::Id` under which the last text-edit target given keyboard focus is stored.
fn ime_seen_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-seen", viewport))
}
/// The `egui::Id` under which the last `ImeOwner` observed by `sync_ime_target` is stored.
fn ime_target_seen_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-target", viewport))
}
/// The `egui::Id` under which the last accepted IME selection is stored.
fn ime_selection_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-selection", viewport))
}
/// The `egui::Id` under which the last published IME owner and context generation are stored.
fn ime_last_context_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-last-context", viewport))
}
/// The `egui::Id` under which the bounded IME surrounding-text snapshot and its cache key are stored.
fn ime_surrounding_cache_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-surrounding", viewport))
}
/// The `egui::Id` under which the typing merge counter is stored.
fn typing_key_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-typing", viewport))
}

/// The immutable IME owner of the current edit session.
///
/// The comp/layer target identity survives ordinary edits and caret moves. The edit session
/// token distinguishes a target that reuses the same numeric ids (a restarted edit, a reopened
/// project, undo that recreates the layer); a different comp or layer is a different owner. No
/// caret, selection, text, document position or view geometry takes part.
fn ime_target(app: &EffectcraftApp) -> Option<egui::ImeTarget> {
    let comp = app.session.active_comp_id()?;
    let edit = app.session.state.text_edit.as_ref()?;
    Some(egui::ImeTarget::new(egui::Id::unique(("effectcraft-ime-text", comp.0, edit.layer.0)), edit.session))
}

/// The layer and edit-session token of the active text edit, or `None` when no editor is open. Caret, selection and
/// text are deliberately excluded.
fn ime_owner(app: &EffectcraftApp) -> Option<ImeOwner> {
    let edit = app.session.state.text_edit.as_ref()?;
    Some(ImeOwner { layer: edit.layer, session: edit.session })
}

/// The bounded committed snapshot used by an IME receipt.
///
/// The cached value is keyed by the exact state that can change it. Repeated observations in
/// one pass/frame (receipts and the final descriptor) therefore reuse one bounded snapshot
/// instead of evaluating an animated Source Text or scanning the document each time.
fn ime_surrounding_cached(app: &EffectcraftApp, ctx: &egui::Context) -> Option<ImeSurrounding> {
    let viewport = ctx.viewport_id();
    let edit = app.session.state.text_edit.as_ref();
    let key = (
        app.session.revision,
        app.session.active_comp_id().map_or(0, |c| c.0),
        edit.map_or(0, |e| e.layer.0),
        edit.map_or(0, |e| e.session),
        edit.map_or(0, |e| e.anchor),
        edit.map_or(0, |e| e.caret),
        app.session.time().0,
    );
    if let Some(cache) = ctx.data(|d| d.get_temp::<SurroundingCache>(ime_surrounding_cache_id(viewport)))
        && cache.key == key
    {
        return cache.value;
    }
    let value = ime_surrounding(&app.session);
    ctx.data_mut(|d| d.insert_temp(ime_surrounding_cache_id(viewport), SurroundingCache { key, value: value.clone() }));
    value
}

/// The committed-text stamp an IME receipt compares against: the bounded snapshot when it is available, else the
/// editor revision marked unavailable. It deliberately does not use the global project revision, so an unrelated edit
/// leaves a wide selection's stamp unchanged.
fn ime_stamp(app: &EffectcraftApp, ctx: &egui::Context) -> egui::ImeMutationStamp {
    match ime_surrounding_cached(app, ctx) {
        Some(w) => egui::ImeMutationStamp::from_committed(&w.text, w.cursor, w.anchor, None),
        // A global project revision would make an unrelated edit (an opacity undo) look like a
        // mutation of a wide selection. The editor revision only moves with committed text and
        // the editor session.
        None => egui::ImeMutationStamp::unavailable_at_revision(ime_stamp_revision(&app.session)),
    }
}

/// Record an accepted IME commit/delete mutation for this viewport and frame.
///
/// Call it only after the command succeeded and actually changed the committed text (`before`
/// and `after` are captured immediately around that mutation). Rejected events, no-ops and
/// failed commands earn no receipt.
fn note_ime_receipt(app: &mut EffectcraftApp, ctx: &egui::Context, before: egui::ImeMutationStamp, after: egui::ImeMutationStamp) {
    let Some(target) = ime_target(app) else { return };
    let viewport = ctx.viewport_id();
    let frame = ctx.cumulative_frame_nr_for(viewport);
    app.ime_tracker.note_input_method_apply(viewport, frame, target, before, after);
}

/// Record an application edit only when it actually changed the committed snapshot.
///
/// A no-op (Backspace at the start of the document, a failed command) must not poison a later
/// accepted IME commit in the same frame.
fn note_ime_application_edit(app: &mut EffectcraftApp, ctx: &egui::Context, before: egui::ImeMutationStamp) {
    if ime_stamp(app, ctx) != before {
        note_ime_application_change(app, ctx);
    }
}

/// Record application-origin work (typing, paste, undo, automatic correction, ...) so `Other`
/// dominates every remaining pass of the frame.
fn note_ime_application_change(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let Some(target) = ime_target(app) else { return };
    let viewport = ctx.viewport_id();
    let frame = ctx.cumulative_frame_nr_for(viewport);
    app.ime_tracker.note_application_change(viewport, frame, target);
}

/// Drop this viewport's visible pre-edit preview.
fn clear_preedit(ui: &egui::Ui) {
    let viewport = ui.ctx().viewport_id();
    ui.data_mut(|d| d.remove::<Preedit>(ime_preedit_id(viewport)));
}

/// Read this viewport's in-flight IME composition state.
fn read_ime_composition(ui: &egui::Ui) -> Option<ImeComposition> {
    let viewport = ui.ctx().viewport_id();
    ui.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(viewport)))
}

/// Store this viewport's composition state, or remove it with `None`.
fn set_ime_composition(ui: &egui::Ui, composition: Option<ImeComposition>) {
    let viewport = ui.ctx().viewport_id();
    ui.data_mut(|d| match composition {
        Some(composition) => {
            d.insert_temp(ime_composition_id(viewport), composition);
        }
        None => d.remove::<ImeComposition>(ime_composition_id(viewport)),
    });
}

/// Mark the composition in flight as interrupted by application work.
///
/// A live visible composition (`Active`) or an observed empty pre-edit whose terminal event is
/// still pending (`TerminalPending`) needs a tombstone; there is no lingering event to reject when
/// no pre-edit was seen. Repeated hidden passes retire an already guarded activation without
/// allocating again; reactivation requests a new fence. Missing acknowledgement stays closed.
fn invalidate_ime_composition(ui: &egui::Ui, app: &EffectcraftApp) {
    if matches!(read_ime_composition(ui), Some(ImeComposition::Active(_) | ImeComposition::TerminalPending(_))) {
        set_ime_composition(ui, Some(ImeComposition::Stale));
        request_reset(app, ui.ctx());
    }
}

/// Keep the composition lifecycle aligned with the current edit target.
///
/// A target change or the end of the edit invalidates a visible composition or an observed empty
/// pre-edit with a still-pending terminal event, and clears the visible preview. The tombstone is
/// kept across arbitrary idle frames until the exact request-bound completion is observed.
fn sync_ime_target(ui: &egui::Ui, app: &EffectcraftApp) {
    let viewport = ui.ctx().viewport_id();
    let current = ime_owner(app);
    let previous = ui.data(|d| d.get_temp::<Option<ImeOwner>>(ime_target_seen_id(viewport))).flatten();
    if previous == current {
        return;
    }
    if previous.is_some() {
        // The previous target was replaced or the editor ended. Any composition it had is now
        // stale, including one whose next pre-edit/commit arrives in a later raw batch.
        invalidate_ime_composition(ui, app);
    }
    // The preview belongs to the previous target even when the new one has the same numeric ids.
    clear_preedit(ui);
    ui.data_mut(|d| d.insert_temp(ime_target_seen_id(viewport), current));
}

/// The active text edit's owner, anchor and caret, or `None` when no editor is open.
fn ime_selection(app: &EffectcraftApp) -> Option<ImeSelection> {
    let edit = app.session.state.text_edit.as_ref()?;
    Some(ImeSelection { owner: ime_owner(app)?, anchor: edit.anchor, caret: edit.caret })
}

/// Accept the selection resulting from an IME event without cancelling its remaining events.
fn remember_ime_selection(ui: &egui::Ui, app: &EffectcraftApp) {
    let viewport = ui.ctx().viewport_id();
    let selection = ime_selection(app);
    ui.data_mut(|d| d.insert_temp(ime_selection_id(viewport), selection));
    let stamp = ime_stamp(app, ui.ctx());
    ui.data_mut(|d| d.insert_temp(committed_id(viewport), stamp));
}

/// An application caret/selection change cannot inherit the old composition's mutations.
/// Run before IME dispatch, and after application events, while the old selection is still
/// recorded. Accepted IME events update that record separately: their own delete must not
/// invalidate the commit that follows it.
fn sync_ime_selection(ui: &egui::Ui, app: &EffectcraftApp) {
    let viewport = ui.ctx().viewport_id();
    let previous = ui.data(|d| d.get_temp::<Option<ImeSelection>>(ime_selection_id(viewport))).flatten();
    let committed = ui.data(|data| data.get_temp::<egui::ImeMutationStamp>(committed_id(viewport)));
    let changed = previous != ime_selection(app) || committed.is_some_and(|previous| previous != ime_stamp(app, ui.ctx()));
    if changed && matches!(read_ime_composition(ui), Some(ImeComposition::Active(_) | ImeComposition::TerminalPending(_))) {
        invalidate_ime_composition(ui, app);
        clear_preedit(ui);
    }
    sync_reset(app, ui.ctx());
    remember_ime_selection(ui, app);
}

/// Truncate a pre-edit preview to `MAX_IME_TEXT_BYTES` on a character boundary, so no character is split.
fn bounded_preedit(mut text: String) -> String {
    if text.len() > MAX_IME_TEXT_BYTES {
        let mut end = MAX_IME_TEXT_BYTES;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// Read this viewport's visible pre-edit preview, whatever owner it belongs to.
fn read_preedit(ui: &egui::Ui) -> Option<Preedit> {
    let viewport = ui.ctx().viewport_id();
    ui.data(|d| d.get_temp::<Preedit>(ime_preedit_id(viewport)))
}

/// Clear a preview that belongs to `owner`, leaving any other viewport's guard untouched.
fn clear_current_preedit(ui: &egui::Ui, owner: ImeOwner) {
    if read_preedit(ui).is_some_and(|p| p.owner == owner) {
        clear_preedit(ui);
    }
}

/// Whether an incoming IME mutation belongs to the currently edited composition.
///
/// With no composition a direct commit/delete (platforms may send one without a preview) is
/// accepted. Once a composition is visible or has an observed empty pre-edit awaiting its terminal
/// event, only its matching owner and uninterrupted selection may mutate the document. A target
/// switch or application caret change leaves a tombstone rejecting the interrupted composition.
fn ime_event_is_stale(ui: &egui::Ui, owner: ImeOwner) -> bool {
    match read_ime_composition(ui) {
        Some(ImeComposition::Stale) => true,
        Some(ImeComposition::Active(active) | ImeComposition::TerminalPending(active)) => active != owner,
        None => false,
    }
}

/// The pre-edit text to draw, only when it belongs to the current edit owner and is non-empty.
fn visible_preedit(ui: &egui::Ui, app: &EffectcraftApp) -> Option<String> {
    let owner = ime_owner(app)?;
    let p = read_preedit(ui)?;
    (p.owner == owner && !p.text.is_empty()).then_some(p.text)
}

/// Publish the descriptor from this viewport's current text state and recorded geometry.
///
/// The root viewport publishes after dock actions, ScriptUI, learn and dialogs. Native panel
/// windows publish after the IME-owning body records geometry and again after their last body;
/// publication can therefore precede later bodies in that window. The overlay only records
/// geometry; this function resolves the current owner and availability when it publishes.
pub(crate) fn finish_ime(app: &mut EffectcraftApp, ui: &egui::Ui) {
    let ctx = ui.ctx();
    let viewport = ctx.viewport_id();
    let frame = ctx.cumulative_frame_nr_for(viewport);
    let pass = ctx.cumulative_pass_nr_for(viewport);
    let geometry = ctx.data(|d| d.get_temp::<ImeGeometry>(ime_geometry_id(viewport))).filter(|(p, _, _)| *p == pass);
    // The window itself must be focused too: a forced hidden-window UI pass keeps egui's widget
    // focus, but no input method may target it. `None` is "unknown" (tests, some platforms).
    let window_focused = window_available(ctx);
    let focused = geometry.is_some() && window_focused && ui.is_enabled() && app.dialog.is_none() && ctx.memory(|m| m.has_focus(focus_id()));
    // Resolve availability, but withhold admission from the synchronizers: they may retire a
    // guard and must not allocate before this pass's actual output is known. Otherwise a
    // destination change in sync could allocate one ID and the output-identity check below a
    // second ID in the same pass.
    set_admission(ctx, false);
    sync_ime_target(ui, app);
    sync_ime_selection(ui, app);
    let update = match (ime_target(app).filter(|_| focused), geometry) {
        (Some(target), Some((_, _rect, _cursor_rect))) => {
            let window = ime_surrounding_cached(app, ctx);
            let committed = window.as_ref().map(|w| egui::CommittedText::new(&w.text, w.cursor, w.anchor));
            // An unavailable observation still carries the editor revision: it is the only
            // identity an IME receipt has when the selection is wider than the platform limit,
            // and it must match `ime_stamp`'s revision exactly.
            let observation = egui::ImeObservation::new(frame, pass, target).committed(committed).unavailable_revision(ime_stamp_revision(&app.session));
            app.ime_tracker.observe(viewport, Some(observation))
        }
        _ => {
            app.ime_tracker.observe(viewport, None);
            // Losing custom publication retires the old ID. Returning to the same editor must
            // request a new fence rather than await a discarded or foreign descriptor's ID.
            invalidate_ime_composition(ui, app);
            clear_preedit(ui);
            None
        }
    };
    let custom = update.as_ref().map(|update| OutputContext(update.owner, update.context_generation, update.purpose));
    if let Some(update) = update {
        let key = (update.owner, update.context_generation);
        let previous = ctx.data(|d| d.get_temp::<(egui::Id, u64)>(ime_last_context_id(viewport)));
        // A new owner or replacement epoch on the same widget focus must cancel the platform's
        // composition too; waiting for egui's focus change is not enough on Windows/macOS/X11.
        let interrupt = previous.is_some_and(|p| p != key);
        if interrupt {
            ctx.memory_mut(|m| m.interrupt_ime());
        }
        ctx.data_mut(|d| d.insert_temp(ime_last_context_id(viewport), key));
        ctx.output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                owner: update.owner,
                context_generation: update.context_generation,
                purpose: update.purpose,
                rect: geometry.map_or(egui::Rect::NOTHING, |(_, r, _)| r),
                cursor_rect: geometry.map_or(egui::Rect::NOTHING, |(_, _, c)| c),
                should_interrupt_composition: interrupt,
                reset_request: None,
                change_cause: update.change_cause,
                surrounding_text: update.surrounding_text,
            });
        });
    } else {
        // Clear only this custom editor's descriptor. A stock TextEdit that drew later in the
        // same pass owns `output.ime` now and must keep it.
        let previous = ctx.data(|d| d.get_temp::<(egui::Id, u64)>(ime_last_context_id(viewport)));
        ctx.output_mut(|o| {
            if previous.is_some_and(|(owner, _)| o.ime.as_ref().is_some_and(|ime| ime.owner == owner)) {
                o.ime = None;
            }
        });
    }
    // `ctx.output` here is the descriptor this function just wrote, not proof that the host
    // accepted it. On the publishing path `actual == custom` is true by construction, so it is
    // only a defensive no-update check (there `custom.is_none()` already decides). The real
    // foreign/stock protection is the owner check in the no-update branch above, not this test.
    let actual = ctx.output(|output| output.ime.as_ref().map(OutputContext::from));
    let eligible = focused && custom.is_some() && actual == custom;
    set_admission(ctx, eligible);
    if !eligible {
        unavailable_controls(ui);
    } else if let Some(guard) = read_reset(ctx) {
        // The confirmed eligible finalizer makes the one destination/output-aware allocation
        // for this pass. A suspended or superseded guard was retired locally by the
        // synchronizers; only here is a new request put into core Memory, carrying the output
        // context actually published above.
        if guard.suspended || guard.destination != ime_selection(app) || guard.output.is_some_and(|old| Some(old) != custom) {
            request_reset_with_output(app, ctx, custom);
        } else if let Some(mut guard) = read_reset(ctx) {
            guard.output = custom;
            ctx.data_mut(|data| data.insert_temp(reset_id(viewport), guard));
        }
    }
}

/// Release one native viewport's IME data and tracker state.
pub(crate) fn end_viewport(app: &mut EffectcraftApp, ctx: &egui::Context, viewport: egui::ViewportId) {
    ctx.data_mut(|d| {
        d.remove::<Preedit>(ime_preedit_id(viewport));
        d.remove::<ImeComposition>(ime_composition_id(viewport));
        d.remove::<ImeGeometry>(ime_geometry_id(viewport));
        d.remove::<Option<(u64, u64)>>(ime_seen_id(viewport));
        d.remove::<Option<ImeOwner>>(ime_target_seen_id(viewport));
        d.remove::<Option<ImeSelection>>(ime_selection_id(viewport));
        d.remove::<(egui::Id, u64)>(ime_last_context_id(viewport));
        d.remove::<SurroundingCache>(ime_surrounding_cache_id(viewport));
        d.remove::<u64>(typing_key_id(viewport));
        d.remove::<ResetGuard>(reset_id(viewport));
        d.remove::<egui::ImeMutationStamp>(committed_id(viewport));
        d.remove::<u64>(input_id(viewport));
        d.remove::<bool>(mode_id(viewport));
        d.remove::<bool>(admission_id(viewport));
    });
    app.ime_tracker.end_viewport(viewport);
}

/// The geometry of an edited text layer.
struct Edited {
    layer: Layer,
    doc: TextDoc,
    lay: TextLayout,
    /// Layer → comp.
    m: Mat3,
    inv: Mat3,
    /// Per caret position: static layout → animated / on-path character placement.
    carets: Vec<Mat3>,
}

impl Edited {
    fn to_screen(&self, map: &ViewerMap, p: (f64, f64)) -> Pos2 {
        let q = self.m.apply(gv2(p.0, p.1));
        map.to_screen([q.x, q.y])
    }
    fn to_layer(&self, map: &ViewerMap, s: Pos2) -> (f64, f64) {
        let c = map.to_comp(s);
        let q = self.inv.apply(gv2(c[0], c[1]));
        (q.x, q.y)
    }
    /// The text's region in layer space: the box, or the laid-out lines.
    fn region(&self) -> [f64; 4] {
        if let Some(b) = self.doc.box_size {
            return [self.doc.box_pos[0], self.doc.box_pos[1], self.doc.box_pos[0] + b[0], self.doc.box_pos[1] + b[1]];
        }
        let b = self.lay.layout.bounds;
        let corners = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].map(|(x, y)| self.lay.to_layer * Point::new(x as f64, y as f64));
        let xs = corners.iter().map(|p| p.x);
        let ys = corners.iter().map(|p| p.y);
        let pad = 6.0;
        [
            xs.clone().fold(f64::INFINITY, f64::min) - pad,
            ys.clone().fold(f64::INFINITY, f64::min) - pad,
            xs.fold(f64::NEG_INFINITY, f64::max) + pad,
            ys.fold(f64::NEG_INFINITY, f64::max) + pad,
        ]
    }
    fn contains(&self, map: &ViewerMap, s: Pos2) -> bool {
        let (x, y) = self.to_layer(map, s);
        let r = self.region();
        x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]
    }
    fn hit(&self, map: &ViewerMap, s: Pos2) -> usize {
        let (x, y) = self.to_layer(map, s);
        self.lay.hit(Point::new(x, y))
    }
    /// Box handle positions (layer space): corners and edge midpoints, clockwise from top-left.
    fn handles(&self) -> Vec<(f64, f64)> {
        let Some(b) = self.doc.box_size else { return vec![] };
        let (x0, y0) = (self.doc.box_pos[0], self.doc.box_pos[1]);
        let (x1, y1) = (x0 + b[0], y0 + b[1]);
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        vec![(x0, y0), (mx, y0), (x1, y0), (x1, my), (x1, y1), (mx, y1), (x0, y1), (x0, my)]
    }
}

fn edited(app: &EffectcraftApp, ectx: &EvalCtx, lid: LayerId) -> Option<Edited> {
    let layer = ectx.comp.layer(lid)?.clone();
    if !matches!(layer.source, LayerSource::Text) {
        return None;
    }
    let doc = layer_doc(&app.session, lid)?;
    let lay = layout_doc(&doc);
    let m = super::viewer::l2c(ectx, &layer).0;
    let inv = m.inverse()?;
    let carets = effectcraft_engine::render::text::caret_maps(ectx, &layer, lay.chars);
    Some(Edited { layer, doc, lay, m, inv, carets })
}

/// Type-tool clicks prefer a selected text layer, as viewer dragging does (#230).
/// Double-clicks retain topmost picking. Both use the same visibility/lock/time/solo filter.
fn text_layer_at(app: &EffectcraftApp, ectx: &EvalCtx, map: &ViewerMap, s: Pos2, prefer_selected: bool) -> Option<Edited> {
    let mut topmost = None;
    for e in super::viewer::selectable_layers(ectx.comp, ectx.time)
        .filter(|l| matches!(l.source, LayerSource::Text))
        .filter_map(|l| edited(app, ectx, l.id))
        .filter(|e| e.contains(map, s))
    {
        if !prefer_selected || app.session.state.selected_layers.contains(&e.layer.id) {
            return Some(e);
        }
        if topmost.is_none() {
            topmost = Some(e);
        }
    }
    topmost
}

fn exec(app: &mut EffectcraftApp, id: &str, p: Value) -> Option<Value> {
    match app.session.execute(id, p) {
        Ok(v) => Some(v),
        Err(e) => {
            app.ui.status = e.to_string();
            None
        }
    }
}

/// The panels a text edit stays open across: the viewers, and Character, Paragraph and
/// Properties, which format the selected characters.
fn keeps_text_edit(p: PanelKind) -> bool {
    super::viewers::id_of(p).is_some() || matches!(p, PanelKind::Character | PanelKind::Paragraph | PanelKind::Properties)
}

/// A click in panel `p`. Any other panel (the Timeline, the Project panel, …) ends the edit, as a
/// click outside the text does, and gives the keyboard back to the app (#831).
pub fn panel_clicked(app: &mut EffectcraftApp, ctx: &egui::Context, p: PanelKind) {
    if app.session.state.text_edit.is_some() && !keeps_text_edit(p) {
        exec(app, "text.endEdit", json!({}));
        // `viewport_id()` takes the context's lock itself, so resolve it before entering
        // `data_mut`; nesting the two calls deadlocks.
        let viewport = ctx.viewport_id();
        ctx.data_mut(|d| d.remove::<Preedit>(ime_preedit_id(viewport)));
        ctx.memory_mut(|m| m.surrender_focus(focus_id()));
    }
}

/// Typing merge key: consecutive keystrokes fold into one undo step until the caret moves.
fn typing_key(ui: &egui::Ui, bump: bool) -> String {
    let id = typing_key_id(ui.ctx().viewport_id());
    let mut n: u64 = ui.data(|d| d.get_temp(id)).unwrap_or(0);
    if bump {
        n += 1;
        ui.data_mut(|d| d.insert_temp(id, n));
    }
    format!("text-typing-{n}")
}

/// Viewer hook: draws the edit overlay and handles text pointer and keyboard input. Returns
/// true when it owns this frame's pointer interaction (the viewer then skips its own gestures).
pub fn hook(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    resp: &egui::Response,
    space_pan: bool,
) -> bool {
    let tool = app.ui.tool;
    let type_tool = matches!(tool, Tool::Type | Tool::TypeVertical);
    sync_ime_target(ui, app);
    sync_ime_selection(ui, app);
    let last_tool_id = egui::Id::unique("viewer-text-last-tool");
    let last_tool = ui.data(|d| d.get_temp::<Tool>(last_tool_id));
    ui.data_mut(|d| d.insert_temp(last_tool_id, tool));
    if last_tool.is_some_and(|previous| matches!(previous, Tool::Type | Tool::TypeVertical) && !type_tool) {
        exec(app, "text.endEdit", json!({}));
        ui.data_mut(|d| d.remove::<Drag>(drag_id()));
    }
    // The session ends when its layer leaves the active comp.
    if let Some(e) = app.session.state.text_edit.clone()
        && ectx.comp.layer(e.layer).is_none()
    {
        exec(app, "text.endEdit", json!({}));
    }
    let editing = app.session.state.text_edit.clone();
    let cur = editing.as_ref().and_then(|e| edited(app, ectx, e.layer));
    let mut owns = false;
    let mut drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id()));
    let (pressed, down, released, mods) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.primary_released(), i.modifiers));
    // The latest primary press of this frame. `press_origin` is already cleared when a press and
    // release arrive in one frame; the input events retain the latest press even after release.
    // An invalid latest press
    // must not become an earlier click or a release-position click.
    let press = ui.input(|i| {
        let latest = i.events.iter().rev().find_map(|event| match event {
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } => Some(*pos),
            _ => None,
        });
        match latest {
            Some(pos) => pos.is_finite().then_some(pos),
            None => i.pointer.press_origin().filter(|pos| pos.is_finite()),
        }
    });
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let handle_hit = |e: &Edited, s: Pos2| e.handles().iter().position(|h| e.to_screen(map, *h).distance(s) < 7.0);

    // ---- press
    if pressed
        && !space_pan
        && resp.hovered()
        && !matches!(tool, Tool::Hand | Tool::Zoom)
        && let Some(pos) = press
        && resp.rect.intersect(ui.clip_rect()).contains(pos)
    {
        let mut handled = false;
        if let Some(e) = &cur {
            if let Some(h) = handle_hit(e, pos) {
                let b = e.doc.box_size.unwrap_or([0.0; 2]);
                drag = Some(Drag::Resize { layer: e.layer.id, handle: h, start_box: [e.doc.box_pos[0], e.doc.box_pos[1], b[0], b[1]] });
                handled = true;
            } else if e.contains(map, pos) {
                let ci = e.hit(map, pos);
                let anchor = if mods.shift { editing.as_ref().map_or(ci, |sel| sel.anchor) } else { ci };
                exec(app, "text.setSelection", json!({"anchor": anchor, "caret": ci}));
                typing_key(ui, true);
                drag = Some(Drag::Select { layer: e.layer.id });
                handled = true;
            } else {
                // Clicking elsewhere commits the edit.
                exec(app, "text.endEdit", json!({}));
            }
        }
        if !handled && type_tool {
            if let Some(e) = text_layer_at(app, ectx, map, pos, true) {
                let ci = e.hit(map, pos);
                exec(app, "text.edit", json!({"layer": e.layer.id.0, "caret": ci}));
                drag = Some(Drag::Select { layer: e.layer.id });
            } else {
                drag = Some(Drag::Create { start: map.to_comp(pos), vertical: tool == Tool::TypeVertical });
            }
            handled = true;
        }
        if handled {
            ui.memory_mut(|m| m.request_focus(focus_id()));
            owns = true;
        }
    }

    // ---- drag / release
    match drag.clone() {
        Some(Drag::Select { layer }) => {
            owns = true;
            if down
                && let Some(p) = pointer
                && let Some(e) = edited(app, ectx, layer)
                && press.is_some_and(|q| q.distance(p) > 2.0)
            {
                let ci = e.hit(map, p);
                if app.session.state.text_edit.as_ref().is_some_and(|s| s.caret != ci) {
                    let anchor = app.session.state.text_edit.as_ref().map_or(ci, |s| s.anchor);
                    exec(app, "text.setSelection", json!({"anchor": anchor, "caret": ci}));
                }
            }
        }
        Some(Drag::Create { start, vertical }) => {
            owns = true;
            let end = pointer.map(|p| map.to_comp(p)).unwrap_or(start);
            let a = map.to_screen(start);
            let b = map.to_screen(end);
            let is_box = a.distance(b) > 6.0;
            if is_box && down {
                painter.rect_stroke(Rect::from_two_pos(a, b), 0.0, Stroke::new(1.0, app.tokens.accent), StrokeKind::Middle);
            }
            if released {
                let mut p = json!({"text": "", "edit": true, "size": 72, "fill": [1.0, 1.0, 1.0], "vertical": vertical});
                if is_box {
                    let (x0, y0) = (start[0].min(end[0]), start[1].min(end[1]));
                    p["box"] = json!([x0, y0, (end[0] - start[0]).abs(), (end[1] - start[1]).abs()]);
                } else {
                    p["position"] = json!([start[0], start[1]]);
                    p["justify"] = json!("left");
                }
                exec(app, "layer.newText", p);
                ui.memory_mut(|m| m.request_focus(focus_id()));
            }
        }
        Some(Drag::Resize { layer, handle, start_box }) => {
            owns = true;
            if down
                && let (Some(p), Some(q)) = (pointer, press)
                && let Some(e) = edited(app, ectx, layer)
            {
                let (px, py) = e.to_layer(map, p);
                let (qx, qy) = e.to_layer(map, q);
                let (dx, dy) = (px - qx, py - qy);
                let [mut x0, mut y0, w, h] = start_box;
                let (mut x1, mut y1) = (x0 + w, y0 + h);
                if matches!(handle, 0 | 6 | 7) {
                    x0 += dx;
                }
                if matches!(handle, 2..=4) {
                    x1 += dx;
                }
                if matches!(handle, 0..=2) {
                    y0 += dy;
                }
                if matches!(handle, 4..=6) {
                    y1 += dy;
                }
                let bx = [x0.min(x1), y0.min(y1), (x1 - x0).abs().max(4.0), (y1 - y0).abs().max(4.0)];
                exec(app, "layer.setText", json!({"layer": layer.0, "box": bx, "merge": format!("text-box-{}", layer.0)}));
            }
        }
        None => {}
    }
    // Double / triple click: word / line; double-click a text layer to edit it.
    if resp.triple_clicked()
        && let Some(p) = pointer
        && let Some(e) = app.session.state.text_edit.clone().and_then(|s| edited(app, ectx, s.layer))
        && e.contains(map, p)
    {
        let (a, b) = e.lay.line_span(e.hit(map, p));
        exec(app, "text.setSelection", json!({"anchor": a, "caret": b}));
        owns = true;
    } else if resp.double_clicked()
        && let Some(p) = pointer
    {
        let cur2 = app.session.state.text_edit.clone().and_then(|s| edited(app, ectx, s.layer));
        match cur2 {
            Some(e) if e.contains(map, p) => {
                let r = effectcraft_engine::keyframe::text_doc::word_at(&e.doc.text, e.hit(map, p));
                exec(app, "text.setSelection", json!({"anchor": r.start, "caret": r.end}));
                owns = true;
            }
            _ if !matches!(tool, Tool::Hand | Tool::Zoom) && !tool.is_pen() && !type_tool => {
                if let Some(e) = text_layer_at(app, ectx, map, p, false) {
                    exec(app, "text.edit", json!({"layer": e.layer.id.0}));
                    ui.memory_mut(|m| m.request_focus(focus_id()));
                    owns = true;
                }
            }
            _ => {}
        }
    }
    if released || !down {
        ui.data_mut(|d| d.remove::<Drag>(drag_id()));
    } else if let Some(d) = drag {
        ui.data_mut(|dd| dd.insert_temp(drag_id(), d));
    }

    // ---- keyboard
    // Pointer handling above may have started or switched a text target: leave the previous
    // composition's identity as a stale guard before this pass's IME events are read.
    sync_ime_target(ui, app);
    sync_ime_selection(ui, app);
    if app.session.state.text_edit.is_some() && app.dialog.is_none() && ui.is_enabled() {
        // Register after pointer handling so new edits take focus in this frame. An inactive
        // editor must not take Tab focus and immediately surrender it before viewer controls.
        ui.interact(Rect::from_min_size(resp.rect.min, vec2(1.0, 1.0)), focus_id(), egui::Sense::focusable_noninteractive());
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(focus_id(), egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true });
        });
        // A session started elsewhere (menu, agent, Properties) takes the keyboard once…
        let seen = ime_seen_id(ui.ctx().viewport_id());
        let key = app.session.state.text_edit.as_ref().map(|e| (e.layer.0, e.session));
        // …and gets it back when a click in the viewer or a formatting panel left nothing holding
        // it; not merely because nothing has the focus (#831).
        let regain = ui.memory(|m| m.focused().is_none()) && keeps_text_edit(app.ui.focused);
        if ui.data(|d| d.get_temp::<Option<(u64, u64)>>(seen)).flatten() != key || regain {
            ui.data_mut(|d| d.insert_temp(seen, key));
            ui.memory_mut(|m| m.request_focus(focus_id()));
        }
        if ui.memory(|m| m.has_focus(focus_id())) {
            keyboard(app, ui, resp.rect);
        }
    } else {
        // Resolve the viewport before taking the data lock: `viewport_id()`
        // acquires the same context lock that `data_mut` holds.
        let viewport = ui.ctx().viewport_id();
        ui.data_mut(|d| d.insert_temp::<Option<(u64, u64)>>(ime_seen_id(viewport), None));
        invalidate_ime_composition(ui, app);
        clear_preedit(ui);
        if ui.memory(|m| m.has_focus(focus_id())) {
            ui.memory_mut(|m| m.surrender_focus(focus_id()));
        }
    }

    // ---- overlay
    if let Some(s) = app.session.state.text_edit.clone()
        && let Some(e) = edited(app, ectx, s.layer)
    {
        draw(app, ui, painter, map, &e, s.range(), s.caret);
        // `draw` records the caret geometry for this pass. The descriptor is published once at
        // the end of the whole pass, after dock/script actions have had their say.
    }
    owns
}

/// This frame's keys, in order, into the edited text, up to a press outside the `viewer`: the
/// panel it lands in decides (after this frame's panels) whether the edit ends, so the keys after
/// it in this frame are neither typed nor run as shortcuts (#831).
fn keyboard(app: &mut EffectcraftApp, ui: &mut egui::Ui, viewer: Rect) {
    let viewport = ui.ctx().viewport_id();
    let frame = ui.ctx().cumulative_frame_nr_for(viewport);
    let first = ui.data_mut(|data| {
        if data.get_temp::<u64>(input_id(viewport)) == Some(frame) {
            false
        } else {
            data.insert_temp(input_id(viewport), frame);
            true
        }
    });
    if !first {
        return;
    }
    if !window_available(ui.ctx()) {
        set_admission(ui.ctx(), false);
        unavailable_controls(ui);
        return;
    }
    set_admission(ui.ctx(), true);
    sync_ime_selection(ui, app);
    let events = ui.input(|i| i.events.clone());
    let ctx = ui.ctx().clone();
    // This clone is the exclusive ordered owner of controls. Removing only those controls from
    // the shared vector does not acknowledge them or move them ahead of preceding mutations.
    ctx.input_mut(|input| input.events.retain(|event| !matches!(event, egui::Event::ImeResetCompleted { .. })));
    let mut custom_mutations = app.session.state.text_edit.is_some();
    for ev in events {
        if let egui::Event::ImeResetCompleted { request_id } = &ev {
            if custom_mutations {
                complete_reset(app, ui, *request_id);
            }
            continue;
        }
        sync_ime_target(ui, app);
        sync_ime_selection(ui, app);
        if matches!(&ev, egui::Event::PointerButton { pos, pressed: true, .. } if !viewer.contains(*pos)) {
            // The press already proves this custom route is leaving (or deferring to the panel
            // that owns the click). Disable admission before invalidating, so retiring the old
            // guard cannot put a new, undeliverable ID into core Memory. The finalizer allocates
            // the one replacement only after it confirms this pass's actual custom output is
            // eligible. A previously issued ID may still be queued in core (which has no cancel)
            // and attach to a later stock field; the suspended local guard ignores its completion.
            set_admission(&ctx, false);
            invalidate_ime_composition(ui, app);
            clear_preedit(ui);
            suspend_reset(&ctx);
            custom_mutations = false;
            continue;
        }
        if !custom_mutations {
            continue;
        }
        let Some(sel) = app.session.state.text_edit.clone() else {
            suspend_reset(&ctx);
            custom_mutations = false;
            continue;
        };
        let owner = ImeOwner { layer: sel.layer, session: sel.session };
        let from_ime = matches!(&ev, egui::Event::Ime(_));
        match ev {
            egui::Event::Text(t) if !t.chars().any(char::is_control) => {
                clear_current_preedit(ui, owner);
                let k = typing_key(ui, false);
                let before = ime_stamp(app, &ctx);
                if exec(app, "text.insert", json!({"text": t, "merge": k})).is_some() {
                    note_ime_application_edit(app, &ctx, before);
                }
            }
            egui::Event::Ime(egui::ImeEvent::Commit(t)) => {
                // Old terminal payloads cannot acknowledge the request or admit a later suffix.
                if ime_event_is_stale(ui, owner) {
                    continue;
                }
                // An accepted commit is the terminal event of its composition (or a direct commit
                // that never had a pre-edit).
                set_ime_composition(ui, None);
                // An empty commit (the tail of a deletion-only batch) changes no text, so it earns
                // no receipt; it still ends the composed preview.
                if !t.is_empty() {
                    let k = typing_key(ui, false);
                    // The receipt covers only this accepted insertion: AutoCorrect and every
                    // other application transformation stays outside it.
                    let before = ime_stamp(app, &ctx);
                    if exec(app, "text.insert", json!({"text": t, "merge": k})).is_some() {
                        let after = ime_stamp(app, &ctx);
                        note_ime_receipt(app, &ctx, before, after);
                    }
                }
                clear_preedit(ui);
            }
            egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                if text.is_empty() {
                    // An empty pre-edit only clears the visible composition. Keep its owner as
                    // terminal-pending: the commit/delete for this composition is a separate
                    // event and a target switch must reject it. Only the matching fence can
                    // lift an already interrupted guard.
                    if !ime_event_is_stale(ui, owner) {
                        set_ime_composition(ui, Some(ImeComposition::TerminalPending(owner)));
                    }
                    clear_preedit(ui);
                } else if ime_event_is_stale(ui, owner) {
                    // This pre-edit belongs to the composition whose target was replaced; an
                    // input method can emit further non-empty pre-edits before its final commit.
                    // Do not revive the old composition on the replacement target, and keep the
                    // tombstone past this batch: the old composition may commit in a later frame.
                    set_ime_composition(ui, Some(ImeComposition::Stale));
                } else {
                    set_ime_composition(ui, Some(ImeComposition::Active(owner)));
                    ui.data_mut(|d| d.insert_temp(ime_preedit_id(viewport), Preedit { owner, text: bounded_preedit(text) }));
                }
            }
            // An input method replacing text around the selection (W26/W27 `Done` batches on
            // Wayland/Windows, E21): delete exactly the reported characters/bytes. The commit
            // that follows shares this typing key, so replacement stays one undo step.
            egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { before_chars, after_chars }) => {
                if ime_event_is_stale(ui, owner) {
                    // A delete-surrounding is not terminal by itself: its paired commit/text
                    // follows in the same composition. Its delete and commit stay rejected
                    // until the request-bound completion.
                    continue;
                }
                let before = ime_stamp(app, &ctx);
                if ime_delete(app, ui, before_chars, after_chars, false) {
                    let after = ime_stamp(app, &ctx);
                    note_ime_receipt(app, &ctx, before, after);
                }
            }
            egui::Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes, after_bytes }) => {
                if ime_event_is_stale(ui, owner) {
                    // Its paired commit/text still belongs to the replaced composition; keep the
                    // guard until the actual matching completion.
                    continue;
                }
                let before = ime_stamp(app, &ctx);
                if ime_delete(app, ui, before_bytes, after_bytes, true) {
                    let after = ime_stamp(app, &ctx);
                    note_ime_receipt(app, &ctx, before, after);
                }
            }
            egui::Event::Copy => {
                if let Some(v) = exec(app, "edit.copy", json!({}))
                    && let Some(t) = v["text"].as_str()
                {
                    ctx.copy_text(t.to_string());
                }
            }
            egui::Event::Cut => {
                let before = ime_stamp(app, &ctx);
                if let Some(v) = exec(app, "edit.cut", json!({}))
                    && let Some(t) = v["text"].as_str()
                {
                    ctx.copy_text(t.to_string());
                    note_ime_application_edit(app, &ctx, before);
                }
                typing_key(ui, true);
            }
            egui::Event::Paste(t) => {
                let before = ime_stamp(app, &ctx);
                if exec(app, "edit.paste", json!({"text": t})).is_some() {
                    note_ime_application_edit(app, &ctx, before);
                }
                typing_key(ui, true);
            }
            egui::Event::Key { key, pressed: true, modifiers: m, .. } => {
                use egui::Key;
                let cmd = m.command;
                let mv = |app: &mut EffectcraftApp, to: &str| {
                    exec(app, "text.moveCaret", json!({"to": to, "extend": m.shift}));
                };
                match key {
                    Key::ArrowLeft | Key::ArrowRight if m.alt => {
                        // Kerning at the caret, tracking over a selection.
                        let step = if cmd { 100.0 } else { 20.0 } * if key == Key::ArrowLeft { -1.0 } else { 1.0 };
                        let doc = layer_doc(&app.session, sel.layer).unwrap_or_default();
                        if sel.is_empty() {
                            if sel.caret < doc.char_len() {
                                let cur = match doc.style_at(sel.caret).kerning {
                                    Kerning::Manual(v) => v,
                                    _ => 0.0,
                                };
                                exec(
                                    app,
                                    "layer.setText",
                                    json!({"layer": sel.layer.0, "range": [sel.caret, sel.caret + 1], "kerning": cur + step, "merge": format!("text-kern-{}", sel.caret)}),
                                );
                            }
                        } else {
                            let r = sel.range();
                            let cur = doc.style_at(r.start).tracking;
                            exec(app, "layer.setText", json!({"layer": sel.layer.0, "range": [r.start, r.end], "tracking": cur + step, "merge": "text-track"}));
                        }
                    }
                    Key::ArrowLeft => mv(app, if cmd { "wordLeft" } else { "left" }),
                    Key::ArrowRight => mv(app, if cmd { "wordRight" } else { "right" }),
                    Key::ArrowUp => mv(app, if cmd { "paraUp" } else { "up" }),
                    Key::ArrowDown => mv(app, if cmd { "paraDown" } else { "down" }),
                    Key::Home => mv(app, if cmd { "start" } else { "lineStart" }),
                    Key::End => mv(app, if cmd { "end" } else { "lineEnd" }),
                    Key::Backspace => {
                        let k = typing_key(ui, false);
                        let before = ime_stamp(app, &ctx);
                        if exec(app, "text.delete", json!({"word": cmd || m.alt, "merge": k})).is_some() {
                            note_ime_application_edit(app, &ctx, before);
                        }
                    }
                    Key::Delete => {
                        let k = typing_key(ui, false);
                        let before = ime_stamp(app, &ctx);
                        if exec(app, "text.delete", json!({"direction": "forward", "word": cmd || m.alt, "merge": k})).is_some() {
                            note_ime_application_edit(app, &ctx, before);
                        }
                    }
                    Key::Enter if cmd => {
                        exec(app, "text.endEdit", json!({}));
                    }
                    Key::Enter => {
                        let k = typing_key(ui, false);
                        let before = ime_stamp(app, &ctx);
                        if exec(app, "text.insert", json!({"text": "\n", "merge": k})).is_some() {
                            note_ime_application_edit(app, &ctx, before);
                        }
                    }
                    Key::Tab => {
                        let k = typing_key(ui, false);
                        let before = ime_stamp(app, &ctx);
                        if exec(app, "text.insert", json!({"text": "\t", "merge": k})).is_some() {
                            note_ime_application_edit(app, &ctx, before);
                        }
                    }
                    Key::Escape => {
                        exec(app, "text.endEdit", json!({}));
                    }
                    // The editor owns exactly Cmd/Ctrl+A (Select All) and Cmd/Ctrl+Z with or
                    // without Shift (Undo, Redo). Other modifiers on those keys aren't editing
                    // chords, so they fall through to the binding match below: Shift+A is
                    // Deselect All's default, and Alt variants are free for a rebound one.
                    Key::A if cmd && !m.shift && !m.alt => {
                        exec(app, "text.setSelection", json!({"select": "all"}));
                    }
                    Key::Z if cmd && !m.alt => {
                        let before = ime_stamp(app, &ctx);
                        if crate::menus::invoke(app, &ctx, if m.shift { "edit.redo" } else { "edit.undo" }, json!({})).is_ok() {
                            note_ime_application_edit(app, &ctx, before);
                        }
                    }
                    Key::Comma | Key::Period if cmd && m.shift => {
                        let doc = layer_doc(&app.session, sel.layer).unwrap_or_default();
                        let r = sel.range();
                        let size = sel.pending.as_ref().map_or_else(|| doc.style_at(r.start).size, |p| p.size);
                        let ns = (size + if key == Key::Period { 2.0 } else { -2.0 }).max(1.0);
                        exec(app, "layer.setText", json!({"layer": sel.layer.0, "range": [r.start, r.end], "size": ns}));
                    }
                    // The app-wide shortcut dispatcher is skipped while this editor holds the
                    // keyboard, so the Deselect All chord (Cmd/Ctrl+Shift+A or F2 by default)
                    // must be matched here and run through the command dispatcher like undo
                    // above. Matching the command's current binding, not a fixed key, keeps
                    // rebound shortcuts working and gives every route one final state; the
                    // forwarding test also refuses chords a focused editor must never surrender
                    // (see `forwardable_during_text_edit`).
                    _ if crate::menus::chord_matches(&app.session, "edit.deselectAll", key, m) && crate::menus::forwardable_during_text_edit(m, key) => {
                        let _ = crate::menus::invoke(app, &ctx, "edit.deselectAll", json!({}));
                    }
                    _ => {}
                }
                if !matches!(key, Key::Backspace | Key::Delete | Key::Enter | Key::Tab) {
                    typing_key(ui, true);
                }
            }
            _ => {}
        }
        let route_ended = app.session.state.text_edit.is_none() || app.dialog.is_some();
        if route_ended {
            set_admission(&ctx, false);
        }
        if from_ime {
            remember_ime_selection(ui, app);
        } else {
            // Arrow/Home/End/Select All (or another application event) may have moved the
            // selection before a queued IME delete/commit later in this same raw batch.
            sync_ime_selection(ui, app);
        }
        if route_ended {
            suspend_reset(&ctx);
            custom_mutations = false;
        }
    }
}

/// Delete the text an IME marks for replacement around the editor's selection.
///
/// `before`/`after` count UTF-8 bytes (`DeleteSurroundingBytes`, what winit reports) or
/// characters (`DeleteSurrounding`). The engine command keeps the selected text — the following
/// `Commit` replaces it — and leaves a character a byte count only partly covers; the deletion
/// and the commit share this typing key, so a replacement stays one undo step.
fn ime_delete(app: &mut EffectcraftApp, ui: &egui::Ui, before: usize, after: usize, bytes: bool) -> bool {
    if (before == 0 && after == 0) || app.session.state.text_edit.is_none() {
        return false;
    }
    let k = typing_key(ui, false);
    exec(app, "text.deleteSurrounding", json!({"before": before, "after": after, "bytes": bytes, "merge": k})).is_some()
}

/// Paint the paragraph box and handles, selection, caret or pre-edit for `e`; register the editor's and box handles'
/// automation regions; request the caret-blink repaint; and record the pass's text and caret rectangles for the IME
/// finalizer.
fn draw(app: &mut EffectcraftApp, ui: &mut egui::Ui, painter: &egui::Painter, map: &ViewerMap, e: &Edited, sel: std::ops::Range<usize>, caret: usize) {
    let t = app.tokens;
    let to_s = |p: Point| e.to_screen(map, (p.x, p.y));
    // Paragraph box and its handles.
    if e.doc.box_size.is_some() {
        let hs: Vec<Pos2> = e.handles().iter().map(|h| e.to_screen(map, *h)).collect();
        let corners = [hs[0], hs[2], hs[4], hs[6], hs[0]];
        for w in corners.windows(2) {
            painter.line_segment([w[0], w[1]], Stroke::new(1.0, Color32::from_white_alpha(170)));
        }
        for (i, h) in hs.iter().enumerate() {
            let r = Rect::from_center_size(*h, vec2(7.0, 7.0));
            painter.rect_filled(r, 0.0, Color32::WHITE);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Middle);
            app.auto.add(&format!("viewer.textBox.handle.{i}"), r, "Text box handle");
        }
    }
    // Selection highlight.
    if !sel.is_empty() {
        for q in e.lay.selection_quads(sel.start, sel.end) {
            let pts: Vec<Pos2> = q.iter().map(|p| to_s(*p)).collect();
            painter.add(egui::Shape::convex_polygon(pts, t.accent.gamma_multiply(0.38), Stroke::NONE));
        }
    }
    // Caret (blinking) and IME composition.
    // The caret follows the animated layout and text on a path.
    let (a, b) = e.lay.caret(caret);
    let cm = e.carets.get(caret).copied().unwrap_or(Mat3::IDENTITY);
    let anim = |p: Point| {
        let q = cm.apply(gv2(p.x, p.y));
        Point::new(q.x, q.y)
    };
    let (a, b) = (anim(a), anim(b));
    let (sa, sb) = (to_s(a), to_s(b));
    let now = ui.input(|i| i.time);
    let preedit = visible_preedit(ui, app);
    if let Some(pe) = preedit {
        let h = (sb - sa).length().max(10.0);
        let g = painter.layout_no_wrap(pe, egui::FontId::proportional(h * 0.7), Color32::WHITE);
        let r = Rect::from_min_size(sa, g.size());
        painter.rect_filled(r.expand(2.0), 2.0, Color32::from_black_alpha(160));
        painter.galley(sa, g, Color32::WHITE);
        painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, Color32::WHITE));
    } else if (now * 2.0) as i64 % 2 == 0 || !sel.is_empty() {
        painter.line_segment([sa, sb], Stroke::new(3.0, Color32::from_black_alpha(160)));
        painter.line_segment([sa, sb], Stroke::new(1.5, Color32::WHITE));
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    // The text region for automation, and the IME candidate window position.
    let r = e.region();
    let quad = [(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])].map(|p| e.to_screen(map, p));
    let rect = Rect::from_points(&quad);
    app.auto.add("viewer.textEdit", rect, &format!("Editing {}", e.layer.name));
    let cursor_rect = Rect::from_two_pos(sa, sb).expand2(vec2(1.0, 0.0));
    // The viewport IME publisher uses this actual
    // caret geometry and the edited layer's committed `TextDoc` (never the bidi or pre-edit
    // display text). An available-empty document stays distinct from unavailable.
    let viewport = ui.ctx().viewport_id();
    let pass = ui.ctx().cumulative_pass_nr_for(viewport);
    ui.data_mut(|d| d.insert_temp(ime_geometry_id(viewport), (pass, rect, cursor_rect)));
}

#[cfg(test)]
mod tests;
