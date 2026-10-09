//! Menu dialogs: a generic parameter form (numeric Transform dialogs, Mask Feather/Opacity/
//! Expansion, Auto-Orient, Go to Time, Add Guide, Sequence Layers, Interpret Footage, Project
//! Settings, placeholders…), View Options and simple message boxes (Settings: `settings`;
//! Keyboard Shortcuts: `shortcut_editor`). Every form runs an engine command with the collected parameters, so whatever a
//! dialog does an agent can do with one `engine.execute`.

use egui::{Color32, vec2};
use serde_json::{Map, Value, json};

use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp};

#[derive(Clone, Debug)]
pub enum FieldKind {
    Number {
        value: f64,
        speed: f64,
    },
    Text(String),
    /// A file path with a Browse… button (the host's open dialog, these extensions).
    Path {
        value: String,
        exts: Vec<String>,
    },
    /// A file to write, with a Browse… button (the host's save dialog).
    SavePath(String),
    /// Explanatory text (no parameter).
    Note(String),
    Bool(bool),
    /// Options as (label, value); `sel` is the chosen index.
    Choice {
        options: Vec<(String, Value)>,
        sel: usize,
    },
}

/// One form field. `key` may index into an array param (`value[1]`).
#[derive(Clone, Debug)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
}

impl Field {
    pub fn num(key: &str, label: &str, value: f64) -> Field {
        Field { key: key.into(), label: label.into(), kind: FieldKind::Number { value, speed: 1.0 } }
    }
    pub fn text(key: &str, label: &str, value: &str) -> Field {
        Field { key: key.into(), label: label.into(), kind: FieldKind::Text(value.into()) }
    }
    pub fn path(key: &str, label: &str, value: &str, exts: &[&str]) -> Field {
        Field { key: key.into(), label: label.into(), kind: FieldKind::Path { value: value.into(), exts: exts.iter().map(|e| e.to_string()).collect() } }
    }
    pub fn save_path(key: &str, label: &str, value: &str) -> Field {
        Field { key: key.into(), label: label.into(), kind: FieldKind::SavePath(value.into()) }
    }
    pub fn note(key: &str, text: &str) -> Field {
        Field { key: key.into(), label: String::new(), kind: FieldKind::Note(text.into()) }
    }
    pub fn bool(key: &str, label: &str, value: bool) -> Field {
        Field { key: key.into(), label: label.into(), kind: FieldKind::Bool(value) }
    }
    pub fn choice(key: &str, label: &str, options: &[(&str, Value)], sel: usize) -> Field {
        Field {
            key: key.into(),
            label: label.into(),
            kind: FieldKind::Choice { options: options.iter().map(|(l, v)| (l.to_string(), v.clone())).collect(), sel },
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Form {
    pub title: String,
    pub command: String,
    pub base: Value,
    pub fields: Vec<Field>,
}

impl Form {
    /// The command parameters: `base` plus every field.
    pub fn params(&self) -> Value {
        let mut m: Map<String, Value> = self.base.as_object().cloned().unwrap_or_default();
        for f in &self.fields {
            let v = match &f.kind {
                FieldKind::Number { value, .. } => json!(value),
                FieldKind::Text(s) | FieldKind::Path { value: s, .. } | FieldKind::SavePath(s) => json!(s),
                FieldKind::Note(_) => continue,
                FieldKind::Bool(b) => json!(b),
                FieldKind::Choice { options, sel } => options.get(*sel).map(|o| o.1.clone()).unwrap_or(Value::Null),
            };
            match f.key.split_once('[') {
                Some((name, idx)) => {
                    let i: usize = idx.trim_end_matches(']').parse().unwrap_or(0);
                    let arr = m.entry(name.to_string()).or_insert_with(|| json!([]));
                    if let Value::Array(a) = arr {
                        while a.len() <= i {
                            a.push(json!(0));
                        }
                        a[i] = v;
                    }
                }
                None => {
                    m.insert(f.key.clone(), v);
                }
            }
        }
        Value::Object(m)
    }
}

/// Open a form dialog.
pub fn form(app: &mut EffectcraftApp, title: &str, command: &str, base: Value, fields: Vec<Field>) {
    app.dialog_state.form = Form { title: title.into(), command: command.into(), base, fields };
    app.dialog = Some(Dialog::Form);
}

/// Open a message box.
pub fn info(app: &mut EffectcraftApp, title: &str, body: &str) {
    app.dialog_state.info = (title.into(), body.into());
    app.dialog = Some(Dialog::Info);
}

fn has(p: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|k| p.get(*k).is_some())
}

/// The files of an import's parameters (`paths` or `path`).
fn import_paths(p: &Value) -> Vec<String> {
    match p.get("paths").or(p.get("path")) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(Value::String(x)) => vec![x.clone()],
        _ => vec![],
    }
}

/// The import includes a file with one of these (lowercase) extensions.
fn imports_ext(p: &Value, exts: &[&str]) -> bool {
    import_paths(p).iter().any(|s| s.rsplit('.').next().is_some_and(|e| exts.contains(&e.to_ascii_lowercase().as_str())))
}

/// The import includes a Photoshop document.
fn is_layered(p: &Value) -> bool {
    imports_ext(p, &["psd", "psb"])
}

/// The import includes a PDF or Illustrator file (which may have several pages).
fn is_pdf(p: &Value) -> bool {
    imports_ext(p, &["pdf", "ai"])
}

/// File ▸ Import ▸ File… picked numbered stills that import as image sequences: ask, as After
/// Effects' "<format> Sequence" checkbox in its Import dialog does (rfd's native dialog can't
/// carry a checkbox), with Force Alphabetical Order and the frame rate. `p`: the import's
/// parameters. Returns whether the dialog opened. Dropped files and agents' `file.import`
/// don't ask (`sequence` defaults to on).
pub fn open_import_sequence(app: &mut EffectcraftApp, p: &Value) -> bool {
    if has(p, &["sequence", "alphabetical"]) {
        return false;
    }
    let runs = effectcraft_engine::sequence::sequences(app.session.services.as_ref(), &import_paths(p), false);
    if runs.is_empty() {
        return false;
    }
    let fields = sequence_fields(app, &runs);
    form(app, "Import Image Sequence", "file.import", p.clone(), fields);
    true
}

/// The Import Image Sequence dialog's fields for `runs` (each a sequence's files).
fn sequence_fields(app: &EffectcraftApp, runs: &[Vec<String>]) -> Vec<Field> {
    use effectcraft_engine::project::sequence::sequence_name;
    let first = runs.first().map(Vec::as_slice).unwrap_or_default();
    let ext = first.first().and_then(|f| f.rsplit_once('.')).map(|(_, e)| e.to_ascii_uppercase()).unwrap_or_default();
    let what = |files: &[String]| {
        let name = sequence_name(files).unwrap_or_default();
        match effectcraft_engine::sequence::missing_report(files) {
            Some(gaps) => format!("{name}: {} files, {gaps}", files.len()),
            None => format!("{name}: {} files", files.len()),
        }
    };
    let note = match runs {
        [one] => format!("{} – numbered stills of one image sequence.", what(one)),
        _ => format!("{} image sequences: {}.", runs.len(), runs.iter().map(|r| what(r)).collect::<Vec<_>>().join("; ")),
    };
    vec![
        Field::note("note", &note),
        Field::bool("sequence", &format!("{ext} Sequence"), true),
        Field::bool("alphabetical", "Force alphabetical order", false),
        Field::num("frameRate", "Frame rate (fps)", app.session.prefs.import.sequence_fps),
        Field::note("after", "Change the frame rate, alpha and start frame later with File ▸ Interpret Footage."),
    ]
}

/// If `id` is a dialog command invoked without its parameters, open its form and return true.
pub fn open_form(app: &mut EffectcraftApp, id: &str, p: &Value) -> bool {
    let s = &app.session;
    let comp = s.active_comp();
    let layer = comp.and_then(|c| s.state.selected_layers.first().and_then(|l| c.layer(*l)));
    let t = s.time();
    let lt = layer.map(|l| l.layer_time(t)).unwrap_or(t);
    let base = p.clone();
    let (title, fields): (String, Vec<Field>) = match id {
        // Photoshop files: Import Kind (Footage / Composition / – Retain Layer Sizes).
        "file.import" if !has(p, &["importAs"]) && is_layered(p) => (
            "Import Photoshop File".into(),
            vec![Field::choice(
                "importAs",
                "Import Kind",
                &[("Footage", json!("footage")), ("Composition", json!("composition")), ("Composition - Retain Layer Sizes", json!("compositionLayerSizes"))],
                1,
            )],
        ),
        // PDF / Illustrator files: Import Kind and the page.
        "file.import" if !has(p, &["importAs", "page"]) && is_pdf(p) => (
            "Import PDF / Illustrator File".into(),
            vec![
                Field::choice("importAs", "Import Kind", &[("Footage", json!("footage")), ("Composition", json!("composition"))], 0),
                Field::num("page", "Page", 1.0),
            ],
        ),
        // Puppet tool ▸ Follow-Through.
        "puppet.follow" if !has(p, &["delay", "amount", "cascade"]) => (
            "Puppet Follow-Through".into(),
            vec![
                Field::num("delay", "Delay (s)", 0.1),
                Field::num("amount", "Amount (%)", 100.0),
                Field::bool("cascade", "Cascade (farther pins trail longer)", true),
            ],
        ),
        // Puppet tool ▸ Record Options.
        "puppet.recordOptions" if !has(p, &["speed", "smoothing", "useDraftDeformation", "showMesh"]) => {
            let o = &s.state.puppet;
            (
                "Puppet Record Options".into(),
                vec![
                    Field::num("speed", "Speed (%)", o.record_speed),
                    Field::num("smoothing", "Smoothing", o.record_smoothing),
                    Field::bool("useDraftDeformation", "Use Draft Deformation", o.record_draft),
                    Field::bool("showMesh", "Show Mesh", o.record_show_mesh),
                ],
            )
        }
        // Double-click a keyframe (or its context menu ▸ Edit Value…): the property's value at
        // that key, like After Effects' value dialog.
        "keys.set" if !has(p, &["value", "newTime"]) => {
            let l = p.get("layer").and_then(Value::as_u64).and_then(|l| comp?.layer(effectcraft_engine::project::LayerId(l)));
            let pr = l.and_then(|l| l.props.find(p.get("prop").and_then(Value::as_u64)?));
            let t = p.get("time").and_then(Value::as_f64).map(effectcraft_engine::time::Tick::from_seconds_f64);
            let Some((pr, t)) = pr.zip(t) else { return false };
            let Some(k) = pr.keys.iter().min_by_key(|k| (k.time.0 - t.0).abs()) else { return false };
            use effectcraft_engine::keyframe::Value as KV;
            let fields = match &k.value {
                KV::Scalar(v) => vec![Field::num("value", &pr.name, *v)],
                KV::Vec2(v) => vec![Field::num("value[0]", "X", v[0]), Field::num("value[1]", "Y", v[1])],
                KV::Vec3(v) => vec![Field::num("value[0]", "X", v[0]), Field::num("value[1]", "Y", v[1]), Field::num("value[2]", "Z", v[2])],
                _ => return false,
            };
            (pr.name.clone(), fields)
        }
        "layer.setTransform" if !has(p, &["value"]) => {
            let prop = p.get("prop").and_then(Value::as_str).unwrap_or("position");
            let cur = layer.and_then(|l| l.transform()).and_then(|tr| tr.get(if prop == "anchorPoint" { "anchor" } else { prop })).map(|pr| pr.value_at(lt));
            let v = cur.map(|c| c.as_vec3()).unwrap_or([0.0; 3]);
            let three = layer.is_some_and(|l| l.is_3d());
            let title = match prop {
                "anchor" => "Anchor Point",
                "position" => "Position",
                "scale" => "Scale",
                "orientation" => "Orientation",
                "rotation" => "Rotation",
                _ => "Opacity",
            };
            let fields = match prop {
                "rotation" | "opacity" => vec![Field::num("value", if prop == "rotation" { "Degrees" } else { "Opacity (%)" }, v[0])],
                "orientation" => vec![Field::num("value[0]", "X", v[0]), Field::num("value[1]", "Y", v[1]), Field::num("value[2]", "Z", v[2])],
                _ if three => vec![Field::num("value[0]", "X", v[0]), Field::num("value[1]", "Y", v[1]), Field::num("value[2]", "Z", v[2])],
                _ => vec![Field::num("value[0]", "X", v[0]), Field::num("value[1]", "Y", v[1])],
            };
            (title.into(), fields)
        }
        "path.freeTransform" if !has(p, &["scale", "rotation", "offset"]) => (
            "Free Transform Points".into(),
            vec![
                Field::num("scale[0]", "Scale X (%)", 100.0),
                Field::num("scale[1]", "Scale Y (%)", 100.0),
                Field::num("rotation", "Rotation (degrees)", 0.0),
                Field::num("offset[0]", "Move X", 0.0),
                Field::num("offset[1]", "Move Y", 0.0),
            ],
        ),
        "layer.mask.set" if !has(p, &["value"]) => {
            let field = p.get("field").and_then(Value::as_str).unwrap_or("feather");
            let first = layer.and_then(|l| l.masks()).and_then(|m| m.groups().next()).and_then(|g| g.get(field)).map(|pr| pr.value_at(lt).as_f64());
            let (title, label, d) = match field {
                "feather" => ("Mask Feather", "Feather (pixels)", 0.0),
                "opacity" => ("Mask Opacity", "Opacity (%)", 100.0),
                _ => ("Mask Expansion", "Expansion (pixels)", 0.0),
            };
            (title.into(), vec![Field::num("value", label, first.unwrap_or(d))])
        }
        "layer.mask.shape" if !has(p, &["rect"]) => {
            let (w, h) = layer.map(|l| effectcraft_engine::render::source_size(&s.project, l)).unwrap_or((100, 100));
            (
                "Mask Shape".into(),
                vec![
                    Field::num("rect[0]", "Left", 0.0),
                    Field::num("rect[1]", "Top", 0.0),
                    Field::num("rect[2]", "Width", w as f64),
                    Field::num("rect[3]", "Height", h as f64),
                    Field::choice("shape", "Shape", &[("Rectangle", json!("rect")), ("Ellipse", json!("ellipse"))], 0),
                ],
            )
        }
        "layer.autoOrient" if !has(p, &["mode"]) => {
            let cur = layer.map(|l| l.auto_orient as usize).unwrap_or(0);
            (
                "Auto-Orientation".into(),
                vec![Field::choice(
                    "mode",
                    "Mode",
                    &[
                        ("Off", json!("off")),
                        ("Orient Along Path", json!("alongPath")),
                        ("Orient Towards Camera", json!("towardsCamera")),
                        ("Orient Towards Point of Interest", json!("towardsPointOfInterest")),
                    ],
                    cur.min(3),
                )],
            )
        }
        "time.set" if !has(p, &["time", "frame", "timecode"]) => {
            let frame = comp.map(|c| c.frame_rate.frame_at(t)).unwrap_or(0);
            ("Go to Time".into(), vec![Field::num("frame", "Frame", frame as f64)])
        }
        "view.addGuide" if !has(p, &["position"]) => {
            let (w, _) = comp.map(|c| (c.width, c.height)).unwrap_or((1920, 1080));
            (
                "Add Guide".into(),
                vec![
                    Field::choice("orientation", "Orientation", &[("Vertical", json!("vertical")), ("Horizontal", json!("horizontal"))], 0),
                    Field::num("position", "Position (pixels)", w as f64 / 2.0),
                ],
            )
        }
        // Several items selected: After Effects' New Composition from Selection dialog.
        "file.newCompFromSelection"
            if s.state.project_selection.len() > 1 && !has(p, &["single", "dimensionsFrom", "duration", "sequence", "addToRenderQueue"]) =>
        {
            let names: Vec<(String, Value)> =
                s.state.project_selection.iter().enumerate().filter_map(|(i, id)| s.project.item(*id).map(|it| (it.name.clone(), json!(i)))).collect();
            let names: Vec<(&str, Value)> = names.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();
            let still = if s.prefs.import.still_footage == "seconds" { s.prefs.import.still_seconds } else { 10.0 };
            (
                "New Composition from Selection".into(),
                vec![
                    Field::choice("single", "Create", &[("Single Composition", json!(true)), ("Multiple Compositions", json!(false))], 0),
                    Field::choice("dimensionsFrom", "Use Dimensions From", &names, 0),
                    Field::num("duration", "Still Duration (seconds)", still),
                    Field::bool("addToRenderQueue", "Add to Render Queue", false),
                    Field::bool("sequence", "Sequence Layers", false),
                    Field::bool("overlap", "Overlap", false),
                    Field::num("overlapDuration", "Duration (seconds)", 1.0),
                    Field::choice(
                        "transition",
                        "Transition",
                        &[
                            ("Off", json!("off")),
                            ("Dissolve Front Layer", json!("dissolveFront")),
                            ("Cross Dissolve Front and Back Layers", json!("crossDissolve")),
                        ],
                        0,
                    ),
                ],
            )
        }
        "layer.sequence" if !has(p, &["overlap"]) => (
            "Sequence Layers".into(),
            vec![
                Field::bool("overlap", "Overlap", false),
                Field::num("duration", "Duration (seconds)", 1.0),
                Field::choice(
                    "transition",
                    "Transition",
                    &[
                        ("Off", json!("off")),
                        ("Dissolve Front Layer", json!("dissolveFront")),
                        ("Cross Dissolve Front and Back Layers", json!("crossDissolve")),
                    ],
                    0,
                ),
            ],
        ),
        "file.interpretFootage" | "file.interpretProxy"
            if !has(p, &["frameRate", "alpha", "loop", "pixelAspect", "colorProfile", "fields", "invertAlpha", "matteColor", "linearLight", "startFrame"]) =>
        {
            let proxy = id == "file.interpretProxy";
            let f = s.state.project_selection.first().and_then(|i| s.project.item(*i)).and_then(|it| match (&it.kind, &it.proxy) {
                (_, Some(px)) if proxy => Some(px.footage.clone()),
                (effectcraft_engine::project::ItemKind::Footage(f), _) if !proxy => Some(f.clone()),
                _ => None,
            });
            let Some(f) = f else { return false };
            let hex = |c: [f32; 3]| format!("#{:02x}{:02x}{:02x}", (c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8);
            let mut fields = vec![
                // Main Options ▸ Alpha.
                Field::choice(
                    "alpha",
                    "Alpha",
                    &[
                        ("Interpret Straight - Unmatted", json!("straight")),
                        ("Interpret Premultiplied - Matted With Color", json!("premultiplied")),
                        ("Ignore", json!("ignore")),
                        ("Guess", json!("guess")),
                    ],
                    f.alpha as usize,
                ),
                Field::text("matteColor", "Matte color (premultiplied)", &hex(f.premul_color)),
                Field::bool("invertAlpha", "Invert Alpha", f.invert_alpha),
                // Main Options ▸ Frame Rate, Fields and Pulldown, Other Options.
                Field::num("frameRate", "Assume this frame rate", f.frame_rate.as_f64()),
                Field::choice(
                    "fields",
                    "Separate Fields",
                    &[("Off", json!("off")), ("Upper Field First", json!("upper")), ("Lower Field First", json!("lower"))],
                    f.fields as usize,
                ),
                Field::num("pixelAspect", "Pixel Aspect Ratio", f.pixel_aspect),
                Field::num("loop", "Loop (times)", f.loop_count as f64),
                // Color.
                Field::choice(
                    "colorProfile",
                    "Assign Profile",
                    &[
                        ("Embedded / sRGB", json!("auto")),
                        ("sRGB IEC61966-2.1", json!("srgb")),
                        ("HDTV (Rec. 709)", json!("rec709")),
                        ("Rec. 2020", json!("rec2020")),
                        ("Display P3", json!("p3")),
                        ("ACEScg", json!("acescg")),
                        ("ACES2065-1", json!("aces2065")),
                        ("Rec. 2100 PQ", json!("rec2100pq")),
                        ("Rec. 2100 HLG", json!("rec2100hlg")),
                    ],
                    f.color_profile.map_or(0, |c| 1 + effectcraft_engine::project::ColorSpace::ALL.iter().position(|x| *x == c).unwrap_or(0)),
                ),
                Field::bool("linearLight", "Interpret As Linear Light", f.linear_light),
            ];
            // Image sequences: their frames (missing ones show colour bars, as in After Effects)
            // and the first frame's number.
            if f.kind == effectcraft_engine::project::FootageKind::Sequence {
                let at = fields.iter().position(|x| x.key == "frameRate").map_or(fields.len(), |i| i + 1);
                let gaps = match effectcraft_engine::sequence::missing_report(&f.sequence) {
                    _ if f.alphabetical => "files in alphabetical order".into(),
                    Some(gaps) => format!("{gaps} shown as color bars"),
                    None => "no missing frames".into(),
                };
                fields.splice(
                    at..at,
                    [
                        Field::note("sequenceInfo", &format!("Image sequence: {} files, {} frames; {gaps}.", f.sequence.len(), f.sequence_frames())),
                        Field::num("startFrame", "Start Frame", f.first_frame_number() as f64),
                    ],
                );
            }
            (if proxy { "Interpret Footage: Proxy".into() } else { "Interpret Footage".into() }, fields)
        }
        "file.projectSettings" if p.as_object().is_none_or(|m| m.is_empty()) => {
            let st = &s.project.settings;
            let gpu_label = match &s.accel {
                Some(a) => format!("Mercury GPU Acceleration ({})", a.name()),
                None => "Mercury GPU Acceleration (no GPU: software)".to_string(),
            };
            let depth = match st.bit_depth.label() {
                l if l.starts_with("16") => 1,
                l if l.starts_with("32") => 2,
                _ => 0,
            };
            (
                "Project Settings".into(),
                vec![
                    Field::choice(
                        "bitDepth",
                        "Depth",
                        &[("8 bits per channel", json!(8)), ("16 bits per channel", json!(16)), ("32 bits per channel (float)", json!(32))],
                        depth,
                    ),
                    Field::choice(
                        "timeDisplay",
                        "Time display style",
                        &[
                            ("Timecode", json!("timecode")),
                            ("Frames", json!("frames")),
                            ("Feet + Frames (35mm)", json!("feet35")),
                            ("Feet + Frames (16mm)", json!("feet16")),
                        ],
                        {
                            use effectcraft_engine::project::TimeDisplayStyle as T;
                            match st.time_display {
                                T::Timecode => 0,
                                T::Frames => 1,
                                T::Feet35 => 2,
                                T::Feet16 => 3,
                            }
                        },
                    ),
                    Field::choice(
                        "colorEngine",
                        "Color engine",
                        &[("Adobe-style built-in", json!("adobe")), ("OCIO (built-in ACES config)", json!("ocio"))],
                        usize::from(st.color_engine == effectcraft_engine::project::ColorEngine::Ocio),
                    ),
                    Field::choice(
                        "workingSpace",
                        "Working space",
                        &[
                            ("None", json!("none")),
                            ("sRGB IEC61966-2.1", json!("srgb")),
                            ("HDTV (Rec. 709)", json!("rec709")),
                            ("Rec. 2020", json!("rec2020")),
                            ("Display P3", json!("p3")),
                            ("ACEScg", json!("acescg")),
                            ("ACES2065-1", json!("aces2065")),
                        ],
                        st.working_space.map_or(0, |c| 1 + effectcraft_engine::project::ColorSpace::WORKING.iter().position(|x| *x == c).unwrap_or(0)),
                    ),
                    Field::bool("linearize", "Linearize working space", st.linearize),
                    Field::bool("blendLinear", "Blend colors using 1.0 gamma", st.blend_linear),
                    Field::choice(
                        "hdr",
                        "HDR on SDR displays and outputs",
                        &[("Clip", json!("clip")), ("Compand", json!("compand")), ("Tone map", json!("toneMap"))],
                        st.hdr as usize,
                    ),
                    Field::choice(
                        "outputSpace",
                        "Output color space",
                        &[
                            ("sRGB IEC61966-2.1 (default)", json!("default")),
                            ("HDTV (Rec. 709)", json!("rec709")),
                            ("Rec. 2020", json!("rec2020")),
                            ("Display P3", json!("p3")),
                            ("Rec. 2100 PQ (HDR)", json!("rec2100pq")),
                            ("Rec. 2100 HLG (HDR)", json!("rec2100hlg")),
                        ],
                        {
                            use effectcraft_engine::project::ColorSpace as C;
                            match st.output_space {
                                Some(C::Rec709) => 1,
                                Some(C::Rec2020) => 2,
                                Some(C::DisplayP3) => 3,
                                Some(C::Rec2100Pq) => 4,
                                Some(C::Rec2100Hlg) => 5,
                                _ => 0,
                            }
                        },
                    ),
                    // Video Rendering and Effects ▸ Use.
                    Field::choice(
                        "renderer",
                        "Video rendering and effects",
                        &[(gpu_label.as_str(), json!("gpu")), ("Mercury Software Only", json!("software"))],
                        usize::from(!st.gpu_acceleration),
                    ),
                ],
            )
        }
        "layer.autoTrace" if !has(p, &["channel", "timeSpan", "threshold"]) => (
            "Auto-trace".into(),
            vec![
                Field::choice("timeSpan", "Time Span", &[("Current Frame", json!("currentFrame")), ("Work Area", json!("workArea"))], 0),
                Field::choice(
                    "channel",
                    "Channel",
                    &[("Alpha", json!("alpha")), ("Red", json!("red")), ("Green", json!("green")), ("Blue", json!("blue")), ("Luminance", json!("luminance"))],
                    0,
                ),
                Field::bool("invert", "Invert", false),
                Field::num("blur", "Blur (pixels before auto-trace)", 1.0),
                Field::num("tolerance", "Tolerance (pixels)", 1.0),
                Field::num("threshold", "Threshold (%)", 50.0),
                Field::num("minimumArea", "Minimum Area (pixels)", 10.0),
                Field::num("cornerRoundness", "Corner Roundness (%)", 50.0),
                Field::bool("applyToNewLayer", "Apply To New Layer", false),
            ],
        ),
        "layer.sceneEditDetection" if !has(p, &["mode"]) => (
            "Scene Edit Detection".into(),
            vec![
                Field::choice(
                    "mode",
                    "Action",
                    &[("Create Markers", json!("markers")), ("Split Layers", json!("split")), ("Split and Precompose", json!("splitPrecompose"))],
                    0,
                ),
                Field::num("threshold", "Sensitivity threshold (0–1, lower finds more)", 0.25),
            ],
        ),
        "layer.alignVideoToData" if !has(p, &["data", "videoStart"]) => {
            let data: Vec<(String, Value)> = s
                .project
                .items
                .values()
                .filter(|i| matches!(&i.kind, effectcraft_engine::project::ItemKind::Footage(f) if f.kind == effectcraft_engine::project::FootageKind::Data))
                .map(|i| (i.name.clone(), json!(i.id.0)))
                .collect();
            if data.is_empty() {
                info(app, "Align Video to Data", "Import a data file (JSON, CSV or TSV with a time column) first: File ▸ Import.");
                return true;
            }
            let opts: Vec<(&str, Value)> = data.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();
            (
                "Align Video to Data".into(),
                vec![
                    Field::choice("data", "Data", &opts, 0),
                    Field::text("key", "Time field (empty: automatic)", ""),
                    Field::text("videoStart", "Video start (ISO date-time, hh:mm:ss, timecode; empty: file date)", ""),
                    Field::num("dataStart", "First sample at (comp seconds)", 0.0),
                ],
            )
        }
        // File ▸ Export ▸ Adobe Premiere Pro Project…: Final Cut Pro XML (or FCPXML / OTIO / EDL).
        "file.exportTimeline" if !has(p, &["path"]) => {
            let name = s.active_comp_id().and_then(|c| s.project.item(c)).map(|i| i.name.clone()).unwrap_or_else(|| "Composition".into());
            let dir = s.path.as_deref().and_then(|p| std::path::Path::new(p).parent()).map(|d| d.to_path_buf()).unwrap_or_default();
            let path = dir.join(format!("{name}.xml")).to_string_lossy().to_string();
            (
                "Export Adobe Premiere Pro Project".into(),
                vec![
                    Field::note("note", "Writes Final Cut Pro XML (.xml), which Premiere Pro opens with File ▸ Import. Native .prproj files are not written."),
                    Field::save_path("path", "File", &path),
                    Field::choice(
                        "format",
                        "Format",
                        &[
                            ("Final Cut Pro XML (.xml) – Premiere Pro", json!("xml")),
                            ("FCPXML (.fcpxml)", json!("fcpxml")),
                            ("OpenTimelineIO (.otio)", json!("otio")),
                            ("CMX 3600 EDL (.edl, V1 only)", json!("edl")),
                            ("AAF (.aaf)", json!("aaf")),
                            ("OMF (.omf, audio)", json!("omf")),
                        ],
                        0,
                    ),
                    Field::choice(
                        "prerender",
                        "Pre-render (ProRes 4444)",
                        &[
                            ("Layers Premiere can't show (text, shapes, effects…)", json!("unsupported")),
                            ("All layers", json!("all")),
                            ("None (leave them out)", json!("none")),
                        ],
                        0,
                    ),
                    Field::choice("precomps", "Precomps", &[("Nested sequences", json!("nest")), ("Pre-render", json!("prerender"))], 0),
                ],
            )
        }
        "file.importPlaceholder" | "file.replaceWithPlaceholder" if p.as_object().is_none_or(|m| m.is_empty()) => (
            "New Placeholder".into(),
            vec![
                Field::text("name", "Name", "Placeholder"),
                Field::num("width", "Width", 1920.0),
                Field::num("height", "Height", 1080.0),
                Field::num("frameRate", "Frame rate", 29.97),
                Field::num("duration", "Duration (seconds)", 30.0),
            ],
        ),
        "file.importSolid" | "file.replaceWithSolid" if p.as_object().is_none_or(|m| m.is_empty()) => {
            let (w, h) = comp.map(|c| (c.width, c.height)).unwrap_or((1920, 1080));
            (
                "Solid Settings".into(),
                vec![
                    Field::text("name", "Name", "Solid"),
                    Field::text("color", "Color (#rrggbb)", "#808080"),
                    Field::num("width", "Width", w as f64),
                    Field::num("height", "Height", h as f64),
                ],
            )
        }
        // File ▸ Save as Template…
        "templates.saveAs" if !has(p, &["name"]) => {
            let name = s
                .path
                .as_deref()
                .and_then(|p| std::path::Path::new(p).file_stem().map(|n| n.to_string_lossy().to_string()))
                .or_else(|| s.active_comp_id().and_then(|c| s.project.item(c)).map(|i| i.name.clone()))
                .unwrap_or_else(|| "My Template".into());
            (
                "Save as Template".into(),
                vec![
                    Field::text("name", "Template name", &name),
                    Field::text("description", "Description", ""),
                    Field::text("category", "Category", "My Templates"),
                    Field::bool("embedFootage", "Embed footage files", true),
                    Field::num("embedLimitMB", "Embed up to (MB)", effectcraft_engine::templates::EMBED_LIMIT_MB),
                ],
            )
        }
        // View ▸ Simulate Output ▸ My Custom RGB…
        "view.customRgb" if p.as_object().is_none_or(|m| m.is_empty()) => {
            let c = &s.prefs.custom_rgb;
            let num = |k: &str, l: &str, v: f64| Field { key: k.into(), label: l.into(), kind: FieldKind::Number { value: v, speed: 0.001 } };
            (
                "My Custom RGB".into(),
                vec![
                    Field::text("name", "Name", &c.name),
                    num("red[0]", "Red x", c.red[0]),
                    num("red[1]", "Red y", c.red[1]),
                    num("green[0]", "Green x", c.green[0]),
                    num("green[1]", "Green y", c.green[1]),
                    num("blue[0]", "Blue x", c.blue[0]),
                    num("blue[1]", "Blue y", c.blue[1]),
                    num("white[0]", "White point x", c.white[0]),
                    num("white[1]", "White point y", c.white[1]),
                    num("gamma", "Gamma", c.gamma),
                    Field::bool("srgbCurve", "Use the sRGB curve (ignore Gamma)", c.srgb_curve),
                    Field::path("icc", "ICC profile (replaces the numbers)", &c.icc, &["icc", "icm"]),
                    Field::bool("preserveRgb", "Preserve RGB", s.state.viewer.simulation.preserve_rgb),
                ],
            )
        }
        _ => return false,
    };
    form(app, &title, id, base, fields);
    true
}

pub fn show_form(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut f = app.dialog_state.form.clone();
    let (mut ok, mut close) = (false, false);
    // Rows are 30 px; a note wraps at about 40 characters a line, so OK and Cancel stay in view.
    let rows: f32 = f
        .fields
        .iter()
        .map(|fl| match &fl.kind {
            FieldKind::Note(text) => 10.0 + 15.0 * (text.chars().count() as f32 / 40.0).ceil().max(1.0),
            _ => 30.0,
        })
        .sum();
    let h = 120.0 + rows;
    let mut regs: Vec<(String, egui::Rect, String)> = vec![];
    let mut browse: Option<usize> = None;
    // Wide enough for a label column and a 240 px popup without a horizontal scroll bar.
    super::dialogs::modal(ctx, &f.title.clone(), vec2(480.0, h), t, |ui| {
        egui::Grid::new("form-grid").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
            for (i, fl) in f.fields.iter_mut().enumerate() {
                ui.label(&fl.label);
                let r = match &mut fl.kind {
                    FieldKind::Number { value, speed } => {
                        let dec = if *speed < 0.01 { 5 } else { 3 };
                        ui.add(egui::DragValue::new(value).speed(*speed).max_decimals(dec)).rect
                    }
                    FieldKind::Text(s) => ui.add(egui::TextEdit::singleline(s).desired_width(220.0)).rect,
                    FieldKind::Path { value, .. } => {
                        ui.horizontal(|ui| {
                            let r = ui.add(egui::TextEdit::singleline(value).desired_width(150.0)).rect;
                            let b = ui.button("Browse…");
                            regs.push((format!("form.field.{}.browse", fl.key), b.rect, "Browse…".into()));
                            if b.clicked() {
                                browse = Some(i);
                            }
                            r
                        })
                        .inner
                    }
                    FieldKind::SavePath(value) => {
                        ui.horizontal(|ui| {
                            let r = ui.add(egui::TextEdit::singleline(value).desired_width(150.0)).rect;
                            let b = ui.button("Browse…");
                            regs.push((format!("form.field.{}.browse", fl.key), b.rect, "Browse…".into()));
                            if b.clicked() {
                                browse = Some(i);
                            }
                            r
                        })
                        .inner
                    }
                    // A fixed width: in the grid's value column a wrapping label would otherwise
                    // shrink to a word per line.
                    FieldKind::Note(text) => {
                        ui.scope(|ui| {
                            ui.set_width(220.0);
                            ui.add(egui::Label::new(egui::RichText::new(text.as_str()).small()).wrap())
                        })
                        .inner
                        .rect
                    }
                    FieldKind::Bool(b) => ui.checkbox(b, "").rect,
                    FieldKind::Choice { options, sel } => {
                        let cur = options.get(*sel).map(|o| o.0.clone()).unwrap_or_default();
                        egui::ComboBox::from_id_salt(("form", fl.key.as_str()))
                            .selected_text(cur)
                            .width(240.0)
                            .show_ui(ui, |ui| {
                                for (i, (l, _)) in options.iter().enumerate() {
                                    ui.selectable_value(sel, i, l);
                                }
                            })
                            .response
                            .rect
                    }
                };
                regs.push((format!("form.field.{}", fl.key), r, fl.label.clone()));
                ui.end_row();
            }
        });
        ui.add_space(14.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = ui.add(egui::Button::new(egui::RichText::new("   OK   ").color(Color32::WHITE)).fill(t.accent));
            regs.push(("form.ok".into(), b.rect, "OK".into()));
            if b.clicked() {
                ok = true;
            }
            let c = ui.button("Cancel");
            regs.push(("form.cancel".into(), c.rect, "Cancel".into()));
            if c.clicked() {
                close = true;
            }
        });
    });
    for (id, r, l) in regs {
        app.auto.add(&id, r, &l);
    }
    if let Some(i) = browse
        && let Some(FieldKind::Path { value, exts }) = f.fields.get_mut(i).map(|f| &mut f.kind)
    {
        let exts: Vec<&str> = exts.iter().map(String::as_str).collect();
        match app.hooks.pick_files.as_ref() {
            Some(pick) => {
                if let Some(path) = pick(&exts).into_iter().next() {
                    *value = path;
                }
            }
            None => app.ui.status = "no file dialog available: type the path".into(),
        }
    }
    if let Some(i) = browse
        && let Some(FieldKind::SavePath(value)) = f.fields.get_mut(i).map(|f| &mut f.kind)
    {
        match app.hooks.save_dialog(value) {
            Some(Some(path)) => *value = path,
            Some(None) => {}
            None => app.ui.status = "no file dialog available: type the path".into(),
        }
    }
    let enter = !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Enter));
    app.dialog_state.form = f.clone();
    if ok || enter {
        app.dialog = None;
        let params = f.params();
        if let Err(e) = app.session.execute(&f.command, params) {
            app.ui.status = e.to_string();
        }
        return;
    }
    if close {
        app.dialog = None;
    }
}

