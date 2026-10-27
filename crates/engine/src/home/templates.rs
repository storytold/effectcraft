//! Project templates: the Home screen's **New from Template** gallery.
//!
//! - **Built-in templates** are original work authored in code (MIT OR Apache-2.0, listed in
//!   `ATTRIBUTION.md`): each builds its project by running engine commands on a scratch session,
//!   so it is exactly what a user could make by hand. Their thumbnails are rendered by our own
//!   renderer the first time they are asked for.
//! - **User templates**: File ▸ Save as Template… writes the open project as an `.ectemplate`
//!   (the open template container of `essential.exportTemplate`: a stored ZIP with
//!   `manifest.json` (`kind: "project"`), `project.ecproj`, `poster.png` and `thumb.txt`) into
//!   the config store's `Templates` folder.
//! - **Embedded footage**: saving embeds the footage files the project reads (default on,
//!   `embedFootage`), as `media/<item>/<file>` entries like `essential.exportTemplate`, up to
//!   [`EMBED_LIMIT_MB`] in all (`embedLimitMB`); footage beyond the cap stays referenced by its
//!   path and the save warns. Creating from the template extracts the files next to the new
//!   project (`projectPath`: `<project dir>/<stem> Footage/`), into `footageDir`, or into the
//!   settings folder's `Template Footage/<name>/` (the browser: its storage), and points the
//!   footage at them.
//! - Opening a template (`templates.create`) makes an untitled copy: the template itself is never
//!   changed.
//!
//! Commands: `templates.list`, `templates.thumbnail`, `templates.create`, `templates.saveAs`,
//! `templates.delete`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_keyframe::{Keyframe, Value as KV};
use effectcraft_project::{ItemId, LayerId, Project};
use effectcraft_time::Tick;
use serde::Serialize;
use serde_json::{Value, json};

use crate::commands::essential::{TEMPLATE_FORMAT, TEMPLATE_VERSION, read_template};
use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session, cmd, query};

/// The config-store folder user templates live in.
pub const TEMPLATES_DIR: &str = "Templates";
/// Template file extension.
pub const EXT: &str = "ectemplate";
/// Default cap on the footage a saved template embeds (MB, all files together).
pub const EMBED_LIMIT_MB: f64 = 256.0;
/// Folder (under the settings folder) that footage extracted from templates goes to.
pub const FOOTAGE_DIR: &str = "Template Footage";
/// Gallery thumbnail size (RGB, letterboxed).
pub const THUMB_W: usize = 256;
pub const THUMB_H: usize = 144;

/// One gallery entry.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateInfo {
    /// Built-in: its id (`lower-third`); user: `user/<file name>`.
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub builtin: bool,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub duration: f64,
    /// Essential Graphics controls of the main composition (names).
    pub controls: Vec<String>,
}

struct Builtin {
    id: &'static str,
    name: &'static str,
    category: &'static str,
    description: &'static str,
    build: fn(&mut Session) -> Result<()>,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "lower-third",
        name: "Lower Third",
        category: "Titles",
        description: "A name and title that slide in on an accent bar. Essential Graphics: Name, Title, Bar Color.",
        build: lower_third,
    },
    Builtin {
        id: "title-card",
        name: "Title Card",
        category: "Titles",
        description: "A centred title that scales up over a gradient, with a subtitle fade. Essential Graphics: Title, Subtitle.",
        build: title_card,
    },
    Builtin {
        id: "logo-reveal",
        name: "Logo Reveal",
        category: "Brand",
        description: "A placeholder logo mark spins and glows into place above the brand name. Essential Graphics: Brand Name, Logo Color.",
        build: logo_reveal,
    },
    Builtin {
        id: "kinetic-type",
        name: "Kinetic Type",
        category: "Titles",
        description: "Three words pop on in rhythm. Essential Graphics: Word 1, Word 2, Word 3.",
        build: kinetic_type,
    },
    Builtin {
        id: "social-square",
        name: "Social Square 1080×1080",
        category: "Social",
        description: "A square post: headline, accent circle and handle. Essential Graphics: Headline, Handle, Accent Color.",
        build: social_square,
    },
    Builtin {
        id: "vertical-story",
        name: "Vertical 9:16 Story",
        category: "Social",
        description: "A 1080×1920 story with a headline and a bobbing call to action. Essential Graphics: Headline, Call to Action.",
        build: vertical_story,
    },
    Builtin {
        id: "slideshow",
        name: "Slideshow",
        category: "Photo",
        description: "Three slides with a slow push-in and cross-fades; replace the colour slides with your photos. Essential Graphics: Caption 1–3.",
        build: slideshow,
    },
    Builtin {
        id: "text-orbit-3d",
        name: "3D Text Orbit",
        category: "3D",
        description: "3D title cards with a camera orbiting around them over a floor. Essential Graphics: Title.",
        build: text_orbit,
    },
];

// ------------------------------------------------------------------------- authoring helpers

fn ex(s: &mut Session, c: &str, p: Value) -> Result<Value> {
    s.execute(c, p)
}

fn layer_of(v: &Value) -> Result<LayerId> {
    v["layer"].as_u64().map(LayerId).ok_or_else(|| EngineError::Other("template: no layer".into()))
}

fn comp(s: &mut Session, name: &str, w: u32, h: u32, secs: f64, bg: &str) -> Result<ItemId> {
    ex(s, "comp.new", json!({"name": name, "width": w, "height": h, "frameRate": 30, "duration": secs, "background": bg}))?;
    s.active_comp_id().ok_or(EngineError::NoComp)
}

fn solid(s: &mut Session, name: &str, color: &str) -> Result<LayerId> {
    layer_of(&ex(s, "layer.newSolid", json!({"name": name, "color": color}))?)
}

fn text(s: &mut Session, txt: &str, pos: [f64; 2], size: f64, fill: &str) -> Result<LayerId> {
    layer_of(&ex(s, "layer.newText", json!({"text": txt, "position": pos, "size": size, "fill": fill}))?)
}

fn shape(s: &mut Session, name: &str, kind: &str, size: [f64; 2], fill: &str, pos: [f64; 2]) -> Result<LayerId> {
    layer_of(&ex(s, "layer.newShape", json!({"name": name, "kind": kind, "size": size, "fill": fill, "position": pos}))?)
}

fn gradient(s: &mut Session, l: LayerId, start: [f64; 2], end: [f64; 2], a: &str, b: &str, radial: bool) -> Result<()> {
    ex(s, "effect.apply", json!({"layers": [l.0], "effect": "ec.generate.gradientramp"}))?;
    let hex = |h: &str| effectcraft_color::Rgba::from_hex(h).map(|c| [c.r as f64, c.g as f64, c.b as f64, 1.0]).unwrap_or([1.0; 4]);
    edit_layer(s, l, |ly| {
        let set = |ly: &mut effectcraft_project::Layer, k: &str, v: KV| {
            if let Some(p) = ly.props.prop_mut(&format!("effects/#1/{k}")) {
                p.value = v;
            }
        };
        set(ly, "start", KV::Vec2(start));
        set(ly, "end", KV::Vec2(end));
        set(ly, "startColor", KV::Color(hex(a)));
        set(ly, "endColor", KV::Color(hex(b)));
        set(ly, "shape", KV::Enum(u32::from(radial)));
    });
    Ok(())
}

fn edit_layer(s: &mut Session, l: LayerId, f: impl FnOnce(&mut effectcraft_project::Layer)) {
    let Some(cid) = s.active_comp_id() else { return };
    if let Some(ly) = Arc::make_mut(&mut s.project).comp_mut(cid).and_then(|c| c.layer_mut(l)) {
        f(ly);
    }
}

