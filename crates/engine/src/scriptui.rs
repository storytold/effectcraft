//! ScriptUI: the windows, dialogs and dockable panels scripts build with After Effects'
//! ScriptUI object model (`new Window("dialog", "Title")`, `win.add("button", undefined, "OK")`,
//! `orientation`, `alignChildren`, `onClick`/`onChange`, `show()`, `close()`,
//! `layout.layout()`…).
//!
//! The scripting engine (`effectcraft-script`) owns the live JavaScript objects; after every
//! script step it publishes a serde description of each open window here ([`ScriptWindow`],
//! a [`Widget`] tree with computed [`Widget::bounds`]). Frontends draw that description (the egui
//! UI renders dialogs, palettes and dockable panels from it) and report what the user does as
//! [`ScriptUiEvent`]s through the `scriptui.*` commands, which hand them to the script's
//! handlers ([`ScriptUi::dispatch`]). Agents use the same commands: list the open script
//! windows, read their widget trees, click buttons and set values.
//!
//! [`layout`] is ScriptUI's automatic layout (orientation, alignChildren / alignment, spacing,
//! margins, preferredSize), so every frontend places widgets the same way.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{EngineError, Result, Session};

/// What kind of window a script made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowKind {
    /// Modal: the script waits in `show()` until it closes.
    #[default]
    Dialog,
    /// Floating, non-modal.
    Palette,
    /// A non-modal document window.
    Window,
    /// A dockable panel (a script from the ScriptUI Panels folder, opened from the Window menu).
    Panel,
}

/// ScriptUI control types (the `type` passed to `add()`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WidgetKind {
    /// The window itself (the tree's root).
    #[default]
    Window,
    Panel,
    Group,
    Button,
    IconButton,
    StaticText,
    EditText,
    Checkbox,
    RadioButton,
    Slider,
    Scrollbar,
    Progressbar,
    DropDownList,
    ListBox,
    TabbedPanel,
    Tab,
    Image,
    /// A type we don't draw (it still lays out as an empty box).
    #[serde(other)]
    Unknown,
}

impl WidgetKind {
    pub fn is_container(self) -> bool {
        matches!(self, WidgetKind::Window | WidgetKind::Panel | WidgetKind::Group | WidgetKind::TabbedPanel | WidgetKind::Tab)
    }
}

/// One control (or container) of a script window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Widget {
    /// Unique within its window (the window itself is 0).
    pub id: u32,
    #[serde(rename = "type")]
    pub kind: WidgetKind,
    /// `properties.name` (agents can address controls by it).
    pub name: String,
    pub text: String,
    /// Slider / scrollbar / progressbar value.
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// Checkbox / radio button state.
    pub checked: bool,
    /// Drop-down list / list box items.
    pub items: Vec<String>,
    /// Selected item indices.
    pub selection: Vec<usize>,
    /// `column` (default for windows and panels), `row` (groups) or `stack`.
    pub orientation: String,
    /// `[horizontal, vertical]`: left / center / right / fill, top / center / bottom / fill.
    pub align_children: Vec<String>,
    /// This control's own alignment in its parent (overrides the parent's alignChildren).
    pub alignment: Vec<String>,
    pub spacing: Option<f64>,
    /// `[left, top, right, bottom]`.
    pub margins: Option<[f64; 4]>,
    pub preferred_size: Option<[f64; 2]>,
    /// Bounds set by the script (`[left, top, right, bottom]` relative to the parent).
    pub fixed_bounds: Option<[f64; 4]>,
    /// Laid-out bounds `[x, y, width, height]` relative to the window's content.
    pub bounds: [f64; 4],
    pub enabled: bool,
    pub visible: bool,
    pub help_tip: String,
    pub multiline: bool,
    pub read_only: bool,
    /// Event handlers the script attached (`onClick`, `onChange`, `onChanging`…).
    pub handlers: Vec<String>,
    /// The mouse events ([`MOUSE_EVENTS`]) a script listener receives when they happen on this
    /// control: its own `addEventListener` listeners and its containers' (events bubble).
    /// Frontends send only these.
    pub mouse: Vec<String>,
    /// The active tab of a tabbed panel (child index).
    pub active_tab: usize,
    /// What the control's `onDraw` handler painted (ScriptUIGraphics), in control coordinates.
    /// Frontends paint it instead of the control's default look, unless it contains
    /// [`DrawOp::Os`] (`drawOSControl()`), which draws the default look first.
    pub draw: Vec<DrawOp>,
    /// The image of an image control or icon button.
    pub image: Option<ImageRef>,
    pub children: Vec<Widget>,
}

/// One segment of a ScriptUIGraphics path (control coordinates, pixels).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum PathSeg {
    /// `moveTo(x, y)`: starts a new subpath.
    M { x: f64, y: f64 },
    /// `lineTo(x, y)`.
    L { x: f64, y: f64 },
    /// `rectPath(x, y, w, h)`: a closed rectangle subpath.
    R { x: f64, y: f64, w: f64, h: f64 },
    /// `ellipsePath(x, y, w, h)`: a closed ellipse inscribed in the rectangle.
    E { x: f64, y: f64, w: f64, h: f64 },
    /// `closePath()`.
    Z,
}

