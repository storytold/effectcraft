//! Evaluation cost: host-request re-runs, prefetching and identical results.

use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::build;
use effectcraft_project::{Comp, ItemKind, LayerId, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::script_runs;
use crate::tests::{Fx, fx};

const ROT: &str = "transform/rotation";
const POS: &str = "transform/position";

/// Representative expressions as `(layer, property, text)`, evaluated at [`TIMES`].
fn representative(f: &Fx) -> Vec<(LayerId, &'static str, &'static str)> {
    vec![
        (f.a, POS, "wiggle(2, 30)"),
        (f.a, ROT, "wiggle(3, 45, 2, 0.6)"),
        (f.a, ROT, "temporalWiggle(3, 0.2)"),
        (f.b, ROT, "thisComp.layer(\"A\").transform.rotation.valueAtTime(time - 0.25)"),
        (f.b, ROT, "thisComp.layer(\"A\").effect(\"Slider Control\")(\"Slider\") * 2 + time"),
        (f.b, ROT, "var s = 0; for (var i = 1; i <= thisComp.numLayers; i++) s += thisComp.layer(i).transform.position[0]; s"),
        (f.b, POS, "var r = thisComp.layer(\"Shape\").sourceRectAtTime(); [r.left, r.top, r.width + r.height]"),
        (f.b, POS, "var r = thisComp.layer(\"Title\").sourceRectAtTime(time); [r.left + r.top, r.width, r.height]"),
        (f.b, ROT, "linear(time, 0, 2, 0, 100) + ease(time, 0.5, 3, 10, 20) + easeIn(time, 0, 1, 0, 5)"),
        (f.b, POS, "ease(time, 0, 2, [0, 0], [100, 50])"),
        (f.a, ROT, "smooth(0.3, 5)"),
        (f.a, ROT, "loopOut('pingpong') + velocity / 10"),
        (f.a, ROT, "nearestKey(time).time + key(2).value + numKeys"),
        (f.b, POS, "thisComp.layer(\"B\").toComp([0, 0]) + thisComp.layer(\"A\").fromWorld([5, 5, 0])"),
        (f.b, ROT, "thisComp.layer(\"A\").transform.rotation.velocityAtTime(time) + thisComp.layer(index - 1).rotation.speed"),
    ]
}

const TIMES: [f64; 4] = [0.0, 0.5, 1.37, 4.2];

fn results(f: &Fx) -> Vec<String> {
    let mut out = vec![];
    for (l, path, text) in representative(f) {
        for t in TIMES {
            out.push(match f.ev(l, path, text, t) {
                Ok(Value::Scalar(x)) => format!("{x:?}"),
                Ok(v) => format!("{:?}", v.components()),
                Err(e) => format!("error {e}"),
            });
        }
    }
    out
}

/// [`results`] before host requests were collected per run and prefetched (bit-exact).
const BASELINE: [&str; 60] = [
    "[68.50842270250163, 33.29488516071788, 0.0]",
    "[43.81508809180965, 40.95415032598344, 0.0]",
    "[79.42061191939588, 46.08444637322505, 0.0]",
    "[29.260151628712812, 53.695929085154, 0.0]",
    "-50.91808396830139",
    "52.73313232371497",
    "126.18531029086893",
    "106.44917641653214",
    "0.0",
    "52.19685849914966",
    "90.0",
    "90.0",
    "0.0",
    "22.5",
    "90.0",
    "90.0",
    "84.0",
    "84.5",
    "85.37",
    "88.2",
    "270.0",
    "270.0",
    "270.0",
    "270.0",
    "[-31.0, -16.0, 94.0]",
    "[-31.0, -16.0, 94.0]",
    "[-31.0, -16.0, 94.0]",
    "[-31.0, -16.0, 94.0]",
    "[-25.5859375, 90.859375, 29.5703125]",
    "[-25.5859375, 90.859375, 29.5703125]",
    "[-25.5859375, 90.859375, 29.5703125]",
    "[-25.5859375, 90.859375, 29.5703125]",
    "10.0",
    "36.875",
    "86.29023616",
    "125.0",
    "[0.0, 0.0, 0.0]",
    "[15.625, 7.8125, 0.0]",
    "[76.483675, 38.2418375, 0.0]",
    "[100.0, 50.0, 0.0]",
    "4.05",
    "45.0",
    "90.0",
    "90.0",
    "4.5",
    "54.00000000000034",
    "56.7",
    "18.0",
    "92.0",
    "92.0",
    "93.0",
    "93.0",
    "[45.0, 5.0, 0.0]",
    "[33.431457505076196, 47.07106781186547, 0.0]",
    "[55.0, 85.0, 0.0]",
    "[55.0, 85.0, 0.0]",
    "90.0",
    "180.00000000000682",
    "0.0",
    "0.0",
];

#[test]
fn representative_results_are_unchanged() {
    let f = fx();
    // First pass: every expression's first time runs without a prediction, later times with one.
    assert_eq!(results(&f), BASELINE);
    // Second pass: every evaluation is predicted (and some predictions are for another time).
    assert_eq!(results(&f), BASELINE);
    // A fresh thread starts without predictions.
    let fresh = std::thread::spawn(move || results(&fx())).join().unwrap();
    assert_eq!(fresh, BASELINE);
}

/// Script runs of one evaluation (on this thread).
fn runs_of(f: &Fx, layer: LayerId, path: &str, text: &str, t: f64) -> (u64, Value) {
    let before = script_runs();
    let v = f.ev(layer, path, text, t).unwrap();
    (script_runs() - before, v)
}

#[test]
fn chained_lookups_run_a_bounded_number_of_times() {
    let f = fx();
    let text = "thisComp.layer(\"A\").effect(\"Slider Control\")(\"Slider\") + thisComp.layer(\"A\").transform.rotation";
    // Each step of the chain depends on the one before: one run per step, the first time.
    let (first, v) = runs_of(&f, f.b, ROT, text, 0.5);
    assert_eq!(v.as_f64(), 42.0 + 45.0);
    assert!(first <= 6, "first evaluation ran {first} times");
    // Later frames answer everything up front: one run.
    for t in [0.6, 0.7, 3.0] {
        let (n, v) = runs_of(&f, f.b, ROT, text, t);
        assert_eq!(n, 1, "at {t}");
        assert_eq!(v.as_f64(), 42.0 + (t * 90.0).min(90.0));
    }
}

/// A comp of `n` solids at x = 1, 2, …, n.
fn many(n: usize) -> (Project, effectcraft_project::ItemId, Vec<LayerId>) {
    let mut p = Project::default();
    let comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(10.0));
    let cid = p.add_item("Main", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let mut layers = vec![];
    for i in 0..n {
        let sid = p.add_item(&format!("S{i}"), Label::Red, None, ItemKind::Solid(Solid { color: [1.0; 3], width: 10, height: 10, pixel_aspect: 1.0 }));
        let mut l = build::layer(&mut p, &comp, &format!("L{}", i + 1), LayerSource::Solid { item: sid }, (10, 10), None);
        l.props.prop_mut(POS).unwrap().value = Value::Vec3([(i + 1) as f64, 0.0, 0.0]);
        layers.push(l);
    }
    let ids = layers.iter().map(|l| l.id).collect();
    p.comp_mut(cid).unwrap().layers = layers;
    (p, cid, ids)
}

#[test]
fn a_loop_over_layers_runs_a_constant_number_of_times() {
    let text = "var s = 0; for (var i = 1; i <= thisComp.numLayers; i++) s += thisComp.layer(i).transform.position[0]; s";
    let mut counts = vec![];
    for n in [10usize, 40] {
        let (p, cid, ids) = many(n);
        let ev = |t: f64| {
            let before = script_runs();
            let v = crate::eval_standalone(&p, cid, ids[0], ROT, text, t).unwrap().as_f64();
            (script_runs() - before, v)
        };
        let expected = (n * (n + 1) / 2) as f64;
        let (first, v) = ev(0.0);
        assert_eq!(v, expected);
        let (second, v) = ev(0.5);
        assert_eq!(v, expected);
        assert_eq!(second, 1, "{n} layers: later frames run once");
        counts.push(first);
    }
    // Every layer's lookups are collected in the same runs: the count doesn't grow with the
    // number of layers (it was about 4 runs per layer).
    assert_eq!(counts[0], counts[1], "runs for 10 and 40 layers: {counts:?}");
    assert!(counts[1] <= 8, "runs: {counts:?}");
}

#[test]
fn time_shifted_reads_cost_at_most_one_extra_run() {
    let f = fx();
    let _ = runs_of(&f, f.a, ROT, "smooth(0.3, 5)", 0.2);
    for t in [0.3, 0.4, 2.5] {
        let (n, _) = runs_of(&f, f.a, ROT, "smooth(0.3, 5)", t);
        assert!(n <= 2, "at {t}: {n} runs");
    }
}

#[test]
fn a_stale_prediction_never_changes_the_result() {
    let mut f = fx();
    let text = "thisComp.layer(\"A\").transform.position[0]";
    assert_eq!(f.ev(f.b, ROT, text, 0.0).unwrap().as_f64(), 50.0);
    // Rename so "A" now names B (at x = 100): the prediction still looks up the old answers.
    let (a, b) = (f.a, f.b);
    f.layer_mut(a).name = "Z".into();
    f.layer_mut(b).name = "A".into();
    assert_eq!(f.ev(f.b, ROT, text, 0.1).unwrap().as_f64(), 100.0);
    // Branches that change with time read different things than last time.
    let text = "time < 1 ? thisComp.layer(\"Z\").transform.position[1] : thisComp.layer(\"A\").effect(1)(1)";
    for (t, want) in [(0.0, 40.0), (2.0, 0.0), (0.5, 40.0)] {
        assert_eq!(f.ev(f.b, ROT, text, t).map(|v| v.as_f64()).unwrap_or(0.0), want, "at {t}");
    }
}

#[test]
fn stand_in_answers_never_reach_a_result() {
    let f = fx();
    // While a run collects misses the slider reads as a stand-in 0; the result uses 42.
    let text =
        "var n = 0; while (n < 1e8 && thisComp.layer(\"A\").effect(\"Slider Control\")(\"Slider\").value == 0) n++; n + thisComp.layer(\"A\").effect(1)(1)";
    assert_eq!(f.ev(f.b, ROT, text, 0.0).unwrap().as_f64(), 42.0);
    // A script that catches errors still sees only real answers.
    let text = "var r; try { r = thisComp.layer(\"A\").name + thisComp.layer(2).name; } catch (e) { r = 'caught'; } r";
    assert_eq!(f.text(text, 0.0), "AB");
    // A missing layer is an error, not a stand-in.
    assert!(f.ev(f.b, ROT, "thisComp.layer(\"Nope\").transform.rotation", 0.0).is_err());
}

#[test]
fn keyframe_api_reads_keys_lazily() {
    let f = fx();
    for (text, want) in [
        ("thisComp.layer(\"A\").transform.rotation.numKeys", 2.0),
        ("thisComp.layer(\"A\").transform.rotation.key(2).time", 1.0),
        ("thisComp.layer(\"A\").transform.rotation.key(2).value", 90.0),
        ("thisComp.layer(\"A\").transform.rotation.nearestKey(0.9).index", 2.0),
        ("thisComp.layer(\"B\").transform.rotation.numKeys", 0.0),
        ("thisComp.layer(\"B\").transform.rotation.loopOut()", 0.0),
    ] {
        assert_eq!(f.ev(f.b, ROT, text, 0.0).unwrap().as_f64(), want, "{text}");
    }
    assert!(f.ev(f.b, ROT, "thisComp.layer(\"B\").transform.rotation.nearestKey(0).time", 0.0).is_err());
}

#[test]
fn shape_source_rect_matches_the_rendered_buffer() {
    use effectcraft_project::build::Ids;
    use effectcraft_render::EvalCtx;
    let f = fx();
    let comp = f.p.comp(f.cid).unwrap().clone();
    let shapes: Vec<Box<dyn Fn(&mut Ids) -> Vec<effectcraft_project::PropGroup>>> = vec![
        Box::new(|_| vec![]),
        Box::new(|ids| {
            let items = vec![build::shape_rect(ids, [60.3, 30.7], [5.2, -3.1], 4.0), build::shape_fill(ids, [1.0, 0.0, 0.0, 1.0])];
            vec![build::shape_group(ids, "R", items)]
        }),
        Box::new(|ids| {
            let star = vec![build::shape_star(ids, true, 5.0, [10.0, 10.0], 40.0, 20.0), build::shape_stroke(ids, [0.0, 0.0, 1.0, 1.0], 7.5)];
            let ellipse = vec![build::shape_ellipse(ids, [33.0, 17.0], [-80.5, 20.25]), build::shape_fill(ids, [0.0, 1.0, 0.0, 1.0])];
            vec![build::shape_group(ids, "S", star), build::shape_group(ids, "E", ellipse)]
        }),
        Box::new(|ids| {
            let items = vec![build::shape_rect(ids, [0.5, 0.5], [0.0, 0.0], 0.0), build::shape_fill(ids, [1.0; 4])];
            vec![build::shape_group(ids, "Tiny", items)]
        }),
    ];
    for (i, make) in shapes.iter().enumerate() {
        let mut p = f.p.clone();
        let mut next = p.next_id;
        let groups = make(&mut Ids(&mut next));
        p.next_id = next;
        let mut l = build::layer(&mut p, &comp, "Shape", LayerSource::Shape, (200, 100), None);
        l.props.sub_mut("contents").unwrap().children = groups.into_iter().map(Into::into).collect();
        let c = p.comp(f.cid).unwrap();
        let ctx = EvalCtx::new(&p, f.cid, c, Tick::from_seconds_f64(0.5));
        let contents = l.props.sub("contents").unwrap();
        let buf = effectcraft_render::shapes::render(&ctx, &l, contents, 1.0);
        let rendered = if buf.img.width <= 4 { None } else { Some((buf.img.width, buf.img.height, buf.offset)) };
        let geometric = effectcraft_render::shapes::buffer_rect(&ctx, &l, contents, 1.0).filter(|(w, _, _)| *w > 4);
        assert_eq!(geometric, rendered, "shape {i}");
    }
}