/// Eased keyframes at seconds (layer time = comp time: template layers start at 0).
fn keys(s: &mut Session, l: LayerId, path: &str, list: &[(f64, KV)]) {
    let rate = s.active_comp().map(|c| c.frame_rate).unwrap_or(effectcraft_time::FrameRate::FPS_30);
    edit_layer(s, l, |ly| {
        if let Some(p) = ly.props.prop_mut(path) {
            p.keys = list.iter().map(|(t, v)| Keyframe::new(rate.snap_nearest(Tick::from_seconds_f64(*t)), v.clone()).eased()).collect();
        }
    });
}

fn set(s: &mut Session, l: LayerId, path: &str, v: KV) {
    edit_layer(s, l, |ly| {
        if let Some(p) = ly.props.prop_mut(path) {
            p.value = v;
        }
    });
}

fn expose(s: &mut Session, l: LayerId, path: &str, name: &str) -> Result<()> {
    ex(s, "essential.addProperty", json!({"layer": l.0, "path": path, "name": name}))?;
    Ok(())
}

fn finish(s: &mut Session, eg_name: &str, poster: f64) -> Result<()> {
    ex(s, "essential.setName", json!({"name": eg_name}))?;
    if let Some(cid) = s.active_comp_id() {
        let rate = s.active_comp().map(|c| c.frame_rate).unwrap_or(effectcraft_time::FrameRate::FPS_30);
        if let Some(c) = Arc::make_mut(&mut s.project).comp_mut(cid) {
            c.poster_time = rate.snap_nearest(Tick::from_seconds_f64(poster));
        }
    }
    Ok(())
}

fn p3(x: f64, y: f64) -> KV {
    KV::Vec3([x, y, 0.0])
}
fn sc(v: f64) -> KV {
    KV::Vec3([v, v, 100.0])
}
fn op(v: f64) -> KV {
    KV::Scalar(v)
}

// ------------------------------------------------------------------------- built-in templates

fn lower_third(s: &mut Session) -> Result<()> {
    comp(s, "Lower Third", 1920, 1080, 6.0, "#101218")?;
    let bg = solid(s, "Preview Background (delete over footage)", "#000000")?;
    gradient(s, bg, [960.0, 200.0], [960.0, 1080.0], "#2B3550", "#0B0D14", false)?;
    let bar = shape(s, "Accent Bar", "rect", [760.0, 150.0], "#3D8BFF", [0.0, 0.0])?;
    keys(s, bar, "transform/position", &[(0.0, p3(-420.0, 860.0)), (0.7, p3(520.0, 860.0)), (5.0, p3(520.0, 860.0)), (5.6, p3(-420.0, 860.0))]);
    let name = text(s, "Alex Rivera", [520.0, 845.0], 64.0, "#FFFFFF")?;
    keys(s, name, "transform/opacity", &[(0.4, op(0.0)), (0.9, op(100.0)), (4.9, op(100.0)), (5.4, op(0.0))]);
    let title = text(s, "Motion Designer", [520.0, 905.0], 34.0, "#DDE6FF")?;
    keys(s, title, "transform/opacity", &[(0.6, op(0.0)), (1.1, op(100.0)), (4.8, op(100.0)), (5.3, op(0.0))]);
    expose(s, name, "text/sourceText", "Name")?;
    expose(s, title, "text/sourceText", "Title")?;
    expose(s, bar, "contents/group#1/contents/fill/color", "Bar Color")?;
    finish(s, "Lower Third", 2.0)
}

fn title_card(s: &mut Session) -> Result<()> {
    comp(s, "Title Card", 1920, 1080, 5.0, "#000000")?;
    let bg = solid(s, "Background", "#000000")?;
    gradient(s, bg, [960.0, 400.0], [960.0, 1400.0], "#5B2A86", "#120A1F", true)?;
    let title = text(s, "YOUR TITLE HERE", [960.0, 560.0], 120.0, "#FFFFFF")?;
    keys(s, title, "transform/scale", &[(0.0, sc(70.0)), (1.2, sc(100.0)), (5.0, sc(106.0))]);
    keys(s, title, "transform/opacity", &[(0.0, op(0.0)), (0.6, op(100.0))]);
    let sub = text(s, "A subtitle that sets the scene", [960.0, 660.0], 40.0, "#E9D8FF")?;
    keys(s, sub, "transform/opacity", &[(0.8, op(0.0)), (1.6, op(100.0))]);
    expose(s, title, "text/sourceText", "Title")?;
    expose(s, sub, "text/sourceText", "Subtitle")?;
    finish(s, "Title Card", 2.5)
}

fn logo_reveal(s: &mut Session) -> Result<()> {
    comp(s, "Logo Reveal", 1920, 1080, 5.0, "#05070C")?;
    let bg = solid(s, "Background", "#000000")?;
    gradient(s, bg, [960.0, 480.0], [960.0, 1300.0], "#14324A", "#04060A", true)?;
    let logo = shape(s, "Logo (replace me)", "star", [300.0, 300.0], "#FFC23D", [960.0, 450.0])?;
    keys(s, logo, "transform/scale", &[(0.0, sc(0.0)), (1.0, sc(110.0)), (1.4, sc(100.0))]);
    keys(s, logo, "transform/rotation", &[(0.0, op(-180.0)), (1.4, op(0.0))]);
    ex(s, "effect.apply", json!({"layers": [logo.0], "effect": "Glow"}))?;
    let brand = text(s, "BRAND NAME", [960.0, 720.0], 84.0, "#FFFFFF")?;
    keys(s, brand, "transform/opacity", &[(1.2, op(0.0)), (2.0, op(100.0))]);
    keys(s, brand, "transform/position", &[(1.2, p3(960.0, 760.0)), (2.0, p3(960.0, 720.0))]);
    expose(s, brand, "text/sourceText", "Brand Name")?;
    expose(s, logo, "contents/group#1/contents/fill/color", "Logo Color")?;
    finish(s, "Logo Reveal", 3.0)
}

fn kinetic_type(s: &mut Session) -> Result<()> {
    comp(s, "Kinetic Type", 1920, 1080, 4.0, "#111111")?;
    let bg = solid(s, "Background", "#F2EDE4")?;
    let _ = bg;
    let words = [("MAKE", 380.0, "#111111", 0.2), ("IT", 560.0, "#E8452C", 0.7), ("MOVE", 740.0, "#111111", 1.2)];
    let mut ids = vec![];
    for (w, y, c, t0) in words {
        let l = text(s, w, [960.0, y], 170.0, c)?;
        keys(s, l, "transform/scale", &[(t0, sc(0.0)), (t0 + 0.25, sc(118.0)), (t0 + 0.4, sc(100.0))]);
        ids.push(l);
    }
    for (k, l) in ids.into_iter().enumerate() {
        expose(s, l, "text/sourceText", &format!("Word {}", k + 1))?;
    }
    finish(s, "Kinetic Type", 2.0)
}

