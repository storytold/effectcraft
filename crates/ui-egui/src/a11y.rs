//! Accessibility helpers (docs/gaps.md gap 25): names, roles and states for the widgets the app
//! paints itself, and the small pieces of list keyboard handling the panels share.
//!
//! egui builds an AccessKit node for every widget that can take the keyboard focus, but a widget
//! made with `ui.interact` has no name and no role until the panel says what it is. These helpers
//! write that onto the widget's node *after* the widget is made each frame (egui rebuilds the
//! tree every pass), and keep whatever egui already put there (its role for a `DragValue`, the
//! numeric value of a `Slider`).
//!
//! Names are the same translated strings the panel draws (`i18n::tr`), never hard-coded English.
//! `tests/ui_accessibility_audit.rs` walks the tree and fails on an unnamed control in the panels
//! that have had their pass.

use egui::accesskit::{HasPopup, Role, Toggled};
use egui::{Context, Id, Response};

/// Set `f` on `r`'s AccessKit node (nothing when AccessKit is off). egui only fills a node's
/// bounds and actions for widgets that can take the keyboard focus; a widget that cannot (the
/// non-current rows of a list) still needs both, or assistive technology cannot place or click it.
fn node(r: &Response, f: impl FnOnce(&mut egui::accesskit::Node)) {
    r.ctx.accesskit_node_builder(r.id, |n| {
        n.set_bounds(egui::accesskit::Rect { x0: r.rect.min.x.into(), y0: r.rect.min.y.into(), x1: r.rect.max.x.into(), y1: r.rect.max.y.into() });
        if r.sense.is_focusable() {
            n.add_action(egui::accesskit::Action::Focus);
        }
        if r.sense.senses_click() {
            n.add_action(egui::accesskit::Action::Click);
        }
        f(n);
    });
}

/// Give `r`'s node a role and leave its name to the caller.
pub(crate) fn role(r: &Response, role: Role) {
    node(r, |n| n.set_role(role));
}

/// Give `r`'s node a role and a name.
pub(crate) fn role_name(r: &Response, role: Role, label: &str) {
    node(r, |n| {
        n.set_role(role);
        n.set_label(label);
    });
}

/// Name `r`'s node (`label`), keeping the role egui gave it (spin button, slider, text input).
pub(crate) fn name(r: &Response, label: &str) {
    node(r, |n| n.set_label(label));
}

/// Name `r`'s node and give it a current value (a spin button's or a text field's text).
pub(crate) fn name_value(r: &Response, label: &str, value: &str) {
    node(r, |n| {
        n.set_label(label);
        n.set_value(value);
    });
}

/// Set `r`'s checked state (a check box, a switch, a toggle button).
pub(crate) fn checked(r: &Response, on: bool) {
    node(r, |n| n.set_toggled(if on { Toggled::True } else { Toggled::False }));
}

/// Set `r`'s open state (a twirl, a disclosure).
pub(crate) fn expanded(r: &Response, open: bool) {
    node(r, |n| n.set_expanded(open));
}

/// A push button; `enabled` false marks it disabled for assistive technology (it stays visible).
pub(crate) fn button(r: &Response, label: &str, enabled: bool) {
    node(r, |n| {
        n.set_role(Role::Button);
        n.set_label(label);
        if !enabled {
            n.set_disabled();
        }
    });
}

/// A button that opens a menu or popup: `open` is whether it is open now.
pub(crate) fn menu_button(r: &Response, label: &str, open: bool) {
    node(r, |n| {
        n.set_role(Role::Button);
        n.set_label(label);
        n.set_has_popup(HasPopup::Menu);
        n.set_expanded(open);
    });
}

/// A button that shows or hides something (a twirl): `expanded` is the state it leads to from.
pub(crate) fn disclosure(r: &Response, label: &str, expanded: bool) {
    node(r, |n| {
        n.set_role(Role::Button);
        n.set_label(label);
        n.set_expanded(expanded);
    });
}

/// A check box.
pub(crate) fn check_box(r: &Response, label: &str, checked: bool, enabled: bool) {
    node(r, |n| {
        n.set_role(Role::CheckBox);
        n.set_label(label);
        n.set_toggled(if checked { Toggled::True } else { Toggled::False });
        if !enabled {
            n.set_disabled();
        }
    });
}

/// A drop-down: `label` says what it sets and `value` is what it is set to.
pub(crate) fn combo_box(r: &Response, label: &str, value: &str, enabled: bool) {
    node(r, |n| {
        n.set_role(Role::ComboBox);
        n.set_label(label);
        n.set_value(value);
        n.set_has_popup(HasPopup::Menu);
        n.set_expanded(false);
        if !enabled {
            n.set_disabled();
        }
    });
}

