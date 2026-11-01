//! The Home screen, laid out like After Effects' (shown over the whole workspace at launch, from
//! the Tools bar's Home button and by the Learn workspace). A left rail holds New Project / Open
//! Project, the Home / Templates / Learn pages and, at its foot, the ArtCraft community links.
//! The Home page welcomes you with quick-start tiles (New Composition, the demo project, Import,
//! New from Template) over the recent projects (File ▸ Open Recent, from Settings): a filterable
//! table with thumbnails and Name / Opened / Size / Kind columns. Templates is the New from
//! Template gallery (`templates`), Learn the guided tutorials (`learn`).
//!
//! Thumbnails: when a project is opened or saved and the viewer has its frame, a 96×54 RGB
//! thumbnail is stored in the config store (`thumb-<hash>.txt`, hex) and shown here.

use std::hash::{Hash, Hasher};

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde::Serialize;
use serde_json::json;

use crate::i18n::{tr, tr_args};
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

pub const THUMB_W: usize = 96;
pub const THUMB_H: usize = 54;

/// One row of the recent projects list.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecentEntry {
    pub index: usize,
    pub path: String,
    pub name: String,
    pub folder: String,
    /// Last modified (`2026-10-02 14:05`, UTC), when the file is readable.
    pub modified: Option<String>,
    /// Last modified, Unix seconds.
    pub modified_secs: Option<i64>,
    /// File size in bytes.
    pub size: Option<u64>,
    pub exists: bool,
}

/// The recent projects, most recent first (Settings ▸ General ▸ recent items).
pub fn recent_entries(prefs: &effectcraft_engine::prefs::Prefs) -> Vec<RecentEntry> {
    prefs
        .recent_projects
        .iter()
        .enumerate()
        .map(|(index, p)| {
            let path = std::path::Path::new(p);
            let meta = std::fs::metadata(path).ok();
            let modified_secs = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
            RecentEntry {
                index,
                path: p.clone(),
                name: path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.clone()),
                folder: path.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default(),
                modified: modified_secs.map(format_utc),
                modified_secs,
                size: meta.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
                exists: meta.is_some(),
            }
        })
        .collect()
}

/// Unix seconds → (proleptic Gregorian year, the same wall-clock instant moved into chrono's range
/// by whole 400-year cycles). The Gregorian calendar repeats every 400 years (146_097 days), so
/// month, day and time of day come from chrono and only the year is carried, which keeps every
/// `i64` exact instead of collapsing out-of-range years to the epoch.
fn utc_parts(secs: i64) -> (i64, chrono::NaiveDateTime) {
    use chrono::Datelike;
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (cycles, rest) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    // `rest + 719_163` (1970-01-01 is CE day 719_163) is always a valid CE day, and `sod` < 86_400.
    let date = i32::try_from(rest + 719_163).ok().and_then(chrono::NaiveDate::from_num_days_from_ce_opt);
    let time = u32::try_from(sod).ok().and_then(|s| chrono::NaiveTime::from_num_seconds_from_midnight_opt(s, 0));
    match (date, time) {
        (Some(d), Some(t)) => {
            let y = i128::from(d.year()) + i128::from(cycles) * 400;
            (i64::try_from(y).unwrap_or(if y < 0 { i64::MIN } else { i64::MAX }), d.and_time(t))
        }
        _ => (1970, chrono::NaiveDateTime::default()), // unreachable
    }
}

/// Unix seconds → `YYYY-MM-DD HH:MM` (UTC). Years far outside chrono's own range are preserved
/// (the old hand-rolled conversion kept them) rather than collapsing to the epoch.
pub fn format_utc(secs: i64) -> String {
    let (y, t) = utc_parts(secs);
    // `{:04}` rather than `%Y`, which writes a `+` before years past 9999.
    format!("{y:04}-{}", t.format("%m-%d %H:%M"))
}

/// How long ago `then` was, as the Home screen's Opened column shows it ("3 hours ago", then the
/// date after a week).
pub fn format_ago(now: i64, then: i64) -> String {
    let d = now.saturating_sub(then);
    let plural = |n: i64, one: &str, many: &str| tr_args(if n == 1 { one } else { many }, &[&n]);
    match d {
        i64::MIN..60 => tr("Just now").to_string(),
        60..3_600 => plural(d / 60, "{} minute ago", "{} minutes ago"),
        3_600..86_400 => plural(d / 3_600, "{} hour ago", "{} hours ago"),
        86_400..172_800 => tr("Yesterday").to_string(),
        172_800..604_800 => plural(d / 86_400, "{} day ago", "{} days ago"),
        // The date part: up to the space, not a fixed 10 bytes, so a year past 9999 stays whole.
        _ => format_utc(then).split(' ').next().unwrap_or_default().to_string(),
    }
}