/// One paint call of an `onDraw` handler. Colours are straight RGBA in 0..1; `None` is a theme
/// colour (a brush or pen made from a theme colour name).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum DrawOp {
    /// `fillPath(brush, path)`.
    Fill { color: Option<[f32; 4]>, path: Vec<PathSeg> },
    /// `strokePath(pen, path)`.
    Stroke { color: Option<[f32; 4]>, width: f64, path: Vec<PathSeg> },
    /// `drawString(text, pen, x, y, font)`: `(x, y)` is the text's top-left.
    Text {
        text: String,
        color: Option<[f32; 4]>,
        x: f64,
        y: f64,
        size: f64,
        #[serde(default)]
        style: String,
    },
    /// `drawOSControl()`: the control's default look.
    Os,
    /// `drawImage(image, x, y, w?, h?)`: `w` / `h` 0 = the image's own size.
    Image {
        #[serde(default)]
        image: Option<ImageRef>,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    },
}

/// A ScriptUIImage (`ScriptUI.newImage`, an image control's or icon button's image, a
/// `drawImage` argument): an image file, or embedded PNG / JPEG bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageRef {
    /// File path (or a resource name we can't resolve: drawn as a placeholder).
    pub src: Option<String>,
    /// Embedded bytes as a binary string (one character per byte, as in resource strings).
    pub data: Option<String>,
    pub name: String,
}

impl ImageRef {
    /// The encoded image bytes (the embedded data, else the file through `services`).
    pub fn bytes(&self, services: &dyn crate::Services) -> Option<Vec<u8>> {
        if let Some(d) = &self.data {
            return d.chars().map(|c| u8::try_from(c as u32).ok()).collect();
        }
        services.read_file(self.src.as_deref()?).ok()
    }
}

impl PathSeg {
    /// The subpaths of `path` as polylines (ellipses flattened to `n` segments), each with
    /// whether it is closed.
    pub fn polylines(path: &[PathSeg], n: usize) -> Vec<(Vec<[f64; 2]>, bool)> {
        fn flush(cur: &mut Vec<[f64; 2]>, out: &mut Vec<(Vec<[f64; 2]>, bool)>, closed: bool) {
            if cur.len() > 1 {
                out.push((std::mem::take(cur), closed));
            } else {
                cur.clear();
            }
        }
        let mut out = vec![];
        let mut cur: Vec<[f64; 2]> = vec![];
        for seg in path {
            match *seg {
                PathSeg::M { x, y } => {
                    flush(&mut cur, &mut out, false);
                    cur.push([x, y]);
                }
                PathSeg::L { x, y } => cur.push([x, y]),
                PathSeg::Z => {
                    let start = cur.first().copied();
                    flush(&mut cur, &mut out, true);
                    // Drawing continues from the subpath's start.
                    cur.extend(start);
                }
                PathSeg::R { x, y, w, h } => {
                    flush(&mut cur, &mut out, false);
                    out.push((vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]], true));
                }
                PathSeg::E { x, y, w, h } => {
                    flush(&mut cur, &mut out, false);
                    let (cx, cy, rx, ry) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
                    let n = n.max(8);
                    let pts = (0..n)
                        .map(|i| {
                            let a = i as f64 / n as f64 * std::f64::consts::TAU;
                            [cx + rx * a.cos(), cy + ry * a.sin()]
                        })
                        .collect();
                    out.push((pts, true));
                }
            }
        }
        flush(&mut cur, &mut out, false);
        out
    }

    /// The area `fillPath` paints, as triangles: every subpath closed, overlapping subpaths
    /// combined by the non-zero winding rule (`even_odd`: the even-odd rule), so concave,
    /// self-intersecting and holed paths fill correctly (frontends' polygon fills are often
    /// convex-only). Exact trapezoid decomposition: the plane is cut into horizontal bands at
    /// every vertex and edge crossing; inside a band no edges cross, so each inside span is a
    /// trapezoid (two triangles).
    pub fn fill_triangles(path: &[PathSeg], n: usize, even_odd: bool) -> Vec<[[f64; 2]; 3]> {
        // Non-horizontal edges, top to bottom, with their winding direction.
        let mut edges: Vec<([f64; 2], [f64; 2], i32)> = vec![];
        for (pts, _) in PathSeg::polylines(path, n) {
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                if !(a[0].is_finite() && a[1].is_finite() && b[0].is_finite() && b[1].is_finite()) || a[1] == b[1] {
                    continue;
                }
                edges.push(if a[1] < b[1] { (a, b, 1) } else { (b, a, -1) });
            }
        }
        if edges.len() < 2 {
            return vec![];
        }
        let x_at = |e: &([f64; 2], [f64; 2], i32), y: f64| e.0[0] + (e.1[0] - e.0[0]) * (y - e.0[1]) / (e.1[1] - e.0[1]);
        let mut ys: Vec<f64> = edges.iter().flat_map(|e| [e.0[1], e.1[1]]).collect();
        // Where two edges cross, a band boundary.
        for i in 0..edges.len() {
            for j in i + 1..edges.len() {
                let (a, b) = (&edges[i], &edges[j]);
                let (lo, hi) = (a.0[1].max(b.0[1]), a.1[1].min(b.1[1]));
                if hi <= lo {
                    continue;
                }
                let (d0, d1) = (x_at(a, lo) - x_at(b, lo), x_at(a, hi) - x_at(b, hi));
                if d0 * d1 < 0.0 {
                    ys.push(lo + (hi - lo) * d0 / (d0 - d1));
                }
            }
        }
        ys.sort_by(f64::total_cmp);
        ys.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        let mut out = vec![];
        let mut row: Vec<(f64, f64, f64, i32)> = vec![];
        for w in ys.windows(2) {
            let (ya, yb) = (w[0], w[1]);
            let ym = (ya + yb) * 0.5;
            row.clear();
            row.extend(edges.iter().filter(|e| e.0[1] < ym && ym < e.1[1]).map(|e| (x_at(e, ym), x_at(e, ya), x_at(e, yb), e.2)));
            row.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0;
            for k in 0..row.len().saturating_sub(1) {
                wind += row[k].3;
                let inside = if even_odd { wind % 2 != 0 } else { wind != 0 };
                if inside {
                    let (l, r) = (row[k], row[k + 1]);
                    out.push([[l.1, ya], [r.1, ya], [r.2, yb]]);
                    out.push([[l.1, ya], [r.2, yb], [l.2, yb]]);
                }
            }
        }
        out
    }
}

