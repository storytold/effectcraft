//! `cargo xtask bench`: a repeatable render benchmark (docs/testing.md "Render benchmark", gaps.md G5).
//!
//! Generates deterministic projects into `target/bench/projects` with `effectcraft-cli` (engine
//! commands, no committed media), renders each at Full and Half plus one single frame, and measures
//! every run with `/usr/bin/time` (macOS `-l`: peak memory footprint, max RSS, instructions
//! retired; Linux `-v`: max RSS; elsewhere wall time only). Prints a table and appends one JSON
//! line per run to `target/bench/results.jsonl`; `--compare OLD.jsonl` prints the % change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use serde_json::{Value, json};

const HELP: &str = "usage: cargo xtask bench [options]

Renders generated scenarios with effectcraft-cli and records memory, CPU and wall time.
Run it under `nice -n 10` on a busy machine; the table notes the load average of every run.

  --bin PATH            use this effectcraft-cli binary. Without it, effectcraft-cli is built once
                        with `cargo build --release` into target/bench (its own target dir:
                        the first build compiles everything, later ones are incremental)
  --scenario a,b        only these scenarios: demo, layers, precomps, footage (default: all)
  --project F --comp C  benchmark this project's comp (name or id) instead of the scenarios
  --start S             first frame time in seconds for --project (default 0)
  --frames N            frames per Full / Half render (default 60; --quick: 8)
  --quick               fewer frames, for a fast check
  --format F            render format (default png: a numbered sequence, deleted after each run)
  --regen               regenerate the footage media (otherwise reused when present)
  --compare OLD.jsonl   print the % change of this run against the latest matching runs in OLD
  --with NEW.jsonl      with --compare: compare two result files, without rendering
  --fail-over PCT       with --compare: exit 1 when a matched run got more than PCT % worse in
                        CPU time, peak memory, max RSS or instructions retired (wall time is
                        left out: it follows the machine's load)

Scenarios: demo (the built-in demo comp), layers (1080p, 100 solid/text/shape layers with Gaussian
Blur, Glow, Levels, Curves, Drop Shadow and keyframes), precomps (static and animated precomps
nested three deep, each used several times), footage (a generated PNG sequence and H.264 clip on
several layers at 1080p, plus a 4K still). Each renders Full, Half and one frame (`frame`); `open`
is the cost of loading the project alone.";

// ---------------------------------------------------------------------------------------- options

#[derive(Debug, Default)]
struct Opts {
    bin: Option<PathBuf>,
    scenarios: Option<Vec<String>>,
    project: Option<PathBuf>,
    comp: Option<String>,
    start: f64,
    frames: Option<u32>,
    quick: bool,
    format: String,
    regen: bool,
    compare: Option<PathBuf>,
    with: Option<PathBuf>,
    fail_over: Option<f64>,
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { format: "png".into(), ..Default::default() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--bin" => o.bin = Some(PathBuf::from(val("--bin")?)),
            "--scenario" | "--scenarios" => o.scenarios = Some(val("--scenario")?.split(',').map(|s| s.trim().to_string()).collect()),
            "--project" => o.project = Some(PathBuf::from(val("--project")?)),
            "--comp" => o.comp = Some(val("--comp")?),
            "--start" => o.start = val("--start")?.parse().map_err(|_| "--start: seconds expected")?,
            "--frames" => o.frames = Some(val("--frames")?.parse().map_err(|_| "--frames: a number expected")?),
            "--quick" => o.quick = true,
            "--format" => o.format = val("--format")?,
            "--regen" => o.regen = true,
            "--compare" => o.compare = Some(PathBuf::from(val("--compare")?)),
            "--with" => o.with = Some(PathBuf::from(val("--with")?)),
            "--fail-over" => {
                o.fail_over = Some(val("--fail-over")?.parse::<f64>().ok().filter(|p| p.is_finite() && *p >= 0.0).ok_or("--fail-over: a percentage expected")?)
            }
            other => return Err(format!("unknown option `{other}`\n\n{HELP}")),
        }
    }
    if o.project.is_some() != o.comp.is_some() {
        return Err("--project and --comp go together".into());
    }
    if o.with.is_some() && o.compare.is_none() {
        return Err("--with needs --compare OLD.jsonl".into());
    }
    if o.fail_over.is_some() && o.compare.is_none() {
        return Err("--fail-over needs --compare OLD.jsonl".into());
    }
    if o.frames == Some(0) {
        return Err("--frames must be at least 1".into());
    }
    Ok(o)
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| matches!(a.as_str(), "-h" | "--help" | "help")) {
        println!("{HELP}");
        return Ok(());
    }
    let o = parse_opts(args)?;
    if let (Some(old), Some(new)) = (&o.compare, &o.with) {
        let (old, new) = (latest_per_key(&read_jsonl(old)?), latest_per_key(&read_jsonl(new)?));
        print!("{}", compare_table(&old, &new));
        return check_regressions(&old, &new, o.fail_over);
    }
    let dir = root.join("target").join("bench");
    std::fs::create_dir_all(dir.join("projects")).map_err(|e| format!("{}: {e}", dir.display()))?;
    let bin = match &o.bin {
        Some(b) => b.clone(),
        None => build_cli(root, &dir)?,
    };
    if !bin.exists() {
        return Err(format!("{}: no such binary", bin.display()));
    }
    let frames = o.frames.unwrap_or(if o.quick { 8 } else { 60 });
    let scenarios = match (&o.project, &o.comp) {
        (Some(p), Some(c)) => vec![user_project(&bin, p, c, o.start)?],
        _ => {
            let want = o.scenarios.clone().unwrap_or_else(|| SCENARIOS.iter().map(|s| s.to_string()).collect());
            let mut v = Vec::new();
            for name in &want {
                eprintln!("bench: generating {name}");
                v.push(generate(&bin, &dir, name, o.regen)?);
            }
            v
        }
    };
    let timer = TimeTool::detect();
    let ctx = RunContext::new(root, &bin, &o);
    let results_path = dir.join("results.jsonl");
    let mut rows = Vec::new();
    for sc in &scenarios {
        for kind in RUNS {
            eprintln!("bench: {} {kind}", sc.name);
            let row = measure(&bin, &dir, sc, kind, frames, &o.format, timer, &ctx)?;
            append_line(&results_path, &row)?;
            rows.push(row);
        }
    }
    print!("{}", table(&rows, timer));
    println!("appended {} runs to {}", rows.len(), results_path.display());
    if let Some(old) = &o.compare {
        let (old, new) = (latest_per_key(&read_jsonl(old)?), latest_per_key(&rows));
        print!("{}", compare_table(&old, &new));
        return check_regressions(&old, &new, o.fail_over);
    }
    Ok(())
}