/// A file size as the Home screen shows it (`12 KB`, `3.4 MB`).
pub fn format_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b < 1024.0 {
        format!("{bytes} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", b / (1024.0 * 1024.0 * 1024.0))
    }
}

/// The Kind column: what a recent file is, by its extension, in the interface language.
fn kind_of(path: &str) -> &'static str {
    tr(match std::path::Path::new(path).extension().and_then(std::ffi::OsStr::to_str).map(str::to_ascii_lowercase).as_deref() {
        Some("ecproj") => "EffectCraft project",
        Some("aep" | "aepx") => "After Effects project",
        Some("json" | "lottie") => "Lottie animation",
        _ => "Project",
    })
}

/// The config-store name of a project's thumbnail.
pub fn thumb_name(path: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    format!("thumb-{:016x}.txt", h.finish())
}

/// Downscale an image to a letterboxed THUMB_W×THUMB_H RGB thumbnail (hex text).
pub fn encode_thumb(img: &egui::ColorImage) -> String {
    let [w, h] = img.size;
    let mut out = String::with_capacity(THUMB_W * THUMB_H * 6);
    let k = (THUMB_W as f32 / w.max(1) as f32).min(THUMB_H as f32 / h.max(1) as f32);
    let (dw, dh) = (w as f32 * k, h as f32 * k);
    let (ox, oy) = ((THUMB_W as f32 - dw) / 2.0, (THUMB_H as f32 - dh) / 2.0);
    for y in 0..THUMB_H {
        for x in 0..THUMB_W {
            let (fx, fy) = ((x as f32 + 0.5 - ox) / k, (y as f32 + 0.5 - oy) / k);
            let c = if fx >= 0.0 && fy >= 0.0 && (fx as usize) < w && (fy as usize) < h {
                let p = img.pixels[fy as usize * w + fx as usize];
                [p.r(), p.g(), p.b()]
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

/// Decode a stored hex thumbnail, rejecting malformed or non-ASCII text.
pub fn decode_thumb(text: &str) -> Option<egui::ColorImage> {
    let t = text.trim();
    if t.len() != THUMB_W * THUMB_H * 6 || !t.is_ascii() {
        return None;
    }
    let b: Vec<u8> = (0..t.len()).step_by(2).map(|i| u8::from_str_radix(t.get(i..i + 2)?, 16).ok()).collect::<Option<_>>()?;
    let px = b.as_chunks::<3>().0.iter().map(|c| Color32::from_rgb(c[0], c[1], c[2])).collect();
    Some(egui::ColorImage::new([THUMB_W, THUMB_H], px))
}

/// Store the viewer's frame as the open project's thumbnail after it is opened or saved (once per
/// saved revision, when the viewer shows that revision).
pub fn capture_thumbnail(app: &mut EffectcraftApp) {
    let s = &app.session;
    let (Some(path), Some(_)) = (s.path.clone(), s.config.as_ref()) else { return };
    if s.is_dirty() {
        return;
    }
    let key = (path.clone(), s.saved_revision);
    if app.home_thumb_saved.as_ref() == Some(&key) {
        return;
    }
    if app.viewer_shown.as_ref().map(|(_, k)| k.revision) != Some(s.revision) {
        return;
    }
    let Some(img) = app.viewer_pixels() else { return };
    let text = encode_thumb(&img);
    if let Some(c) = &app.session.config
        && let Err(e) = c.write(&thumb_name(&path), &text)
    {
        log::warn!("project thumbnail: {e}");
    }
    app.home_thumbs.remove(&path);
    app.home_thumb_saved = Some(key);
}

fn thumb_texture(app: &mut EffectcraftApp, ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
    if let Some(t) = app.home_thumbs.get(path) {
        return t.clone();
    }
    let tex = app
        .session
        .config
        .as_ref()
        .and_then(|c| c.read(&thumb_name(path)))
        .and_then(|t| decode_thumb(&t))
        .map(|img| ctx.load_texture(format!("home-thumb-{path}"), img, egui::TextureOptions::LINEAR));
    app.home_thumbs.insert(path.to_string(), tex.clone());
    tex
}

/// Width of the Home screen's left rail.
const RAIL_W: f32 = 236.0;

/// Draw the Home screen over `rect` (the whole workspace).
pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.app_bg);
    let mut actions: Vec<(&str, serde_json::Value)> = vec![];
    let rail = Rect::from_min_size(rect.min, vec2(RAIL_W.min(rect.width() * 0.4), rect.height()));
    p.rect_filled(rail, 0.0, t.panel_bg);
    p.line_segment([rail.right_top(), rail.right_bottom()], Stroke::new(1.0, t.separator));
    rail_ui(app, ui, &p, rail, &mut actions);
    let main = Rect::from_min_max(pos2(rail.max.x + 40.0, rect.min.y + 32.0), pos2(rect.max.x - 40.0, rect.max.y - 16.0));
    if app.ui.home_learn {
        super::learn::home_tab(app, ui, main);
    } else if app.ui.home_templates {
        super::templates::home_tab(app, ui, main);
    } else {
        home_page(app, ui, &p, main, &ctx, &mut actions);
    }
    run_actions(app, &ctx, actions);
}

/// The left rail: New / Open Project, the pages, and the community links at its foot.
fn rail_ui(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, rail: Rect, actions: &mut Vec<(&'static str, serde_json::Value)>) {
    let t = app.tokens;
    let x0 = rail.min.x + 16.0;
    let w = rail.width() - 32.0;
    let mut y = rail.min.y + 20.0;
    for (label, id, primary) in [("New Project", "file.newProject", true), ("Open Project…", "file.open", false)] {
        let r = Rect::from_min_size(pos2(x0, y), vec2(w, 34.0));
        if widgets::text_button(ui, r, tr(label), primary, &t, egui::Id::unique(("home", id))).clicked() {
            actions.push((id, json!({})));
        }
        app.auto.add(&format!("home.{id}"), r, label);
        y += 42.0;
    }
    y += 10.0;
    // Pages.
    let page = if app.ui.home_learn {
        2
    } else if app.ui.home_templates {
        1
    } else {
        0
    };
    for (k, (label, icon)) in [("Home", Icon::Home), ("Templates", Icon::Grid), ("Learn", Icon::Sparkle)].into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(rail.min.x + 8.0, y), vec2(rail.width() - 16.0, 34.0));
        let resp = ui.interact(r, egui::Id::unique(("home-tab", label)), Sense::click());
        let on = page == k;
        if on || resp.hovered() {
            p.rect_filled(r, 6.0, if on { t.row_selected } else { t.hover });
        }
        let col = if on { t.tab_text_active } else { t.text_dim };
        icons::paint(p, Rect::from_center_size(pos2(r.min.x + 20.0, r.center().y), vec2(16.0, 16.0)), icon, col);
        p.text(pos2(r.min.x + 40.0, r.center().y), Align2::LEFT_CENTER, tr(label), Tokens::medium(13.0), col);
        app.auto.add(&format!("home.tab.{}", label.to_ascii_lowercase()), r, label);
        if resp.clicked() {
            app.ui.home_learn = k == 2;
            app.ui.home_templates = k == 1;
        }
        y += 38.0;
    }
    // Community links at the foot (below the pages when the window is short).
    let links = [
        (Icon::Chat, "Join the ArtCraft Discord", "help.discord"),
        (Icon::Globe, "getartcraft.com", "help.website"),
        (Icon::Globe, "EffectCraft home page", "help.appPage"),
        (Icon::Code, "EffectCraft on GitHub", "help.github"),
    ];
    let sib = effectcraft_engine::links::SIBLINGS;
    let sib_rows = sib.len().div_ceil(2) as f32;
    let foot_h = 22.0 + links.len() as f32 * 30.0 + 22.0 + sib_rows * 26.0;
    let mut cy = (rail.max.y - 16.0 - foot_h).max(y + 16.0);
    p.line_segment([pos2(x0, cy - 10.0), pos2(x0 + w, cy - 10.0)], Stroke::new(1.0, t.separator));
    p.text(pos2(x0, cy + 6.0), Align2::LEFT_CENTER, tr("Community"), Tokens::semibold(12.0), t.text_dim);
    cy += 22.0;
    for (icon, label, cmd) in links {
        let r = Rect::from_min_size(pos2(x0, cy), vec2(w, 26.0));
        let resp = ui.interact(r, egui::Id::unique(("home-link", cmd)), Sense::click());
        let discord = cmd == "help.discord";
        if discord {
            p.rect_filled(r, 13.0, Color32::from_rgb(0x58, 0x65, 0xf2));
        } else if resp.hovered() {
            p.rect_filled(r, 13.0, t.hover);
        }
        let col = if discord { Color32::WHITE } else { t.text };
        icons::paint(p, Rect::from_center_size(pos2(r.min.x + 16.0, r.center().y), vec2(13.0, 13.0)), icon, col);
        p.text(pos2(r.min.x + 30.0, r.center().y), Align2::LEFT_CENTER, tr(label), Tokens::medium(12.0), col);
        app.auto.add(&format!("home.{cmd}"), r, label);
        if resp.clicked() {
            let _ = app.session.execute(cmd, json!({}));
        }
        cy += 30.0;
    }
    p.text(pos2(x0, cy + 8.0), Align2::LEFT_CENTER, tr("More ArtCraft apps"), Tokens::ui(11.0), t.text_faint);
    cy += 20.0;
    let sw = (w - 6.0) / 2.0;
    for (i, (name, slug)) in sib.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x0 + (i % 2) as f32 * (sw + 6.0), cy + (i / 2) as f32 * 26.0), vec2(sw, 22.0));
        let resp = ui.interact(r, egui::Id::unique(("sib", *slug)), Sense::click());
        p.rect_filled(r, 11.0, if resp.hovered() { t.hover } else { t.field_bg });
        p.text(r.center(), Align2::CENTER_CENTER, *name, Tokens::ui(11.0), t.text);
        app.auto.add(&format!("home.sibling.{slug}"), r, name);
        if resp.clicked() {
            let _ = app.session.execute("help.sibling", json!({"app": slug}));
        }
    }
}