impl Widget {
    pub fn find(&self, id: u32) -> Option<&Widget> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }
    pub fn find_mut(&mut self, id: u32) -> Option<&mut Widget> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }
    /// Every control, depth first.
    pub fn walk<'a>(&'a self, out: &mut Vec<&'a Widget>) {
        out.push(self);
        for c in &self.children {
            c.walk(out);
        }
    }
    /// A control by id (number or `"#12"`), `properties.name` or text.
    pub fn lookup(&self, key: &Value) -> Option<&Widget> {
        let mut all = vec![];
        self.walk(&mut all);
        match key {
            Value::Number(n) => n.as_u64().and_then(|n| self.find(n as u32)),
            Value::String(s) => {
                if let Some(n) = s.strip_prefix('#').and_then(|n| n.parse::<u32>().ok()) {
                    return self.find(n);
                }
                all.iter().find(|w| !w.name.is_empty() && w.name == *s).or_else(|| all.iter().find(|w| w.id != 0 && w.text == *s)).copied()
            }
            _ => None,
        }
    }
}

/// An open script window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScriptWindow {
    /// Unique in the process.
    pub id: u32,
    /// The script context that owns it (its handlers run there).
    pub host: u32,
    pub kind: WindowKind,
    pub title: String,
    /// The script that made it.
    pub script: String,
    /// Shown and not closed.
    pub visible: bool,
    /// The script is waiting in this dialog's `show()`.
    pub modal: bool,
    /// The window and its controls (`root.bounds` is the content size).
    pub root: Widget,
}

/// A user (or agent) action on a script window control.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptUiEvent {
    pub window: u32,
    pub widget: u32,
    /// `click`, `change`, `changing`, `close`, or a mouse event ([`MOUSE_EVENTS`]).
    pub kind: String,
    /// The new value: text, number, bool, or selected index(es); for `close`, the result; for
    /// mouse events (and a mouse `click`), the pointer: `{x, y, button, detail, screenX, screenY,
    /// altKey, ctrlKey, metaKey, shiftKey}` with `x`/`y` relative to the control.
    pub value: Value,
}

/// The ScriptUI mouse events scripts can listen for (`addEventListener`).
pub const MOUSE_EVENTS: [&str; 6] = ["mousedown", "mouseup", "mousemove", "mouseover", "mouseout", "click"];

/// Hands an event to the script that owns the window; returns what the handler reported
/// (`{ok, output, error}`) and updates [`Session::script_ui`].
pub type ScriptUiDispatch = fn(&mut Session, &ScriptUiEvent) -> std::result::Result<Value, String>;

/// Runs the scripts' `app.scheduleTask` tasks that are due; returns
/// `{ran, output, errors: [{message, line, file}], next (ms until the next task is due, or null)}`
/// and updates [`Session::script_ui`]. Frontends call it every frame (see [`ScriptUi::run_tasks`]).
pub type ScriptTaskRunner = fn(&mut Session) -> Value;

/// The open script windows.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptUi {
    pub windows: Vec<ScriptWindow>,
    /// Bumped whenever a window changes.
    pub revision: u64,
    /// The scripting engine's event hand-off (set with [`Session::script`]).
    #[serde(skip)]
    pub dispatch: Option<ScriptUiDispatch>,
    /// The scripting engine's scheduled-task runner (set with [`Session::script`]).
    #[serde(skip)]
    pub tasks: Option<ScriptTaskRunner>,
    /// Bumped whenever the project is closed or replaced (New / Open / Close Project): repeating
    /// tasks of scripts without an open window stop then.
    #[serde(skip)]
    pub project_epoch: u64,
    /// The script contexts of this session with scheduled tasks (kept by the scripting engine).
    #[serde(skip)]
    pub task_hosts: Vec<u32>,
}