/// `cargo build -p effectcraft-cli --release` into `target/bench`.
fn build_cli(root: &Path, dir: &Path) -> Result<PathBuf, String> {
    eprintln!("bench: building effectcraft-cli (--release) into {} (one build; pass --bin to skip)", dir.display());
    let st = Command::new(env!("CARGO"))
        .current_dir(root)
        .env("CARGO_TARGET_DIR", dir)
        .args(["build", "-p", "effectcraft-cli", "--release"])
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("building effectcraft-cli failed".into());
    }
    Ok(dir.join("release").join(format!("effectcraft-cli{}", std::env::consts::EXE_SUFFIX)))
}

// -------------------------------------------------------------------------------------- scenarios

const SCENARIOS: &[&str] = &["demo", "layers", "precomps", "footage"];
/// `open` loads the project only; `full` / `half` render `frames` frames; `frame` renders one.
const RUNS: &[&str] = &["open", "full", "half", "frame"];

struct Scenario {
    name: String,
    /// `--demo` or `--project F`.
    source: Vec<String>,
    comp: String,
    fps: f64,
    start: f64,
}

type Cmds = Vec<(&'static str, Value)>;

/// A deterministic value in [0, 1) for layer `i`, stream `k`.
fn rnd(i: u32, k: u32) -> f64 {
    let mut h = i.wrapping_mul(0x9E37_79B9) ^ k.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    f64::from(h % 10_000) / 10_000.0
}

fn hex(i: u32) -> String {
    let c = |k| (40.0 + rnd(i, k) * 215.0) as u8;
    format!("#{:02x}{:02x}{:02x}", c(11), c(12), c(13))
}

fn comp(name: &str, w: u32, h: u32, duration: f64) -> (&'static str, Value) {
    ("comp.new", json!({"name": name, "width": w, "height": h, "frameRate": 30, "duration": duration}))
}

fn key(path: &str, time: f64, value: Value) -> (&'static str, Value) {
    ("prop.addKey", json!({"path": path, "time": time, "value": value}))
}

fn set(path: &str, value: Value) -> (&'static str, Value) {
    ("prop.set", json!({"path": path, "value": value}))
}

fn effect(id: &str) -> (&'static str, Value) {
    ("effect.apply", json!({"effect": id}))
}

/// Position keys from a start to an end point across `dur` seconds.
fn moving(c: &mut Cmds, i: u32, w: f64, h: f64, dur: f64) {
    c.push(key("transform/position", 0.0, json!([rnd(i, 1) * w, rnd(i, 2) * h, 0])));
    c.push(key("transform/position", dur, json!([rnd(i, 3) * w, rnd(i, 4) * h, 0])));
}

/// The common effects, by layer: blur (animated), glow, levels + curves, drop shadow, blur + shadow.
fn common_effects(c: &mut Cmds, i: u32) {
    match i % 5 {
        0 => {
            c.push(effect("ec.blur.gaussian"));
            c.push(key("effects/#1/blurriness", 0.0, json!(2.0 + rnd(i, 5) * 10.0)));
            c.push(key("effects/#1/blurriness", 4.0, json!(10.0 + rnd(i, 6) * 20.0)));
        }
        1 => {
            c.push(effect("ec.stylize.glow"));
            c.push(set("effects/#1/radius", json!(20.0)));
            c.push(set("effects/#1/threshold", json!(40.0)));
        }
        2 => {
            c.push(effect("ec.color.levels"));
            c.push(set("effects/#1/gamma", json!(1.3)));
            c.push(effect("ec.color.curves"));
            c.push(set("effects/#2/rgb", json!("0,0 0.4,0.55 1,1")));
        }
        3 => {
            c.push(effect("ec.perspective.dropshadow"));
            c.push(set("effects/#1/distance", json!(12.0)));
            c.push(set("effects/#1/softness", json!(20.0)));
        }
        _ => {
            c.push(effect("ec.blur.gaussian"));
            c.push(set("effects/#1/blurriness", json!(6.0)));
            c.push(effect("ec.perspective.dropshadow"));
            c.push(set("effects/#2/softness", json!(10.0)));
        }
    }
}

/// One solid, text or shape layer (by `i % 3`) with keyframes.
fn graphic_layer(c: &mut Cmds, i: u32, w: f64, h: f64, dur: f64) {
    match i % 3 {
        0 => c.push((
            "layer.newSolid",
            json!({"name": format!("Solid {i}"), "color": hex(i), "width": 200 + (rnd(i, 7) * 400.0) as u32, "height": 120 + (rnd(i, 8) * 300.0) as u32}),
        )),
        1 => c.push(("layer.newText", json!({"text": format!("Layer {i}"), "size": 40 + (rnd(i, 7) * 80.0) as u32, "fill": hex(i)}))),
        _ => {
            let kind = ["rect", "ellipse", "star", "polygon", "rounded"][(i as usize / 3) % 5];
            let s = 80.0 + rnd(i, 7) * 260.0;
            c.push(("layer.newShape", json!({"kind": kind, "size": [s, s], "fill": hex(i), "stroke": "#ffffff", "strokeWidth": 4})));
        }
    }
    moving(c, i, w, h, dur);
    if i.is_multiple_of(2) {
        c.push(key("transform/rotation", 0.0, json!(0.0)));
        c.push(key("transform/rotation", dur, json!(rnd(i, 9) * 360.0 - 180.0)));
    }
    if i.is_multiple_of(3) {
        c.push(key("transform/scale", 0.0, json!([60, 60, 100])));
        c.push(key("transform/scale", dur, json!([120, 120, 100])));
    }
    if i.is_multiple_of(5) {
        c.push(key("transform/opacity", 0.0, json!(30.0)));
        c.push(key("transform/opacity", dur / 2.0, json!(100.0)));
    }
}

/// `layers`: a 1080p comp with 100 layers, the common effects and keyframes.
fn layers_cmds() -> Cmds {
    let mut c = vec![comp("Layers", 1920, 1080, 10.0), ("layer.newSolid", json!({"name": "Background", "color": "#202838"}))];
    for i in 0..100 {
        graphic_layer(&mut c, i, 1920.0, 1080.0, 4.0);
        common_effects(&mut c, i);
    }
    c.push(("layer.newAdjustment", json!({"name": "Grade"})));
    c.push(effect("ec.color.curves"));
    c.push(set("effects/#1/rgb", json!("0,0.03 0.5,0.52 1,0.97")));
    c
}

/// `precomps`: static and animated 960x540 tiles, a 1080p `Mid` using them, and `Main` using
/// `Mid` and the tiles again (three levels deep).
fn precomps_cmds() -> Cmds {
    let mut c = vec![comp("Static Tile", 960, 540, 10.0), ("layer.newSolid", json!({"name": "Tile BG", "color": "#334455"}))];
    for i in 0..6u32 {
        let kind = ["rect", "ellipse", "star"][i as usize % 3];
        c.push(("layer.newShape", json!({"kind": kind, "size": [140, 140], "fill": hex(i), "position": [120 + i * 140, 270]})));
    }
    c.push(("layer.newText", json!({"text": "STATIC", "size": 90, "fill": "#ffffff"})));
    c.push(effect("ec.perspective.dropshadow"));
    c.push(set("effects/#1/softness", json!(15.0)));

    c.push(comp("Anim Tile", 960, 540, 10.0));
    c.push(("layer.newSolid", json!({"name": "Tile BG", "color": "#443322"})));
    for i in 10..18 {
        graphic_layer(&mut c, i, 960.0, 540.0, 3.0);
        if i.is_multiple_of(4) {
            common_effects(&mut c, i);
        }
    }

    c.push(comp("Mid", 1920, 1080, 10.0));
    c.push(("layer.newSolid", json!({"name": "Mid BG", "color": "#101010"})));
    for (n, item) in ["Anim Tile", "Static Tile", "Anim Tile", "Static Tile"].iter().enumerate() {
        let (x, y) = (480 + (n % 2) * 960, 270 + (n / 2) * 540);
        c.push(("layer.addItem", json!({"item": item, "position": [x, y]})));
    }
    c.push(effect("ec.stylize.glow"));
    c.push(set("effects/#1/radius", json!(25.0)));

    c.push(comp("Main", 1920, 1080, 10.0));
    c.push(("layer.newSolid", json!({"name": "Main BG", "color": "#000000"})));
    for (n, item) in ["Mid", "Static Tile", "Mid", "Anim Tile", "Mid", "Static Tile", "Anim Tile"].iter().enumerate() {
        let i = 40 + n as u32;
        c.push(("layer.addItem", json!({"item": item})));
        c.push(set("transform/scale", json!([45, 45, 100])));
        moving(&mut c, i, 1920.0, 1080.0, 5.0);
        if n % 2 == 0 {
            c.push(key("transform/rotation", 0.0, json!(-10.0)));
            c.push(key("transform/rotation", 5.0, json!(10.0)));
        }
        if n == 2 {
            c.push(effect("ec.perspective.dropshadow"));
            c.push(set("effects/#1/softness", json!(30.0)));
        }
    }
    c
}

/// The comps the footage media are rendered from: `Source` (1080p, animated) and `Still 4K`.
fn footage_source_cmds() -> Cmds {
    let mut c = vec![comp("Source", 1920, 1080, 3.0), ("layer.newSolid", json!({"name": "BG", "color": "#183048"}))];
    for i in 60..76 {
        graphic_layer(&mut c, i, 1920.0, 1080.0, 2.0);
    }
    c.push(comp("Still 4K", 3840, 2160, 1.0));
    c.push(("layer.newSolid", json!({"name": "BG", "color": "#402818"})));
    for i in 80..110 {
        graphic_layer(&mut c, i, 3840.0, 2160.0, 1.0);
    }
    c
}

/// `footage`: the PNG sequence and H.264 clip on several layers at 1080p, plus the 4K still.
fn footage_cmds(seq_first: &Path, clip: &Path, still: &Path) -> Cmds {
    let import = |p: &Path, extra: Value| {
        let mut v = json!({"paths": [p.to_string_lossy()], "addToComp": true});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        ("file.import", v)
    };
    // Duplicate the top layer (the one just added) and select the copy (now on top).
    let dup = |c: &mut Cmds| {
        c.push(("edit.duplicate", json!({"layers": ["#1"]})));
        c.push(("layer.select", json!({"layers": ["#1"]})));
    };
    let mut c = vec![comp("Footage", 1920, 1080, 10.0)];
    c.push(import(still, json!({})));
    c.push(key("transform/scale", 0.0, json!([50, 50, 100])));
    c.push(key("transform/scale", 4.0, json!([70, 70, 100])));
    c.push(effect("ec.color.curves"));
    c.push(set("effects/#1/rgb", json!("0,0 0.5,0.6 1,1")));
    c.push(import(clip, json!({})));
    c.push(set("transform/scale", json!([60, 60, 100])));
    moving(&mut c, 200, 1920.0, 1080.0, 4.0);
    dup(&mut c);
    c.push(("layer.setBlendMode", json!({"mode": "Screen"})));
    c.push(effect("ec.blur.gaussian"));
    c.push(set("effects/#1/blurriness", json!(12.0)));
    dup(&mut c);
    moving(&mut c, 201, 1920.0, 1080.0, 4.0);
    c.push(import(seq_first, json!({"sequence": true, "frameRate": 30})));
    c.push(set("transform/scale", json!([50, 50, 100])));
    moving(&mut c, 202, 1920.0, 1080.0, 4.0);
    c.push(effect("ec.color.levels"));
    c.push(set("effects/#1/gamma", json!(1.2)));
    dup(&mut c);
    c.push(set("transform/opacity", json!(60.0)));
    moving(&mut c, 203, 1920.0, 1080.0, 4.0);
    dup(&mut c);
    c.push(effect("ec.perspective.dropshadow"));
    moving(&mut c, 204, 1920.0, 1080.0, 4.0);
    c
}

/// Run `cmds` with `effectcraft-cli --empty run … --save-as out`.
fn build_project(bin: &Path, cmds: &Cmds, out: &Path) -> Result<(), String> {
    let mut cmd = Command::new(bin);
    cmd.arg("--empty").arg("run");
    for (id, params) in cmds {
        cmd.arg(id).arg(params.to_string());
    }
    cmd.arg("--save-as").arg(out);
    cli(&mut cmd)
}

/// Run a CLI command quietly; on failure, the tail of its output is the error.
fn cli(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.stdin(Stdio::null()).output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let tail: Vec<&str> = err.lines().rev().take(8).collect();
    Err(format!("effectcraft-cli failed ({}):\n{}", out.status, tail.into_iter().rev().collect::<Vec<_>>().join("\n")))
}

fn generate(bin: &Path, dir: &Path, name: &str, regen: bool) -> Result<Scenario, String> {
    let projects = dir.join("projects");
    let project = |file: &str, comp: &str| Scenario {
        name: name.into(),
        source: vec!["--project".into(), projects.join(file).to_string_lossy().into_owned()],
        comp: comp.into(),
        fps: 30.0,
        start: 0.0,
    };
    match name {
        "demo" => Ok(Scenario { name: name.into(), source: vec!["--demo".into()], comp: "EffectCraft Intro".into(), fps: 30_000.0 / 1001.0, start: 0.0 }),
        "layers" => {
            build_project(bin, &layers_cmds(), &projects.join("layers.ecproj"))?;
            Ok(project("layers.ecproj", "Layers"))
        }
        "precomps" => {
            build_project(bin, &precomps_cmds(), &projects.join("precomps.ecproj"))?;
            Ok(project("precomps.ecproj", "Main"))
        }
        "footage" => {
            let media = projects.join("media");
            let (seq_dir, clip, still) = (media.join("seq"), media.join("clip.mp4"), media.join("still4k.png"));
            let seq_first = seq_dir.join("src_00000.png");
            if regen || !seq_first.exists() || !clip.exists() || !still.exists() {
                let _ = std::fs::remove_dir_all(&media);
                std::fs::create_dir_all(&seq_dir).map_err(|e| e.to_string())?;
                let src = projects.join("footage-source.ecproj");
                build_project(bin, &footage_source_cmds(), &src)?;
                let render = |args: &[&str], out: &Path| cli(Command::new(bin).arg("--project").arg(&src).args(args).arg("--out").arg(out));
                render(&["render", "--comp", "Source", "--format", "png", "--start", "0", "--end", "2"], &seq_dir.join("src.png"))?;
                render(&["render", "--comp", "Source", "--format", "h264", "--start", "0", "--end", "3"], &clip)?;
                render(&["render-frame", "--comp", "Still 4K", "--time", "0"], &still)?;
                if !seq_first.exists() {
                    return Err(format!("{}: the sequence render wrote no first frame", seq_first.display()));
                }
            }
            build_project(bin, &footage_cmds(&seq_first, &clip, &still), &projects.join("footage.ecproj"))?;
            Ok(project("footage.ecproj", "Footage"))
        }
        other => Err(format!("unknown scenario `{other}` (demo, layers, precomps, footage)")),
    }
}

/// `--project F --comp C`: the comp's frame rate from `info --json`.
fn user_project(bin: &Path, project: &Path, comp: &str, start: f64) -> Result<Scenario, String> {
    let out = Command::new(bin).arg("--project").arg(project).args(["info", "--json"]).output().map_err(|e| e.to_string())?;
    let info: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("info --json: {e}"))?;
    let fps = info["project"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["type"] == "Composition" && (i["name"] == comp || i["id"].as_u64().is_some_and(|id| id.to_string() == comp)))
        .and_then(|i| i["frameRate"].as_f64())
        .filter(|f| *f > 0.0)
        .ok_or_else(|| format!("{}: no comp `{comp}`", project.display()))?;
    let stem = project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Scenario { name: format!("{stem}/{comp}"), source: vec!["--project".into(), project.to_string_lossy().into_owned()], comp: comp.into(), fps, start })
}