/// The Home page: the welcome, quick-start tiles and the recent projects table.
fn home_page(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    area: Rect,
    ctx: &egui::Context,
    actions: &mut Vec<(&'static str, serde_json::Value)>,
) {
    let t = app.tokens;
    let (x0, w) = (area.min.x, area.width());
    let mut y = area.min.y;
    p.text(pos2(x0, y + 14.0), Align2::LEFT_CENTER, tr("Welcome to EffectCraft"), Tokens::semibold(26.0), t.tab_text_active);
    y += 40.0;
    p.text(
        pos2(x0, y + 8.0),
        Align2::LEFT_CENTER,
        tr_args("Motion graphics and visual effects · version {}", &[&env!("CARGO_PKG_VERSION")]),
        Tokens::ui(12.5),
        t.text_dim,
    );
    y += 36.0;
    // Quick start.
    let tiles = [
        ("New Composition", "app.newComp", Icon::NewComp),
        ("Open Demo Project", "file.openDemoProject", Icon::Play),
        ("Import Footage…", "file.import", Icon::Footage),
        ("New from Template", "templates", Icon::Grid),
    ];
    let gap = 12.0;
    let tw = ((w - gap * 3.0) / 4.0).clamp(120.0, 220.0);
    for (k, (label, id, icon)) in tiles.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(x0 + k as f32 * (tw + gap), y), vec2(tw, 64.0));
        let resp = ui.interact(r, egui::Id::unique(("home-tile", id)), Sense::click());
        p.rect_filled(r, 8.0, if resp.hovered() { t.hover } else { t.panel_bg });
        p.rect_stroke(r, 8.0, Stroke::new(1.0, if resp.hovered() { t.accent } else { t.separator }), StrokeKind::Inside);
        let ir = Rect::from_center_size(pos2(r.min.x + 30.0, r.center().y), vec2(32.0, 32.0));
        p.rect_filled(ir, 7.0, t.accent.gamma_multiply(0.18));
        icons::paint(p, ir.shrink(8.0), icon, t.accent);
        p.with_clip_rect(r.shrink(2.0)).text(pos2(ir.max.x + 12.0, r.center().y), Align2::LEFT_CENTER, tr(label), Tokens::medium(12.5), t.text);
        app.auto.add(&format!("home.{id}"), r, label);
        if resp.clicked() {
            if id == "templates" {
                app.ui.home_templates = true;
                app.ui.home_learn = false;
            } else {
                actions.push((id, json!({})));
            }
        }
    }
    y += 64.0 + 36.0;

    // Recent: heading, filter, clear.
    p.text(pos2(x0, y + 12.0), Align2::LEFT_CENTER, tr("Recent"), Tokens::semibold(17.0), t.tab_text_active);
    let entries = recent_entries(&app.session.prefs);
    let fid = egui::Id::unique("home-filter");
    let mut filter: String = ctx.data(|d| d.get_temp(fid)).unwrap_or_default();
    let fr = Rect::from_min_size(pos2(x0 + w - 240.0, y), vec2(240.0, 26.0));
    widgets::search_field(ui, fr, &mut filter, tr("Filter recent files"), &t);
    app.auto.add("home.filter", fr, "Filter recent files");
    ctx.data_mut(|d| d.insert_temp(fid, filter.clone()));
    if !entries.is_empty() {
        let cr = Rect::from_min_size(pos2(fr.min.x - 96.0, y + 3.0), vec2(84.0, 20.0));
        let resp = ui.interact(cr, egui::Id::unique("home-clear-recent"), Sense::click());
        p.text(cr.right_center(), Align2::RIGHT_CENTER, tr("Clear list"), Tokens::ui(11.5), if resp.hovered() { t.text } else { t.text_faint });
        app.auto.add("home.clearRecent", cr, "Clear Recent Projects");
        if resp.clicked() {
            actions.push(("file.clearRecent", json!({})));
        }
    }
    y += 42.0;
    // Column headings (Name / Opened / Size / Kind, as in After Effects).
    let (opened_x, size_x, kind_x) = (x0 + w * 0.52, x0 + w * 0.68, x0 + w * 0.80);
    for (label, x) in [("NAME", x0 + 76.0), ("OPENED", opened_x), ("SIZE", size_x), ("KIND", kind_x)] {
        p.text(pos2(x, y), Align2::LEFT_CENTER, tr(label), Tokens::medium(10.5), t.text_faint);
    }
    y += 12.0;
    p.line_segment([pos2(x0, y), pos2(x0 + w, y)], Stroke::new(1.0, t.separator));
    y += 4.0;
    let q = filter.trim().to_lowercase();
    let shown: Vec<&RecentEntry> =
        entries.iter().filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q) || e.folder.to_lowercase().contains(&q)).collect();
    if shown.is_empty() {
        let c = pos2(area.center().x, y + 90.0);
        icons::paint(p, Rect::from_center_size(c, vec2(40.0, 40.0)), Icon::Folder, t.text_faint);
        let (head, sub) = if entries.is_empty() {
            ("Your recent files will show here", "Create a new project, or open the demo project to look around.")
        } else {
            ("No recent files match the filter", "Try another name or folder.")
        };
        p.text(c + vec2(0.0, 40.0), Align2::CENTER_CENTER, tr(head), Tokens::semibold(14.0), t.text);
        p.text(c + vec2(0.0, 62.0), Align2::CENTER_CENTER, tr(sub), Tokens::ui(12.0), t.text_faint);
    }
    let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let row_h = 48.0;
    for e in shown {
        let r = Rect::from_min_size(pos2(x0, y), vec2(w, row_h - 2.0));
        if r.min.y > area.max.y {
            break;
        }
        let resp = ui.interact(r, egui::Id::unique(("home-recent", e.index)), Sense::click()).on_hover_text(&e.path);
        if resp.hovered() {
            p.rect_filled(r, 6.0, t.hover);
        }
        let tr = Rect::from_min_size(pos2(r.min.x + 6.0, r.center().y - 18.0), vec2(64.0, 36.0));
        match thumb_texture(app, ctx, &e.path) {
            Some(tex) => {
                p.image(tex.id(), tr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                p.rect_filled(tr, 3.0, t.field_bg);
                icons::paint(p, Rect::from_center_size(tr.center(), vec2(16.0, 16.0)), Icon::Comp, t.text_faint);
            }
        }
        p.rect_stroke(tr, 3.0, Stroke::new(1.0, t.separator), StrokeKind::Inside);
        let name_clip = p.with_clip_rect(Rect::from_min_max(pos2(tr.max.x + 6.0, r.min.y), pos2(opened_x - 12.0, r.max.y)));
        name_clip.text(
            pos2(tr.max.x + 6.0, r.center().y - 8.0),
            Align2::LEFT_CENTER,
            &e.name,
            Tokens::medium(13.0),
            if e.exists { t.tab_text_active } else { t.text_dim },
        );
        let sub = if e.exists { e.folder.clone() } else { format!("{} {}", e.folder, crate::i18n::tr("(missing)")) };
        name_clip.text(pos2(tr.max.x + 6.0, r.center().y + 9.0), Align2::LEFT_CENTER, sub, Tokens::ui(11.0), t.text_faint);
        let cell = |x: f32, x1: f32, text: String| {
            p.with_clip_rect(Rect::from_min_max(pos2(x, r.min.y), pos2(x1 - 8.0, r.max.y))).text(
                pos2(x, r.center().y),
                Align2::LEFT_CENTER,
                text,
                Tokens::ui(12.0),
                t.text_dim,
            );
        };
        cell(opened_x, size_x, e.modified_secs.map(|m| format_ago(now, m)).unwrap_or_else(|| "—".into()));
        cell(size_x, kind_x, e.size.map(format_size).unwrap_or_else(|| "—".into()));
        cell(kind_x, r.max.x, kind_of(&e.path).to_string());
        app.auto.add(&format!("home.recent.{}", e.index), r, &e.path);
        if resp.clicked() {
            actions.push(("file.openRecent", json!({"index": e.index})));
        }
        y += row_h;
    }
}