impl ScriptUi {
    pub fn window(&self, id: u32) -> Option<&ScriptWindow> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Run the scripts' due `app.scheduleTask` tasks ([`ScriptTaskRunner`]); `None` without a
    /// scripting engine.
    pub fn run_tasks(s: &mut Session) -> Option<Value> {
        let f = s.script_ui.tasks?;
        // A task may close its ScriptUI panel: forget it then.
        let open = crate::commands::scripts::open_panel_names(s);
        let r = f(s);
        crate::commands::scripts::forget_closed_panels(s, &open);
        Some(r)
    }

    /// Replace the windows a script host published (laying them out).
    pub fn publish(&mut self, host: u32, mut windows: Vec<ScriptWindow>) {
        for w in &mut windows {
            w.host = host;
            layout(w);
        }
        windows.retain(|w| w.visible);
        let mut out: Vec<ScriptWindow> = vec![];
        // Keep the order windows first appeared in.
        for old in self.windows.drain(..) {
            if old.host != host {
                out.push(old);
            } else if let Some(i) = windows.iter().position(|w| w.id == old.id) {
                out.push(windows.remove(i));
            }
        }
        out.extend(windows);
        self.windows = out;
        self.revision += 1;
    }
}

// ---------------------------------------------------------------- layout

/// Average character width and line height of the default dialog font (points).
const CHAR_W: f64 = 7.0;
const LINE_H: f64 = 16.0;

fn text_w(s: &str) -> f64 {
    s.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f64 * CHAR_W
}

fn lines(s: &str) -> f64 {
    s.lines().count().max(1) as f64
}

fn orientation(w: &Widget) -> &str {
    match w.orientation.as_str() {
        "" => match w.kind {
            WidgetKind::Group => "row",
            WidgetKind::TabbedPanel => "stack",
            _ => "column",
        },
        o => o,
    }
}

fn margins(w: &Widget) -> [f64; 4] {
    w.margins.unwrap_or(match w.kind {
        WidgetKind::Window => [15.0; 4],
        WidgetKind::Panel => [10.0, 15.0, 10.0, 10.0],
        WidgetKind::TabbedPanel => [0.0, 24.0, 0.0, 0.0],
        WidgetKind::Tab => [10.0; 4],
        _ => [0.0; 4],
    })
}

fn spacing(w: &Widget) -> f64 {
    w.spacing.unwrap_or(10.0)
}

fn align_children(w: &Widget) -> (String, String) {
    let d = match w.kind {
        WidgetKind::Group => ("center", "center"),
        _ => ("center", "top"),
    };
    let a = |i: usize, d: &str| w.align_children.get(i).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| d.to_string());
    // A single value applies to the axis it names.
    if w.align_children.len() == 1 {
        let v = w.align_children[0].clone();
        return match v.as_str() {
            "top" | "bottom" => (d.0.into(), v),
            "fill" if orientation(w) == "row" => (d.0.into(), v),
            _ => (v, d.1.into()),
        };
    }
    (a(0, d.0), a(1, d.1))
}

/// Preferred size of a control (its own content; containers include their children).
pub fn preferred(w: &Widget) -> [f64; 2] {
    let ps = w.preferred_size.unwrap_or([0.0; 2]);
    let content = if w.kind.is_container() {
        let kids: Vec<[f64; 2]> = w.children.iter().filter(|c| c.visible).map(preferred).collect();
        let m = margins(w);
        let sp = spacing(w);
        let n = kids.len() as f64;
        let (cw, ch) = match orientation(w) {
            "row" => (kids.iter().map(|k| k[0]).sum::<f64>() + sp * (n - 1.0).max(0.0), kids.iter().map(|k| k[1]).fold(0.0, f64::max)),
            "stack" => (kids.iter().map(|k| k[0]).fold(0.0, f64::max), kids.iter().map(|k| k[1]).fold(0.0, f64::max)),
            _ => (kids.iter().map(|k| k[0]).fold(0.0, f64::max), kids.iter().map(|k| k[1]).sum::<f64>() + sp * (n - 1.0).max(0.0)),
        };
        let title_w = if matches!(w.kind, WidgetKind::Panel) { text_w(&w.text) + 20.0 } else { 0.0 };
        [(cw + m[0] + m[2]).max(title_w), ch + m[1] + m[3]]
    } else {
        match w.kind {
            WidgetKind::Button | WidgetKind::IconButton => [(text_w(&w.text) + 24.0).max(80.0), 25.0],
            WidgetKind::StaticText => [text_w(&w.text) + 4.0, LINE_H * lines(&w.text)],
            WidgetKind::EditText => {
                if w.multiline {
                    [(text_w(&w.text) + 12.0).max(160.0), (LINE_H * lines(&w.text) + 8.0).max(60.0)]
                } else {
                    [(text_w(&w.text) + 12.0).max(40.0), 22.0]
                }
            }
            WidgetKind::Checkbox | WidgetKind::RadioButton => [text_w(&w.text) + 24.0, 18.0],
            WidgetKind::Slider | WidgetKind::Scrollbar => [100.0, 22.0],
            WidgetKind::Progressbar => [100.0, 10.0],
            WidgetKind::DropDownList => [w.items.iter().map(|i| text_w(i)).fold(0.0, f64::max) + 36.0, 22.0],
            WidgetKind::ListBox => [(w.items.iter().map(|i| text_w(i)).fold(0.0, f64::max) + 24.0).max(80.0), (w.items.len().clamp(3, 10) as f64 * 18.0 + 4.0)],
            _ => [20.0, 20.0],
        }
    };
    [if ps[0] > 0.0 { ps[0] } else { content[0] }, if ps[1] > 0.0 { ps[1] } else { content[1] }]
}

