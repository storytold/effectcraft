//! The keyboard in the in-window menu bar (#279), as in After Effects on Windows: Alt pressed and
//! released on its own focuses the menu bar (and leaves it again); Left / Right move between its
//! menus, Down, Enter or Space opens one. In an open menu Up / Down move the highlight through
//! the entries (skipping disabled ones, wrapping around), Right opens the highlighted submenu and
//! Left closes it again (in the top menu they open the next / previous menu), Enter or Space
//! chooses the highlighted entry and Escape closes the menus. The highlight is egui's keyboard
//! focus, so Enter and Space are egui's own clicks on it.
//!
//! While a menu is open or the bar has the keyboard, keys go to the menus only: the app's
//! shortcuts and the panels don't see them (Space must not start a preview).

use egui::containers::menu::{MenuState, SubMenu, find_menu_root};
use egui::{Context, Event, Id, Key, Modifiers, Popup, Pos2, Rect, Response, Ui};

/// An entry of an open menu as drawn this frame.
#[derive(Clone, Copy)]
struct Row {
    id: Id,
    enabled: bool,
    submenu: bool,
}

/// An open menu (or submenu) as drawn this frame.
#[derive(Default)]
struct Level {
    /// The menu's root `Ui` (where egui keeps which submenu is open).
    menu: Option<Id>,
    /// The submenu entry it opened from (none for a top-level menu).
    owner: Option<Id>,
    /// Drawn only to measure it (its first frame): its entries can't take the highlight yet.
    sizing: bool,
    rows: Vec<Row>,
}

/// The menu bar's buttons and open menus, collected while they are drawn.
#[derive(Default)]
pub struct Nav {
    /// The bar's menu buttons and their menus' popups.
    tops: Vec<(Id, Id)>,
    /// Open menus by depth: the top-level menu, then each open submenu.
    levels: Vec<Level>,
    /// The titles and open menus drawn this frame: a press outside them is an outside press,
    /// which closes the menu (native behaviour; the press itself still reaches what's under it).
    rects: Vec<Rect>,
    /// A title press opened (or kept) the menu this frame: releasing the button over an entry
    /// chooses it (After Effects' press-and-browse).
    browsing: bool,
}

/// What to highlight once a menu opened from the keyboard has been drawn: the first entry of the
/// menu at this depth.
#[derive(Clone, Copy)]
struct FirstEntry(usize);

/// Submenus opened from the keyboard, with where the pointer was: egui closes an open submenu
/// while the pointer rests on another entry of its menu, so they stay open until it moves.
#[derive(Clone, Default)]
struct KeyOpened {
    submenus: Vec<(Id, Id)>,
    pointer: Option<Pos2>,
}

/// The temp-data id of the [`FirstEntry`] depth to highlight after a keyboard-opened menu is
/// first drawn.
fn pending_id() -> Id {
    Id::unique("menu-keys-first")
}

/// The temp-data id of the [`KeyOpened`] record: submenus opened from the keyboard and the
/// pointer position they were opened at.
fn opened_id() -> Id {
    Id::unique("menu-keys-opened")
}

/// The temp-data id of the flag saying the menu bar currently holds the keyboard.
fn active_id() -> Id {
    Id::unique("menu-keys-active")
}

/// The temp-data id of the `(armed, held)` Alt-key state used to detect an Alt tap.
fn alt_id() -> Id {
    Id::unique("menu-keys-alt")
}

impl Nav {
    /// A top-level menu button of the bar.
    pub fn top(&mut self, button: &Response) {
        self.tops.push((button.id, Popup::default_response_id(button)));
    }

    /// The rect `rect` (a menu title or an open menu) holds, so a press there doesn't close.
    pub fn cover(&mut self, rect: Rect) {
        self.rects.push(rect);
    }

    /// Whether `pos` is inside a title or menu drawn this frame.
    pub fn covers(&self, pos: Pos2) -> bool {
        self.rects.iter().any(|r| r.contains(pos))
    }