fn social_square(s: &mut Session) -> Result<()> {
    comp(s, "Social Square", 1080, 1080, 6.0, "#0E1A2B")?;
    let bg = solid(s, "Background", "#000000")?;
    gradient(s, bg, [0.0, 0.0], [1080.0, 1080.0], "#0F4C5C", "#0B1320", false)?;
    let dot = shape(s, "Accent Circle", "ellipse", [400.0, 400.0], "#FF6B6B", [830.0, 250.0])?;
    keys(s, dot, "transform/scale", &[(0.0, sc(0.0)), (0.8, sc(100.0)), (6.0, sc(112.0))]);
    let head = text(s, "Big news\ndrops today", [540.0, 600.0], 104.0, "#FFFFFF")?;
    keys(s, head, "transform/opacity", &[(0.3, op(0.0)), (1.0, op(100.0))]);
    let handle = text(s, "@yourhandle", [540.0, 930.0], 44.0, "#BFE9F0")?;
    keys(s, handle, "transform/opacity", &[(0.9, op(0.0)), (1.5, op(100.0))]);
    expose(s, head, "text/sourceText", "Headline")?;
    expose(s, handle, "text/sourceText", "Handle")?;
    expose(s, dot, "contents/group#1/contents/fill/color", "Accent Color")?;
    finish(s, "Social Square", 2.0)
}

fn vertical_story(s: &mut Session) -> Result<()> {
    comp(s, "Vertical Story", 1080, 1920, 8.0, "#120E24")?;
    let bg = solid(s, "Background", "#000000")?;
    gradient(s, bg, [540.0, 0.0], [540.0, 1920.0], "#FF8A3D", "#4A1FB8", false)?;
    let card = shape(s, "Card", "rounded", [880.0, 820.0], "#FFFFFF", [540.0, 860.0])?;
    set(s, card, "transform/opacity", op(18.0));
    keys(s, card, "transform/position", &[(0.0, p3(540.0, 1100.0)), (0.8, p3(540.0, 860.0))]);
    let head = text(s, "NEW\nDROP", [540.0, 800.0], 200.0, "#FFFFFF")?;
    keys(s, head, "transform/scale", &[(0.3, sc(80.0)), (1.1, sc(100.0))]);
    keys(s, head, "transform/opacity", &[(0.3, op(0.0)), (0.9, op(100.0))]);
    let cta = text(s, "Swipe up ↑", [540.0, 1640.0], 56.0, "#FFFFFF")?;
    keys(
        s,
        cta,
        "transform/position",
        &[(1.0, p3(540.0, 1640.0)), (1.6, p3(540.0, 1610.0)), (2.2, p3(540.0, 1640.0)), (2.8, p3(540.0, 1610.0)), (3.4, p3(540.0, 1640.0))],
    );
    expose(s, head, "text/sourceText", "Headline")?;
    expose(s, cta, "text/sourceText", "Call to Action")?;
    finish(s, "Vertical Story", 1.5)
}

fn slideshow(s: &mut Session) -> Result<()> {
    comp(s, "Slideshow", 1920, 1080, 9.0, "#000000")?;
    let slides = [
        ("Slide 1 (replace with a photo)", "#3A6EA5", "#0F2238", "First caption"),
        ("Slide 2", "#C0603A", "#3A160A", "Second caption"),
        ("Slide 3", "#4F8A4B", "#13260F", "Third caption"),
    ];
    let mut caps = vec![];
    // Bottom to top: each later slide fades in over the one before.
    for (k, (name, a, b, cap)) in slides.into_iter().enumerate() {
        let t0 = k as f64 * 3.0;
        let l = solid(s, name, "#000000")?;
        gradient(s, l, [300.0, 200.0], [1700.0, 1000.0], a, b, false)?;
        keys(s, l, "transform/scale", &[(t0, sc(100.0)), (t0 + 3.6, sc(112.0))]);
        // A simple placeholder picture: a sun over a hill.
        let sun = shape(s, &format!("Sun {}", k + 1), "ellipse", [260.0, 260.0], "#FFE7A8", [1380.0 - 300.0 * k as f64, 330.0])?;
        let hill = shape(s, &format!("Hill {}", k + 1), "ellipse", [2600.0, 900.0], b, [700.0 + 400.0 * k as f64, 1150.0])?;
        keys(s, sun, "transform/position", &[(t0, p3(1380.0 - 300.0 * k as f64, 330.0)), (t0 + 3.6, p3(1420.0 - 300.0 * k as f64, 300.0))]);
        if k > 0 {
            for x in [l, sun, hill] {
                keys(s, x, "transform/opacity", &[(t0 - 0.6, op(0.0)), (t0, op(100.0))]);
            }
        }
        let c = text(s, cap, [960.0, 960.0], 54.0, "#FFFFFF")?;
        let fade_in = if k == 0 { 0.2 } else { t0 };
        keys(s, c, "transform/opacity", &[(fade_in, op(0.0)), (fade_in + 0.5, op(100.0)), (t0 + 2.5, op(100.0)), (t0 + 2.9, op(0.0))]);
        caps.push(c);
    }
    for (k, c) in caps.into_iter().enumerate() {
        expose(s, c, "text/sourceText", &format!("Caption {}", k + 1))?;
    }
    finish(s, "Slideshow", 1.5)
}

fn text_orbit(s: &mut Session) -> Result<()> {
    comp(s, "3D Text Orbit", 1920, 1080, 8.0, "#070910")?;
    let floor = shape(s, "Floor", "ellipse", [1000.0, 1000.0], "#1C2740", [960.0, 760.0])?;
    ex(s, "layer.setSwitch", json!({"layers": [floor.0], "switch": "threeD", "value": true}))?;
    set(s, floor, "transform/position", KV::Vec3([960.0, 760.0, 0.0]));
    set(s, floor, "transform/rotationX", op(90.0));
    let title = text(s, "ORBIT", [960.0, 520.0], 220.0, "#FFFFFF")?;
    ex(s, "layer.setSwitch", json!({"layers": [title.0], "switch": "threeD", "value": true}))?;
    let sub = text(s, "in three dimensions", [960.0, 640.0], 48.0, "#8FB8FF")?;
    ex(s, "layer.setSwitch", json!({"layers": [sub.0], "switch": "threeD", "value": true}))?;
    set(s, sub, "transform/position", KV::Vec3([960.0, 640.0, -120.0]));
    let cam = layer_of(&ex(s, "layer.newCamera", json!({"name": "Orbit Camera", "type": "twoNode", "preset": "35mm", "poi": [960.0, 560.0, 0.0]}))?)?;
    // A circle around the title (radius 1500) in eight eased steps.
    let n = 8;
    let list: Vec<(f64, KV)> = (0..=n)
        .map(|i| {
            let a = (-50.0 + 100.0 * i as f64 / n as f64).to_radians();
            (8.0 * i as f64 / n as f64, KV::Vec3([960.0 + 1500.0 * a.sin(), 380.0, -1500.0 * a.cos()]))
        })
        .collect();
    let rate = s.active_comp().map(|c| c.frame_rate).unwrap_or(effectcraft_time::FrameRate::FPS_30);
    edit_layer(s, cam, |ly| {
        if let Some(p) = ly.props.prop_mut("transform/position") {
            p.keys = list.iter().map(|(t, v)| Keyframe::new(rate.snap_nearest(Tick::from_seconds_f64(*t)), v.clone())).collect();
        }
        if let Some(p) = ly.props.prop_mut("transform/poi") {
            p.value = KV::Vec3([960.0, 560.0, 0.0]);
        }
    });
    expose(s, title, "text/sourceText", "Title")?;
    finish(s, "3D Text Orbit", 3.0)
}

// ------------------------------------------------------------------------- built-in access

fn builtin(id: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.id == id)
}

/// Build a built-in template's project (the main comp is the one Essential Graphics names).
pub fn build_builtin(id: &str) -> Result<Project> {
    let b = builtin(id).ok_or_else(|| bad("templates.create", format!("unknown template `{id}` (see templates.list)")))?;
    let mut s = Session::default();
    (b.build)(&mut s).map_err(|e| EngineError::Other(format!("template {id}: {e}")))?;
    let mut p = (*s.project).clone();
    p.fix_next_id();
    Ok(p)
}