pub fn show_info(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let (title, body) = app.dialog_state.info.clone();
    let mut close = false;
    super::dialogs::modal(ctx, &title, vec2(440.0, 160.0), t, |ui| {
        ui.label(body);
        ui.add_space(16.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(egui::RichText::new("   OK   ").color(Color32::WHITE)).fill(t.accent)).clicked() {
                close = true;
            }
        });
    });
    if close {
        app.dialog = None;
    }
}

pub fn show_view_options(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut close = false;
    super::dialogs::modal(ctx, "View Options", vec2(360.0, 330.0), t, |ui| {
        let v = &mut app.ui.viewer;
        ui.checkbox(&mut v.show_layer_controls, "Layer controls");
        ui.checkbox(&mut v.show_masks, "Masks");
        ui.checkbox(&mut v.rulers, "Rulers");
        ui.checkbox(&mut v.guides, "Guides");
        ui.checkbox(&mut v.grid, "Grid");
        ui.checkbox(&mut v.safe_margins, "Title/action safe");
        ui.checkbox(&mut v.transparency_grid, "Transparency grid");
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(egui::RichText::new("   OK   ").color(Color32::WHITE)).fill(t.accent)).clicked() {
                close = true;
            }
        });
    });
    if close {
        app.dialog = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_params_build_arrays_and_choices() {
        let f = Form {
            title: "T".into(),
            command: "layer.setTransform".into(),
            base: json!({"prop": "position"}),
            fields: vec![
                Field::num("value[0]", "X", 10.0),
                Field::num("value[1]", "Y", 20.0),
                Field::choice("mode", "M", &[("A", json!("a")), ("B", json!("b"))], 1),
            ],
        };
        assert_eq!(f.params(), json!({"prop": "position", "value": [10.0, 20.0], "mode": "b"}));
    }

    /// File ▸ Export ▸ Adobe Premiere Pro Project… opens a form that says it writes Final Cut Pro
    /// XML; its fields register automation ids and OK runs `file.exportTimeline`.
    #[test]
    fn premiere_export_dialog() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, &app.tokens);
        app.session.execute("comp.new", json!({"name": "Spot", "width": 64, "height": 36, "frameRate": 25, "duration": 1})).unwrap();
        app.session.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
        crate::menus::invoke(&mut app, &ctx, "file.exportTimeline", json!({})).unwrap();
        assert!(matches!(app.dialog, Some(Dialog::Form)));
        let f = &app.dialog_state.form;
        assert_eq!(f.command, "file.exportTimeline");
        assert!(matches!(&f.fields[0].kind, FieldKind::Note(t) if t.contains("Final Cut Pro XML (.xml)")));
        let dir = std::env::temp_dir().join(format!("effectcraft-ui-premiere-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Spot.xml").to_string_lossy().to_string();
        app.dialog_state.form.fields[1].kind = FieldKind::SavePath(path.clone());
        ctx.run_ui(Default::default(), |ui| {
            app.auto.begin_frame();
            crate::panels::dialogs::show(&mut app, ui.ctx());
        })
        .textures_delta
        .clear();
        for id in ["form.field.path", "form.field.path.browse", "form.field.format", "form.field.prerender", "form.field.precomps", "form.ok"] {
            assert!(app.auto.find(id).is_some(), "{id}");
        }
        let params = app.dialog_state.form.params();
        assert!(params.get("note").is_none());
        assert_eq!(params["format"], "xml");
        let r = app.session.execute("file.exportTimeline", params).unwrap();
        assert_eq!(r["path"], path.as_str());
        assert!(std::fs::read_to_string(&path).unwrap().contains("<xmeml"));
    }
}