    /// A title press is in progress (its button still held): see [`Nav::browsing`].
    pub fn set_browsing(&mut self) {
        self.browsing = true;
    }

    /// Whether the menu bar is mid press-and-browse (a title was pressed this pointer-down and
    /// hasn't been released yet): releasing over an entry then chooses it.
    pub fn browsing(&self) -> bool {
        self.browsing
    }

    /// The popup of the open top-level menu (only one can be open), if any.
    pub fn open_popup(&self, ctx: &Context) -> Option<Id> {
        self.tops.iter().find(|(_, p)| Popup::is_id_open(ctx, *p)).map(|(_, p)| *p)
    }

    /// The position in the bar of the open top-level menu, if any.
    pub fn open_index(&self, ctx: &Context) -> Option<usize> {
        self.tops.iter().position(|(_, p)| Popup::is_id_open(ctx, *p))
    }

    /// An entry drawn at `depth` (0: a top-level menu) in the menu `ui`. Hovering an entry moves
    /// the keyboard highlight to it, and a highlighted entry scrolls into view.
    pub fn entry(&mut self, ui: &Ui, depth: usize, r: &Response, submenu: bool) {
        while self.levels.len() <= depth {
            self.levels.push(Level::default());
        }
        let Some(level) = self.levels.get_mut(depth) else { return };
        if level.menu.is_none() {
            level.menu = Some(find_menu_root(ui).scope_id);
            level.sizing = ui.is_sizing_pass();
        }
        level.rows.push(Row { id: r.id, enabled: r.enabled(), submenu });
        if r.gained_focus() {
            r.scroll_to_me(None);
        }
        let ctx = ui.ctx();
        let keyboard = active(ctx) && ctx.memory(|m| m.focused()).is_some();
        if keyboard && r.enabled() && !r.has_focus() && r.hovered() && ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
            r.request_focus();
        }
    }

    /// Submenu entry `button` at `depth` showed its submenu this frame (drawn at `depth + 1`).
    pub fn opened(&mut self, depth: usize, button: &Response) {
        if let Some(level) = self.levels.get_mut(depth + 1) {
            level.owner = Some(button.id);
        }
    }

    /// The keyboard highlight: (depth, index) of the focused entry.
    fn focused(&self, focus: Option<Id>) -> Option<(usize, usize)> {
        let f = focus?;
        self.levels.iter().enumerate().rev().find_map(|(d, l)| l.rows.iter().position(|r| r.id == f).map(|i| (d, i)))
    }
}

/// Whether the menu bar had the keyboard last frame (a menu open or the bar focused).
pub fn active(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp(active_id())).unwrap_or(false)
}

/// Alt pressed and released with nothing else in between (no other modifier, key, click or wheel),
/// off macOS. Alt only counts when it went down with no other modifier held: Windows switches the
/// keyboard layout with Alt+Shift, and releasing Shift first leaves Alt held alone, which must not
/// take the keyboard from the field being typed in (#394).
fn alt_tapped(ctx: &Context) -> bool {
    if cfg!(target_os = "macos") {
        return false;
    }
    // (armed, the modifiers held after the last change)
    let (mut armed, mut held): (bool, Modifiers) = ctx.data(|d| d.get_temp(alt_id())).unwrap_or_default();
    let mut tapped = false;
    ctx.input(|i| {
        for e in &i.events {
            match e {
                Event::ModifiersChanged(m) => {
                    tapped |= armed && *m == Modifiers::NONE;
                    armed = *m == Modifiers::ALT && held == Modifiers::NONE;
                    held = *m;
                }
                Event::PointerMoved(_) | Event::MouseMoved(_) | Event::Zoom(_) => {}
                _ => armed = false,
            }
        }
    });
    ctx.data_mut(|d| d.insert_temp(alt_id(), (armed, held)));
    tapped
}