fn run_actions(app: &mut EffectcraftApp, ctx: &egui::Context, actions: Vec<(&str, serde_json::Value)>) {
    for (id, params) in actions {
        let before = (app.session.path.clone(), app.session.revision);
        match crate::menus::invoke(app, ctx, id, params) {
            Err(e) => app.ui.status = e,
            // Asking to save the open project first: Home stays until it is answered.
            Ok(r) if r.get("dialog").is_some() => {}
            // Leave Home unless only the list changed or a file dialog was cancelled.
            Ok(_) => {
                let cancelled = matches!(id, "file.open" | "file.import") && (app.session.path.clone(), app.session.revision) == before;
                if id != "file.clearRecent" && !cancelled {
                    app.ui.start_screen = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ascii_thumbnail_is_rejected_without_panicking() {
        let malformed = format!("aé{}", "0".repeat(THUMB_W * THUMB_H * 6 - 3));
        assert!(decode_thumb(&malformed).is_none());
    }

    #[test]
    fn recent_list_comes_from_settings() {
        let mut prefs = effectcraft_engine::prefs::Prefs::default();
        prefs.push_recent("/projects/a/Alpha.ecproj");
        prefs.push_recent("/projects/b/Beta.ecproj");
        let e = recent_entries(&prefs);
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].name.as_str(), e[0].folder.as_str(), e[0].index), ("Beta", "/projects/b", 0));
        assert_eq!(e[1].name, "Alpha");
        assert!(!e[0].exists && e[0].modified.is_none());
        // A real file has a date.
        let dir = std::env::temp_dir().join(format!("ec-home-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Real.ecproj");
        std::fs::write(&f, "{}").unwrap();
        prefs.push_recent(&f.to_string_lossy());
        let e = recent_entries(&prefs);
        assert!(e[0].exists && e[0].modified.as_ref().is_some_and(|d| d.len() == 16), "{:?}", e[0]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dates_and_thumbnails() {
        assert_eq!(format_utc(0), "1970-01-01 00:00");
        assert_eq!(format_utc(1_790_000_000), "2026-09-21 14:13");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00");
        // Years far outside chrono's own range are preserved (the old hand-rolled conversion kept
        // them), including negatives and the i64 extremes, rather than collapsing to the epoch.
        assert_eq!(format_utc(10_000_000_000_000), "318857-05-20 17:46");
        assert_eq!(format_utc(-10_000_000_000_000), "-314918-08-13 06:13");
        assert_eq!(format_utc(i64::MAX), "292277026596-12-04 15:30");
        assert_eq!(format_utc(i64::MIN), "-292277022657-01-27 08:29");
        let img = egui::ColorImage::new([200, 100], vec![Color32::from_rgb(10, 200, 30); 200 * 100]);
        let text = encode_thumb(&img);
        let back = decode_thumb(&text).unwrap();
        assert_eq!(back.size, [THUMB_W, THUMB_H]);
        // Letterboxed: the middle is the image, the top row is the background (2:1 into 16:9).
        assert_eq!(back.pixels[THUMB_H / 2 * THUMB_W + THUMB_W / 2], Color32::from_rgb(10, 200, 30));
        assert_ne!(back.pixels[THUMB_W / 2], Color32::from_rgb(10, 200, 30));
        assert!(decode_thumb("abc").is_none());
        assert_ne!(thumb_name("/a.ecproj"), thumb_name("/b.ecproj"));
    }

    #[test]
    fn opened_and_size_columns() {
        let now = 1_790_000_000;
        assert_eq!(format_ago(now, now - 5), "Just now");
        assert_eq!(format_ago(now, now - 60), "1 minute ago");
        assert_eq!(format_ago(now, now - 3 * 3_600), "3 hours ago");
        assert_eq!(format_ago(now, now - 90_000), "Yesterday");
        assert_eq!(format_ago(now, now - 3 * 86_400), "3 days ago");
        assert_eq!(format_ago(now, now - 30 * 86_400), "2026-08-22");
        assert_eq!(format_ago(now, now + 100), "Just now");
        assert_eq!((format_size(900), format_size(13_000), format_size(3_500_000)), ("900 B".into(), "13 KB".into(), "3.3 MB".into()));
        assert_eq!((kind_of("/a/B.ecproj"), kind_of("c.AEP"), kind_of("x")), ("EffectCraft project", "After Effects project", "Project"));
        // After a week it is the date, whole even for a five-digit year.
        assert_eq!(format_ago(i64::MAX, 253_402_300_800), "10000-01-01");
    }

    /// The Home screen's own text goes through the interface catalog (#574): the Opened and Kind
    /// columns read in the interface language, and English stays English.
    #[test]
    fn home_text_follows_the_interface_language() {
        let now = 1_790_000_000;
        crate::i18n::set_current("zh-hans");
        let shown = (format_ago(now, now - 5), format_ago(now, now - 3 * 3_600), format_ago(now, now - 90_000), kind_of("/a/B.ecproj"));
        let catalog = (tr("Just now").to_string(), tr_args("{} hours ago", &[&3]), tr("Yesterday").to_string(), tr("EffectCraft project"));
        crate::i18n::set_current("en");
        assert_eq!(shown, catalog);
        assert_eq!(tr("Filter recent files"), "Filter recent files");
    }

    #[test]
    fn home_screen_registers_its_controls() {
        let mut s = effectcraft_engine::Session::default();
        s.prefs.push_recent("/projects/Alpha.ecproj");
        let mut app = EffectcraftApp::new(s);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        ctx.run_ui(Default::default(), |ui| {
            app.auto.begin_frame();
            show(&mut app, ui, Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0)));
        })
        .textures_delta
        .clear();
        for id in [
            "home.file.newProject",
            "home.file.open",
            "home.app.newComp",
            "home.file.openDemoProject",
            "home.help.discord",
            "home.help.github",
            "home.recent.0",
            "home.filter",
            "home.file.import",
            "home.templates",
            "home.tab.home",
            "home.tab.templates",
            "home.tab.learn",
        ] {
            assert!(app.auto.find(id).is_some(), "{id}");
        }
    }

    #[test]
    fn learn_tab_lists_and_starts_tutorials() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.ui.start_screen = true;
        app.ui.home_learn = true;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens, crate::i18n::language(&app));
        let frame = |app: &mut EffectcraftApp| {
            ctx.run_ui(Default::default(), |ui| {
                app.auto.begin_frame();
                show(app, ui, Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0)));
                crate::panels::learn::coach(app, ui.ctx());
            })
            .textures_delta
            .clear();
        };
        frame(&mut app);
        frame(&mut app);
        for t in effectcraft_engine::learn::tutorials() {
            assert!(app.auto.find(&format!("home.learn.{}.start", t.id)).is_some(), "{}", t.id);
        }
        assert!(app.auto.find("home.recent.0").is_none());
        // Start through the session (as the Start button does); the coach card appears.
        app.session.execute("learn.start", json!({"id": "animate-title"})).unwrap();
        app.ui.start_screen = false;
        frame(&mut app);
        frame(&mut app);
        for id in ["learn.coach", "learn.showMe", "learn.next", "learn.close"] {
            assert!(app.auto.find(id).is_some(), "{id}");
        }
        // Show me performs the step and the coach moves on.
        app.session.execute("learn.step", json!({"action": "showMe"})).unwrap();
        assert_eq!(app.session.learn.as_ref().unwrap().step, 1);
        assert!(app.session.active_comp().is_some());
    }
}