fn place(w: &mut Widget, x: f64, y: f64, width: f64, height: f64) {
    w.bounds = [x, y, width, height];
    if !w.kind.is_container() {
        return;
    }
    let m = margins(w);
    let sp = spacing(w);
    let (ah, av) = align_children(w);
    let inner = [m[0], m[1], (width - m[0] - m[2]).max(0.0), (height - m[1] - m[3]).max(0.0)];
    let orient = orientation(w).to_string();
    let mut cursor = 0.0;
    for c in w.children.iter_mut() {
        if !c.visible {
            c.bounds = [0.0; 4];
            continue;
        }
        if let Some(b) = c.fixed_bounds {
            let (cx, cy, cw, ch) = (x + b[0], y + b[1], b[2] - b[0], b[3] - b[1]);
            place(c, cx, cy, cw, ch);
            continue;
        }
        let pref = preferred(c);
        let h_align = c.alignment.first().filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| ah.clone());
        let v_align = c.alignment.get(1).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| av.clone());
        let along = |a: &str, free: f64| match a {
            "center" => free / 2.0,
            "right" | "bottom" => free,
            _ => 0.0,
        };
        let (cx, cy, cw, ch) = match orient.as_str() {
            "row" => {
                let ch = if v_align == "fill" { inner[3] } else { pref[1] };
                let cy = inner[1] + along(&v_align, inner[3] - ch);
                let r = (inner[0] + cursor, cy, pref[0], ch);
                cursor += pref[0] + sp;
                r
            }
            "stack" => {
                let cw = if h_align == "fill" { inner[2] } else { pref[0] };
                let ch = if v_align == "fill" { inner[3] } else { pref[1] };
                (inner[0] + along(&h_align, inner[2] - cw), inner[1] + along(&v_align, inner[3] - ch), cw, ch)
            }
            _ => {
                let cw = if h_align == "fill" { inner[2] } else { pref[0] };
                let cx = inner[0] + along(&h_align, inner[2] - cw);
                let r = (cx, inner[1] + cursor, cw, pref[1]);
                cursor += pref[1] + sp;
                r
            }
        };
        place(c, x + cx, y + cy, cw, ch);
    }
    // Rows centre their content run as a whole when it is narrower than the row.
    if orient == "row" && (ah == "center" || ah == "right") {
        let used = (cursor - sp).max(0.0);
        let shift = along_shift(&ah, inner[2] - used);
        if shift > 0.0 {
            for c in w.children.iter_mut().filter(|c| c.visible && c.fixed_bounds.is_none()) {
                shift_all(c, shift, 0.0);
            }
        }
    }
    // Columns aligned to the bottom / centre move the whole run.
    if orient == "column" && (av == "center" || av == "bottom") {
        let used = (cursor - sp).max(0.0);
        let shift = along_shift(&av, inner[3] - used);
        if shift > 0.0 {
            for c in w.children.iter_mut().filter(|c| c.visible && c.fixed_bounds.is_none()) {
                shift_all(c, 0.0, shift);
            }
        }
    }
}

fn along_shift(a: &str, free: f64) -> f64 {
    match a {
        "center" => (free / 2.0).max(0.0),
        "right" | "bottom" => free.max(0.0),
        _ => 0.0,
    }
}

fn shift_all(w: &mut Widget, dx: f64, dy: f64) {
    w.bounds[0] += dx;
    w.bounds[1] += dy;
    for c in &mut w.children {
        shift_all(c, dx, dy);
    }
}

/// ScriptUI automatic layout: size the window to its preferred size (or the size the script
/// gave it) and place every control (`bounds` relative to the window's top-left).
pub fn layout(w: &mut ScriptWindow) {
    let pref = preferred(&w.root);
    let (width, height) = match w.root.fixed_bounds {
        Some(b) if b[2] > b[0] && b[3] > b[1] => (b[2] - b[0], b[3] - b[1]),
        _ => (pref[0], pref[1]),
    };
    place(&mut w.root, 0.0, 0.0, width, height);
}

// ---------------------------------------------------------------- commands