/// The next enabled entry of `rows` after `from` (`step` 1) or before it (-1), wrapping around;
/// from none: the first (or last) one.
fn step(rows: &[Row], from: Option<usize>, step: isize) -> Option<Id> {
    let n = rows.len() as isize;
    let start = from.map_or(if step > 0 { -1 } else { n }, |i| i as isize);
    (1..=n).map(|k| (start + step * k).rem_euclid(n.max(1))).filter_map(|i| rows.get(i as usize)).find(|r| r.enabled).map(|r| r.id)
}

fn focus(ctx: &Context, id: Id) {
    ctx.memory_mut(|m| m.request_focus(id));
}

/// The bar's menu `i`, wrapping around.
fn nth_top(nav: &Nav, i: isize) -> Option<(Id, Id)> {
    nav.tops.get(i.rem_euclid((nav.tops.len() as isize).max(1)) as usize).copied()
}

/// Move the bar's keyboard focus to its menu `i` (wrapping).
fn focus_top(ctx: &Context, nav: &Nav, i: isize) {
    if let Some((button, _)) = nth_top(nav, i) {
        focus(ctx, button);
    }
}

/// Open the top-level menu `i` (wrapping) with its first entry highlighted once drawn.
fn open_top(ctx: &Context, nav: &Nav, i: isize) {
    let Some((button, popup)) = nth_top(nav, i) else { return };
    Popup::open_id(ctx, popup);
    focus(ctx, button);
    ctx.data_mut(|d| {
        d.insert_temp(pending_id(), FirstEntry(0));
        d.remove::<KeyOpened>(opened_id());
    });
}

/// Open the submenu of entry `row` in the menu at `depth`, its first entry highlighted once drawn.
fn open_submenu(ctx: &Context, nav: &Nav, depth: usize, row: Id) {
    let Some(menu) = nav.levels.get(depth).and_then(|l| l.menu) else { return };
    let sub = SubMenu::id_from_widget_id(row);
    show_submenu(ctx, menu, sub);
    let pointer = ctx.pointer_hover_pos();
    ctx.data_mut(|d| {
        let mut o: KeyOpened = d.get_temp(opened_id()).unwrap_or_default();
        o.submenus.retain(|(m, _)| *m != menu);
        o.submenus.push((menu, sub));
        o.pointer = pointer;
        d.insert_temp(opened_id(), o);
        d.insert_temp(pending_id(), FirstEntry(depth + 1));
    });
}