/// The main composition: the one with Essential Graphics controls, else the first comp.
pub fn main_comp(p: &Project) -> Option<ItemId> {
    p.comps().find(|(_, c)| c.essential.as_ref().is_some_and(|e| !e.controls.is_empty())).or_else(|| p.comps().next()).map(|(id, _)| *id)
}

fn controls_of(p: &Project, cid: Option<ItemId>) -> Vec<String> {
    cid.and_then(|c| p.comp(c)).and_then(|c| c.essential.as_ref()).map(|e| e.flat().iter().map(|c| c.name.clone()).collect()).unwrap_or_default()
}

fn info_from(id: String, name: String, description: String, category: String, builtin: bool, p: &Project) -> TemplateInfo {
    let cid = main_comp(p);
    let c = cid.and_then(|c| p.comp(c));
    TemplateInfo {
        id,
        name,
        description,
        category,
        builtin,
        width: c.map(|c| c.width).unwrap_or(0),
        height: c.map(|c| c.height).unwrap_or(0),
        frame_rate: c.map(|c| c.frame_rate.as_f64()).unwrap_or(0.0),
        duration: c.map(|c| c.duration.seconds()).unwrap_or(0.0),
        controls: controls_of(p, cid),
    }
}

/// Built projects (and thumbnails) are cached for the process: they never change.
type Cache = Mutex<std::collections::HashMap<String, (Project, Option<String>)>>;
fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn cached_builtin(id: &str) -> Result<Project> {
    if let Some((p, _)) = cache().lock().ok().and_then(|c| c.get(id).cloned()) {
        return Ok(p);
    }
    let p = build_builtin(id)?;
    if let Ok(mut c) = cache().lock() {
        c.insert(id.to_string(), (p.clone(), None));
    }
    Ok(p)
}

// ------------------------------------------------------------------------- thumbnails

/// Letterbox RGBA pixels into a THUMB_W×THUMB_H RGB thumbnail, as hex text (`thumb.txt`).
pub fn encode_thumb(w: u32, h: u32, rgba: &[u8]) -> String {
    let (w, h) = (w as usize, h as usize);
    let k = (THUMB_W as f32 / w.max(1) as f32).min(THUMB_H as f32 / h.max(1) as f32);
    let (dw, dh) = (w as f32 * k, h as f32 * k);
    let (ox, oy) = ((THUMB_W as f32 - dw) / 2.0, (THUMB_H as f32 - dh) / 2.0);
    let mut out = String::with_capacity(THUMB_W * THUMB_H * 6);
    for y in 0..THUMB_H {
        for x in 0..THUMB_W {
            let (fx, fy) = ((x as f32 + 0.5 - ox) / k, (y as f32 + 0.5 - oy) / k);
            let c = if fx >= 0.0 && fy >= 0.0 && (fx as usize) < w && (fy as usize) < h {
                let i = (fy as usize * w + fx as usize) * 4;
                [rgba[i], rgba[i + 1], rgba[i + 2]]
            } else {
                [0x16, 0x17, 0x1c]
            };
            for v in c {
                out.push_str(&format!("{v:02x}"));
            }
        }
    }
    out
}

/// Decode a thumbnail to RGB bytes (THUMB_W×THUMB_H×3).
pub fn decode_thumb(text: &str) -> Option<Vec<u8>> {
    let t = text.trim();
    if t.len() != THUMB_W * THUMB_H * 6 || !t.is_ascii() {
        return None;
    }
    (0..t.len()).step_by(2).map(|i| u8::from_str_radix(t.get(i..i + 2)?, 16).ok()).collect()
}

/// Render a project's main comp at its poster time into a thumbnail.
pub fn render_thumb(p: &Project) -> Option<String> {
    let cid = main_comp(p)?;
    let t = p.comp(cid)?.poster_time;
    let mut s = Session::default();
    s.replace_project(p.clone(), None);
    let (w, h, rgba) = s.render_rgba8(cid, t, (THUMB_W * 2) as u32).ok()?;
    Some(encode_thumb(w, h, &rgba))
}

/// A template's thumbnail (hex RGB, THUMB_W×THUMB_H); built-in ones are rendered on first use.
pub fn thumbnail(s: &Session, id: &str) -> Result<String> {
    if let Some(name) = id.strip_prefix("user/") {
        let bytes = read_user(s, name)?;
        let entries = effectcraft_lottie::zip::read_stored(&bytes);
        if let Some((_, t)) = entries.iter().find(|(k, _)| k == "thumb.txt") {
            return Ok(String::from_utf8_lossy(t).into_owned());
        }
        let (_, p, _) = read_template(&bytes).map_err(|e| bad("templates.thumbnail", e))?;
        return render_thumb(&p).ok_or_else(|| bad("templates.thumbnail", "nothing to render"));
    }
    if let Some(Some(t)) = cache().lock().ok().and_then(|c| c.get(id).map(|e| e.1.clone())) {
        return Ok(t);
    }
    let p = cached_builtin(id)?;
    let t = render_thumb(&p).ok_or_else(|| bad("templates.thumbnail", "nothing to render"))?;
    if let Ok(mut c) = cache().lock() {
        c.insert(id.to_string(), (p, Some(t.clone())));
    }
    Ok(t)
}

// ------------------------------------------------------------------------- user templates

/// Where user templates are written: `<config dir>/Templates` (None: in the store itself).
fn user_dir(s: &Session) -> Option<PathBuf> {
    s.config.as_ref()?.dir().map(|d| d.join(TEMPLATES_DIR))
}

/// File names of the user templates (`*.ectemplate`), sorted.
pub fn user_files(s: &Session) -> Vec<String> {
    let Some(cfg) = &s.config else { return vec![] };
    let mut v: Vec<String> = match user_dir(s) {
        Some(d) => cfg.files().list(&d).into_iter().map(|(n, _)| n).collect(),
        None => cfg.list(TEMPLATES_DIR),
    };
    v.retain(|n| n.to_ascii_lowercase().ends_with(&format!(".{EXT}")));
    v.sort();
    v
}

fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(bad("templates", format!("bad template file name `{name}`")));
    }
    Ok(())
}

fn read_user(s: &Session, file: &str) -> Result<Vec<u8>> {
    check_name(file)?;
    let cfg = s.config.as_ref().ok_or_else(|| bad("templates", "no settings store: user templates are unavailable"))?;
    match user_dir(s) {
        Some(d) => s.services.read_file(&d.join(file).to_string_lossy()).map_err(|e| EngineError::Other(format!("cannot read template {file}: {e}"))),
        None => cfg
            .read(&format!("{TEMPLATES_DIR}/{file}"))
            .and_then(|t| effectcraft_track::roto::rle::base64_decode(&t))
            .ok_or_else(|| bad("templates", format!("no user template `{file}`"))),
    }
}

fn write_user(s: &Session, file: &str, bytes: &[u8]) -> Result<String> {
    check_name(file)?;
    let cfg = s.config.as_ref().ok_or_else(|| bad("templates.saveAs", "no settings store: user templates are unavailable"))?;
    match user_dir(s) {
        Some(d) => {
            let path = d.join(file);
            cfg.files().write(&path, bytes).map_err(|e| EngineError::Other(format!("cannot write {}: {e}", path.display())))?;
            Ok(path.to_string_lossy().into_owned())
        }
        None => {
            let name = format!("{TEMPLATES_DIR}/{file}");
            cfg.write(&name, &effectcraft_track::roto::rle::base64_encode(bytes)).map_err(|e| EngineError::Other(format!("cannot write {name}: {e}")))?;
            Ok(name)
        }
    }
}

