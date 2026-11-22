//! The OpenColorIO config a project works with (Project Settings ▸ Color ▸ OCIO Config).
//!
//! Which config is active, for a project whose Color Engine is OCIO:
//!
//! 1. the `OCIO` environment variable ([`crate::Session::ocio_env`]), when set: it overrides the
//!    project's own choice without changing it, as Nuke documents ("If you have specified an OCIO
//!    environment variable … Nuke always uses the configuration file specified there", with the
//!    project's config controls disabled);
//! 2. the project's `ocio_config`;
//! 3. the built-in config (ACES working spaces).
//!
//! Settings ▸ Project ▸ Default OCIO Config only seeds new projects' `ocio_config`.
//!
//! A relative `ocio_config` is taken from the project file's folder ([`resolve_stored`]) and kept
//! absolute; a config is read only when the session's services can see it ([`Readable`]). A
//! notice that the `OCIO` variable's config can't be used is shown once a session ([`Notice`]).
//!
//! A config is read with [`effectcraft_effects::ocio_config::load_config`]. Its `scene_linear`
//! role (else `compositing_linear`, else `reference`) is identified as one of the working spaces
//! numerically: test colours are converted through the config from that space to its
//! `aces_interchange` role (ACES2065-1 by OCIO's definition) and compared with each working
//! space's own conversion to ACES2065-1. Names are never compared, so differently named ACES
//! configs resolve alike. Only the working space comes from the config today; its displays,
//! views, looks, colour spaces and output transforms are listed ([`status`]) but not applied.

use effectcraft_color::{ColorSpace, Conversion};
use effectcraft_effects::ocio_config::{Config, load_config};
use effectcraft_project::{ColorEngine, HdrMode, Project, ProjectSettings};
use serde_json::{Value, json};

/// Where the active config comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The `OCIO` environment variable.
    Env,
    /// The project's own config, the same file as the studio default.
    StudioDefault,
    /// The project's own config.
    Project,
    /// No config chosen: the built-in one.
    BuiltIn,
}

impl Source {
    pub fn id(self) -> &'static str {
        match self {
            Source::Env => "env",
            Source::StudioDefault => "studioDefault",
            Source::Project => "project",
            Source::BuiltIn => "builtIn",
        }
    }
}

/// The config a project would work with and where it comes from (`path` is `None` for the
/// built-in config).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Active {
    pub source: Source,
    pub path: Option<String>,
}

/// A non-empty, trimmed path.
fn some_path(p: Option<&str>) -> Option<&str> {
    p.map(str::trim).filter(|p| !p.is_empty())
}

/// The active config for project settings `st`, with environment value `env` and studio default
/// `studio` (the project's `ocio_config` is reported as the studio default when it is the same
/// file).
pub fn active(env: Option<&str>, st: &ProjectSettings, studio: Option<&str>) -> Active {
    if let Some(e) = some_path(env) {
        // A relative `OCIO` is taken from the working directory, as OpenColorIO does.
        let e = std::path::absolute(e).map_or_else(|_| e.to_string(), |p| p.to_string_lossy().into_owned());
        return Active { source: Source::Env, path: Some(e) };
    }
    match some_path(st.ocio_config.as_deref()) {
        Some(p) => {
            let source = if some_path(studio) == Some(p) { Source::StudioDefault } else { Source::Project };
            Active { source, path: Some(p.to_string()) }
        }
        None => Active { source: Source::BuiltIn, path: None },
    }
}

/// The working spaces the OCIO Color Engine accepts.
pub fn ocio_working_spaces() -> impl Iterator<Item = ColorSpace> {
    ColorSpace::WORKING.into_iter().filter(|c| c.is_linear())
}

/// Colours a working space is identified with: primaries, white, greys, over-range and
/// saturated mixes (a curve or a different matrix moves at least one of them).
const PROBES: [[f64; 3]; 8] =
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.18, 0.18, 0.18], [0.02, 0.02, 0.02], [4.0, 0.5, 0.05], [0.1, 0.6, 0.3]];

/// How far (relative to the value, at least absolute 1e-3) a converted probe may be from the
/// working space's own conversion: configs write their matrices to 6–10 digits, while the
/// working spaces closest to each other (Rec. 709 and P3) differ by more than 0.02.
const TOLERANCE: f64 = 2e-3;

