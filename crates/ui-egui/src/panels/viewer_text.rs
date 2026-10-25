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

fn drag_id() -> egui::Id {
    egui::Id::unique("viewer-text-drag")
}
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

fn reset_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("effectcraft.ime.reset", viewport))
}
fn mode_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("effectcraft.ime.ui-pass", viewport))
}
fn committed_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("effectcraft.ime.committed", viewport))
}
fn input_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("effectcraft.ime.input-frame", viewport))
}
fn admission_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("effectcraft.ime.can-request", viewport))
}

fn set_admission(ctx: &egui::Context, allowed: bool) {
    let id = admission_id(ctx.viewport_id());
    ctx.data_mut(|data| data.insert_temp(id, allowed));
}

fn window_available(ctx: &egui::Context) -> bool {
    ctx.input(|input| {
        input.focused && input.viewport().focused.unwrap_or(true) && input.viewport().minimized != Some(true) && input.viewport().occluded != Some(true)
    })
}

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

fn read_reset(ctx: &egui::Context) -> Option<ResetGuard> {
    let id = reset_id(ctx.viewport_id());
    ctx.data(|data| data.get_temp(id))
}

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

fn request_reset(app: &EffectcraftApp, ctx: &egui::Context) {
    let output = read_reset(ctx).and_then(|guard| guard.output);
    request_reset_with_output(app, ctx, output);
}

fn sync_reset(app: &EffectcraftApp, ctx: &egui::Context) {
    if let Some(guard) = read_reset(ctx)
        && !guard.suspended
        && (guard.destination != ime_selection(app) || guard.committed != ime_stamp(app, ctx))
    {
        request_reset(app, ctx);
    }
}

fn suspend_reset(ctx: &egui::Context) {
    if let Some(mut guard) = read_reset(ctx) {
        guard.request = None;
        guard.suspended = true;
        let id = reset_id(ctx.viewport_id());
        ctx.data_mut(|data| data.insert_temp(id, guard));
    }
}

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

fn unavailable_controls(ui: &egui::Ui) {
    suspend_reset(ui.ctx());
    // No custom mutation route runs here, so controls can only be obsolete. Stock text/keys
    // remain in place for their actual TextEdit owner.
    ui.ctx().input_mut(|input| input.events.retain(|event| !matches!(event, egui::Event::ImeResetCompleted { .. })));
}

pub(crate) fn begin_ui(ctx: &egui::Context) {
    set_admission(ctx, false);
    let id = ime_geometry_id(ctx.viewport_id());
    ctx.data_mut(|data| data.remove::<ImeGeometry>(id));
}

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

fn ime_preedit_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-preedit", viewport))
}
fn ime_composition_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-composition", viewport))
}
fn ime_geometry_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-geometry", viewport))
}
fn ime_seen_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-seen", viewport))
}
fn ime_target_seen_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-target", viewport))
}
fn ime_selection_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-selection", viewport))
}
fn ime_last_context_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-last-context", viewport))
}
fn ime_surrounding_cache_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::unique(("viewer-text-ime-surrounding", viewport))
}
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

fn clear_preedit(ui: &egui::Ui) {
    let viewport = ui.ctx().viewport_id();
    ui.data_mut(|d| d.remove::<Preedit>(ime_preedit_id(viewport)));
}

fn read_ime_composition(ui: &egui::Ui) -> Option<ImeComposition> {
    let viewport = ui.ctx().viewport_id();
    ui.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(viewport)))
}

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

fn visible_preedit(ui: &egui::Ui, app: &EffectcraftApp) -> Option<String> {
    let owner = ime_owner(app)?;
    let p = read_preedit(ui)?;
    (p.owner == owner && !p.text.is_empty()).then_some(p.text)
}

/// Publish the settled descriptor for one completed viewport pass.
///
/// This runs at the very end of a root pass (after dock actions, ScriptUI, learn and dialogs)
/// and at the end of each panel-window pass after that window's actions. The overlay only
/// records geometry; it never publishes early with a snapshot that a later action can replace.
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
    // The outgoing IME descriptor is published by the pass-end finalizer, with this actual
    // caret geometry and the edited layer's committed `TextDoc` (never the bidi or pre-edit
    // display text). An available-empty document stays distinct from unavailable.
    let viewport = ui.ctx().viewport_id();
    let pass = ui.ctx().cumulative_pass_nr_for(viewport);
    ui.data_mut(|d| d.insert_temp(ime_geometry_id(viewport), (pass, rect, cursor_rect)));
}

#[cfg(test)]
mod tests {
    //! Incoming IME edits on the custom text editor (the Type tool's own session, not a stock
    //! `TextEdit`). Winit reports the bytes around the selection *before* the commit it belongs
    //! to, so the deletion and the insert must share one undo step.

    use super::*;
    use effectcraft_engine::Session;
    use effectcraft_engine::commands::text_edit::TextEdit;
    use serde_json::json;