fn remove_user(s: &Session, file: &str) -> Result<()> {
    check_name(file)?;
    let cfg = s.config.as_ref().ok_or_else(|| bad("templates.delete", "no settings store"))?;
    if !user_files(s).iter().any(|f| f == file) {
        return Err(bad("templates.delete", format!("no user template `{file}`")));
    }
    match user_dir(s) {
        Some(d) => cfg.files().remove(&d.join(file)),
        None => cfg.remove(&format!("{TEMPLATES_DIR}/{file}")),
    }
    .map_err(|e| EngineError::Other(format!("cannot delete {file}: {e}")))
}

/// A safe file name for a template name.
pub fn file_name_for(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_alphanumeric() || " -_()".contains(c) { c } else { '_' }).collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    format!("{}.{EXT}", if clean.is_empty() { "Template".into() } else { clean })
}

fn str_of(m: &Value, k: &str) -> String {
    m.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// Every template: built-in ones first, then the user's.
pub fn list(s: &Session) -> Vec<TemplateInfo> {
    let mut v = vec![];
    for b in BUILTINS {
        if let Ok(p) = cached_builtin(b.id) {
            v.push(info_from(b.id.into(), b.name.into(), b.description.into(), b.category.into(), true, &p));
        }
    }
    for f in user_files(s) {
        let Ok(bytes) = read_user(s, &f) else { continue };
        let Ok((m, p, _)) = read_template(&bytes) else { continue };
        let stem = f.strip_suffix(&format!(".{EXT}")).unwrap_or(&f).to_string();
        let name = Some(str_of(&m, "name")).filter(|n| !n.is_empty()).unwrap_or(stem);
        let cat = Some(str_of(&m, "category")).filter(|n| !n.is_empty()).unwrap_or_else(|| "My Templates".into());
        v.push(info_from(format!("user/{f}"), name, str_of(&m, "description"), cat, false, &p));
    }
    v
}

/// A template's project, its name and its archive entries (embedded footage), by id or by
/// `.ectemplate` file.
type Opened = (Project, String, Vec<(String, Vec<u8>)>);

fn project_of(s: &Session, id: Option<&str>, path: Option<&str>) -> Result<Opened> {
    if let Some(path) = path {
        let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
        let (m, p, e) = read_template(&bytes).map_err(|e| bad("templates.create", e))?;
        return Ok((p, str_of(&m, "name"), e));
    }
    let id = id.ok_or_else(|| bad("templates.create", "missing `id` (see templates.list) or `path`"))?;
    if let Some(f) = id.strip_prefix("user/") {
        let (m, p, e) = read_template(&read_user(s, f)?).map_err(|e| bad("templates.create", e))?;
        return Ok((p, str_of(&m, "name"), e));
    }
    let b = builtin(id).ok_or_else(|| bad("templates.create", format!("unknown template `{id}` (see templates.list)")))?;
    Ok((cached_builtin(id)?, b.name.to_string(), vec![]))
}

fn base_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or("footage").to_string()
}

/// What [`embed_footage`] did.
#[derive(Default)]
struct Embedded {
    /// `{item, files, bytes}` per embedded footage item (the manifest's `media`).
    media: Vec<Value>,
    bytes: u64,
    /// Footage left referenced: `{item, name, bytes?, reason}`.
    skipped: Vec<Value>,
}

/// Embed the footage files `proj` reads as `media/<item>/<file>` entries (paths rewritten to
/// the entries), until `limit` bytes; the rest stays referenced.
fn embed_footage(s: &Session, proj: &mut Project, entries: &mut Vec<(String, Vec<u8>)>, limit: u64) -> Embedded {
    let mut out = Embedded::default();
    let mut ids: Vec<ItemId> = proj.items.keys().copied().collect();
    ids.sort_by_key(|i| i.0);
    for id in ids {
        let Some(it) = proj.items.get_mut(&id) else { continue };
        let name = it.name.clone();
        let effectcraft_project::ItemKind::Footage(f) = &mut it.kind else { continue };
        if f.data.is_some() || f.path.is_empty() {
            continue;
        }
        let files: Vec<String> = if f.sequence.is_empty() { vec![f.path.clone()] } else { f.sequence.clone() };
        let mut data = vec![];
        let mut missing = None;
        for file in &files {
            match s.services.read_file(file) {
                Ok(b) => data.push((file.clone(), b)),
                Err(e) => {
                    missing = Some(format!("{file}: {e}"));
                    break;
                }
            }
        }
        if let Some(reason) = missing {
            out.skipped.push(json!({"item": id.0, "name": name, "reason": reason}));
            continue;
        }
        let size: u64 = data.iter().map(|(_, b)| b.len() as u64).sum();
        if out.bytes + size > limit {
            out.skipped.push(json!({"item": id.0, "name": name, "bytes": size, "reason": "over the size limit"}));
            continue;
        }
        let mut stored = vec![];
        for (file, bytes) in data {
            let entry = format!("media/{}/{}", id.0, base_name(&file));
            entries.push((entry.clone(), bytes));
            stored.push(entry);
        }
        f.path = stored[0].clone();
        if !f.sequence.is_empty() {
            f.sequence = stored.clone();
        }
        out.bytes += size;
        out.media.push(json!({"item": id.0, "files": stored, "bytes": size}));
    }
    out
}

/// Where footage extracted from template `name` goes (see the module docs).
fn footage_dir(s: &Session, name: &str, footage_dir: Option<&str>, project_path: Option<&str>) -> String {
    if let Some(d) = footage_dir.filter(|d| !d.is_empty()) {
        return d.trim_end_matches(['/', '\\']).to_string();
    }
    if let Some(pp) = project_path.map(std::path::Path::new) {
        let stem = pp.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Project".into());
        return pp.parent().unwrap_or(std::path::Path::new("")).join(format!("{stem} Footage")).to_string_lossy().into_owned();
    }
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || " -_()".contains(c) { c } else { '_' }).collect();
    let safe = if safe.trim().is_empty() { "Template".to_string() } else { safe.trim().to_string() };
    let base = s.config.as_ref().and_then(|c| c.dir()).map(|d| d.join(FOOTAGE_DIR));
    // A fresh folder per created project.
    let mut k = 1;
    loop {
        let folder = if k == 1 { safe.clone() } else { format!("{safe} {k}") };
        // Virtual storage keys use '/' independently of the host OS. Native settings
        // folders retain their platform path semantics.
        let ds = base.as_ref().map(|d| d.join(&folder).to_string_lossy().into_owned()).unwrap_or_else(|| format!("/{FOOTAGE_DIR}/{folder}"));
        if !s.services.exists(&ds) || k > 999 {
            return ds;
        }
        k += 1;
    }
}

/// Write a template's embedded footage (`media/…` entries) into `dir` and point the project's
/// footage at the files. Returns the files written.
fn extract_footage(s: &Session, proj: &mut Project, entries: &[(String, Vec<u8>)], dir: &str) -> Result<Vec<String>> {
    let mut written = std::collections::HashMap::new();
    let mut used = std::collections::HashSet::new();
    for (k, v) in entries.iter().filter(|(k, _)| k.starts_with("media/")) {
        // `media/<item>/<file>` → one folder; a name another item took gets the item's id.
        let file = base_name(k);
        let item = k.trim_start_matches("media/").split('/').next().unwrap_or("0");
        let name = if used.insert(file.clone()) { file } else { format!("{item}-{file}") };
        let out = format!("{dir}/{name}");
        s.services.store_file(&out, v).map_err(|e| EngineError::Other(format!("cannot write {out}: {e}")))?;
        if let Some(imp) = &s.importer {
            imp.register(&out, v);
        }
        written.insert(k.clone(), out);
    }
    for it in proj.items.values_mut() {
        if let effectcraft_project::ItemKind::Footage(f) = &mut it.kind {
            if let Some(w) = written.get(&f.path) {
                f.path = w.clone();
            }
            for x in &mut f.sequence {
                if let Some(w) = written.get(x) {
                    *x = w.clone();
                }
            }
        }
    }
    let mut v: Vec<String> = written.into_values().collect();
    v.sort();
    Ok(v)
}

