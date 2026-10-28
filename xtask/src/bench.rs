//! `cargo xtask bench`: a repeatable render benchmark (docs/testing.md "Render benchmark", gaps.md G5).
//!
//! Generates deterministic projects into `target/bench/projects` with `effectcraft-cli` (engine
//! commands, no committed media), renders each at Full and Half plus one single frame, and measures
//! every run with `/usr/bin/time` (macOS `-l`: peak memory footprint, max RSS, instructions
//! retired; Linux `-v`: max RSS; elsewhere wall time only). Prints a table and appends one JSON
//! line per run to `target/bench/results.jsonl`; with `--repeat N` (N > 1) one `"kind": "summary"`
//! line per key follows, whose metrics are the medians. `--compare OLD.jsonl` prints the % change.

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
  --repeat N            run every scenario N times (default 1). Every run is appended as before,
                        followed by one `kind: summary` row per key whose metrics are the
                        medians (`summary` holds median, p90, min and max; p90 is the nearest
                        rank, so it equals max for N < 10). Not allowed with --with
  --compare OLD.jsonl   print the % change of this run against the latest matching runs in OLD.
                        The latest row per key is compared, so a side is a median only when its
                        summary is the latest row; each side is labelled `median of N` or `1 run`
                        and mixing them is warned about (refused with --fail-over)
  --with NEW.jsonl      with --compare: compare two result files, without rendering
  --fail-over PCT       with --compare: exit 1 when a matched run got more than PCT % worse in
                        CPU time, peak memory, max RSS or instructions retired (wall time is
                        left out: it follows the machine's load). Requires matching successful
                        latest runs and comparable resource measurements for every key

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
    repeat: u32,
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { format: "png".into(), repeat: 1, ..Default::default() };
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
            "--repeat" => o.repeat = val("--repeat")?.parse().ok().filter(|n| *n >= 1).ok_or("--repeat: a number of at least 1 expected")?,
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
    if o.with.is_some() && args.iter().any(|a| a == "--repeat") {
        return Err("--repeat has no effect with --with (nothing is rendered)".into());
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
    for i in 1..=o.repeat {
        for sc in &scenarios {
            for kind in RUNS {
                if o.repeat > 1 {
                    eprintln!("bench: {} {kind} ({i}/{})", sc.name, o.repeat);
                } else {
                    eprintln!("bench: {} {kind}", sc.name);
                }
                let row = measure(&bin, &dir, sc, &Measurement { kind, frames, format: &o.format }, timer, &ctx)?;
                append_line(&results_path, &row)?;
                rows.push(row);
            }
        }
    }
    print!("{}", table(&rows, timer));
    let summaries = if o.repeat > 1 { summarize(&rows) } else { vec![] };
    for row in &summaries {
        append_line(&results_path, row)?;
    }
    print!("{}", summary_table(&summaries));
    println!("appended {} runs and {} summary rows to {}", rows.len(), summaries.len(), results_path.display());
    if let Some(old) = &o.compare {
        // Summaries come after their runs, so the latest row per key is the median when present.
        rows.extend(summaries);
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

/// Run `cmds` with `effectcraft-cli --empty run --args-file … --save-as out`: a file, because the
/// `layers` scenario is longer than a Windows command line (32 767 characters).
fn build_project(bin: &Path, cmds: &Cmds, out: &Path) -> Result<(), String> {
    let mut lines = String::new();
    for (id, params) in cmds {
        lines.push_str(&format!("{id}\n{params}\n"));
    }
    let args = out.with_extension("args");
    std::fs::write(&args, lines).map_err(|e| format!("{}: {e}", args.display()))?;
    let r = cli(Command::new(bin).arg("--empty").arg("run").arg("--args-file").arg(&args).arg("--save-as").arg(out));
    let _ = std::fs::remove_file(&args);
    r
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

/// `YYYY-MM-DD HH:MM UTC` for a Unix time. Years far outside chrono's own range are preserved
/// (the old hand-rolled conversion kept them) rather than collapsing to the epoch.
fn utc(t: u64) -> String {
    use chrono::Datelike;
    // Reduce by whole 400-year Gregorian cycles (146_097 days) into chrono's range, convert the
    // remainder with chrono, and carry the year back; `t` is `u64`, so `days` fits an `i64`.
    let (days, sod) = ((t / 86_400) as i64, (t % 86_400) as i64);
    let (cycles, rest) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    let date = i32::try_from(rest + 719_163).ok().and_then(chrono::NaiveDate::from_num_days_from_ce_opt);
    let time = u32::try_from(sod).ok().and_then(|s| chrono::NaiveTime::from_num_seconds_from_midnight_opt(s, 0));
    let (y, dt) = match (date, time) {
        (Some(d), Some(ti)) => {
            let yy = i128::from(d.year()) + i128::from(cycles) * 400;
            (i64::try_from(yy).unwrap_or(i64::MAX), d.and_time(ti))
        }
        _ => (1970, chrono::NaiveDateTime::default()),
    };
    // `{:04}` rather than `%Y`, which writes a `+` before years past 9999.
    format!("{y:04}-{}", dt.format("%m-%d %H:%M UTC"))
}

struct Measurement<'a> {
    kind: &'a str,
    frames: u32,
    format: &'a str,
}

fn measure(bin: &Path, dir: &Path, sc: &Scenario, measurement: &Measurement<'_>, timer: TimeTool, ctx: &RunContext) -> Result<Value, String> {
    let Measurement { kind, frames, format } = *measurement;
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
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let context = |error| format!("{}:{}: {error}", path.display(), index.saturating_add(1));
            let row: Value = serde_json::from_str(line).map_err(|error| context(format!("invalid JSON: {error}")))?;
            validate_row(&row).map_err(context)?;
            Ok(row)
        })
        .collect()
}

/// Validate comparison keys and measurements; unavailable platform metrics may be null/absent.
fn validate_row(row: &Value) -> Result<(), String> {
    for (field, value) in [
        ("scenario", &row["scenario"]),
        ("run", &row["run"]),
        ("settings.resolution", &row["settings"]["resolution"]),
        ("settings.format", &row["settings"]["format"]),
    ] {
        if value.as_str().is_none_or(|text| text.trim().is_empty()) {
            return Err(format!("`{field}` must be a nonempty string"));
        }
    }
    if row["settings"]["frames"].as_u64().is_none() {
        return Err("`settings.frames` must be a nonnegative integer".into());
    }
    if row["ok"].as_bool().is_none() {
        return Err("`ok` must be a boolean".into());
    }
    if !row["metrics"].is_object() {
        return Err("`metrics` must be an object".into());
    }
    if row["metrics"]["wall_s"].is_null() {
        return Err("`metrics.wall_s` is required".into());
    }
    for (field, _) in COMPARED {
        let value = &row["metrics"][*field];
        if !value.is_null() && value.as_f64().is_none_or(|number| !number.is_finite() || number < 0.0) {
            return Err(format!("`metrics.{field}` must be a finite nonnegative number or null"));
        }
    }
    Ok(())
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

/// Median, p90 (nearest rank), min and max of `v`; `None` when empty.
fn stats(v: &[f64]) -> Option<[f64; 4]> {
    let mut v: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let median = if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 };
    let p90 = v[(n * 9).div_ceil(10).max(1) - 1];
    Some([median, p90, v[0], v[n - 1]])
}

const STATS: [&str; 4] = ["median", "p90", "min", "max"];

/// One `"kind": "summary"` row per key over repeated runs: the key fields (and `date`, `load`,
/// ...) of its latest run, `metrics` set to the medians of the successful runs (byte and
/// instruction counts rounded), `summary` with every statistic, and `ok` only when every
/// run succeeded (so `--fail-over` still refuses a key where a run failed).
fn summarize(rows: &[Value]) -> Vec<Value> {
    let mut groups: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for r in rows {
        groups.entry(key_of(r)).or_default().push(r);
    }
    let mut out = vec![];
    for runs in groups.values() {
        let Some(last) = runs.last() else { continue };
        let good: Vec<&Value> = runs.iter().copied().filter(|r| r["ok"] == true).collect();
        let failed = runs.len() - good.len();
        let mut row = (*last).clone();
        let mut metrics = serde_json::Map::new();
        let mut summary = serde_json::Map::new();
        for field in COMPARED.iter().map(|(f, _)| *f).chain(["user_s", "sys_s"]) {
            let values: Vec<f64> = good.iter().filter_map(|r| r["metrics"][field].as_f64()).collect();
            let st = stats(&values);
            let integer = field.ends_with("_bytes") || field == "instructions";
            let median = |s: [f64; 4]| if integer { serde_json::json!(s[0].round() as u64) } else { serde_json::json!(s[0]) };
            metrics.insert(field.to_string(), st.map_or(Value::Null, median));
            summary.insert(field.to_string(), st.map_or(Value::Null, |s| STATS.iter().zip(s).map(|(k, v)| (k.to_string(), serde_json::json!(v))).collect()));
        }
        if metrics["wall_s"].is_null() {
            metrics.insert("wall_s".into(), serde_json::json!(0.0));
        }
        row["kind"] = Value::String("summary".into());
        row["metrics"] = Value::Object(metrics);
        row["summary"] = serde_json::json!({"runs": runs.len(), "failed": failed, "stats": summary});
        row["ok"] = Value::Bool(failed == 0);
        if failed > 0 {
            row["error"] = Value::String(format!("{failed} of {} runs failed", runs.len()));
        }
        out.push(row);
    }
    out
}

fn summary_table(summaries: &[Value]) -> String {
    let Some(first) = summaries.first() else { return String::new() };
    let mut s = format!("\nsummary over {} runs per key\n", first["summary"]["runs"]);
    s.push_str(&format!("{:<22} {:<6} {:<6} {:>8} {:>8} {:>9} {:>9} {:>8}\n", "scenario", "run", "stat", "wall s", "cpu s", "peak MB", "maxRSS MB", "Ginstr"));
    for r in summaries {
        let st = &r["summary"]["stats"];
        for stat in STATS {
            let ginstr = st["instructions"][stat].as_f64().map(|i| format!("{:.1}", i / 1e9)).unwrap_or_else(|| "-".into());
            s.push_str(&format!(
                "{:<22} {:<6} {:<6} {:>8} {:>8} {:>9} {:>9} {:>8}\n",
                clip(r["scenario"].as_str().unwrap_or(""), 22),
                r["run"].as_str().unwrap_or(""),
                stat,
                num(&st["wall_s"][stat], 2),
                num(&st["cpu_s"][stat], 2),
                mb(&st["peak_footprint_bytes"][stat]),
                mb(&st["max_rss_bytes"][stat]),
                ginstr,
            ));
        }
        if r["ok"] != true {
            s.push_str(&format!("  ({})\n", r["error"].as_str().unwrap_or("")));
        }
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

/// The latest result per key, including failures that supersede older successful runs.
fn latest_per_key(rows: &[Value]) -> BTreeMap<String, Value> {
    let mut map = BTreeMap::new();
    for r in rows {
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
    if old.is_empty() || new.is_empty() {
        return Err("insufficient benchmark evidence: baseline and candidate must contain runs".into());
    }
    let mut missing = vec![];
    for (key, baseline) in old {
        match new.get(key) {
            None => missing.push(format!("missing candidate run: {key}")),
            Some(candidate) => {
                for (label, row) in [("baseline", baseline), ("candidate", candidate)] {
                    if row["ok"] != true {
                        missing.push(format!("failed latest {label} run: {key}"));
                    }
                }
                if is_summary(baseline) != is_summary(candidate) {
                    missing.push(format!("median compared against a single run ({} vs {}): {key}", sample(baseline), sample(candidate)));
                }
                if !GATED.iter().any(|field| pct(baseline["metrics"][*field].as_f64(), candidate["metrics"][*field].as_f64()).is_some()) {
                    missing.push(format!("no comparable resource measurements: {key}"));
                }
            }
        }
    }
    for key in new.keys().filter(|key| !old.contains_key(*key)) {
        missing.push(format!("missing baseline run: {key}"));
    }
    if !missing.is_empty() {
        return Err(format!("insufficient benchmark evidence:\n  {}", missing.join("\n  ")));
    }
    let bad = regressions(old, new, limit);
    if bad.is_empty() {
        println!("no run got more than {limit}% worse");
        Ok(())
    } else {
        Err(format!("{} regressions over {limit}%:\n  {}", bad.len(), bad.join("\n  ")))
    }
}

fn is_summary(row: &Value) -> bool {
    row["kind"] == "summary"
}

/// `median of N` for a summary row, `1 run` otherwise.
fn sample(row: &Value) -> String {
    if is_summary(row) { format!("median of {}", row["summary"]["runs"]) } else { "1 run".into() }
}

fn compare_table(old: &BTreeMap<String, Value>, new: &BTreeMap<String, Value>) -> String {
    let mut s = String::from("\n% change against the earlier run (negative = better; instructions retired are the least load-sensitive)\n");
    s.push_str(&format!("{:<22} {:<6} {:>6}", "scenario", "run", "frames"));
    for (_, label) in COMPARED {
        s.push_str(&format!(" {label:>8}"));
    }
    s.push_str(&format!(" {:>11}  {}\n", "load1 o→n", "sample o→n"));
    let mut matched = 0;
    let mut mixed = 0;
    for (k, n) in new {
        let Some(o) = old.get(k) else { continue };
        matched += 1;
        s.push_str(&format!(
            "{:<22} {:<6} {:>6}",
            clip(n["scenario"].as_str().unwrap_or(""), 22),
            n["run"].as_str().unwrap_or(""),
            n["settings"]["frames"].as_u64().unwrap_or(0)
        ));
        if o["ok"] != true || n["ok"] != true {
            s.push_str(&format!(" (failed latest {} run)\n", if n["ok"] != true { "candidate" } else { "baseline" }));
            continue;
        }
        for (field, _) in COMPARED {
            s.push_str(&format!(" {:>8}", fmt_pct(pct(o["metrics"][field].as_f64(), n["metrics"][field].as_f64()))));
        }
        mixed += usize::from(is_summary(o) != is_summary(n));
        s.push_str(&format!(" {:>11}  {}→{}\n", format!("{}→{}", num(&o["load"][0], 0), num(&n["load"][0], 0)), sample(o), sample(n)));
    }
    if mixed > 0 {
        s.push_str(&format!("warning: {mixed} keys compare a median against a single run\n"));
    }
    if matched == 0 {
        s.push_str("(no matching runs: compare the same scenarios, frames and format)\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_preserves_out_of_range_years() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(1_700_000_000), "2023-11-14 22:13 UTC");
        // Years far outside chrono's own range are preserved (the old hand-rolled conversion kept
        // them) rather than collapsing to the epoch.
        assert_eq!(utc(253_402_300_800), "10000-01-01 00:00 UTC");
        assert_eq!(utc(10_000_000_000_000), "318857-05-20 17:46 UTC");
        assert_eq!(utc(u64::MAX), "584554051223-11-09 07:00 UTC");
    }

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

    fn five_runs() -> Vec<Value> {
        [10.0, 30.0, 11.0, 12.0, 9.0].map(|w| row("demo", "full", w, Some(w as u64 * 100 + 1), true)).to_vec()
    }

    #[test]
    fn stats_median_p90() {
        assert_eq!(stats(&[]), None);
        assert_eq!(stats(&[3.0]), Some([3.0, 3.0, 3.0, 3.0]));
        assert_eq!(stats(&[4.0, 1.0, 3.0, 2.0]), Some([2.5, 4.0, 1.0, 4.0]));
        let ten: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(stats(&ten), Some([5.5, 9.0, 1.0, 10.0]));
    }

    #[test]
    fn stats_small_n_p90_is_max() {
        // N = 2: the median is the mean, so one outlier moves it by half; p90 is the max.
        assert_eq!(stats(&[1.0, 9.0]), Some([5.0, 9.0, 1.0, 9.0]));
        // N = 3: the median ignores the outlier; p90 is still the max.
        assert_eq!(stats(&[1.0, 9.0, 1.1]), Some([1.1, 9.0, 1.0, 9.0]));
    }

    #[test]
    fn summary_row_is_valid_and_median() {
        let mut rows = five_runs();
        rows.push(row("demo", "half", 5.0, None, true));
        let sums = summarize(&rows);
        assert_eq!(sums.len(), 2);
        let full = sums.iter().find(|r| r["run"] == "full").unwrap();
        assert_eq!(full["kind"], "summary");
        assert_eq!(full["metrics"]["wall_s"], 11.0, "median, not the 30 s outlier");
        assert_eq!(full["summary"]["stats"]["wall_s"]["p90"], 30.0);
        assert_eq!(full["summary"]["stats"]["wall_s"]["min"], 9.0);
        assert_eq!(full["summary"]["runs"], 5);
        assert_eq!(full["ok"], true);
        assert!(validate_row(full).is_ok());
        assert!(summary_table(&sums).contains("median"));
    }

    #[test]
    fn summary_keeps_user_sys_and_rounds_integers() {
        let mut runs = five_runs()[..2].to_vec(); // peaks 1001 and 3001: even N
        for (r, (u, sy)) in runs.iter_mut().zip([(1.0, 0.5), (3.0, 1.5)]) {
            r["metrics"]["user_s"] = json!(u);
            r["metrics"]["sys_s"] = json!(sy);
        }
        let m = &summarize(&runs)[0]["metrics"];
        assert_eq!((m["user_s"].clone(), m["sys_s"].clone()), (json!(2.0), json!(1.0)));
        assert_eq!(m["peak_footprint_bytes"], json!(2001));
        assert!(m["peak_footprint_bytes"].is_u64());
    }

    #[test]
    fn failed_run_fails_summary() {
        let mut bad = five_runs();
        bad[1]["ok"] = json!(false);
        let s = summarize(&bad);
        assert_eq!((s[0]["ok"].clone(), s[0]["metrics"]["wall_s"].clone()), (json!(false), json!(10.5)));
    }

    #[test]
    fn repeat_parse() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_opts(&[]).unwrap().repeat, 1);
        assert_eq!(parse_opts(&a(&["--repeat", "5"])).unwrap().repeat, 5);
        assert!(parse_opts(&a(&["--repeat", "0"])).is_err());
        assert!(parse_opts(&a(&["--repeat", "3", "--compare", "a", "--with", "b"])).is_err());
        assert!(parse_opts(&a(&["--repeat", "3", "--compare", "a"])).is_ok());
    }

    #[test]
    fn compare_prefers_summary() {
        // The summary is appended after its runs, so it is the latest row: median 11 vs 10 → +10 %.
        let old = latest_per_key(&[row("demo", "full", 10.0, Some(1001), true)]);
        let mut rows = five_runs();
        rows.extend(summarize(&rows));
        let new = latest_per_key(&rows);
        assert_eq!(new["demo full full 60 png"]["metrics"]["wall_s"], 11.0);
        assert!(regressions(&old, &new, 5.0).iter().any(|r| r.contains("cpu_s +10.0%")));
    }

    #[test]
    fn median_against_single_run_is_labelled_and_refused_by_gate() {
        let old = latest_per_key(&[row("demo", "full", 10.0, Some(1001), true)]);
        let new = latest_per_key(&summarize(&five_runs()));
        let table = compare_table(&old, &new);
        assert!(table.contains("1 run→median of 5"), "{table}");
        assert!(table.contains("warning: 1 keys compare a median against a single run"), "{table}");
        let e = check_regressions(&old, &new, Some(50.0)).unwrap_err();
        assert!(e.contains("median compared against a single run"), "{e}");
        assert!(check_regressions(&old, &new, None).is_ok(), "exploratory compare still allowed");
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

        // A successful run supersedes an earlier failure for the same key.
        let old = latest_per_key(&[
            row("layers", "full", 1.0, None, false),
            row("layers", "full", 20.0, Some(1000), true),
            row("layers", "full", 10.0, Some(2000), true),
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