fn window_p(s: &Session, p: &Value, cmd: &str) -> Result<u32> {
    let key = p.get("window");
    let w = match key {
        Some(Value::Number(n)) => n.as_u64().map(|n| n as u32).filter(|n| s.script_ui.window(*n).is_some()),
        Some(Value::String(t)) => s.script_ui.windows.iter().find(|w| w.title == *t).map(|w| w.id),
        None if s.script_ui.windows.len() == 1 => s.script_ui.windows.first().map(|w| w.id),
        _ => None,
    };
    w.ok_or_else(|| crate::commands::bad(cmd, "no such script window (give `window`: id or title from scriptui.list)"))
}

fn widget_p(s: &Session, win: u32, p: &Value, cmd: &str) -> Result<u32> {
    let w = s.script_ui.window(win).ok_or_else(|| crate::commands::bad(cmd, "window closed"))?;
    let key = p.get("widget").ok_or_else(|| crate::commands::bad(cmd, "missing `widget` (id, `#id`, properties.name or text)"))?;
    w.root.lookup(key).map(|w| w.id).ok_or_else(|| crate::commands::bad(cmd, format!("no control {key} in window {win}")))
}

fn dispatch(s: &mut Session, ev: ScriptUiEvent) -> Result<Value> {
    let f = s.script_ui.dispatch.ok_or_else(|| EngineError::Other("scripting is not available in this build".into()))?;
    // The event may close a ScriptUI panel (its tab, or the script itself): forget it then.
    let open = crate::commands::scripts::open_panel_names(s);
    let r = f(s, &ev).map_err(EngineError::Other);
    crate::commands::scripts::forget_closed_panels(s, &open);
    r
}

/// `scriptui.list`: the open script windows.
pub(crate) fn list(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!(
        s.script_ui
            .windows
            .iter()
            .map(|w| json!({"window": w.id, "title": w.title, "kind": w.kind, "script": w.script, "modal": w.modal, "size": [w.root.bounds[2], w.root.bounds[3]]}))
            .collect::<Vec<_>>()
    ))
}

/// `scriptui.get`: one window's control tree.
pub(crate) fn get(s: &mut Session, p: &Value) -> Result<Value> {
    let id = window_p(s, p, "scriptui.get")?;
    Ok(serde_json::to_value(s.script_ui.window(id)).unwrap_or(Value::Null))
}

/// `scriptui.click`: press a button / toggle a checkbox / pick a radio button.
pub(crate) fn click(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "scriptui.click";
    let window = window_p(s, p, c)?;
    let widget = widget_p(s, window, p, c)?;
    let w = s.script_ui.window(window).and_then(|w| w.root.find(widget)).cloned().unwrap_or_default();
    if !w.enabled {
        return Err(crate::commands::bad(c, format!("control {widget} is disabled")));
    }
    dispatch(s, ScriptUiEvent { window, widget, kind: "click".into(), value: Value::Null })
}

/// `scriptui.set`: set a control's value (edit text, slider, checkbox, list selection) as the
/// user would, firing onChanging / onChange.
pub(crate) fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "scriptui.set";
    let window = window_p(s, p, c)?;
    let widget = widget_p(s, window, p, c)?;
    let w = s.script_ui.window(window).and_then(|w| w.root.find(widget)).cloned().unwrap_or_default();
    if !w.enabled {
        return Err(crate::commands::bad(c, format!("control {widget} is disabled")));
    }
    let mut value = p.get("value").cloned().ok_or_else(|| crate::commands::bad(c, "missing `value`"))?;
    // List selections by item text.
    if matches!(w.kind, WidgetKind::DropDownList | WidgetKind::ListBox)
        && let Value::String(t) = &value
    {
        let i = w.items.iter().position(|it| it == t).ok_or_else(|| crate::commands::bad(c, format!("no item `{t}` (items: {:?})", w.items)))?;
        value = json!(i);
    }
    let kind = if p.get("changing").and_then(Value::as_bool) == Some(true) { "changing" } else { "change" };
    dispatch(s, ScriptUiEvent { window, widget, kind: kind.into(), value })
}

/// `script.runTasks`: run the scripts' `app.scheduleTask` tasks that are due now (frontends do
/// it every frame; headless hosts and agents call this).
pub(crate) fn run_tasks(s: &mut Session, _: &Value) -> Result<Value> {
    ScriptUi::run_tasks(s).ok_or_else(|| EngineError::Other("scripting is not available in this build".into()))
}