// ------------------------------------------------------------------------- commands

fn list_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let with_thumbs = p.get("thumbnails").and_then(Value::as_bool).unwrap_or(false);
    let mut out = vec![];
    for t in list(s) {
        let mut j = serde_json::to_value(&t).unwrap_or_default();
        if with_thumbs {
            j["thumbnail"] = json!(thumbnail(s, &t.id).ok());
        }
        out.push(j);
    }
    Ok(json!(out))
}

fn thumb_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_p(p, "id").ok_or_else(|| bad("templates.thumbnail", "missing `id`"))?;
    Ok(json!({"id": id, "width": THUMB_W, "height": THUMB_H, "rgb": thumbnail(s, id)?}))
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut proj, name, entries) = project_of(s, str_p(p, "id"), str_p(p, "path"))?;
    let project_path = str_p(p, "projectPath").filter(|p| !p.is_empty());
    let mut footage = vec![];
    if entries.iter().any(|(k, _)| k.starts_with("media/")) {
        let dir = footage_dir(s, &name, str_p(p, "footageDir"), project_path);
        footage = extract_footage(s, &mut proj, &entries, &dir)?;
    }
    let cid = main_comp(&proj);
    let poster = cid.and_then(|c| proj.comp(c)).map(|c| c.poster_time).unwrap_or(Tick::ZERO);
    s.replace_project(proj, None);
    s.update_sentinel();
    if let Some(c) = cid {
        s.open_comp(c);
        s.set_time(poster);
    }
    let mut untitled = true;
    if let Some(pp) = project_path {
        s.execute("file.saveAs", json!({"path": pp}))?;
        untitled = false;
    }
    s.toast(format!("New project from template “{name}”"));
    Ok(json!({"template": name, "comp": cid.map(|c| c.0), "untitled": untitled, "footage": footage}))
}

fn save_as(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "templates.saveAs";
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "missing `name`"))?.to_string();
    if s.project.comps().next().is_none() {
        return Err(bad(C, "the project has no compositions"));
    }
    let file = file_name_for(&name);
    let overwrite = p.get("overwrite").and_then(Value::as_bool).unwrap_or(true);
    if !overwrite && user_files(s).contains(&file) {
        return Err(bad(C, format!("a template named “{name}” already exists")));
    }
    let cid = s.active_comp_id().or_else(|| main_comp(&s.project)).ok_or(EngineError::NoComp)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    // The active comp becomes the template's main comp: give it the poster time it shows now.
    let mut proj = (*s.project).clone();
    if let Some(c) = proj.comp_mut(cid)
        && c.poster_time == Tick::ZERO
    {
        c.poster_time = s.time_of(cid);
    }
    let comp_name = proj.item(cid).map(|i| i.name.clone()).unwrap_or_default();
    let manifest = json!({
        "format": TEMPLATE_FORMAT,
        "version": TEMPLATE_VERSION,
        "kind": "project",
        "generator": format!("EffectCraft {}", env!("CARGO_PKG_VERSION")),
        "name": name,
        "description": str_p(p, "description").unwrap_or_default(),
        "category": str_p(p, "category").filter(|c| !c.is_empty()).unwrap_or("My Templates"),
        "comp": cid.0,
        "compName": comp_name,
        "width": comp.width,
        "height": comp.height,
        "frameRate": comp.frame_rate.as_f64(),
        "duration": comp.duration.seconds(),
        "controls": controls_of(&proj, Some(cid)),
        "poster": "poster.png",
        "thumbnail": "thumb.txt",
    });
    let mut manifest = manifest;
    // Embedded footage (default on, capped): the rest stays referenced, with a warning.
    let mut media_entries = vec![];
    let mut warning = None;
    if p.get("embedFootage").and_then(Value::as_bool).unwrap_or(true) {
        let limit_mb = p.get("embedLimitMB").and_then(Value::as_f64).unwrap_or(EMBED_LIMIT_MB).max(0.0);
        let e = embed_footage(s, &mut proj, &mut media_entries, (limit_mb * 1024.0 * 1024.0) as u64);
        let over: Vec<&Value> = e.skipped.iter().filter(|v| v["reason"] == "over the size limit").collect();
        if !over.is_empty() {
            let mb = over.iter().filter_map(|v| v["bytes"].as_u64()).sum::<u64>() as f64 / 1048576.0;
            warning = Some(format!("{} footage item(s) ({mb:.1} MB) are over the {limit_mb:.0} MB embedding limit and stay linked to their files", over.len()));
        }
        manifest["media"] = json!(e.media);
        manifest["embeddedBytes"] = json!(e.bytes);
        manifest["linkedFootage"] = json!(e.skipped);
    }
    let t = s.time_of(cid);
    let mut entries: Vec<(String, Vec<u8>)> =
        vec![("manifest.json".into(), serde_json::to_vec_pretty(&manifest).unwrap_or_default()), ("project.ecproj".into(), proj.to_json().into_bytes())];
    entries.extend(media_entries);
    if let Ok((w, h, rgba)) = s.render_rgba8(cid, t, 320) {
        if let Ok(png) = crate::commands::comp_more::encode_png(&rgba, w, h) {
            entries.push(("poster.png".into(), png));
        }
        entries.push(("thumb.txt".into(), encode_thumb(w, h, &rgba).into_bytes()));
    }
    let bytes = effectcraft_lottie::zip::store(&entries);
    let at = write_user(s, &file, &bytes)?;
    match &warning {
        Some(w) => s.events.push(crate::Event::Toast { message: format!("Saved template “{name}”: {w}"), error: false }),
        None => s.toast(format!("Saved template “{name}”")),
    }
    Ok(json!({
        "id": format!("user/{file}"),
        "name": name,
        "path": at,
        "bytes": bytes.len(),
        "embedded": manifest["media"].as_array().map_or(0, |m| m.len()),
        "embeddedBytes": manifest["embeddedBytes"],
        "linked": manifest["linkedFootage"],
        "warning": warning,
    }))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_p(p, "id").ok_or_else(|| bad("templates.delete", "missing `id`"))?;
    let file = id.strip_prefix("user/").ok_or_else(|| bad("templates.delete", "only user templates (`user/…`) can be deleted"))?;
    remove_user(s, file)?;
    Ok(json!({"deleted": id}))
}