/// The config's scene-linear space's name and the working space it is, numerically; why not.
pub fn resolve_working_space(cfg: &Config) -> Result<(String, ColorSpace), String> {
    let role = |r: &str| cfg.roles.iter().find(|(k, _)| k.eq_ignore_ascii_case(r)).and_then(|(_, n)| cfg.space(n));
    let (role_name, sl) = ["scene_linear", "compositing_linear", "reference"]
        .into_iter()
        .find_map(|r| role(r).map(|s| (r, s)))
        .ok_or("the config has no scene_linear, compositing_linear or reference role")?;
    if sl.is_data || sl.display {
        return Err(format!("the config's {role_name} space `{}` is a data or display space, not scene-linear", sl.name));
    }
    let aces = role("aces_interchange").ok_or_else(|| {
        format!(
            "the config has no aces_interchange role, so its {role_name} space `{}` can't be identified (OCIO 2.2 configs name ACES2065-1 with it)",
            sl.name
        )
    })?;
    let path = cfg.path(sl, aces);
    let unsupported = path.unsupported_list();
    if !unsupported.is_empty() {
        return Err(format!("the conversion from `{}` to ACES2065-1 uses transforms EffectCraft can't apply yet: {}", sl.name, unsupported.join(", ")));
    }
    let got: Vec<[f64; 3]> = PROBES.iter().map(|c| path.apply(*c, false)).collect();
    let matches = |ws: ColorSpace| {
        let conv = Conversion::new(ws, true, ColorSpace::Aces2065, true);
        PROBES.iter().zip(&got).all(|(c, g)| {
            let c32 = c.map(|v| v as f32);
            let want = conv.as_ref().map_or(c32, |k| k.apply(c32));
            (0..3).all(|i| (g[i] - f64::from(want[i])).abs() <= TOLERANCE * f64::from(want[i]).abs().max(1.0))
        })
    };
    if let Some(ws) = ocio_working_spaces().find(|w| matches(*w)) {
        return Ok((sl.name.clone(), ws));
    }
    let supported = ocio_working_spaces().map(ColorSpace::label).collect::<Vec<_>>().join(", ");
    // Linear light in another working space's primaries (a linear Rec. 709 config): say which.
    // (sRGB and Rec. 709 share their primaries: both are named.)
    let same: Vec<&str> = ColorSpace::WORKING.into_iter().filter(|w| !w.is_linear() && matches(*w)).map(ColorSpace::label).collect();
    if !same.is_empty() {
        return Err(format!(
            "the config's {role_name} space `{}` is linear light with {} primaries; the OCIO Color Engine works in {supported} only",
            sl.name,
            same.join(" / ")
        ));
    }
    Err(format!("the config's {role_name} space `{}` matches none of the OCIO working spaces ({supported})", sl.name))
}

/// What the active config gives the project: its working space, or why it can't be used.
pub enum Resolved {
    /// The built-in config.
    BuiltIn,
    /// A config file that can't be read: the built-in config is used instead.
    Unreadable(String),
    /// A config that can't give a working space.
    Refused(String),
    /// A config and the working space of its scene-linear space.
    Space(ColorSpace),
}

/// Whether the session may read a config file, and why not: the file exists and the session's
/// [`crate::Services`] may read it (an agent confined to its folders can't), and the path wasn't
/// refused when the project was opened ([`resolve_stored`]).
pub type Readable<'a> = &'a dyn Fn(&str) -> Result<(), String>;

/// What the active config depends on besides the project: the `OCIO` environment variable, the
/// studio default and which files can be read.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub env: Option<&'a str>,
    pub studio: Option<&'a str>,
    pub readable: Readable<'a>,
}

/// A notice about the active config; `once`: the `OCIO` environment variable's config can't be
/// used, which a session says only once (every project has the same problem).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub once: bool,
}

/// Why the config at `p` isn't read: a relative path (resolved against the project's folder when
/// it is chosen or opened, [`resolve_stored`], so one left over has no folder), or one the
/// session can't read.
fn gate(p: &str, readable: Readable) -> Result<(), String> {
    if !std::path::Path::new(p).is_absolute() {
        return Err(format!("{p}: a relative path, and the project has no folder to take it from"));
    }
    readable(p)
}

/// Resolve the config at `path` (`None`: built-in). Only an absolute path is read: a relative
/// one is resolved against the project's folder when it is chosen or opened ([`resolve_stored`]).
pub fn resolve(path: Option<&str>, readable: Readable) -> Resolved {
    let Some(p) = some_path(path) else { return Resolved::BuiltIn };
    if let Err(e) = gate(p, readable) {
        return Resolved::Unreadable(e);
    }
    match load_config(p) {
        Err(e) => Resolved::Unreadable(e),
        Ok(cfg) => match resolve_working_space(&cfg) {
            Ok((_, ws)) => Resolved::Space(ws),
            Err(e) => Resolved::Refused(e),
        },
    }
}