/// A row of a tree: selected, its depth (`level`, 1 at the top), whether it is open (`None` for a
/// leaf) and its place among its siblings.
pub(crate) fn tree_item(r: &Response, label: &str, selected: bool, level: usize, expanded: Option<bool>) {
    node(r, |n| {
        n.set_role(Role::TreeItem);
        n.set_label(label);
        n.set_selected(selected);
        n.set_level(level);
        if let Some(open) = expanded {
            n.set_expanded(open);
        }
    });
}

/// An entry of a flat list: `position` is its 1-based place among `size`.
pub(crate) fn list_item(r: &Response, label: &str, selected: bool, position: usize, size: usize) {
    node(r, |n| {
        n.set_role(Role::ListItem);
        n.set_label(label);
        n.set_selected(selected);
        n.set_position_in_set(position);
        n.set_size_of_set(size);
    });
}

/// An entry of a menu or popup list; `checked` is `Some` for an entry that is on or off.
pub(crate) fn menu_item(r: &Response, label: &str, checked: Option<bool>) {
    node(r, |n| {
        n.set_label(label);
        match checked {
            Some(on) => {
                n.set_role(Role::MenuItemCheckBox);
                n.set_toggled(if on { Toggled::True } else { Toggled::False });
            }
            None => n.set_role(Role::MenuItem),
        }
    });
}

/// A text field with a name (and its text as the value).
pub(crate) fn text_input(r: &Response, label: &str, text: &str) {
    node(r, |n| {
        n.set_role(Role::TextInput);
        n.set_label(label);
        n.set_value(text);
    });
}

/// A progress bar: `fraction` 0..=1 and the sentence it shows.
pub(crate) fn progress(r: &Response, label: &str, fraction: f32) {
    node(r, |n| {
        n.set_role(Role::ProgressIndicator);
        n.set_label(label);
        n.set_min_numeric_value(0.0);
        n.set_max_numeric_value(1.0);
        n.set_numeric_value(f64::from(fraction.clamp(0.0, 1.0)));
    });
}

// ------------------------------------------------------------------------------------------
// Things a panel paints that are not controls

/// Make `rect` a node of the tree (egui paints text and shapes into the painter and leaves them
/// out of it) and give it a role and a name. Not focusable: it is for assistive technology to
/// read, not for the keyboard.
fn static_node(ui: &egui::Ui, id: Id, rect: egui::Rect, role: Role, label: &str, value: Option<&str>) {
    let r = ui.interact(rect, id, egui::Sense::hover());
    node(&r, |n| {
        n.set_role(role);
        n.set_label(label);
        if let Some(v) = value {
            n.set_value(v);
        }
    });
}

/// A read-out painted at `rect` (a colour channel, the time, a status line): `label` is the whole
/// sentence it shows, e.g. "Red: 128".
pub(crate) fn readout(ui: &egui::Ui, id: Id, rect: egui::Rect, label: &str) {
    static_node(ui, id, rect, Role::Label, label, None);
}

/// A picture with no text of its own (a colour swatch, a scope, a waveform): `label` says what it
/// shows in a few words.
pub(crate) fn graphic(ui: &egui::Ui, id: Id, rect: egui::Rect, label: &str) {
    static_node(ui, id, rect, Role::Image, label, None);
}

/// A log or console output (newest line last): `label` says which, `text` is what it holds.
pub(crate) fn log(ui: &egui::Ui, id: Id, rect: egui::Rect, label: &str, text: &str) {
    static_node(ui, id, rect, Role::Log, label, Some(text));
}

/// A toggle button (it stays pressed while it is on).
pub(crate) fn toggle_button(r: &Response, label: &str, on: bool) {
    node(r, |n| {
        n.set_role(Role::Button);
        n.set_label(label);
        n.set_toggled(if on { Toggled::True } else { Toggled::False });
    });
}

// ------------------------------------------------------------------------------------------
// List keyboard handling

/// Ask for the keyboard focus on widget `id` once it is drawn (a list moves its focus to the row
/// the arrow keys reached, which may not be focusable, or drawn, until the next pass). `key`
/// names the list.
pub(crate) fn focus_next_pass(ctx: &Context, key: &str, id: Id) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(Id::new(("a11y-pending-focus", key)), (id, pass)));
}

/// True once, for the widget `focus_next_pass` named for list `key`: the caller then requests the
/// focus. A request nobody answers within a few passes is dropped.
pub(crate) fn take_pending_focus(ctx: &Context, key: &str, id: Id) -> bool {
    let slot = Id::new(("a11y-pending-focus", key));
    let now = ctx.cumulative_pass_nr();
    let Some((want, at)) = ctx.data(|d| d.get_temp::<(Id, u64)>(slot)) else { return false };
    if now > at.saturating_add(4) {
        ctx.data_mut(|d| d.remove::<(Id, u64)>(slot));
        return false;
    }
    if want != id || now == at {
        return false;
    }
    ctx.data_mut(|d| d.remove::<(Id, u64)>(slot));
    true
}