fn has_comps(s: &Session) -> std::result::Result<(), String> {
    if s.project.comps().next().is_some() { Ok(()) } else { Err("the project has no compositions".into()) }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        query!(
            "templates.list",
            "Project Templates",
            "{thumbnails?: bool} → [{id, name, description, category, builtin, width, height, frameRate, duration, controls, thumbnail? (hex RGB 256×144)}]",
            list_cmd
        ),
        query!("templates.thumbnail", "Template Thumbnail", "{id} → {id, width, height, rgb (hex)}", thumb_cmd),
        cmd!(
            "templates.create",
            "New Project from Template",
            [],
            None,
            "{id (templates.list) | path (.ectemplate), projectPath? (save the new project there; embedded footage goes to `<stem> Footage/` next to it), footageDir?} → an untitled copy (embedded footage extracted: `footage` lists the files)",
            always,
            create
        ),
        cmd!(
            "templates.saveAs",
            "Save as Template...",
            ["File"],
            None,
            "{name, description?, category?, overwrite?: bool (default true), embedFootage?: bool (default true: the footage files go into the template), embedLimitMB? (default 256: footage beyond it stays linked, with a warning)} → {id, path, embedded, embeddedBytes, linked: [{item, name, reason}], warning?}",
            has_comps,
            save_as
        ),
        cmd!("templates.delete", "Delete Template", [], None, "{id: user/<file>}", always, delete),
        cmd!("file.newFromTemplate", "New Project from Template...", ["File", "New"], None, "{} → the Home screen's Templates tab", always, |s, p| {
            crate::commands::frontend(s, "file.newFromTemplate", p)
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DirConfig, MemoryConfig};

    #[test]
    fn non_ascii_thumbnail_is_rejected_without_panicking() {
        let malformed = format!("aé{}", "0".repeat(THUMB_W * THUMB_H * 6 - 3));
        assert!(decode_thumb(&malformed).is_none());
    }

    #[test]
    fn every_builtin_builds_with_controls_and_renders() {
        let s = Session::default();
        let l = list(&s);
        assert_eq!(l.len(), BUILTINS.len());
        assert!(l.len() >= 8);
        for t in &l {
            assert!(t.builtin && !t.controls.is_empty(), "{t:?}");
            assert!(t.width > 0 && t.duration > 0.0, "{t:?}");
            let th = decode_thumb(&thumbnail(&s, &t.id).unwrap()).unwrap();
            // Not a blank frame: some variety in the pixels.
            let distinct: std::collections::BTreeSet<[u8; 3]> = th.as_chunks::<3>().0.iter().map(|c| [c[0] / 16, c[1] / 16, c[2] / 16]).collect();
            assert!(distinct.len() > 3, "{}: thumbnail looks blank", t.id);
        }
        let sq = l.iter().find(|t| t.id == "social-square").unwrap();
        assert_eq!((sq.width, sq.height), (1080, 1080));
        let v = l.iter().find(|t| t.id == "vertical-story").unwrap();
        assert_eq!((v.width, v.height), (1080, 1920));
        assert_eq!(l.iter().find(|t| t.id == "lower-third").unwrap().controls, ["Name", "Title", "Bar Color"]);
    }

    #[test]
    fn create_opens_an_untitled_copy() {
        let mut s = Session::default();
        let r = s.execute("templates.create", json!({"id": "title-card"})).unwrap();
        assert_eq!(r["template"], "Title Card");
        assert!(s.path.is_none());
        let cid = s.active_comp_id().unwrap();
        assert_eq!(Some(cid.0), r["comp"].as_u64());
        assert!(s.time() > Tick::ZERO);
        // The Essential Graphics controls came along and drive the title.
        let eg = s.execute("essential.list", json!({})).unwrap();
        assert_eq!(eg["controls"][0]["name"], "Title");
        assert!(s.execute("templates.create", json!({"id": "nope"})).is_err());
        assert!(s.execute("templates.create", json!({})).is_err());
    }

    fn user_roundtrip(mut s: Session) {
        s.execute("templates.create", json!({"id": "kinetic-type"})).unwrap();
        s.execute("layer.newSolid", json!({"name": "Mine", "color": "#ff00ff"})).unwrap();
        let r = s.execute("templates.saveAs", json!({"name": "My Promo", "description": "Mine", "category": "Work"})).unwrap();
        assert_eq!(r["id"], "user/My Promo.ectemplate");
        let l = list(&s);
        let mine = l.iter().find(|t| !t.builtin).unwrap();
        assert_eq!((mine.name.as_str(), mine.category.as_str(), mine.description.as_str()), ("My Promo", "Work", "Mine"));
        assert_eq!(mine.controls, ["Word 1", "Word 2", "Word 3"]);
        assert!(decode_thumb(&thumbnail(&s, &mine.id).unwrap()).is_some());
        let j = s.execute("templates.list", json!({"thumbnails": true})).unwrap();
        assert!(j.as_array().unwrap().iter().all(|t| t["thumbnail"].is_string()));
        // Overwrite protection.
        assert!(s.execute("templates.saveAs", json!({"name": "My Promo", "overwrite": false})).is_err());
        // Open: an untitled copy with the added layer.
        s.execute("file.newProject", json!({})).unwrap();
        s.execute("templates.create", json!({"id": mine.id})).unwrap();
        assert!(s.path.is_none());
        assert!(s.active_comp().unwrap().layers.iter().any(|l| l.name == "Mine"));
        // Built-ins can't be deleted; user ones can.
        assert!(s.execute("templates.delete", json!({"id": "title-card"})).is_err());
        s.execute("templates.delete", json!({"id": mine.id})).unwrap();
        assert!(list(&s).iter().all(|t| t.builtin));
        assert!(s.execute("templates.delete", json!({"id": "user/My Promo.ectemplate"})).is_err());
        assert!(s.execute("templates.create", json!({"id": "user/../x.ectemplate"})).is_err());
    }

    #[test]
    fn user_templates_in_a_memory_store() {
        let s = Session { config: Some(Arc::new(MemoryConfig::default())), ..Default::default() };
        user_roundtrip(s);
    }

    #[test]
    fn user_templates_in_the_config_folder() {
        let dir = std::env::temp_dir().join(format!("ec-templates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Session { config: Some(Arc::new(DirConfig::new(&dir))), ..Default::default() };
        s.execute("templates.create", json!({"id": "lower-third"})).unwrap();
        s.execute("templates.saveAs", json!({"name": "On Disk"})).unwrap();
        let f = dir.join(TEMPLATES_DIR).join("On Disk.ectemplate");
        // A real template file: the Essential Graphics reader understands it too.
        let info = s.execute("essential.templateInfo", json!({"path": f.to_string_lossy()})).unwrap();
        assert_eq!((info["kind"].as_str(), info["name"].as_str()), (Some("project"), Some("On Disk")));
        s.execute("templates.create", json!({"path": f.to_string_lossy()})).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let dir2 = std::env::temp_dir().join(format!("ec-templates-b-{}", std::process::id()));
        let s = Session { config: Some(Arc::new(DirConfig::new(&dir2))), ..Default::default() };
        user_roundtrip(s);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    /// Writes every built-in template's poster frame to target/test-out/templates for review.
    #[test]
    #[ignore]
    fn dump_posters() {
        let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/templates");
        std::fs::create_dir_all(&d).unwrap();
        for b in BUILTINS {
            let p = build_builtin(b.id).unwrap();
            let cid = main_comp(&p).unwrap();
            let t = p.comp(cid).unwrap().poster_time;
            let mut s = Session::default();
            s.replace_project(p, None);
            let (w, h, rgba) = s.render_rgba8(cid, t, 640).unwrap();
            let png = crate::commands::comp_more::encode_png(&rgba, w, h).unwrap();
            std::fs::write(d.join(format!("{}.png", b.id)), png).unwrap();
        }
    }

    fn add_footage(s: &mut Session, name: &str, path: &str, sequence: Vec<String>) -> ItemId {
        let f = effectcraft_project::Footage { path: path.into(), width: 4, height: 4, has_video: true, sequence, ..Default::default() };
        Arc::make_mut(&mut s.project).add_item(name, effectcraft_color::Label::Aqua, None, effectcraft_project::ItemKind::Footage(f))
    }

    fn footage_paths(s: &Session) -> Vec<(String, String, Vec<String>)> {
        let mut v: Vec<_> = s
            .project
            .items
            .values()
            .filter_map(|i| match &i.kind {
                effectcraft_project::ItemKind::Footage(f) => Some((i.name.clone(), f.path.clone(), f.sequence.clone())),
                _ => None,
            })
            .collect();
        v.sort();
        v
    }

    /// Saving embeds the footage (capped, with a warning); creating extracts it next to the
    /// new project, or into the settings folder, and points the footage at the copies.
    #[test]
    fn templates_embed_and_extract_footage() {
        let root = std::env::temp_dir().join(format!("ec-tpl-footage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let media = root.join("media");
        std::fs::create_dir_all(&media).unwrap();
        let file = |n: &str, len: usize, b: u8| {
            let p = media.join(n);
            std::fs::write(&p, vec![b; len]).unwrap();
            p.to_string_lossy().into_owned()
        };
        let still = file("still.png", 1000, 1);
        let seq: Vec<String> = (0..3).map(|k| file(&format!("shot_{k:03}.png"), 500, 10 + k as u8)).collect();
        let big = file("big.mov", 2 * 1024 * 1024, 7);
        let mut s = Session { config: Some(Arc::new(DirConfig::new(root.join("config")))), ..Default::default() };
        s.execute("templates.create", json!({"id": "lower-third"})).unwrap();
        add_footage(&mut s, "Still", &still, vec![]);
        add_footage(&mut s, "Shot", &seq[0], seq.clone());
        add_footage(&mut s, "Big", &big, vec![]);
        add_footage(&mut s, "Gone", &media.join("gone.png").to_string_lossy(), vec![]);
        let r = s.execute("templates.saveAs", json!({"name": "With Footage", "embedLimitMB": 1})).unwrap();
        assert_eq!(r["embedded"], 2, "{r}");
        assert_eq!(r["embeddedBytes"], 2500);
        let linked = r["linked"].as_array().unwrap();
        assert_eq!(linked.len(), 2, "{r}");
        let warning = r["warning"].as_str().unwrap();
        assert!(warning.contains("1 footage item") && warning.contains("1 MB"), "{warning}");
        assert!(s.events.iter().any(|e| matches!(e, crate::Event::Toast { message, .. } if message.contains("embedding limit"))));
        // The archive holds the files under media/<item>/.
        let bytes = read_user(&s, "With Footage.ectemplate").unwrap();
        let entries = effectcraft_lottie::zip::read_stored(&bytes);
        let names: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).filter(|k| k.starts_with("media/")).collect();
        assert_eq!(names.len(), 4, "{names:?}");
        assert!(names.iter().any(|n| n.ends_with("/still.png")) && names.iter().any(|n| n.ends_with("/shot_002.png")));
        // The originals go away: the template still has them.
        std::fs::remove_file(&still).unwrap();
        for f in &seq {
            std::fs::remove_file(f).unwrap();
        }
        let proj_path = root.join("work").join("Promo.ecproj");
        std::fs::create_dir_all(proj_path.parent().unwrap()).unwrap();
        let r = s.execute("templates.create", json!({"id": "user/With Footage.ectemplate", "projectPath": proj_path.to_string_lossy()})).unwrap();
        assert_eq!(r["untitled"], false);
        assert_eq!(r["footage"].as_array().unwrap().len(), 4, "{r}");
        assert_eq!(s.path.as_deref(), Some(&*proj_path.to_string_lossy()));
        assert!(proj_path.exists());
        let dir = root.join("work").join("Promo Footage");
        let fp = footage_paths(&s);
        let get = |n: &str| fp.iter().find(|f| f.0 == n).unwrap().clone();
        assert_eq!(std::path::Path::new(&get("Still").1), dir.join("still.png"));
        assert_eq!(std::fs::read(dir.join("still.png")).unwrap(), vec![1u8; 1000]);
        let shot = get("Shot");
        assert_eq!(shot.2.len(), 3);
        assert_eq!(shot.1, shot.2[0]);
        assert_eq!(std::fs::read(&shot.2[2]).unwrap(), vec![12u8; 500]);
        // Over the cap: still linked to the original file.
        assert_eq!(get("Big").1, big);
        // Without a project path: a fresh folder under the settings folder each time.
        s.execute("templates.create", json!({"id": "user/With Footage.ectemplate"})).unwrap();
        let base = root.join("config").join(FOOTAGE_DIR);
        assert!(base.join("With Footage").join("still.png").exists());
        assert!(s.path.is_none());
        s.execute("templates.create", json!({"id": "user/With Footage.ectemplate"})).unwrap();
        assert!(base.join("With Footage 2").join("still.png").exists());
        // Not embedding: no media entries.
        let r = s.execute("templates.saveAs", json!({"name": "Linked", "embedFootage": false})).unwrap();
        assert_eq!(r["embedded"], 0);
        let entries = effectcraft_lottie::zip::read_stored(&read_user(&s, "Linked.ectemplate").unwrap());
        assert!(entries.iter().all(|(k, _)| !k.starts_with("media/")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The browser: no settings folder; the files go to browser storage quietly
    /// (`Services::store_file`) and the media layer is told about them (`Importer::register`).
    #[test]
    fn template_footage_into_browser_storage() {
        #[derive(Default)]
        struct Mem(Mutex<std::collections::HashMap<String, Vec<u8>>>, Mutex<Vec<String>>);
        struct Svc(Arc<Mem>);
        impl crate::Services for Svc {
            fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
                self.0.0.lock().unwrap().get(path).cloned().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, path.to_string()))
            }
            fn write_file(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
                Err(std::io::Error::other("downloads are not for extracted footage"))
            }
            fn store_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
                self.0.0.lock().unwrap().insert(path.into(), data.to_vec());
                Ok(())
            }
            fn exists(&self, path: &str) -> bool {
                self.0.0.lock().unwrap().keys().any(|k| k == path || k.starts_with(&format!("{path}/")))
            }
        }
        struct Imp(Arc<Mem>);
        impl crate::Importer for Imp {
            fn probe(&self, path: &str) -> std::result::Result<effectcraft_project::Footage, String> {
                Err(path.into())
            }
            fn register(&self, path: &str, _: &[u8]) {
                self.0.1.lock().unwrap().push(path.into());
            }
        }
        let mem = Arc::new(Mem::default());
        mem.0.lock().unwrap().insert("/clip.png".into(), vec![3; 64]);
        let mut s = Session {
            config: Some(Arc::new(MemoryConfig::default())),
            services: Arc::new(Svc(mem.clone())),
            importer: Some(Arc::new(Imp(mem.clone()))),
            ..Default::default()
        };
        s.execute("templates.create", json!({"id": "title-card"})).unwrap();
        add_footage(&mut s, "Clip", "/clip.png", vec![]);
        s.execute("templates.saveAs", json!({"name": "Web"})).unwrap();
        s.execute("templates.create", json!({"id": "user/Web.ectemplate"})).unwrap();
        let want = format!("/{FOOTAGE_DIR}/Web/clip.png");
        assert_eq!(mem.0.lock().unwrap().get(&want), Some(&vec![3; 64]));
        assert_eq!(*mem.1.lock().unwrap(), vec![want.clone()]);
        assert!(footage_paths(&s).iter().any(|f| f.1 == want));
        s.execute("templates.create", json!({"id": "user/Web.ectemplate"})).unwrap();
        assert!(mem.0.lock().unwrap().contains_key(&format!("/{FOOTAGE_DIR}/Web 2/clip.png")));
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name_for("A/B: C"), "A_B_ C.ectemplate");
        assert_eq!(file_name_for(""), "Template.ectemplate");
        assert!(!file_name_for("...").starts_with('.'));
        assert!(check_name("../x").is_err());
    }
}