/// A notice for a project whose active config can't be used as saved (a missing file, a config
/// that no longer gives its working space), for the open toast.
pub fn notice(ctx: Ctx, st: &ProjectSettings) -> Option<Notice> {
    if st.color_engine != ColorEngine::Ocio {
        return None;
    }
    let a = active(ctx.env, st, ctx.studio);
    let path = a.path.as_deref()?;
    let env = a.source == Source::Env;
    let (from, from_cap) = if env {
        ("the OCIO environment variable's config", "The OCIO environment variable's config")
    } else {
        ("the project's OCIO config", "The project's OCIO config")
    };
    let n = |text: String, once: bool| Some(Notice { text, once });
    match resolve(Some(path), ctx.readable) {
        Resolved::Unreadable(e) => n(format!("Can't read {from} ({e}); using the built-in ACES config instead."), env),
        Resolved::Refused(e) => n(format!("{from_cap} can't set the working space ({e}); the project keeps its working space."), env),
        Resolved::Space(ws) if st.working_space != Some(ws) => n(
            format!(
                "{from_cap} works in {}, but the project's working space is {}; choose {} in Project Settings to match.",
                ws.label(),
                st.working_space.map_or("None", ColorSpace::label),
                ws.label()
            ),
            false,
        ),
        _ => None,
    }
}

/// The active config, for command results and Project Settings: its source and path, whether it
/// could be read, the working space it gives, and its displays, views, looks and colour spaces,
/// which are listed for information only (not applied yet).
pub fn status(ctx: Ctx, st: &ProjectSettings) -> Value {
    let a = active(ctx.env, st, ctx.studio);
    let mut out = json!({
        "source": a.source.id(),
        "path": a.path,
        "projectConfig": st.ocio_config,
        "envConfig": some_path(ctx.env),
        "studioDefault": some_path(ctx.studio),
        "colorEngine": if st.color_engine == ColorEngine::Ocio { "ocio" } else { "adobe" },
        // The config only matters to OCIO-managed projects.
        "inUse": st.color_engine == ColorEngine::Ocio,
        "supportedWorkingSpaces": ocio_working_spaces().map(ColorSpace::id).collect::<Vec<_>>(),
    });
    let Some(path) = a.path.as_deref() else {
        out["loaded"] = json!(true);
        out["name"] = json!("built-in ACES config");
        return out;
    };
    let fail = |mut out: Value, e: String| {
        out["loaded"] = json!(false);
        out["error"] = json!(e);
        out["fallback"] = json!("builtIn");
        out
    };
    if let Err(e) = gate(path, ctx.readable) {
        return fail(out, e);
    }
    let cfg = match load_config(path) {
        Ok(c) => c,
        Err(e) => return fail(out, e),
    };
    out["loaded"] = json!(true);
    out["name"] = json!(cfg.name);
    match resolve_working_space(&cfg) {
        Ok((name, ws)) => {
            out["sceneLinear"] = json!(name);
            out["workingSpace"] = json!(ws.id());
        }
        Err(e) => out["workingSpaceError"] = json!(e),
    }
    let not_applied = "listed for information: not applied yet (the viewer and renders use the built-in display pipeline)";
    out["displays"] = json!({
        "applied": false,
        "note": not_applied,
        "items": cfg.displays.iter().map(|(d, v)| json!({"display": d, "views": v.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()})).collect::<Vec<_>>(),
    });
    out["viewTransforms"] = json!({"applied": false, "note": not_applied, "items": cfg.view_transforms.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()});
    out["looks"] = json!({"applied": false, "note": not_applied, "items": cfg.looks.iter().map(|l| l.name.as_str()).collect::<Vec<_>>()});
    out["colorSpaces"] = json!({
        "applied": false,
        "note": "listed for information: footage is interpreted with the built-in colour profiles, not these spaces",
        "items": cfg.space_names(),
    });
    out["roles"] = json!(cfg.roles.iter().map(|(r, s)| json!({"role": r, "space": s})).collect::<Vec<_>>());
    out["unsupportedTransforms"] = json!(cfg.unsupported());
    out
}