/// Handle this frame's keys once the bar and its open menus are drawn (`nav`).
pub fn handle(ctx: &Context, nav: &Nav) {
    let was_active = active(ctx);
    let open = nav.tops.iter().position(|(_, p)| Popup::is_id_open(ctx, *p));
    let focus_id = ctx.memory(|m| m.focused());
    let bar = focus_id.and_then(|f| nav.tops.iter().position(|(b, _)| *b == f));
    if alt_tapped(ctx) {
        if open.is_some() || bar.is_some() {
            Popup::close_all(ctx);
            if let Some(f) = focus_id {
                ctx.memory_mut(|m| m.surrender_focus(f));
            }
        } else if let Some((first, _)) = nav.tops.first() {
            focus(ctx, *first);
        }
        ctx.data_mut(|d| d.insert_temp(active_id(), true));
        strip_keys(ctx);
        return;
    }
    let now_active = open.is_some() || bar.is_some();
    ctx.data_mut(|d| d.insert_temp(active_id(), now_active));
    if !now_active {
        ctx.data_mut(|d| {
            d.remove::<FirstEntry>(pending_id());
            d.remove::<KeyOpened>(opened_id());
        });
        if was_active {
            // (The key that closed the menus, e.g. Enter on an entry, stays with them.)
            strip_keys(ctx);
        }
        return;
    }
    keep_key_opened(ctx);
    // A menu opened from the keyboard and now shown: highlight its first entry (if it has an
    // enabled one).
    if let Some(FirstEntry(depth)) = ctx.data(|d| d.get_temp(pending_id()))
        && let Some(level) = nav.levels.get(depth).filter(|l| !l.sizing)
    {
        if let Some(id) = step(&level.rows, None, 1) {
            focus(ctx, id);
        }
        ctx.data_mut(|d| d.remove::<FirstEntry>(pending_id()));
    }
    let pressed =
        |k: Key| ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Key { key, pressed: true, modifiers, .. } if *key == k && !modifiers.any())));
    let cur = nav.focused(focus_id);
    let row = cur.and_then(|(d, i)| nav.levels.get(d)?.rows.get(i).copied());
    for (key, dir) in [(Key::ArrowDown, 1), (Key::ArrowUp, -1)] {
        if !pressed(key) {
            continue;
        }
        match (open, cur) {
            (Some(_), Some((d, i))) => {
                if let Some(id) = nav.levels.get(d).and_then(|l| step(&l.rows, Some(i), dir)) {
                    focus(ctx, id);
                }
            }
            // Opened with the pointer: the deepest open menu's first (last) entry.
            (Some(_), None) => {
                if let Some(id) = nav.levels.last().and_then(|l| step(&l.rows, None, dir)) {
                    focus(ctx, id);
                }
            }
            (None, _) if dir > 0 => open_top(ctx, nav, bar.unwrap_or(0) as isize),
            (None, _) => {}
        }
    }
    let top = open.or(bar).unwrap_or(0) as isize;
    if pressed(Key::ArrowRight) {
        match (cur, row) {
            (Some((d, _)), Some(r)) if r.submenu => open_submenu(ctx, nav, d, r.id),
            _ if open.is_some() => open_top(ctx, nav, top + 1),
            _ => focus_top(ctx, nav, top + 1),
        }
    }
    if pressed(Key::ArrowLeft) {
        match cur {
            Some((d, _)) if d > 0 => {
                let (menu, owner) = (nav.levels.get(d - 1).and_then(|l| l.menu), nav.levels.get(d).and_then(|l| l.owner));
                if let Some(menu) = menu {
                    MenuState::from_id(ctx, menu, |s| s.open_item = None);
                    ctx.data_mut(|data| {
                        if let Some(mut o) = data.get_temp::<KeyOpened>(opened_id()) {
                            o.submenus.retain(|(m, _)| *m != menu);
                            data.insert_temp(opened_id(), o);
                        }
                    });
                }
                if let Some(owner) = owner {
                    focus(ctx, owner);
                }
            }
            _ if open.is_some() => open_top(ctx, nav, top - 1),
            _ => focus_top(ctx, nav, top - 1),
        }
    }
    if pressed(Key::Enter) || pressed(Key::Space) {
        match (cur, row) {
            // egui toggles a submenu on Enter: it opens (again), highlighting its first entry.
            (Some((d, _)), Some(r)) if r.submenu => open_submenu(ctx, nav, d, r.id),
            // A top-level menu that the press opened.
            (None, _) if bar.is_some() && open.is_some() => ctx.data_mut(|d| {
                d.insert_temp(pending_id(), FirstEntry(0));
            }),
            _ => {}
        }
    }
    // The arrows move the highlight here, not egui's spatial focus search.
    ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
    strip_keys(ctx);
    ctx.request_repaint();
}

/// Keep the submenus opened from the keyboard open while the pointer hasn't moved.
fn keep_key_opened(ctx: &Context) {
    let Some(o) = ctx.data(|d| d.get_temp::<KeyOpened>(opened_id())) else { return };
    if ctx.pointer_hover_pos() != o.pointer {
        ctx.data_mut(|d| d.remove::<KeyOpened>(opened_id()));
        return;
    }
    for (menu, sub) in o.submenus {
        show_submenu(ctx, menu, sub);
    }
}

/// Make `sub` the open submenu of `menu`. (egui forgets an open submenu that wasn't shown last
/// frame, so it counts as shown now.)
fn show_submenu(ctx: &Context, menu: Id, sub: Id) {
    MenuState::mark_shown(ctx, sub);
    MenuState::from_id(ctx, menu, |s| s.open_item = Some(sub));
}

/// The rest of the frame (shortcuts, panels) doesn't see this frame's keys.
fn strip_keys(ctx: &Context) {
    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Key { .. } | Event::Text(_))));
}