/// `scriptui.mouse`: a mouse event on a control, as the pointer would make it: the script's
/// `addEventListener` listeners on the control and its containers receive a MouseEvent
/// (`clientX`/`clientY` relative to the control, `button`, `detail` = click count, modifier
/// keys). A `click` also does what clicking the control does (toggle a checkbox, press a button).
pub(crate) fn mouse(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "scriptui.mouse";
    let window = window_p(s, p, c)?;
    let widget = widget_p(s, window, p, c)?;
    let ty = p.get("type").and_then(Value::as_str).unwrap_or("click").to_ascii_lowercase();
    let Some(ty) = MOUSE_EVENTS.iter().find(|t| **t == ty) else {
        return Err(crate::commands::bad(c, format!("unknown mouse event `{ty}` (one of {})", MOUSE_EVENTS.join(", "))));
    };
    let w = s.script_ui.window(window).and_then(|w| w.root.find(widget)).cloned().unwrap_or_default();
    if !w.enabled {
        return Err(crate::commands::bad(c, format!("control {widget} is disabled")));
    }
    // Hostile input stays finite and bounded.
    const FAR: f64 = 1.0e6;
    let num = |k: &str, lo: f64, hi: f64, d: f64| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite()).map_or(d, |v| v.clamp(lo, hi));
    let flag = |k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(false);
    let clicks = if matches!(*ty, "mousemove" | "mouseover" | "mouseout") { 0.0 } else { 1.0 };
    let value = json!({
        "x": num("x", -FAR, FAR, w.bounds[2] / 2.0),
        "y": num("y", -FAR, FAR, w.bounds[3] / 2.0),
        "screenX": num("screenX", -FAR, FAR, 0.0),
        "screenY": num("screenY", -FAR, FAR, 0.0),
        "button": num("button", 0.0, 2.0, 0.0).round(),
        "detail": num("detail", 0.0, 1000.0, clicks).round(),
        "altKey": flag("altKey"),
        "ctrlKey": flag("ctrlKey"),
        "metaKey": flag("metaKey"),
        "shiftKey": flag("shiftKey"),
    });
    dispatch(s, ScriptUiEvent { window, widget, kind: (*ty).to_string(), value })
}