/// A new project's colour settings: the OCIO engine with Settings ▸ Project ▸ Default OCIO
/// Config, or with the `OCIO` environment variable's config, and the working space of the
/// active config's scene_linear role. A default that can't be used leaves the project as it
/// is and says why. (A template keeps its own settings.)
pub fn seed_new_project(ctx: Ctx, st: &mut ProjectSettings) -> Option<Notice> {
    let studio = ctx.studio.unwrap_or("").trim();
    let env = some_path(ctx.env);
    if studio.is_empty() && env.is_none() {
        return None;
    }
    let mut next = st.clone();
    next.ocio_config = (!studio.is_empty()).then(|| studio.to_string());
    next.color_engine = ColorEngine::Ocio;
    next.hdr = HdrMode::ToneMap;
    let active = active(env, &next, Some(studio));
    let from_env = active.source == Source::Env;
    let from = if from_env { "The OCIO environment variable's config" } else { "The default OCIO config" };
    // A new project has no folder: the default must be an absolute path.
    if !from_env && !std::path::Path::new(studio).is_absolute() {
        return Some(Notice { text: format!("{from} ({studio}) is a relative path: give an absolute path in Settings ▸ Project."), once: false });
    }
    match resolve(active.path.as_deref(), ctx.readable) {
        Resolved::Space(ws) => next.working_space = Some(ws),
        // A missing file (a studio share not mounted yet) keeps the choice, on the built-in config.
        Resolved::Unreadable(e) => {
            next.working_space = Some(ColorSpace::AcesCg);
            *st = next;
            return Some(Notice { text: format!("{from} can't be read ({e}); the new project uses the built-in ACES config until it can."), once: from_env });
        }
        Resolved::Refused(e) => return Some(Notice { text: format!("{from} can't be used for new projects: {e}."), once: from_env }),
        Resolved::BuiltIn => next.working_space = Some(ColorSpace::AcesCg),
    }
    *st = next;
    None
}

/// A project's stored config path as the document saved at `doc` names it (a project file or a
/// template): a relative path is taken from the document's folder and stored as the absolute
/// path, so Save As elsewhere keeps the same file, and a path the stored-path rules don't follow
/// (another computer's share, a URL; see [`crate::doc_path`]) is kept but not read. Returns that
/// path and why.
pub fn resolve_stored(st: &mut ProjectSettings, doc: Option<&str>) -> Option<(String, String)> {
    let stored = some_path(st.ocio_config.as_deref())?.to_string();
    match crate::doc_path::resolve(&stored, doc) {
        Ok(p) => {
            st.ocio_config = Some(p.to_string_lossy().into_owned());
            None
        }
        Err(why) => Some((stored.clone(), crate::doc_path::refusal(&stored, why))),
    }
}

impl crate::Session {
    /// [`Readable`] for this session: the path wasn't refused when the project opened, and the
    /// session's services can see the file.
    pub fn ocio_readable(&self) -> impl Fn(&str) -> Result<(), String> + use<> {
        let services = self.services.clone();
        let refused = self.ocio_refused.clone();
        move |p: &str| {
            if let Some((_, why)) = refused.as_ref().filter(|(r, _)| r == p) {
                return Err(why.clone());
            }
            match services.stamp(p) {
                Some(_) => Ok(()),
                None => Err(format!("{p}: not found, or outside the folders this session may read")),
            }
        }
    }

    /// The project's active OCIO config ([`status`]).
    pub fn ocio_status(&self) -> Value {
        let readable = self.ocio_readable();
        let ctx = Ctx { env: self.ocio_env.as_deref(), studio: Some(&self.prefs.project.default_ocio_config), readable: &readable };
        status(ctx, &self.project.settings)
    }

    /// Toast a notice; one about the `OCIO` environment variable's config only the first time.
    pub fn ocio_notify(&mut self, n: &Notice) {
        if n.once && std::mem::replace(&mut self.ocio_env_noticed, true) {
            return;
        }
        self.toast(n.text.clone());
    }

    /// [`seed_new_project`] for this session's environment and settings.
    pub fn seed_new_project_color(&self, st: &mut ProjectSettings) -> Option<Notice> {
        let readable = self.ocio_readable();
        let ctx = Ctx { env: self.ocio_env.as_deref(), studio: Some(&self.prefs.project.default_ocio_config), readable: &readable };
        seed_new_project(ctx, st)
    }

    /// [`notice`] for the open project.
    pub fn ocio_notice(&self) -> Option<Notice> {
        let readable = self.ocio_readable();
        let ctx = Ctx { env: self.ocio_env.as_deref(), studio: Some(&self.prefs.project.default_ocio_config), readable: &readable };
        notice(ctx, &self.project.settings)
    }

    /// [`seed_new_project`] for the untitled project a session starts with, once the settings
    /// are loaded (File ▸ New ▸ New Project does the same): only while it is still empty and
    /// unsaved, and without marking it modified.
    pub fn seed_launch_project_color(&mut self) {
        if self.path.is_some() || *self.project != Project::default() {
            return;
        }
        let mut st = self.project.settings.clone();
        let notice = self.seed_new_project_color(&mut st);
        if st != self.project.settings {
            std::sync::Arc::make_mut(&mut self.project).settings = st;
            self.mark_saved();
        }
        if let Some(n) = notice {
            self.ocio_notify(&n);
        }
    }
}