    fn editor(text: &str, anchor: usize, caret: usize) -> (EffectcraftApp, LayerId) {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "IME", "width": 160, "height": 90, "duration": 1})).unwrap();
        let layer = s.execute("layer.newText", json!({"text": text})).unwrap()["layer"].as_u64().unwrap();
        let lid = LayerId(layer);
        s.state.text_edit = Some(TextEdit { layer: lid, anchor, caret, ..Default::default() });
        (EffectcraftApp::new(s), lid)
    }

    fn doc_text(app: &EffectcraftApp, lid: LayerId) -> String {
        layer_doc(&app.session, lid).map(|d| d.text).unwrap_or_default()
    }

    fn feed(app: &mut EffectcraftApp, events: Vec<egui::Event>) {
        let ctx = egui::Context::default();
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 180.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| keyboard(app, ui, egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 180.0))));
        out.textures_delta.clear();
    }

    #[test]
    fn preedit_preview_is_bounded_on_a_character_boundary() {
        let text = "é".repeat(MAX_IME_TEXT_BYTES);
        let capped = bounded_preedit(text);
        assert!(capped.len() <= MAX_IME_TEXT_BYTES);
        assert!(capped.is_char_boundary(capped.len()));
        assert_eq!(capped.len() % "é".len(), 0, "the cap never leaves half a character");
    }

    /// Ending the edit from another panel clears this viewport's pre-edit. The production path
    /// used to resolve `viewport_id()` (a context read lock) inside `data_mut` (a write lock);
    /// that nested context access deadlocked every pass that drew the viewer.
    #[test]
    fn ending_an_edit_from_another_panel_clears_the_preedit() {
        let (mut app, lid) = editor("abc", 0, 0);
        let session = app.session.state.text_edit.as_ref().map_or(1, |e| e.session);
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let viewport = ui.ctx().viewport_id();
            let preedit = Preedit { owner: ImeOwner { layer: lid, session }, text: "x".into() };
            ui.data_mut(|d| d.insert_temp(ime_preedit_id(viewport), preedit));
            panel_clicked(&mut app, ui.ctx(), PanelKind::Timeline);
            let viewport = ui.ctx().viewport_id();
            assert!(ui.data(|d| d.get_temp::<Preedit>(ime_preedit_id(viewport))).is_none(), "the ended edit leaves no preview behind");
        });
        out.textures_delta.clear();
        assert!(app.session.state.text_edit.is_none(), "a click in the Timeline ends the text edit");
    }

    #[test]
    fn ime_delete_surrounding_bytes_and_the_commit_are_one_undo_step() {
        // "café😀": the last character is four bytes, so a byte-count deletion that treated bytes
        // as characters would corrupt the document.
        let (mut app, lid) = editor("café😀", 5, 5);
        feed(
            &mut app,
            vec![
                egui::Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes: 4, after_bytes: 0 }),
                egui::Event::Ime(egui::ImeEvent::Commit("X".into())),
            ],
        );
        assert_eq!(doc_text(&app, lid), "caféX", "the emoji is replaced by the committed text");
        app.session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(doc_text(&app, lid), "café😀", "the replacement is one undo step");
    }

    #[test]
    fn ime_delete_surrounding_chars_keeps_the_selection_for_the_commit() {
        // Stock egui deletes the characters around the selection, not the selection itself: the
        // commit that follows replaces "bcd", it does not insert next to a deleted document.
        let (mut app, lid) = editor("abcdef", 1, 4);
        feed(&mut app, vec![egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { before_chars: 1, after_chars: 99 })]);
        assert_eq!(doc_text(&app, lid), "bcd", "one character before and everything after go");
        let sel = app.session.state.text_edit.clone().expect("editing continues");
        assert_eq!(sel.range(), 0..3, "the selected text is still selected");
        // A deletion-only batch (no commit) leaves "bcd", not an empty document.
        app.session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(doc_text(&app, lid), "abcdef", "clamped deletion is still one undo step");

        // Delete surround + commit: the commit replaces the kept selection.
        let (mut app, lid) = editor("abcdef", 1, 4);
        feed(
            &mut app,
            vec![
                egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { before_chars: 1, after_chars: 99 }),
                egui::Event::Ime(egui::ImeEvent::Commit("XY".into())),
            ],
        );
        assert_eq!(doc_text(&app, lid), "XY", "the commit replaces the selection");
        app.session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(doc_text(&app, lid), "abcdef", "the deletion and its commit are one undo step");
    }

    /// One UI pass with the editor focused, events applied, and the overlay's observation.
    struct ImePass {
        ctx: egui::Context,
        viewport: egui::ViewportId,
        rect: egui::Rect,
    }

    impl ImePass {
        fn new(viewport: egui::ViewportId) -> Self {
            let ctx = egui::Context::default();
            install_hooks(&ctx);
            Self { ctx, viewport, rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 180.0)) }
        }

        /// Run one `Context` frame. `overlay` is false for a pass in which the viewer (and so
        /// the edit overlay) never drew; `discard` forces a second pass of the same frame.
        fn frame(&mut self, app: &mut EffectcraftApp, events: Vec<egui::Event>, overlay: bool, discard: bool) -> Option<egui::output::IMEOutput> {
            let ctx = self.ctx.clone();
            let rect = self.rect;
            let viewport = self.viewport;
            let mut viewports = egui::ViewportIdMap::default();
            viewports.insert(self.viewport, egui::ViewportInfo::default());
            let raw = egui::RawInput { viewport_id: self.viewport, viewports, screen_rect: Some(rect), events, ..Default::default() };
            let mut asked = false;
            let mut out = ctx.run_ui(raw, |ui| {
                begin_ui(ui.ctx());
                ui.interact(rect, focus_id(), egui::Sense::focusable_noninteractive());
                ui.memory_mut(|m| m.request_focus(focus_id()));
                sync_ime_target(ui, app);
                keyboard(app, ui, rect);
                if overlay {
                    let pass = ui.ctx().cumulative_pass_nr_for(viewport);
                    ui.data_mut(|d| d.insert_temp(ime_geometry_id(viewport), (pass, rect, rect)));
                }
                finish_ime(app, ui);
                if discard && !asked {
                    asked = true;
                    ui.ctx().request_discard("ime pass test");
                }
            });
            out.textures_delta.clear();
            out.platform_output.ime
        }
    }

    fn ime_text(ime: &egui::output::IMEOutput) -> (&str, usize, usize) {
        let p = ime.surrounding_text.available().expect("committed surrounding text");
        (p.text.as_str(), p.cursor, p.anchor)
    }

    #[test]
    fn discarded_custom_request_then_home_rearms_the_identical_caret() {
        let (mut app, lid) = editor("abcd", 4, 4);
        app.ui.start_screen = false;
        app.ui.tool = Tool::Type;
        app.ui.focused = PanelKind::Composition;
        let ctx = egui::Context::default();
        install_hooks(&ctx);
        let mut frame = eframe::Frame::_new_kittest();
        let raw = || egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), ..Default::default() };
        for _ in 0..3 {
            let mut output = ctx.run_ui(raw(), |ui| {
                eframe::App::logic(&mut app, ui.ctx(), &mut frame);
                app.draw(ui, &frame);
            });
            output.textures_delta.clear();
        }
        let mut input = raw();
        input.events.push(egui::Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None }));
        let mut output = ctx.run_ui(input, |ui| {
            eframe::App::logic(&mut app, ui.ctx(), &mut frame);
            app.draw(ui, &frame);
        });
        output.textures_delta.clear();
        assert!(output.platform_output.ime.is_some());
        app.session.execute("text.setSelection", json!({"caret": 1})).unwrap();
        let selection = app.session.state.text_edit.clone();
        let mut passes = 0;
        let mut discarded = None;
        let mut output = ctx.run_ui(raw(), |ui| {
            passes += 1;
            if passes > 1 {
                app.ui.start_screen = true;
            }
            eframe::App::logic(&mut app, ui.ctx(), &mut frame);
            app.draw(ui, &frame);
            if passes == 1 {
                assert!(ui.ctx().output(|output| output.ime.is_some()));
                discarded = read_reset(ui.ctx()).and_then(|guard| guard.request);
                ui.ctx().request_discard("custom IME descriptor discarded before Home");
            }
        });
        output.textures_delta.clear();
        assert!(passes >= 2);
        let first = discarded.expect("the actual discarded custom pass requested a fence");
        assert!(output.platform_output.ime.is_none());
        assert!(read_reset(&ctx).unwrap().suspended);
        assert!(read_reset(&ctx).unwrap().request.is_none());
        assert_eq!(app.session.state.text_edit, selection, "Home did not move the model caret");
        app.ui.start_screen = false;
        let mut output = ctx.run_ui(raw(), |ui| {
            eframe::App::logic(&mut app, ui.ctx(), &mut frame);
            app.draw(ui, &frame);
        });
        output.textures_delta.clear();
        let second = output.platform_output.ime.unwrap().reset_request.expect("same caret reactivation publishes a new real ID");
        assert_ne!(first, second);
        let mut output = ctx.run_ui(raw(), |ui| {
            eframe::App::logic(&mut app, ui.ctx(), &mut frame);
            app.draw(ui, &frame);
        });
        output.textures_delta.clear();
        assert!(output.platform_output.ime.unwrap().reset_request.is_none());
        let mut input = raw();
        input.events.extend([
            egui::Event::ImeResetCompleted { request_id: first },
            egui::Event::Ime(egui::ImeEvent::Commit("X".into())),
            egui::Event::ImeResetCompleted { request_id: second },
            egui::Event::Ime(egui::ImeEvent::Preedit { text: "が".into(), active_range_chars: None }),
            egui::Event::Ime(egui::ImeEvent::Commit("が".into())),
        ]);
        let mut output = ctx.run_ui(input, |ui| {
            eframe::App::logic(&mut app, ui.ctx(), &mut frame);
            app.draw(ui, &frame);
        });
        output.textures_delta.clear();
        assert_eq!(doc_text(&app, lid), "aがbcd");
    }

    #[test]
    fn viewport_fences_and_duplicate_completion_keep_their_exact_lifetime() {
        let (mut app, lid) = editor("abcd", 4, 4);
        let mut root = ImePass::new(egui::ViewportId::ROOT);
        let mut detached = ImePass::new(egui::ViewportId::from_hash_of("effect-ime-detached"));
        let preview = || egui::Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None });
        root.frame(&mut app, vec![preview()], true, false);
        detached.frame(&mut app, vec![preview()], true, false);
        app.session.execute("text.setSelection", json!({"caret": 1})).unwrap();
        let first = root.frame(&mut app, vec![], true, false).unwrap().reset_request.unwrap();
        let second = detached.frame(&mut app, vec![], true, false).unwrap().reset_request.unwrap();
        assert_ne!(first, second);
        detached.frame(&mut app, vec![egui::Event::ImeResetCompleted { request_id: first }, egui::Event::Ime(egui::ImeEvent::Commit("X".into()))], true, false);
        assert_eq!(doc_text(&app, lid), "abcd");
        let pending = detached.ctx.data(|data| data.get_temp::<ResetGuard>(reset_id(detached.viewport))).unwrap().request;
        assert_eq!(pending, Some(second), "a root marker cannot release the detached viewport's full ID");
        let output = detached
            .frame(
                &mut app,
                vec![
                    egui::Event::ImeResetCompleted { request_id: second },
                    preview(),
                    egui::Event::ImeResetCompleted { request_id: second },
                    egui::Event::Ime(egui::ImeEvent::Commit("が".into())),
                ],
                true,
                false,
            )
            .unwrap();
        assert_eq!(doc_text(&app, lid), "aがbcd");
        assert!(output.reset_request.is_none(), "a duplicate control cannot reconstruct the cleared guard");
        // An actual context-generation change at unchanged selection still supersedes a wait.
        root.frame(&mut app, vec![], true, false);
        let replaced = root.frame(&mut app, vec![], false, false);
        assert!(replaced.is_none());
        let replacement = root.frame(&mut app, vec![], true, false).unwrap().reset_request.unwrap();
        assert_ne!(first, replacement);
    }

    #[test]
    fn ime_descriptor_sends_committed_text_and_preserves_a_reversed_selection() {
        let (mut app, _) = editor("café😀", 2, 5);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        let first = pass.frame(&mut app, vec![], true, false).expect("the focused editor owns IME");
        assert_eq!(first.owner, ime_target(&app).expect("an edit target").owner);
        assert!(first.context_generation > 0);
        assert_eq!(ime_text(&first), ("café😀", 9, 2), "UTF-8 bytes, with the caret's direction kept");
        assert_eq!(first.change_cause, egui::ImeChangeCause::Other, "starting a context is application origin");
        assert_eq!((first.rect, first.cursor_rect), (pass.rect, pass.rect), "the real caret geometry");

        let again = pass.frame(&mut app, vec![], true, false).expect("still editing");
        assert_eq!(again.context_generation, first.context_generation, "an unchanged pass keeps the context");
        assert_eq!(again.surrounding_text, first.surrounding_text);
    }

    #[test]
    fn accepted_ime_commit_is_input_method_and_ordinary_typing_is_other() {
        let (mut app, lid) = editor("ab", 2, 2);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        let first = pass.frame(&mut app, vec![], true, false).expect("editing");
        let generation = first.context_generation;

        let commit = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("日".into()))], true, false).expect("still editing");
        assert_eq!(doc_text(&app, lid), "ab日");
        assert_eq!(ime_text(&commit), ("ab日", 5, 5));
        assert_eq!(commit.change_cause, egui::ImeChangeCause::InputMethod, "only the accepted commit earns it");
        assert_eq!(commit.context_generation, generation, "an ordinary text change is not a new context");

        let typed = pass.frame(&mut app, vec![egui::Event::Text("x".into())], true, false).expect("still editing");
        assert_eq!(doc_text(&app, lid), "ab日x");
        assert_eq!(typed.change_cause, egui::ImeChangeCause::Other, "typing is application work");
        assert_eq!(typed.context_generation, generation);
    }

    #[test]
    fn ime_delete_and_commit_chain_is_input_method_with_a_reversed_selection() {
        let (mut app, lid) = editor("abcdef", 4, 1);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        pass.frame(&mut app, vec![], true, false).expect("editing");

        let deleted = pass
            .frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { before_chars: 1, after_chars: 1 })], true, false)
            .expect("still editing");
        assert_eq!(doc_text(&app, lid), "bcdf", "one character goes before and one after the selection");
        assert_eq!(ime_text(&deleted), ("bcdf", 0, 3), "the selected text and its reversed direction stay");
        assert_eq!(deleted.change_cause, egui::ImeChangeCause::InputMethod);

        let committed = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("X".into()))], true, false).expect("still editing");
        assert_eq!(doc_text(&app, lid), "Xf", "the commit replaces the kept selection");
        assert_eq!(committed.change_cause, egui::ImeChangeCause::InputMethod, "the delete and commit are one accepted chain");
        app.session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(doc_text(&app, lid), "abcdef", "the replacement is one undo step");
    }

    #[test]
    fn an_application_edit_in_the_same_frame_dominates_the_ime_cause() {
        // Application edit before the IME commit.
        let (mut app, lid) = editor("ab", 2, 2);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        pass.frame(&mut app, vec![], true, false).expect("editing");
        let mixed = pass
            .frame(&mut app, vec![egui::Event::Text("x".into()), egui::Event::Ime(egui::ImeEvent::Commit("日".into()))], true, false)
            .expect("still editing");
        assert_eq!(doc_text(&app, lid), "abx日");
        assert_eq!(mixed.change_cause, egui::ImeChangeCause::Other, "the earlier application edit dominates");

        // Application edit after the IME commit.
        let (mut app, lid) = editor("ab", 2, 2);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        pass.frame(&mut app, vec![], true, false).expect("editing");
        let mixed = pass
            .frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("日".into())), egui::Event::Text("x".into())], true, false)
            .expect("still editing");
        assert_eq!(doc_text(&app, lid), "ab日x");
        assert_eq!(mixed.change_cause, egui::ImeChangeCause::Other, "the later application edit dominates");
    }

    #[test]
    fn preedit_is_excluded_from_the_committed_snapshot_and_keeps_the_context() {
        let (mut app, lid) = editor("ab", 2, 2);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        let first = pass.frame(&mut app, vec![], true, false).expect("editing");

        let preview = pass
            .frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false)
            .expect("still editing");
        assert_eq!(doc_text(&app, lid), "ab", "the pre-edit never enters the document");
        assert_eq!(ime_text(&preview), ("ab", 2, 2), "only committed text is sent");
        assert_eq!(preview.context_generation, first.context_generation, "pre-edit updates keep the composition");

        let commit = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("に".into()))], true, false).expect("still editing");
        assert_eq!(doc_text(&app, lid), "abに");
        assert_eq!(commit.change_cause, egui::ImeChangeCause::InputMethod, "clearing the pre-edit and committing is IME work");
        assert_eq!(commit.context_generation, first.context_generation);
    }

    #[test]
    fn empty_and_unavailable_surrounding_text_are_distinct() {
        let (mut app, _) = editor("", 0, 0);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        let empty = pass.frame(&mut app, vec![], true, false).expect("editing");
        assert_eq!(ime_text(&empty), ("", 0, 0), "an available-empty document is not unavailable");

        let text = "a".repeat(5000);
        let (mut app, _) = editor(&text, 0, 5000);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        let wide = pass.frame(&mut app, vec![], true, false).expect("editing");
        assert!(matches!(wide.surrounding_text, egui::output::IMESurroundingText::Unavailable), "a selection wider than the limit is unavailable");
        assert_eq!(wide.owner, ime_target(&app).expect("an edit target").owner, "identity survives unavailable text");
    }

    #[test]
    fn an_unrelated_undo_keeps_a_wide_selection_composition() {
        // A selection wider than the IME limit reports unavailable surrounding text, whose
        // receipt identity is a revision. That revision must follow the editor, not the whole
        // project: undoing an unrelated opacity change used to look like an editor mutation and
        // cancel an active composition.
        let text = "a".repeat(5000);
        let (mut app, lid) = editor(&text, 0, 5000);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        let first = pass.frame(&mut app, vec![], true, false).expect("editing");
        assert!(matches!(first.surrounding_text, egui::output::IMESurroundingText::Unavailable), "the wide selection is unavailable");

        let composing = pass
            .frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false)
            .expect("still editing");
        assert_eq!(composing.context_generation, first.context_generation, "the pre-edit keeps the context");
        let owner = ime_owner(&app).expect("an editor owner");
        assert!(matches!(
            pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))),
            Some(ImeComposition::Active(active)) if active == owner
        ));

        app.session.execute("prop.set", json!({"layer": lid.0, "path": "transform/opacity", "value": 50})).unwrap();
        app.session.execute("edit.undo", json!({})).unwrap();
        let after = pass.frame(&mut app, vec![], true, false).expect("still editing");
        assert_eq!(after.context_generation, first.context_generation, "an unrelated undo does not start a new context");
        assert!(
            matches!(
                pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))),
                Some(ImeComposition::Active(active)) if active == owner
            ),
            "the wide-selection composition survives an unrelated undo"
        );
        assert!(read_reset(&pass.ctx).is_none(), "no IME reset was requested");

        // The composition is still the live one: its commit is accepted and earns the
        // input-method cause rather than being poisoned as leftover application work.
        let commit = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("x".into()))], true, false).expect("still editing");
        assert_eq!(doc_text(&app, lid), "x", "the preserved composition commits into the wide selection");
        assert_eq!(commit.change_cause, egui::ImeChangeCause::InputMethod);
    }

    #[test]
    fn a_failed_batch_with_a_transient_edit_does_not_revive_a_wide_selection_composition() {
        // The final document equals the pre-batch one, but the intermediate insert/delete was a
        // real document change. Its IME revision must stay the high-water mark; otherwise the
        // active wide-selection composition would look current and accept its old commit.
        let text = "a".repeat(5000);
        let (mut app, lid) = editor(&text, 0, 5000);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        pass.frame(&mut app, vec![], true, false).expect("editing");
        pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false)
            .expect("still editing");
        let owner = ime_owner(&app).expect("an editor owner");
        assert!(matches!(
            pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))),
            Some(ImeComposition::Active(active)) if active == owner
        ));

        let r = app.session.execute(
            "engine.batch",
            json!({"atomic": true, "steps": [
                {"command": "text.insert", "params": {"text": "x", "range": [5000, 5000]}},
                {"command": "text.delete", "params": {}},
                {"command": "no.suchCommand", "params": {}}
            ]}),
        );
        assert!(r.is_err());
        assert_eq!(doc_text(&app, lid), text, "the batch restored the final document");

        pass.frame(&mut app, vec![], true, false).expect("still editing");
        assert!(read_reset(&pass.ctx).is_some(), "the advanced revision retires the old IME context");
        assert!(
            matches!(pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))), Some(ImeComposition::Stale)),
            "the failed batch's transient edit leaves the old composition stale"
        );
        let stale = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("z".into()))], true, false).expect("still editing");
        assert_eq!(doc_text(&app, lid), text, "the stale composition cannot commit into the restored document");
        assert_ne!(stale.change_cause, egui::ImeChangeCause::InputMethod, "a rejected commit earns no IME receipt");
    }

    #[test]
    fn a_wide_content_change_invalidates_the_active_composition() {
        let text = "a".repeat(5000);
        let (mut app, lid) = editor(&text, 0, 5000);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        pass.frame(&mut app, vec![], true, false).expect("editing");
        pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false)
            .expect("still editing");

        let replaced = "b".repeat(5000);
        app.session.execute("layer.setText", json!({"layer": lid.0, "text": replaced})).unwrap();
        let after = pass.frame(&mut app, vec![], true, false).expect("still editing");
        assert_eq!(doc_text(&app, lid), replaced);
        assert!(after.should_interrupt_composition, "replacing the wide document starts a new IME context");
        assert!(
            matches!(pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))), Some(ImeComposition::Stale)),
            "the old composition is stale after the content change"
        );
        pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("z".into()))], true, false).expect("still editing");
        assert_eq!(doc_text(&app, lid), replaced, "a delayed commit from the old document is rejected");
    }

    #[test]
    fn a_wide_selection_change_invalidates_the_active_composition() {
        let text = "a".repeat(5000);
        let (mut app, lid) = editor(&text, 0, 5000);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        pass.frame(&mut app, vec![], true, false).expect("editing");
        pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false)
            .expect("still editing");

        // Still wider than the IME window limit, so this exercises the unavailable-text
        // selection identity rather than merely making the surrounding text available.
        app.session.execute("text.setSelection", json!({"start": 0, "end": 4500})).unwrap();
        pass.frame(&mut app, vec![], true, false).expect("still editing");
        assert!(read_reset(&pass.ctx).is_some(), "an application selection change retires the composition");
        assert!(
            matches!(pass.ctx.data(|d| d.get_temp::<ImeComposition>(ime_composition_id(pass.viewport))), Some(ImeComposition::Stale)),
            "the old composition cannot follow the selection"
        );
        pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("z".into()))], true, false).expect("still editing");
        assert_eq!(doc_text(&app, lid), text, "the stale composition cannot mutate the wide selection");
    }

    #[test]
    fn replaced_edit_session_and_hidden_passes_start_a_new_context() {
        let (mut app, _) = editor("text", 4, 4);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        let before = pass.frame(&mut app, vec![], true, false).expect("editing");
        if let Some(edit) = app.session.state.text_edit.as_mut() {
            edit.session = edit.session.wrapping_add(1);
        }
        let replaced = pass.frame(&mut app, vec![], true, false).expect("editing");
        assert!(replaced.context_generation > before.context_generation, "a recreated target does not inherit the old context");
        assert_eq!(replaced.change_cause, egui::ImeChangeCause::Other);
        assert!(replaced.should_interrupt_composition, "a new replacement epoch cancels the platform composition");

        let (mut app, _) = editor("abc", 3, 3);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        let visible = pass.frame(&mut app, vec![], true, false).expect("editing");
        assert!(pass.frame(&mut app, vec![], false, false).is_none(), "a hidden editor reports no IME owner");
        let back = pass.frame(&mut app, vec![], true, false).expect("editing again");
        assert!(back.context_generation > visible.context_generation, "reappearing starts exactly one new context");
    }

    #[test]
    fn preedit_from_a_replaced_target_cannot_commit_into_the_new_session() {
        let (mut app, first) = editor("A", 1, 1);
        let second = app.session.execute("layer.newText", json!({"text": "B"})).unwrap()["layer"].as_u64().unwrap();
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "か".into(), active_range_chars: None })], true, false)
            .expect("editing the first layer");

        app.session.execute("text.edit", json!({"layer": second, "caret": 1})).unwrap();
        let switched = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("日".into()))], true, false).expect("editing the second layer");
        assert_eq!(doc_text(&app, LayerId(second)), "B", "a delayed commit from the old composition is ignored");
        assert_eq!(doc_text(&app, first), "A", "the old layer is not changed either");
        assert_eq!(switched.owner, ime_target(&app).expect("second target").owner);
        assert!(switched.should_interrupt_composition, "the target switch interrupts the old composition");
    }

    #[test]
    fn hidden_pass_clears_this_viewports_preedit_preview() {
        let (mut app, _) = editor("ab", 2, 2);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "に".into(), active_range_chars: None })], true, false).expect("editing");
        assert!(pass.ctx.data(|d| d.get_temp::<Preedit>(ime_preedit_id(pass.viewport))).is_some());
        assert!(pass.frame(&mut app, vec![], false, false).is_none());
        assert!(pass.ctx.data(|d| d.get_temp::<Preedit>(ime_preedit_id(pass.viewport))).is_none(), "a pass without the overlay clears this viewport's preview");
    }

    #[test]
    fn a_discarded_pass_keeps_the_frame_cause_and_the_context() {
        let (mut app, lid) = editor("ab", 2, 2);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        let first = pass.frame(&mut app, vec![], true, false).expect("editing");
        let committed = pass.frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Commit("日".into()))], true, true).expect("the final pass of the frame");
        assert_eq!(doc_text(&app, lid), "ab日", "the discarded pass did not replay the event");
        assert_eq!(committed.change_cause, egui::ImeChangeCause::InputMethod, "an unchanged later pass keeps the frame's cause");
        assert_eq!(committed.context_generation, first.context_generation, "both passes are one context");
    }

    #[test]
    fn two_viewports_keep_independent_ime_contexts() {
        let (mut app, _) = editor("ab", 2, 2);
        let mut first = ImePass::new(egui::ViewportId::ROOT);
        let mut second = ImePass::new(egui::ViewportId::from_hash_of("ime-test-second-viewport"));
        let one = first.frame(&mut app, vec![], true, false).expect("editing in the first viewport");
        let two = second.frame(&mut app, vec![], true, false).expect("editing in the second viewport");
        assert_ne!(one.context_generation, two.context_generation, "each viewport has its own generation");
        let again = first.frame(&mut app, vec![], true, false).expect("editing in the first viewport");
        assert_eq!(again.context_generation, one.context_generation, "the second viewport did not disturb the first");
    }

    /// Draw one real `EffectcraftApp::draw` pass, so `draw`'s all-exit finalizer runs.
    fn draw_app_pass(app: &mut EffectcraftApp, ctx: &egui::Context, frame: &eframe::Frame, events: Vec<egui::Event>) -> egui::FullOutput {
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| app.draw(ui, frame));
        out.textures_delta.clear();
        out
    }

    /// One headless pass with the custom viewer and a real stock `TextEdit` side by side.
    ///
    /// The first call focuses the custom editor before `keyboard` (the outside press arrives
    /// there). The second call leaves the stock field's focus alone, so the stock descriptor
    /// is published by the same `output.ime` writer that an unrelated field would use.
    fn stock_pass(
        pass: &mut ImePass,
        app: &mut EffectcraftApp,
        events: Vec<egui::Event>,
        stock: &mut String,
        custom_focus: bool,
    ) -> Option<egui::output::IMEOutput> {
        let ctx = pass.ctx.clone();
        let viewport = pass.viewport;
        let custom = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(160.0, 90.0));
        let stock_rect = egui::Rect::from_min_size(egui::pos2(180.0, 30.0), egui::vec2(120.0, 24.0));
        let stock_id = egui::Id::unique("effectcraft-ime-test-stock");
        let mut viewports = egui::ViewportIdMap::default();
        viewports.insert(viewport, egui::ViewportInfo::default());
        let raw = egui::RawInput { viewport_id: viewport, viewports, screen_rect: Some(pass.rect), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            begin_ui(ui.ctx());
            if custom_focus {
                ui.interact(custom, focus_id(), egui::Sense::focusable_noninteractive());
                ui.memory_mut(|m| m.request_focus(focus_id()));
                sync_ime_target(ui, app);
                keyboard(app, ui, custom);
            } else {
                // The stock field holds focus after the outside press; keep that ownership while
                // the second pass publishes the actual stock descriptor.
                ui.memory_mut(|m| m.request_focus(stock_id));
            }
            let _ = ui.put(stock_rect, egui::TextEdit::singleline(stock).id(stock_id));
            finish_ime(app, ui);
        });
        out.textures_delta.clear();
        out.platform_output.ime
    }

    #[test]
    fn outside_pointer_to_stock_allocates_no_new_request_and_defers_the_replacement() {
        let (mut app, lid) = editor("abcd", 4, 4);
        let mut pass = ImePass::new(egui::ViewportId::ROOT);
        let preview = || egui::Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None });
        pass.frame(&mut app, vec![preview()], true, false).expect("the custom editor starts a composition");
        app.session.execute("text.setSelection", json!({"caret": 1})).unwrap();
        let first = pass.frame(&mut app, vec![], true, false).expect("the caret change publishes a request").reset_request.expect("the real request ID");
        // Complete the first interruption, then start a fresh composition with no waiting guard.
        pass.frame(&mut app, vec![egui::Event::ImeResetCompleted { request_id: first }], true, false);
        let active = pass.frame(&mut app, vec![preview()], true, false).expect("a fresh composition");
        assert!(active.reset_request.is_none(), "a fresh composition does not wait on an old request");
        assert_eq!(doc_text(&app, lid), "abcd");

        // The press outside the viewer also gives the stock field focus. It must not allocate a
        // new request: core would attach that request to the stock descriptor next pass.
        let outside = egui::pos2(240.0, 42.0);
        let mut stock = String::new();
        stock_pass(
            &mut pass,
            &mut app,
            vec![
                egui::Event::PointerMoved(outside),
                egui::Event::PointerButton { pos: outside, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            ],
            &mut stock,
            true,
        );
        let guard = pass.ctx.data(|d| d.get_temp::<ResetGuard>(reset_id(pass.viewport))).expect("the custom guard is retired locally");
        assert!(guard.suspended && guard.request.is_none(), "the outside press retires the old request without allocating another");
        let stock_ime = stock_pass(&mut pass, &mut app, vec![], &mut stock, false).expect("the actual stock field owns IME");
        assert_ne!(stock_ime.owner, ime_target(&app).expect("the custom target is still live").owner);
        assert!(stock_ime.reset_request.is_none(), "a new outside-press request would surface on the stock output");
        assert_eq!(doc_text(&app, lid), "abcd");

        // Returning to a confirmed eligible custom output allocates the one replacement.
        let replacement = pass.frame(&mut app, vec![], true, false).expect("the custom editor owns IME again").reset_request.expect("the deferred replacement");
        assert_ne!(first, replacement);
        assert!(pass.frame(&mut app, vec![], true, false).expect("still editing").reset_request.is_none(), "the replacement is one-shot");
    }

    #[test]
    fn fonts_and_device_loss_all_exit_finalizers_suspend_without_allocating() {
        let (mut app, _lid) = editor("abcd", 1, 1);
        app.ui.start_screen = false;
        app.ui.tool = Tool::Type;
        app.ui.focused = PanelKind::Composition;
        app.show_panel(PanelKind::Composition);
        let ctx = egui::Context::default();
        install_hooks(&ctx);
        let frame = eframe::Frame::_new_kittest();
        let preview = || egui::Event::Ime(egui::ImeEvent::Preedit { text: "é".into(), active_range_chars: None });
        // The first pass installs fonts and the next draws the real viewer.
        for _ in 0..3 {
            draw_app_pass(&mut app, &ctx, &frame, vec![]);
        }
        draw_app_pass(&mut app, &ctx, &frame, vec![preview()]);
        app.session.execute("text.setSelection", json!({"caret": 2})).unwrap();
        let first = draw_app_pass(&mut app, &ctx, &frame, vec![])
            .platform_output
            .ime
            .expect("the normal pass publishes the custom descriptor")
            .reset_request
            .expect("one request for the caret interruption");

        // A real font-install pass draws nothing, but `draw` still reaches the finalizer.
        app.styled = false;
        let fonts = draw_app_pass(&mut app, &ctx, &frame, vec![]);
        assert!(!app.is_ready(), "the font-install pass cannot draw");
        assert!(fonts.platform_output.ime.is_none(), "no custom descriptor is published");
        let guard = ctx.data(|d| d.get_temp::<ResetGuard>(reset_id(egui::ViewportId::ROOT))).expect("the waiting guard");
        assert!(guard.suspended && guard.request.is_none(), "the all-exit finalizer retired it without allocating");
        let second = draw_app_pass(&mut app, &ctx, &frame, vec![])
            .platform_output
            .ime
            .expect("returning to a drawing pass publishes the custom descriptor")
            .reset_request
            .expect("the deferred request");
        assert_ne!(first, second);

        // The device-loss early return is the other all-exit path. Preset the close bookkeeping
        // so this test does not autosave; the pass must still retire the waiting guard.
        app.gpu_failure_applied = Some(true);
        app.device_lost_close = Some(true);
        let lost = draw_app_pass(&mut app, &ctx, &frame, vec![]);
        assert!(lost.platform_output.ime.is_none(), "a lost renderer publishes no descriptor");
        let guard = ctx.data(|d| d.get_temp::<ResetGuard>(reset_id(egui::ViewportId::ROOT))).expect("the waiting guard");
        assert!(guard.suspended && guard.request.is_none(), "the device-loss finalizer retired it without allocating");
        app.gpu_failure_applied = None;
        let third = draw_app_pass(&mut app, &ctx, &frame, vec![])
            .platform_output
            .ime
            .expect("a drawing pass publishes the custom descriptor again")
            .reset_request
            .expect("the deferred request is one-shot");
        assert_ne!(third, second);
    }

    #[test]
    fn ime_delete_without_a_session_or_with_an_empty_range_does_nothing() {
        let (mut app, lid) = editor("abc", 1, 1);
        feed(&mut app, vec![egui::Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes: 0, after_bytes: 0 })]);
        assert_eq!(doc_text(&app, lid), "abc");
        app.session.state.text_edit = None;
        feed(&mut app, vec![egui::Event::Ime(egui::ImeEvent::DeleteSurroundingBytes { before_bytes: 2, after_bytes: 0 })]);
        assert_eq!(doc_text(&app, lid), "abc");
    }
}