/// `scriptui.close`: close a window (a dialog's `show()` returns `result`, default 2 = Cancel).
pub(crate) fn close(s: &mut Session, p: &Value) -> Result<Value> {
    let window = window_p(s, p, "scriptui.close")?;
    let result = p.get("result").cloned().unwrap_or(json!(2));
    dispatch(s, ScriptUiEvent { window, widget: 0, kind: "close".into(), value: result })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(kind: WidgetKind, text: &str) -> Widget {
        Widget { kind, text: text.into(), enabled: true, visible: true, ..Default::default() }
    }

    #[test]
    fn column_and_row_layout() {
        let mut row = w(WidgetKind::Group, "");
        row.id = 1;
        let mut ok = w(WidgetKind::Button, "OK");
        ok.id = 2;
        let mut cancel = w(WidgetKind::Button, "Cancel");
        cancel.id = 3;
        row.children = vec![ok, cancel];
        let mut label = w(WidgetKind::StaticText, "Name:");
        label.id = 4;
        let mut root = w(WidgetKind::Window, "Dialog");
        root.children = vec![label, row];
        let mut win = ScriptWindow { id: 1, root, visible: true, ..Default::default() };
        layout(&mut win);
        let r = &win.root;
        // Window: 15 px margins, children stacked with 10 px spacing.
        let row = r.find(1).unwrap();
        assert_eq!(row.bounds[2], 170.0, "two 80 px buttons + 10 spacing");
        assert_eq!(r.bounds[2], 200.0);
        assert_eq!(r.find(4).unwrap().bounds[1], 15.0);
        assert_eq!(row.bounds[1], 15.0 + 16.0 + 10.0);
        // Centred (the default alignChildren of windows).
        assert_eq!(row.bounds[0], 15.0);
        let label = r.find(4).unwrap();
        assert!((label.bounds[0] + label.bounds[2] / 2.0 - 100.0).abs() < 1e-9);
        assert_eq!(r.find(3).unwrap().bounds[0], 15.0 + 80.0 + 10.0);
        // alignChildren left / fill.
        win.root.align_children = vec!["fill".into(), "top".into()];
        layout(&mut win);
        assert_eq!(win.root.find(4).unwrap().bounds[2], 170.0);
        win.root.align_children = vec!["left".into()];
        layout(&mut win);
        assert_eq!(win.root.find(4).unwrap().bounds[0], 15.0);
        // preferredSize wins.
        win.root.find_mut(2).unwrap().preferred_size = Some([120.0, 30.0]);
        layout(&mut win);
        assert_eq!(win.root.find(2).unwrap().bounds[2..], [120.0, 30.0]);
        // Lookup by name / text / id.
        win.root.find_mut(2).unwrap().name = "ok".into();
        assert_eq!(win.root.lookup(&json!("ok")).unwrap().id, 2);
        assert_eq!(win.root.lookup(&json!("Cancel")).unwrap().id, 3);
        assert_eq!(win.root.lookup(&json!("#4")).unwrap().id, 4);
        assert_eq!(win.root.lookup(&json!(1)).unwrap().id, 1);
    }

    #[test]
    fn draw_lists_flatten_to_polylines_and_round_trip() {
        let path = vec![
            PathSeg::M { x: 0.0, y: 0.0 },
            PathSeg::L { x: 10.0, y: 0.0 },
            PathSeg::L { x: 10.0, y: 10.0 },
            PathSeg::Z,
            PathSeg::R { x: 1.0, y: 2.0, w: 3.0, h: 4.0 },
            PathSeg::E { x: 0.0, y: 0.0, w: 20.0, h: 10.0 },
        ];
        let lines = PathSeg::polylines(&path, 16);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], (vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]], true));
        assert_eq!(lines[1].0[2], [4.0, 6.0]);
        assert_eq!(lines[2].0.len(), 16);
        assert!((lines[2].0[0][0] - 20.0).abs() < 1e-9 && (lines[2].0[4][1] - 10.0).abs() < 1e-9);
        let op = DrawOp::Fill { color: Some([1.0, 0.0, 0.0, 1.0]), path };
        let j = serde_json::to_value(&op).unwrap();
        assert_eq!(j["op"], "fill");
        assert_eq!(j["path"][0], json!({"k": "M", "x": 0.0, "y": 0.0}));
        assert_eq!(serde_json::from_value::<DrawOp>(j).unwrap(), op);
        let t: DrawOp = serde_json::from_value(json!({"op": "text", "text": "Hi", "color": null, "x": 1, "y": 2, "size": 14})).unwrap();
        assert!(matches!(t, DrawOp::Text { size, .. } if size == 14.0));
    }

    #[test]
    fn publish_replaces_a_hosts_windows() {
        let mut ui = ScriptUi::default();
        let win = |id: u32, visible: bool| ScriptWindow { id, visible, root: w(WidgetKind::Window, ""), ..Default::default() };
        ui.publish(7, vec![win(1, true), win(2, true)]);
        ui.publish(8, vec![win(3, true)]);
        assert_eq!(ui.windows.iter().map(|w| (w.id, w.host)).collect::<Vec<_>>(), [(1, 7), (2, 7), (3, 8)]);
        ui.publish(7, vec![win(2, true), win(1, false)]);
        assert_eq!(ui.windows.iter().map(|w| w.id).collect::<Vec<_>>(), [2, 3]);
        let j = serde_json::to_value(&ui).unwrap();
        assert_eq!(j["windows"][0]["root"]["type"], "window");
    }

    fn area(tris: &[[[f64; 2]; 3]]) -> f64 {
        tris.iter().map(|t| ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[2][0] - t[0][0]) * (t[1][1] - t[0][1])).abs() * 0.5).sum()
    }

    fn poly(pts: &[[f64; 2]]) -> Vec<PathSeg> {
        let mut v = vec![PathSeg::M { x: pts[0][0], y: pts[0][1] }];
        v.extend(pts[1..].iter().map(|p| PathSeg::L { x: p[0], y: p[1] }));
        v.push(PathSeg::Z);
        v
    }

    fn covers(tris: &[[[f64; 2]; 3]], p: [f64; 2]) -> bool {
        let s = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        tris.iter().any(|t| {
            let (d0, d1, d2) = (s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0]));
            (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0)
        })
    }

    #[test]
    fn concave_self_intersecting_and_holed_fills() {
        // An L (concave): area 3, the notch stays empty.
        let l = poly(&[[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [1.0, 1.0], [1.0, 2.0], [0.0, 2.0]]);
        let t = PathSeg::fill_triangles(&l, 48, false);
        assert!((area(&t) - 3.0).abs() < 1e-9, "{}", area(&t));
        assert!(covers(&t, [0.5, 1.5]) && !covers(&t, [1.5, 1.5]));
        // A five-pointed star drawn in one stroke: non-zero fills the centre, even-odd doesn't.
        let star: Vec<[f64; 2]> = (0..5)
            .map(|i| {
                let a = (i * 2 % 5) as f64 * std::f64::consts::TAU / 5.0 - std::f64::consts::FRAC_PI_2;
                [50.0 + 40.0 * a.cos(), 50.0 + 40.0 * a.sin()]
            })
            .collect();
        let nz = PathSeg::fill_triangles(&poly(&star), 48, false);
        let eo = PathSeg::fill_triangles(&poly(&star), 48, true);
        assert!(covers(&nz, [50.0, 50.0]) && !covers(&eo, [50.0, 50.0]));
        assert!(covers(&eo, [50.0, 15.0]), "a point of the star");
        assert!(area(&nz) > area(&eo));
        // A rectangle with a hole (an inner square the other way round).
        let mut holed = poly(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
        holed.extend(poly(&[[3.0, 3.0], [3.0, 7.0], [7.0, 7.0], [7.0, 3.0]]));
        let h = PathSeg::fill_triangles(&holed, 48, false);
        assert!((area(&h) - 84.0).abs() < 1e-9 && !covers(&h, [5.0, 5.0]));
        // rectPath + ellipsePath in one path; an unclosed subpath fills as if closed.
        let shapes = vec![PathSeg::R { x: 0.0, y: 0.0, w: 4.0, h: 2.0 }, PathSeg::E { x: 10.0, y: 0.0, w: 20.0, h: 20.0 }];
        let a = area(&PathSeg::fill_triangles(&shapes, 256, false));
        assert!((a - 8.0 - std::f64::consts::PI * 100.0).abs() < 1.0, "{a}");
        let open = vec![PathSeg::M { x: 0.0, y: 0.0 }, PathSeg::L { x: 4.0, y: 0.0 }, PathSeg::L { x: 0.0, y: 4.0 }];
        assert!((area(&PathSeg::fill_triangles(&open, 48, false)) - 8.0).abs() < 1e-9);
    }
}
