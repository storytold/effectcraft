//! Everyday-operation performance: a large generated project and the engine-side measurements
//! behind `effectcraft-cli bench --ops` (open, save, auto-save, large edits with undo/redo, first
//! frame). The UI-side numbers (first UI frame, timeline and Project panel draw) live in
//! `effectcraft-ui-egui`'s `bench` module. See `docs/architecture.md` ▸ Performance.

// A benchmark harness (`effectcraft-cli bench --ops`, tests): a failed step should fail loudly,
// as in tests and benches (AGENTS.md, "Never crash"). Nothing here runs in the app.
#![allow(clippy::expect_used)]

use std::sync::Arc;

use effectcraft_keyframe::Value as KValue;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{AlphaMode, Comp, Expression, Footage, FootageKind, ItemId, ItemKind, Layer, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use crate::Session;

/// What [`large_project`] builds.
#[derive(Clone, Debug)]
pub struct LargeSpec {
    /// Compositions (the main comp, a precomp nesting chain and leaf comps).
    pub comps: usize,
    /// Layers in the main comp (the timeline stress case).
    pub main_layers: usize,
    /// Layers in every other comp.
    pub comp_layers: usize,
    /// Footage items: a third image sequences, a third movies, a third stills.
    pub footage: usize,
    /// Frames per image sequence.
    pub sequence_frames: usize,
    /// Depth of the precomp nesting chain under the main comp.
    pub nest_depth: usize,
    /// Every n-th layer gets expressions (0 = none).
    pub expression_every: usize,
    /// Folder the footage paths point into (the files need not exist).
    pub media_dir: String,
}

impl Default for LargeSpec {
    /// 200 comps, 5,000 layers in the main comp (plus 10 in each other comp), 300 footage items
    /// (100 sequences of 240 frames, 100 movies, 100 stills), 20 levels of precomp nesting and
    /// expressions on every 4th layer.
    fn default() -> Self {
        LargeSpec {
            comps: 200,
            main_layers: 5000,
            comp_layers: 10,
            footage: 300,
            sequence_frames: 240,
            nest_depth: 20,
            expression_every: 4,
            media_dir: "/nonexistent/effectcraft-bench-media".into(),
        }
    }
}

impl LargeSpec {
    /// A small version for tests.
    pub fn small() -> Self {
        LargeSpec { comps: 12, main_layers: 300, comp_layers: 4, footage: 30, sequence_frames: 24, nest_depth: 5, expression_every: 3, ..Default::default() }
    }
    /// Total layers [`large_project`] makes.
    pub fn total_layers(&self) -> usize {
        self.main_layers + self.comps.saturating_sub(1) * self.comp_layers
    }
}

fn base_comp(name: &str) -> Comp {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": name, "width": 1920, "height": 1080, "frameRate": 30, "duration": 10})).expect("comp.new");
    s.active_comp().expect("comp").clone()
}

fn footage_item(spec: &LargeSpec, i: usize) -> Footage {
    let dir = &spec.media_dir;
    let base = Footage { width: 1920, height: 1080, pixel_aspect: 1.0, has_video: true, alpha: AlphaMode::Straight, ..Default::default() };
    match i % 3 {
        0 => {
            let frames: Vec<String> = (0..spec.sequence_frames).map(|f| format!("{dir}/seq{i:03}/shot{i:03}_{f:05}.png")).collect();
            Footage {
                path: frames.first().cloned().unwrap_or_default(),
                kind: FootageKind::Sequence,
                frame_rate: FrameRate::FPS_30,
                duration: Tick::from_seconds_f64(spec.sequence_frames as f64 / 30.0),
                codec: "PNG".into(),
                sequence: frames,
                ..base
            }
        }
        1 => Footage {
            path: format!("{dir}/clip{i:03}.mp4"),
            kind: FootageKind::Video,
            frame_rate: FrameRate::FPS_30,
            duration: Tick::from_seconds_f64(20.0),
            has_audio: true,
            codec: "H.264".into(),
            ..base
        },
        _ => Footage { path: format!("{dir}/still{i:03}.png"), kind: FootageKind::Still, codec: "PNG".into(), ..base },
    }
}

fn set_expr(l: &mut Layer, path: &str, text: &str) {
    if let Some(p) = l.props.prop_mut(path) {
        p.expr = Some(Expression { text: text.into(), enabled: true });
    }
}

/// One layer of a varied mix: solids, footage, text, shapes, nulls, some with an effect, some
/// with expressions, parented to the layer above every 7th.
fn make_layer(p: &mut Project, comp: &Comp, spec: &LargeSpec, n: usize, solid: ItemId, footage: &[ItemId], prev: Option<&Layer>) -> Layer {
    let (src, size) = match n % 5 {
        0 => (LayerSource::Solid { item: solid }, (1920, 1080)),
        1 if !footage.is_empty() => (LayerSource::Footage { item: footage[n % footage.len()] }, (1920, 1080)),
        2 => (LayerSource::Text, (0, 0)),
        3 => (LayerSource::Shape, (0, 0)),
        _ => (LayerSource::Null, (100, 100)),
    };
    let mut l = build::layer(p, comp, &format!("Layer {n}"), src, size, None);
    if n.is_multiple_of(6)
        && let Some(fx) = crate::effects::find("ec.blur.gaussian")
    {
        let mut next = p.next_id;
        let mut g = crate::effects::instantiate(fx, &mut Ids(&mut next), fx.name, [1920.0, 1080.0]);
        p.next_id = next;
        if let Some(b) = g.prop_mut("blurriness") {
            b.value = KValue::Scalar(4.0);
        }
        if let Some(e) = l.props.sub_mut("effects") {
            e.children.push(g.into());
        }
    }
    if spec.expression_every > 0 && n.is_multiple_of(spec.expression_every) {
        set_expr(&mut l, "transform/position", "wiggle(2, 20)");
        set_expr(&mut l, "transform/rotation", "time * 30 + index");
        set_expr(&mut l, "transform/opacity", "50 + 50 * Math.sin(time * 2)");
    }
    if n.is_multiple_of(7)
        && let Some(prev) = prev
    {
        l.parent = Some(prev.id);
    }
    // Stagger the bars so the timeline isn't one block.
    let span = comp.duration.0.max(1);
    let off = (n as i64 * 7919) % span;
    l.in_point = Tick(off / 2);
    l
}

/// A large project for performance work (see [`LargeSpec`]). The main comp is named `Main` and
/// is the first comp; the nesting chain is `Nest 1` (inside Main) … `Nest {depth}`.
pub fn large_project(spec: &LargeSpec) -> Project {
    let mut p = Project::default();
    let template = base_comp("Main");
    let media = p.add_item("Media", crate::color::Label::None, None, ItemKind::Folder);
    let comps_folder = p.add_item("Comps", crate::color::Label::None, None, ItemKind::Folder);
    let footage: Vec<ItemId> = (0..spec.footage)
        .map(|i| {
            let f = footage_item(spec, i);
            let name = std::path::Path::new(&f.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            p.add_item(&name, crate::color::Label::None, Some(media), ItemKind::Footage(f))
        })
        .collect();
    let solid = p.add_item(
        "Bench Solid",
        crate::color::Label::None,
        Some(media),
        ItemKind::Solid(Solid { color: [0.2, 0.4, 0.8], width: 1920, height: 1080, pixel_aspect: 1.0 }),
    );

    // Leaf comps and the nesting chain are built bottom-up so each precomp layer can point at
    // an existing item.
    let mut n = 0usize;
    let fill = |p: &mut Project, comp: &mut Comp, count: usize, n: &mut usize| {
        let mut layers: Vec<Layer> = Vec::with_capacity(count);
        for _ in 0..count {
            let l = make_layer(p, comp, spec, *n, solid, &footage, layers.last());
            *n += 1;
            layers.push(l);
        }
        comp.layers = layers;
    };
    let others = spec.comps.saturating_sub(1);
    let depth = spec.nest_depth.min(others);
    // Nesting chain: Nest {depth} (deepest) … Nest 1.
    let mut below: Option<ItemId> = None;
    for d in (1..=depth).rev() {
        let mut c = template.clone();
        fill(&mut p, &mut c, spec.comp_layers.saturating_sub(1), &mut n);
        if let Some(b) = below {
            let l = build::layer(&mut p, &c, &format!("Nest {}", d + 1), LayerSource::Comp { item: b }, (1920, 1080), None);
            c.layers.insert(0, l);
        }
        below = Some(p.add_item(&format!("Nest {d}"), crate::color::Label::None, Some(comps_folder), ItemKind::Comp(Arc::new(c))));
    }
    let mut leaves = vec![];
    for i in depth..others {
        let mut c = template.clone();
        fill(&mut p, &mut c, spec.comp_layers, &mut n);
        leaves.push(p.add_item(&format!("Shot {:03}", i + 1), crate::color::Label::None, Some(comps_folder), ItemKind::Comp(Arc::new(c))));
    }
    // Main: the chain's top, a few leaf precomps, then the bulk.
    let mut main = template;
    let mut layers = vec![];
    if let Some(b) = below {
        layers.push(build::layer(&mut p, &main, "Nest 1", LayerSource::Comp { item: b }, (1920, 1080), None));
    }
    for (k, leaf) in leaves.iter().take(spec.main_layers.saturating_sub(1) / 10).enumerate() {
        layers.push(build::layer(&mut p, &main, &format!("Shot {:03}", k + 1), LayerSource::Comp { item: *leaf }, (1920, 1080), None));
    }
    while layers.len() < spec.main_layers {
        let l = make_layer(&mut p, &main, spec, n, solid, &footage, layers.last());
        n += 1;
        layers.push(l);
    }
    main.layers = layers;
    p.add_item("Main", crate::color::Label::None, None, ItemKind::Comp(Arc::new(main)));
    // `Main` must be the first comp `replace_project` opens: give it the smallest id.
    let main_id = p.items.values().find(|i| i.name == "Main").map(|i| i.id).expect("main");
    let first = p.items.keys().next().copied().expect("items");
    if first != main_id {
        let item = p.items.remove(&main_id).expect("main");
        let other = p.items.remove(&first).expect("first");
        // Swap ids: references to `first` (the Media folder) are the footage/solid parents.
        p.items.insert(first, effectcraft_project::Item { id: first, ..item });
        p.items.insert(main_id, effectcraft_project::Item { id: main_id, ..other });
        for it in p.items.values_mut() {
            if it.parent == Some(first) {
                it.parent = Some(main_id);
            }
        }
    }
    p.fix_next_id();
    p
}

/// One measurement.
#[derive(Clone, Debug)]
pub struct Measure {
    pub name: &'static str,
    pub ms: f64,
    pub note: String,
}

impl Measure {
    pub fn json(&self) -> Value {
        json!({"name": self.name, "ms": (self.ms * 1000.0).round() / 1000.0, "note": self.note})
    }
}

/// Time `f` (best of `runs`).
pub fn time_best<T>(runs: usize, mut f: impl FnMut() -> T) -> (f64, T) {
    let mut best = f64::INFINITY;
    let mut out = None;
    for _ in 0..runs.max(1) {
        let t0 = web_time::Instant::now();
        let r = f();
        best = best.min(t0.elapsed().as_secs_f64() * 1000.0);
        out = Some(r);
    }
    (best, out.expect("ran"))
}

/// The engine-side everyday operations on a large project: build, save, open (into a session
/// from `fresh`), first frame, small and large edits with undo/redo, auto-save. Files go into
/// `dir`. Returns the measurements and the opened session (for the UI-side numbers).
pub fn ops_bench(spec: &LargeSpec, dir: &std::path::Path, fresh: &dyn Fn() -> Session) -> (Vec<Measure>, Session) {
    let mut out = vec![];
    let (ms, project) = time_best(1, || large_project(spec));
    let comps = project.comps().count();
    let layers: usize = project.comps().map(|(_, c)| c.layers.len()).sum();
    out.push(Measure { name: "generate", ms, note: format!("{comps} comps, {layers} layers, {} items", project.items.len()) });

    let path = dir.join("bench-large.ecproj");
    let path_s = path.to_string_lossy().to_string();
    let mut s = fresh();
    s.replace_project(project, None);
    let (ms, r) = time_best(3, || s.execute("file.saveAs", json!({"path": path_s})).expect("save"));
    out.push(Measure { name: "save", ms, note: format!("{} bytes", r["bytes"]) });

    // Open: what File ▸ Open does, into a fresh session.
    let (ms, mut s) = time_best(3, || {
        let mut s = fresh();
        s.execute("file.open", json!({"path": path_s})).expect("open");
        s
    });
    out.push(Measure { name: "open", ms, note: format!("active comp {:?}", s.state.active_comp.map(|c| c.0)) });
    // Its parts: reading the file and parsing it (the rest is replacing the session's project).
    let (ms, text) = time_best(3, || std::fs::read_to_string(&path).unwrap_or_default());
    out.push(Measure { name: "openRead", ms, note: format!("{} MB", text.len() / 1_000_000) });
    let (ms, _) = time_best(3, || Project::from_json(&text).expect("parse"));
    out.push(Measure { name: "openParse", ms, note: "JSON → Project".into() });
    let compact = Project::from_json(&text).expect("parse").to_json_compact();
    drop(text);
    let (ms, _) = time_best(3, || Project::from_json(&compact).expect("parse"));
    out.push(Measure { name: "openParseCompact", ms, note: format!("the same as compact JSON ({} MB, as auto-saves are written)", compact.len() / 1_000_000) });
    drop(compact);

    // First frame of the main comp at 1/4 resolution (the viewer's first request).
    let cid = s.active_comp_id().expect("main comp");
    let (ms, _) = time_best(1, || {
        let opts = crate::render::RenderOpts { scale: 0.25, ..Default::default() };
        s.render(cid, Tick::ZERO, opts)
    });
    out.push(Measure { name: "firstFrame", ms, note: "Main at 1/4, cold".into() });

    // A small edit in the big project (one property on one layer): the everyday cost of the
    // undo snapshot.
    let first = s.active_comp().and_then(|c| c.layers.get(10)).map(|l| l.id.0).expect("layer");
    let mut v = 0.0;
    let (ms, _) = time_best(20, || {
        v += 1.0;
        s.execute("prop.set", json!({"layer": first, "path": "transform/opacity", "value": 50.0 + v})).expect("prop.set")
    });
    out.push(Measure { name: "smallEdit", ms, note: "prop.set on one layer of Main".into() });
    let (ms, _) = time_best(5, || {
        s.undo();
        s.redo();
    });
    out.push(Measure { name: "smallUndoRedo", ms, note: "one undo + one redo".into() });

    // A large edit: every layer of Main at once.
    let all: Vec<u64> = s.active_comp().map(|c| c.layers.iter().map(|l| l.id.0).collect()).unwrap_or_default();
    let mut on = false;
    let (ms, _) = time_best(3, || {
        on = !on;
        s.execute("layer.setSwitch", json!({"layers": all, "switch": "shy", "value": on})).expect("setSwitch")
    });
    out.push(Measure { name: "largeEdit", ms, note: format!("shy switch on {} layers", all.len()) });
    let (ms, _) = time_best(3, || s.undo());
    out.push(Measure { name: "largeUndo", ms, note: String::new() });
    let (ms, _) = time_best(3, || s.redo());
    out.push(Measure { name: "largeRedo", ms, note: String::new() });

    // Auto-save: the full cost and what the UI thread pays.
    let auto = dir.join("autosave");
    let _ = s.execute("prefs.set", json!({"values": {"autoSave.location": "custom", "autoSave.folder": auto.to_string_lossy()}}));
    s.path = Some(path_s.clone());
    let (ms, _) = time_best(3, || s.autosave_now().expect("autosave"));
    out.push(Measure { name: "autosaveSync", ms, note: "serialise + write on the calling thread".into() });
    s.autosave.background = true;
    let (ms, _) = time_best(3, || s.autosave_now().expect("autosave"));
    out.push(Measure { name: "autosaveUiThread", ms, note: "background auto-save: UI-thread share".into() });
    s.autosave.background = false;
    let _ = s.autosave_wait();
    (out, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_project_round_trips_and_opens_main() {
        let spec = LargeSpec::small();
        let p = large_project(&spec);
        assert_eq!(p.comps().count(), spec.comps);
        let layers: usize = p.comps().map(|(_, c)| c.layers.len()).sum();
        assert!(layers >= spec.total_layers() - spec.comps, "{layers}");
        let footage = p.items.values().filter(|i| matches!(i.kind, ItemKind::Footage(_))).count();
        assert_eq!(footage, spec.footage);
        let back = Project::from_json(&p.to_json()).unwrap();
        assert_eq!(back, p);
        let mut s = Session::default();
        s.replace_project(p, None);
        assert_eq!(s.active_comp().map(|c| c.layers.len()), Some(spec.main_layers));
        // The nesting chain is `nest_depth` deep.
        let mut depth = 0;
        let mut cur = s.project.items.values().find(|i| i.name == "Nest 1").map(|i| i.id);
        while let Some(c) = cur {
            depth += 1;
            cur = s.project.comp(c).and_then(|c| {
                c.layers.iter().find_map(|l| match l.source {
                    LayerSource::Comp { item } => Some(item),
                    _ => None,
                })
            });
        }
        assert_eq!(depth, spec.nest_depth);
    }

    #[test]
    fn ops_bench_runs_on_a_small_project() {
        let dir = std::env::temp_dir().join(format!("ec-ops-bench-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (m, s) = ops_bench(&LargeSpec::small(), &dir, &Session::default);
        let names: Vec<&str> = m.iter().map(|m| m.name).collect();
        for want in ["generate", "save", "open", "firstFrame", "smallEdit", "largeEdit", "largeUndo", "largeRedo", "autosaveSync", "autosaveUiThread"] {
            assert!(names.contains(&want), "{want}");
        }
        assert!(m.iter().all(|m| m.ms.is_finite() && m.ms >= 0.0));
        assert!(s.autosave.last_path.as_deref().is_some_and(|p| std::path::Path::new(p).is_file()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