// ------------------------------------------------------------------------------------- measuring

#[derive(Clone, Copy, Debug, PartialEq)]
enum TimeTool {
    /// macOS `/usr/bin/time -l`.
    Bsd,
    /// GNU `/usr/bin/time -v`.
    Gnu,
    /// Wall time only.
    None,
}

impl TimeTool {
    fn detect() -> Self {
        if !Path::new("/usr/bin/time").exists() {
            return TimeTool::None;
        }
        if cfg!(target_os = "macos") {
            TimeTool::Bsd
        } else if cfg!(target_os = "linux") {
            TimeTool::Gnu
        } else {
            TimeTool::None
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Metrics {
    wall_s: f64,
    user_s: Option<f64>,
    sys_s: Option<f64>,
    max_rss: Option<u64>,
    peak_footprint: Option<u64>,
    instructions: Option<u64>,
}

/// macOS `/usr/bin/time -l` output: `  0.33 real  0.67 user  0.15 sys`, then `<number>  <label>` rows
/// (max RSS in bytes).
fn parse_bsd(text: &str) -> Option<Metrics> {
    let mut m = Metrics::default();
    let mut seen = false;
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() == 6 && words.get(1) == Some(&"real") && words.get(3) == Some(&"user") && words.get(5) == Some(&"sys") {
            let f = |i: usize| words.get(i).and_then(|w| w.parse::<f64>().ok());
            m.wall_s = f(0)?;
            m.user_s = f(2);
            m.sys_s = f(4);
            seen = true;
            continue;
        }
        let Some((num, label)) = line.trim().split_once(char::is_whitespace) else { continue };
        let Ok(n) = num.parse::<u64>() else { continue };
        match label.trim() {
            "maximum resident set size" => m.max_rss = Some(n),
            "peak memory footprint" => m.peak_footprint = Some(n),
            "instructions retired" => m.instructions = Some(n),
            _ => {}
        }
    }
    seen.then_some(m)
}

/// GNU `/usr/bin/time -v` output: `\tLabel: value` rows (max RSS in kilobytes, wall as h:mm:ss or m:ss).
fn parse_gnu(text: &str) -> Option<Metrics> {
    let mut m = Metrics::default();
    let mut seen = false;
    for line in text.lines() {
        let Some((label, value)) = line.trim().rsplit_once(": ") else { continue };
        let value = value.trim();
        match label {
            "User time (seconds)" => m.user_s = value.parse().ok(),
            "System time (seconds)" => m.sys_s = value.parse().ok(),
            "Maximum resident set size (kbytes)" => m.max_rss = value.parse::<u64>().ok().map(|k| k.saturating_mul(1024)),
            l if l.starts_with("Elapsed (wall clock) time") => {
                m.wall_s = parse_clock(value)?;
                seen = true;
            }
            _ => {}
        }
    }
    seen.then_some(m)
}

/// `h:mm:ss`, `m:ss.ss` or plain seconds.
fn parse_clock(s: &str) -> Option<f64> {
    s.split(':').try_fold(0.0, |acc: f64, part| part.parse::<f64>().ok().map(|v| acc * 60.0 + v))
}

/// Load averages from `sysctl -n vm.loadavg` (`{ 1.2 3.4 5.6 }`) or `/proc/loadavg` (`1.2 3.4 5.6 1/2 3`).
fn parse_loadavg(s: &str) -> Option<[f64; 3]> {
    let v: Vec<f64> = s.split_whitespace().filter(|w| *w != "{" && *w != "}").take(3).filter_map(|w| w.parse().ok()).collect();
    match v.as_slice() {
        [a, b, c] => Some([*a, *b, *c]),
        _ => None,
    }
}

fn loadavg() -> Option<[f64; 3]> {
    if let Ok(s) = std::fs::read_to_string("/proc/loadavg") {
        return parse_loadavg(&s);
    }
    let out = Command::new("sysctl").args(["-n", "vm.loadavg"]).output().ok()?;
    parse_loadavg(&String::from_utf8_lossy(&out.stdout))
}

/// What every result line shares.
struct RunContext {
    run_id: String,
    git: String,
    dirty: bool,
    bin: String,
    cores: usize,
    quick: bool,
}

impl RunContext {
    fn new(root: &Path, bin: &Path, o: &Opts) -> Self {
        let git = |args: &[&str]| {
            Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        RunContext {
            run_id: unix_now().to_string(),
            git: git(&["rev-parse", "--short=10", "HEAD"]).unwrap_or_default(),
            dirty: git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty()),
            bin: bin.to_string_lossy().into_owned(),
            cores: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
            quick: o.quick,
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn iso_now() -> String {
    utc(unix_now())
}

/// `YYYY-MM-DD HH:MM UTC` for a Unix time (civil-from-days, no time zone database).
fn utc(t: u64) -> String {
    let (days, secs) = ((t / 86_400) as i64, t % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", secs / 3600, secs % 3600 / 60)
}

#[allow(clippy::too_many_arguments)]
fn measure(bin: &Path, dir: &Path, sc: &Scenario, kind: &str, frames: u32, format: &str, timer: TimeTool, ctx: &RunContext) -> Result<Value, String> {
    let out_dir = dir.join("out").join(format!("{}-{kind}", sc.name.replace(['/', '\\', ' '], "_")));
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    // Half a frame before the next one: the end is inclusive, and this is robust to rounding.
    let end = sc.start + (f64::from(frames) - 0.5) / sc.fps;
    let mid = sc.start + f64::from(frames / 2) / sc.fps;
    let mut args: Vec<String> = sc.source.clone();
    let (resolution, n) = match kind {
        "open" => {
            args.push("info".into());
            ("-", 0)
        }
        "frame" => {
            args.extend(["render-frame", "--comp", &sc.comp, "--time", &format!("{mid:.6}")].map(String::from));
            args.extend(["--out".into(), out_dir.join("frame.png").to_string_lossy().into_owned()]);
            ("full", 1)
        }
        res => {
            let ext = match format {
                "h264" | "hevc" | "av1" => "mp4",
                "prores" => "mov",
                "jpeg" => "jpg",
                "tiff" => "tif",
                f => f,
            };
            args.extend(["render", "--comp", &sc.comp, "--format", format, "--resolution", res].map(String::from));
            args.extend(["--start".into(), format!("{:.6}", sc.start), "--end".into(), format!("{end:.6}")]);
            args.extend(["--out".into(), out_dir.join(format!("f.{ext}")).to_string_lossy().into_owned()]);
            (res, frames)
        }
    };
    let load = loadavg();
    let time_file = dir.join("time.txt");
    let _ = std::fs::remove_file(&time_file);
    let mut cmd = match timer {
        TimeTool::Bsd | TimeTool::Gnu => {
            let mut c = Command::new("/usr/bin/time");
            c.arg(if timer == TimeTool::Bsd { "-l" } else { "-v" }).arg("-o").arg(&time_file).arg(bin);
            c
        }
        TimeTool::None => Command::new(bin),
    };
    cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let t0 = Instant::now();
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    let wall = t0.elapsed().as_secs_f64();
    let text = std::fs::read_to_string(&time_file).unwrap_or_default();
    let parsed = match timer {
        TimeTool::Bsd => parse_bsd(&text),
        TimeTool::Gnu => parse_gnu(&text),
        TimeTool::None => None,
    };
    let m = parsed.unwrap_or(Metrics { wall_s: wall, ..Default::default() });
    // Count what a sequence render wrote: the frames actually rendered.
    let written = std::fs::read_dir(&out_dir).map(|d| d.flatten().count()).unwrap_or(0);
    let frames_done = if kind == "full" || kind == "half" { if matches!(format, "png" | "jpeg" | "tiff" | "exr") { written as u32 } else { n } } else { n };
    let _ = std::fs::remove_dir_all(&out_dir);
    let error = (!out.status.success()).then(|| {
        let err = String::from_utf8_lossy(&out.stderr);
        err.lines().rev().find(|l| !l.trim().is_empty() && !l.contains("Command exited")).unwrap_or("failed").trim().to_string()
    });
    if let Some(e) = &error {
        eprintln!("bench: {} {kind} failed: {e}", sc.name);
    }
    Ok(json!({
        "run_id": ctx.run_id,
        "date": iso_now(),
        "git": ctx.git,
        "dirty": ctx.dirty,
        "bin": ctx.bin,
        "os": std::env::consts::OS,
        "cores": ctx.cores,
        "load": load,
        "scenario": sc.name,
        "run": kind,
        "settings": {"resolution": resolution, "frames": n, "format": if kind == "full" || kind == "half" { format } else { "png" }, "quick": ctx.quick, "comp": sc.comp},
        "frames_rendered": frames_done,
        "ok": error.is_none(),
        "error": error,
        "metrics": {
            "wall_s": m.wall_s,
            "user_s": m.user_s,
            "sys_s": m.sys_s,
            "cpu_s": m.user_s.zip(m.sys_s).map(|(u, s)| u + s),
            "max_rss_bytes": m.max_rss,
            "peak_footprint_bytes": m.peak_footprint,
            "instructions": m.instructions,
        },
    }))
}

fn append_line(path: &Path, v: &Value) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    writeln!(f, "{v}").map_err(|e| format!("{}: {e}", path.display()))
}

fn read_jsonl(path: &Path) -> Result<Vec<Value>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect())
}

// ---------------------------------------------------------------------------------------- output

fn mb(v: &Value) -> String {
    v.as_f64().map(|b| format!("{:.0}", b / 1_048_576.0)).unwrap_or_else(|| "-".into())
}

fn num(v: &Value, digits: usize) -> String {
    v.as_f64().map(|x| format!("{x:.digits$}")).unwrap_or_else(|| "-".into())
}

fn table(rows: &[Value], timer: TimeTool) -> String {
    let mut s = String::new();
    if let Some(r) = rows.first() {
        s.push_str(&format!(
            "\nbench {} (git {}{}), {} cores, {}\n",
            r["date"].as_str().unwrap_or(""),
            r["git"].as_str().unwrap_or("?"),
            if r["dirty"] == true { "+dirty" } else { "" },
            r["cores"],
            r["bin"].as_str().unwrap_or("")
        ));
    }
    if timer == TimeTool::None {
        s.push_str("(no /usr/bin/time: wall time only)\n");
    }
    s.push_str(&format!(
        "{:<22} {:<6} {:>6} {:>8} {:>9} {:>8} {:>9} {:>9} {:>8} {:>6}\n",
        "scenario", "run", "frames", "wall s", "ms/frame", "cpu s", "peak MB", "maxRSS MB", "Ginstr", "load1"
    ));
    for r in rows {
        let m = &r["metrics"];
        let frames = r["frames_rendered"].as_u64().unwrap_or(0);
        let per = m["wall_s"].as_f64().filter(|_| frames > 0).map(|w| format!("{:.0}", w * 1000.0 / frames as f64)).unwrap_or_else(|| "-".into());
        let ginstr = m["instructions"].as_f64().map(|i| format!("{:.1}", i / 1e9)).unwrap_or_else(|| "-".into());
        let status = if r["ok"] == true { String::new() } else { format!("  FAILED: {}", r["error"].as_str().unwrap_or("")) };
        s.push_str(&format!(
            "{:<22} {:<6} {:>6} {:>8} {:>9} {:>8} {:>9} {:>9} {:>8} {:>6}{status}\n",
            clip(r["scenario"].as_str().unwrap_or(""), 22),
            r["run"].as_str().unwrap_or(""),
            frames,
            num(&m["wall_s"], 2),
            per,
            num(&m["cpu_s"], 2),
            mb(&m["peak_footprint_bytes"]),
            mb(&m["max_rss_bytes"]),
            ginstr,
            num(&r["load"][0], 1),
        ));
    }
    s
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n.saturating_sub(1)).chain(['…']).collect() }
}

/// What a result is compared by: scenario, run, resolution, frames and format.
fn key_of(v: &Value) -> String {
    let s = &v["settings"];
    format!(
        "{} {} {} {} {}",
        v["scenario"].as_str().unwrap_or(""),
        v["run"].as_str().unwrap_or(""),
        s["resolution"].as_str().unwrap_or(""),
        s["frames"],
        s["format"].as_str().unwrap_or("")
    )
}

/// The last successful result per key (a results file accumulates runs; the latest one counts).
fn latest_per_key(rows: &[Value]) -> BTreeMap<String, Value> {
    let mut map = BTreeMap::new();
    for r in rows.iter().filter(|r| r["ok"] == true) {
        map.insert(key_of(r), r.clone());
    }
    map
}

/// `(new - old) / old` in percent; `None` without both values or with a zero baseline.
fn pct(old: Option<f64>, new: Option<f64>) -> Option<f64> {
    let (o, n) = (old?, new?);
    (o != 0.0).then(|| (n - o) / o * 100.0)
}

fn fmt_pct(p: Option<f64>) -> String {
    p.map(|p| format!("{p:+.1}%")).unwrap_or_else(|| "-".into())
}

const COMPARED: &[(&str, &str)] =
    &[("wall_s", "wall"), ("cpu_s", "cpu"), ("peak_footprint_bytes", "peak"), ("max_rss_bytes", "maxRSS"), ("instructions", "instr")];

/// Metrics `--fail-over` checks: the ones that don't swing with the machine's load.
const GATED: &[&str] = &["cpu_s", "peak_footprint_bytes", "max_rss_bytes", "instructions"];

/// Every matched run and metric that got more than `limit` % worse.
fn regressions(old: &BTreeMap<String, Value>, new: &BTreeMap<String, Value>, limit: f64) -> Vec<String> {
    let mut out = vec![];
    for (k, n) in new {
        let Some(o) = old.get(k) else { continue };
        for field in GATED {
            if let Some(p) = pct(o["metrics"][field].as_f64(), n["metrics"][field].as_f64()).filter(|p| *p > limit) {
                out.push(format!("{} {}: {field} {}", n["scenario"].as_str().unwrap_or("?"), n["run"].as_str().unwrap_or("?"), fmt_pct(Some(p))));
            }
        }
    }
    out
}

fn check_regressions(old: &BTreeMap<String, Value>, new: &BTreeMap<String, Value>, limit: Option<f64>) -> Result<(), String> {
    let Some(limit) = limit else { return Ok(()) };
    let bad = regressions(old, new, limit);
    if bad.is_empty() {
        println!("no run got more than {limit}% worse");
        Ok(())
    } else {
        Err(format!("{} regressions over {limit}%:\n  {}", bad.len(), bad.join("\n  ")))
    }
}

fn compare_table(old: &BTreeMap<String, Value>, new: &BTreeMap<String, Value>) -> String {
    let mut s = String::from("\n% change against the earlier run (negative = better; instructions retired are the least load-sensitive)\n");
    s.push_str(&format!("{:<22} {:<6} {:>6}", "scenario", "run", "frames"));
    for (_, label) in COMPARED {
        s.push_str(&format!(" {label:>8}"));
    }
    s.push_str(&format!(" {:>11}\n", "load1 o→n"));
    let mut matched = 0;
    for (k, n) in new {
        let Some(o) = old.get(k) else { continue };
        matched += 1;
        s.push_str(&format!(
            "{:<22} {:<6} {:>6}",
            clip(n["scenario"].as_str().unwrap_or(""), 22),
            n["run"].as_str().unwrap_or(""),
            n["settings"]["frames"].as_u64().unwrap_or(0)
        ));
        for (field, _) in COMPARED {
            s.push_str(&format!(" {:>8}", fmt_pct(pct(o["metrics"][field].as_f64(), n["metrics"][field].as_f64()))));
        }
        s.push_str(&format!(" {:>11}\n", format!("{}→{}", num(&o["load"][0], 0), num(&n["load"][0], 0))));
    }
    if matched == 0 {
        s.push_str("(no matching runs: compare the same scenarios, frames and format)\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const BSD: &str = "        0.33 real         0.67 user         0.15 sys
           743882752  maximum resident set size
                   0  average shared memory size
                 211  page reclaims
          8461141340  instructions retired
             9206654  cycles elapsed
           728548408  peak memory footprint
";

    const GNU: &str = "\tCommand being timed: \"effectcraft-cli --demo info\"
\tUser time (seconds): 1.25
\tSystem time (seconds): 0.30
\tPercent of CPU this job got: 150%
\tElapsed (wall clock) time (h:mm:ss or m:ss): 1:02.50
\tMaximum resident set size (kbytes): 204800
\tExit status: 0
";

    #[test]
    fn parses_bsd_time() {
        let m = parse_bsd(BSD).expect("parsed");
        assert_eq!(m.wall_s, 0.33);
        assert_eq!((m.user_s, m.sys_s), (Some(0.67), Some(0.15)));
        assert_eq!(m.max_rss, Some(743_882_752));
        assert_eq!(m.peak_footprint, Some(728_548_408));
        assert_eq!(m.instructions, Some(8_461_141_340));
        assert_eq!(parse_bsd("garbage\n12 maximum resident set size\n"), None);
    }

    #[test]
    fn parses_gnu_time() {
        let m = parse_gnu(GNU).expect("parsed");
        assert_eq!(m.wall_s, 62.5);
        assert_eq!((m.user_s, m.sys_s), (Some(1.25), Some(0.3)));
        assert_eq!(m.max_rss, Some(204_800 * 1024));
        assert_eq!((m.peak_footprint, m.instructions), (None, None));
        assert_eq!(parse_gnu("User time (seconds): 1.0\n"), None);
        assert_eq!(parse_clock("1:02:03"), Some(3723.0));
        assert_eq!(parse_clock("0:00.04"), Some(0.04));
        assert_eq!(parse_clock("x:1"), None);
    }

    #[test]
    fn parses_load_average() {
        assert_eq!(parse_loadavg("{ 44.53 60.43 71.70 }\n"), Some([44.53, 60.43, 71.70]));
        assert_eq!(parse_loadavg("0.52 0.58 0.59 1/467 12345\n"), Some([0.52, 0.58, 0.59]));
        assert_eq!(parse_loadavg(""), None);
    }

    fn row(scenario: &str, run: &str, wall: f64, peak: Option<u64>, ok: bool) -> Value {
        json!({"scenario": scenario, "run": run, "ok": ok, "load": [10.0, 0, 0],
               "settings": {"resolution": "full", "frames": 60, "format": "png"},
               "metrics": {"wall_s": wall, "cpu_s": wall * 2.0, "peak_footprint_bytes": peak, "max_rss_bytes": 0, "instructions": null}})
    }

    #[test]
    fn compare_maths() {
        assert_eq!(pct(Some(10.0), Some(8.0)), Some(-20.0));
        assert_eq!(pct(Some(4.0), Some(5.0)), Some(25.0));
        assert_eq!(pct(Some(0.0), Some(5.0)), None);
        assert_eq!(pct(None, Some(5.0)), None);
        assert_eq!(fmt_pct(Some(-20.0)), "-20.0%");
        assert_eq!(fmt_pct(Some(25.0)), "+25.0%");
        assert_eq!(fmt_pct(None), "-");

        // The latest successful run per key is the baseline; failed runs don't count.
        let old = latest_per_key(&[
            row("layers", "full", 20.0, Some(1000), true),
            row("layers", "full", 10.0, Some(2000), true),
            row("layers", "full", 1.0, None, false),
        ]);
        assert_eq!(old.len(), 1);
        let new = latest_per_key(&[row("layers", "full", 8.0, Some(1500), true), row("demo", "half", 1.0, None, true)]);
        let t = compare_table(&old, &new);
        let line = t.lines().find(|l| l.starts_with("layers")).expect("layers row");
        // wall 10 → 8, cpu 20 → 16, peak 2000 → 1500, max RSS 0 (no baseline), instructions missing.
        assert!(line.contains("-20.0%") && line.contains("-25.0%"), "{line}");
        assert_eq!(line.matches("-20.0%").count(), 2, "{line}");
        assert!(!t.contains("demo"), "unmatched runs are left out: {t}");
        assert!(compare_table(&BTreeMap::new(), &new).contains("no matching runs"));
    }

    #[test]
    fn fail_over_flags_resource_regressions_but_not_wall_time() {
        let old = latest_per_key(&[row("layers", "full", 10.0, Some(1000), true), row("demo", "half", 1.0, Some(1000), true)]);
        // Twice the wall time (and so CPU) on layers; 5% more peak on demo.
        let new = latest_per_key(&[row("layers", "full", 20.0, Some(1000), true), row("demo", "half", 1.0, Some(1050), true)]);
        let bad = regressions(&old, &new, 10.0);
        assert_eq!(bad, ["layers full: cpu_s +100.0%"]);
        assert_eq!(regressions(&old, &new, 3.0).len(), 2);
        assert!(check_regressions(&old, &new, None).is_ok());
        assert!(check_regressions(&old, &new, Some(150.0)).is_ok());
        assert!(check_regressions(&old, &new, Some(10.0)).is_err_and(|e| e.contains("cpu_s")));
        // Wall time alone never fails the check.
        let mut slower = new.clone();
        for v in slower.values_mut() {
            v["metrics"]["cpu_s"] = json!(2.0);
            v["metrics"]["wall_s"] = json!(500.0);
        }
        let mut base = old.clone();
        for v in base.values_mut() {
            v["metrics"]["cpu_s"] = json!(2.0);
            v["metrics"]["peak_footprint_bytes"] = json!(1050);
        }
        assert!(regressions(&base, &slower, 1.0).is_empty());
        assert!(parse_opts(&["--fail-over".into(), "5".into()]).is_err(), "needs --compare");
        assert!(parse_opts(&["--compare".into(), "a".into(), "--fail-over".into(), "x".into()]).is_err());
    }

    #[test]
    fn options_and_scenarios() {
        let a = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        let o = parse_opts(&a("--bin /x/cli --quick --scenario demo,layers --compare old.jsonl")).expect("opts");
        assert_eq!(o.bin.as_deref(), Some(Path::new("/x/cli")));
        assert!(o.quick);
        assert_eq!(o.scenarios, Some(vec!["demo".to_string(), "layers".to_string()]));
        assert!(parse_opts(&a("--project p.ecproj")).is_err(), "--project needs --comp");
        assert!(parse_opts(&a("--with new.jsonl")).is_err(), "--with needs --compare");
        assert!(parse_opts(&a("--frames 0")).is_err());
        assert!(parse_opts(&a("--bogus")).is_err());
        // The generated projects are deterministic and as large as documented.
        assert_eq!(layers_cmds(), layers_cmds());
        let layers = layers_cmds().iter().filter(|(id, _)| id.starts_with("layer.new")).count();
        assert_eq!(layers, 102, "background + 100 layers + adjustment");
        assert!(rnd(3, 1) < 1.0 && rnd(3, 1) != rnd(4, 1));
    }
}
