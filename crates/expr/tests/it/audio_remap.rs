//! Production-expression integration tests, including nested mixing and adverse work patterns.
use crate::audio_workload as workload;
use effectcraft_expr::Expressions;
use effectcraft_keyframe::Value;
use effectcraft_project::{Layer, Property};
use effectcraft_render::{
    EvalCtx, ExprHost,
    audio::{AudioError, MixControl, MixSources, mix_comp, mix_comp_with_control},
};
use effectcraft_time::Tick;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Forces the full JavaScript path as an independent oracle for prepared expressions.
struct JavaScript;
impl ExprHost for JavaScript {
    /// Delegate unchanged property evaluation to the production JavaScript engine.
    fn eval(&self, ctx: &EvalCtx, layer: &Layer, prop: &Property, value: &Value) -> Result<Value, String> {
        Expressions.eval(ctx, layer, prop, value)
    }
}

/// Check numeric semantics and the boundary where expressions must fall back to JavaScript.
#[test]
fn scalar_preparation_matches_javascript_and_rejects_other_syntax() {
    let (p, cid) = workload::project(1, 0, None);
    let comp = p.comp(cid).unwrap();
    let layer = &comp.layers[0];
    let prop = layer.props.get("timeRemap").unwrap();
    let texts = [
        "0.25 + time * 0.5",
        "time * time * .01 + value",
        "(time + value) / 3 - -0.5",
        "1e-3 * time;",
        "value / (time-time)",
        "-0",
        "1e308 * 1e308 - 1e308",
        "time - value - 1",
        "time / 3 / 7",
        "+time",
    ];
    for text in texts {
        let prepared = Expressions.prepare_audio(text).unwrap();
        for time in [-123.0, -0.0, 0.0, 1.0 / 48000.0, 0.1, 1.0, 179.99] {
            let ctx = EvalCtx::new(&p, cid, comp, Tick::from_seconds_f64(time));
            for value in [-0.0, 0.25, 123.75] {
                let actual = prepared.evaluate(ctx.time.seconds(), value).map(f64::to_bits);
                let expected = effectcraft_expr::evaluate(&Expressions, &ctx, layer, prop, &Value::Scalar(value), text).ok().map(|v| v.as_f64().to_bits());
                assert_eq!(actual, expected, "{text}, time={time}, value={value}");
            }
        }
    }
    for text in [
        "Math.sin(time)",
        "time++",
        "--time",
        "0x10",
        "012",
        "time ** 2",
        "var x = time; x",
        "time ? 1 : 0",
        "value[0]",
        "time // comment",
        "time +",
        "/*comment*/time",
    ] {
        assert!(Expressions.prepare_audio(text).is_none(), "{text}");
    }
    assert!(Expressions.prepare_audio(&format!("{}time{}", "(".repeat(40), ")".repeat(40))).is_none());
    assert!(Expressions.prepare_audio(&"time+".repeat(300)).is_none());
}

/// Compare complete mixed buffers, including layered/nested projects and a non-dividing sample rate.
#[test]
fn medium_nested_project_matches_full_runtime_and_block_boundaries() {
    for (layers, depth, text) in
        [(12, 0, "0.25 + time * time * 0.01"), (4, 2, "0.25 + time * 0.5"), (2, 0, "0.25 + Math.sin(time) * Math.sin(time)"), (2, 0, "value / (time-time)")]
    {
        let (p, cid) = workload::project(layers, depth, Some(text));
        let source = workload::Source::default();
        let expected = mix_comp(&p, &source, Some(&JavaScript), cid, Tick::ZERO, 256, 44100).unwrap();
        let actual = mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, 256, 44100).unwrap();
        assert_eq!(actual, expected, "{layers} layers, depth {depth}, {text}");
        let mut split = mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, 73, 44100).unwrap();
        let mut start = Tick::from_units(73, 44100);
        if start.to_units_floor(44100) < 73 {
            start.0 += 1;
        }
        split.extend(mix_comp(&p, &source, Some(&Expressions), cid, start, 183, 44100).unwrap());
        assert_eq!(split, actual, "block boundary: {text}");
    }
}

/// Distinguish a reusable jump pattern from new-page traversal that must exhaust shared work.
#[test]
fn repeated_jumps_cache_pages_and_unbounded_traversal_returns_error() {
    let (p, cid) = workload::project(1, 0, Some("Math.floor(time * 48000) % 2 ? 10 : 0"));
    let source = workload::Source::default();
    let samples = mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, 1024, 48000).unwrap();
    assert_eq!(samples.len(), 2048);
    assert_eq!(mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, usize::MAX, 48000), Err(AudioError::WorkLimit));
    assert_eq!(source.reads.load(Ordering::Relaxed), 2);
    for depth in [0, 2] {
        let (p, cid) = workload::project(4, depth, Some("time * 100000"));
        let source = workload::Source::default();
        let control = MixControl { max_work: Some(100_000), ..Default::default() };
        assert_eq!(
            mix_comp_with_control(&MixSources { project: &p, footage: &source, expr: Some(&Expressions) }, cid, Tick::ZERO, 1024, 48000, &control),
            Err(AudioError::WorkLimit)
        );
        assert!(source.frames.load(Ordering::Relaxed) <= 100_000);
    }
}

/// Interrupt an expensive script and reuse its thread-local runtime for a successful mix.
#[test]
fn cancellation_interrupts_running_javascript_and_does_not_poison_next_mix() {
    let (p, cid) = workload::project(1, 0, Some("var n = 0; for (var i = 0; i < 1000000; i++) { n += Math.sin(i); } n"));
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let (done, completion) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let source = workload::Source::default();
        let control = MixControl { cancel: Some(signal), ..Default::default() };
        let result = mix_comp_with_control(&MixSources { project: &p, footage: &source, expr: Some(&Expressions) }, cid, Tick::ZERO, 1024, 48000, &control);
        assert_eq!(result, Err(AudioError::Cancelled));
        assert_eq!(source.reads.load(Ordering::Relaxed), 0);
        let (p, cid) = workload::project(1, 0, Some("Math.sin(time)"));
        assert!(mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, 32, 48000).is_ok());
        done.send(()).unwrap();
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    cancel.store(true, Ordering::Relaxed);
    completion.recv_timeout(std::time::Duration::from_secs(3)).expect("cancellation must interrupt the script, not wait for every sample");
    worker.join().unwrap();
}

/// A runtime resource limit aborts the mix instead of retrying the same script per sample.
#[test]
fn expression_resource_limits_abort_audio_and_recover() {
    let (p, cid) = workload::project(1, 0, Some("for (;;) {}"));
    let source = workload::Source::default();
    assert_eq!(mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, 2, 48000), Err(AudioError::WorkLimit));
    assert_eq!(source.reads.load(Ordering::Relaxed), 0);
    let (p, cid) = workload::project(1, 0, Some("Math.sin(time)"));
    assert!(mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, 32, 48000).is_ok());
}

/// One deadline reaches a running script; nested layers share the same yield allowance.
#[test]
fn audio_deadline_and_yield_allowance_are_cumulative() {
    for depth in [0, 2] {
        let (p, cid) = workload::project(4, depth, Some("function g() { for(var i=0;i<900000;i++) {} } for(var k=0;k<900000;k++) g()"));
        let source = workload::Source::default();
        for control in [
            MixControl { max_expression_time: Some(std::time::Duration::from_millis(50)), max_expression_yields: Some(usize::MAX), ..Default::default() },
            MixControl { max_expression_time: Some(std::time::Duration::from_secs(30)), max_expression_yields: Some(2), ..Default::default() },
        ] {
            let start = std::time::Instant::now();
            assert_eq!(
                mix_comp_with_control(&MixSources { project: &p, footage: &source, expr: Some(&Expressions) }, cid, Tick::ZERO, 1600, 48000, &control),
                Err(AudioError::WorkLimit)
            );
            assert!(start.elapsed() < std::time::Duration::from_secs(3));
        }
    }
    // Two individually small evaluations together exhaust the same allowance, rather than each
    // receiving a fresh budget. A deterministic host makes this independent of machine speed.
    struct Yielding;
    impl ExprHost for Yielding {
        /// Preserve the keyframed value outside controlled audio evaluation.
        fn eval(&self, _: &EvalCtx, _: &Layer, _: &Property, v: &Value) -> Result<Value, String> {
            Ok(v.clone())
        }
        /// Charge a deterministic yield to verify sharing across evaluations.
        fn eval_audio(&self, _: &EvalCtx, _: &Layer, _: &Property, v: &Value, c: &effectcraft_render::audio::AudioEvalControl) -> Result<Value, String> {
            c.yielded();
            Ok(v.clone())
        }
    }
    let (p, cid) = workload::project(4, 1, Some("time"));
    let control = MixControl { max_expression_yields: Some(2), ..Default::default() };
    assert_eq!(
        mix_comp_with_control(&MixSources { project: &p, footage: &workload::Source::default(), expr: Some(&Yielding) }, cid, Tick::ZERO, 10, 48000, &control),
        Err(AudioError::WorkLimit)
    );
    let (p, cid) = workload::project(1, 0, Some("Math.sin(time)"));
    assert!(mix_comp(&p, &workload::Source::default(), Some(&Expressions), cid, Tick::ZERO, 32, 48000).is_ok());
}

/// Put an expression on levels or a Tone parameter inside two remapped precomps.
fn parameter_project(effect: bool, text: &str, layers: usize) -> (effectcraft_project::Project, effectcraft_project::ItemId) {
    use effectcraft_project::{Expression, build::Ids};
    let (mut p, cid) = workload::project(layers, 2, None);
    let leaf =
        p.comps().find(|(_, c)| c.layers.iter().any(|l| matches!(l.source, effectcraft_project::LayerSource::Footage { .. }))).map(|(id, _)| *id).unwrap();
    for i in 0..layers {
        let expr = Some(Expression { text: text.into(), enabled: true });
        if effect {
            let spec = effectcraft_effects::find("ec.audio.tone").unwrap();
            let mut group = effectcraft_effects::instantiate(spec, &mut Ids(&mut p.next_id), spec.name, [16.0, 16.0]);
            let prop = group.children.iter_mut().find_map(|n| if let effectcraft_project::Node::Prop(p) = n { Some(p) } else { None }).unwrap();
            prop.expr = expr;
            p.comp_mut(leaf).unwrap().layers[i].props.sub_mut("effects").unwrap().children.push(group.into());
        } else {
            p.comp_mut(leaf).unwrap().layers[i].props.sub_mut("audio").unwrap().prop_mut("levels").unwrap().expr = expr;
        }
    }
    (p, cid)
}

/// Production parameter scripts obey resource limits; ordinary errors retain keyframed fallback.
#[test]
fn nested_audio_parameters_observe_script_limits_and_recover() {
    for effect in [false, true] {
        let (p, cid) = parameter_project(effect, "var n=0; for(var i=0;i<10000;i++) n+=Math.sin(i); value", 1);
        let source = workload::Source::default();
        let control = MixControl { max_expression_yields: Some(0), max_expression_time: Some(std::time::Duration::from_secs(30)), ..Default::default() };
        assert_eq!(
            mix_comp_with_control(&MixSources { project: &p, footage: &source, expr: Some(&Expressions) }, cid, Tick::ZERO, 2, 48000, &control),
            Err(AudioError::WorkLimit),
            "effect={effect}"
        );
        assert_eq!(source.reads.load(Ordering::Relaxed), 0);
        // The same thread's runtime recovers; normal script errors still fall back to keys.
        let (p, cid) = parameter_project(effect, "throw new Error('ordinary error')", 1);
        let actual = mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, 32, 48000).unwrap();
        let expected = mix_comp(&p, &source, None, cid, Tick::ZERO, 32, 48000).unwrap();
        assert_eq!(actual, expected);
    }
}

/// Each layer consumes the same allowance; reaching the limit prevents subsequent evaluation.
#[test]
fn nested_audio_parameters_share_one_yield_allowance() {
    #[derive(Default)]
    struct Yielding(std::sync::atomic::AtomicUsize);
    impl ExprHost for Yielding {
        /// Preserve the keyframed value outside controlled audio evaluation.
        fn eval(&self, _: &EvalCtx, _: &Layer, _: &Property, v: &Value) -> Result<Value, String> {
            Ok(v.clone())
        }
        /// Charge a deterministic yield to verify sharing across evaluations.
        fn eval_audio(&self, _: &EvalCtx, _: &Layer, _: &Property, v: &Value, c: &effectcraft_render::audio::AudioEvalControl) -> Result<Value, String> {
            self.0.fetch_add(1, Ordering::Relaxed);
            c.yielded();
            Ok(v.clone())
        }
    }
    for effect in [false, true] {
        let (p, cid) = parameter_project(effect, "value", 8);
        let host = Yielding::default();
        let control = MixControl { max_expression_yields: Some(2), ..Default::default() };
        assert_eq!(
            mix_comp_with_control(&MixSources { project: &p, footage: &workload::Source::default(), expr: Some(&host) }, cid, Tick::ZERO, 10, 48000, &control),
            Err(AudioError::WorkLimit)
        );
        assert_eq!(host.0.load(Ordering::Relaxed), 3);
    }
}

/// Cancellation interrupts a running volume/effect script and leaves its runtime reusable.
#[test]
fn cancellation_interrupts_nested_audio_parameters() {
    for effect in [false, true] {
        let (p, cid) = parameter_project(effect, "function g() { for(var i=0;i<900000;i++) {} } for(var k=0;k<900000;k++) g()", 1);
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let (done, completion) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let source = workload::Source::default();
            let control = MixControl { cancel: Some(signal), max_expression_yields: Some(usize::MAX), ..Default::default() };
            assert_eq!(
                mix_comp_with_control(&MixSources { project: &p, footage: &source, expr: Some(&Expressions) }, cid, Tick::ZERO, 2, 48000, &control),
                Err(AudioError::Cancelled)
            );
            assert_eq!(source.reads.load(Ordering::Relaxed), 0);
            let (p, cid) = parameter_project(effect, "value", 1);
            assert!(mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, 32, 48000).is_ok());
            done.send(()).unwrap();
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel.store(true, Ordering::Relaxed);
        completion.recv_timeout(std::time::Duration::from_secs(3)).expect("parameter script must observe cancellation");
        worker.join().unwrap();
    }
}

/// Property-read pass exhaustion is a resource failure, including when reached from audio.
#[test]
fn property_read_pass_limits_abort_audio_and_recover() {
    // A fresh host request on each replay reaches the pass limit without a costly JS loop.
    let text = "globalThis.audioPassProbe = (globalThis.audioPassProbe || 0) + 1; thisLayer.transform.opacity.valueAtTime(globalThis.audioPassProbe, true)";
    let (p, cid) = workload::project(1, 0, Some(text));
    let layer = &p.comp(cid).unwrap().layers[0];
    let error = effectcraft_expr::eval_property(&p, cid, layer.id, "timeRemap", 0.0).unwrap_err();
    assert!(error.contains("too many property reads"), "must exercise the pass limit: {error}");
    let source = workload::Source::default();
    let control = MixControl { max_expression_yields: Some(usize::MAX), max_expression_time: Some(std::time::Duration::from_secs(30)), ..Default::default() };
    assert_eq!(
        mix_comp_with_control(&MixSources { project: &p, footage: &source, expr: Some(&Expressions) }, cid, Tick::ZERO, 2, 48000, &control),
        Err(AudioError::WorkLimit)
    );
    assert_eq!(source.reads.load(Ordering::Relaxed), 0);
    let (p, cid) = workload::project(1, 0, Some("Math.sin(time)"));
    assert!(mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, 32, 48000).is_ok());
}

#[test]
fn audio_result_copy_shares_the_expression_budget() {
    let (p, cid) = workload::project(1, 0, Some("Array(65536).fill(1)"));
    let source = workload::Source::default();
    let control = MixControl { max_expression_yields: Some(0), ..Default::default() };
    assert_eq!(
        mix_comp_with_control(&MixSources { project: &p, footage: &source, expr: Some(&Expressions) }, cid, Tick::ZERO, 32, 48000, &control),
        Err(AudioError::WorkLimit)
    );
    let (p, cid) = workload::project(1, 0, Some("Math.sin(time)"));
    assert!(mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, 32, 48000).is_ok());
}

#[test]
fn audio_finalization_does_not_invoke_returned_accessors() {
    let (p, cid) = workload::project(1, 0, Some("({ get __isProp() { return true; }, value: 0.5 + time })"));
    let source = workload::Source::default();
    let expected = mix_comp(&p, &source, None, cid, Tick::ZERO, 32, 48000).unwrap();
    let actual = mix_comp(&p, &source, Some(&Expressions), cid, Tick::ZERO, 32, 48000).unwrap();
    assert_eq!(actual, expected, "an accessor result uses the keyframed value");
}

#[test]
fn returned_property_proxies_match_explicit_values_in_audio() {
    use effectcraft_keyframe::Keyframe;
    for (property, output, first, last) in
        [("rotation", "timeRemap", Value::Scalar(0.5), Value::Scalar(1.5)), ("position", "audio/levels", Value::Vec2([-6.0, -12.0]), Value::Vec2([-3.0, -6.0]))]
    {
        let (mut p, cid) = workload::project(1, 0, None);
        let layer = &mut p.comp_mut(cid).unwrap().layers[0];
        layer.props.prop_mut(&format!("transform/{property}")).unwrap().keys =
            vec![Keyframe::new(Tick::ZERO, first), Keyframe::new(Tick::from_seconds_f64(1.0), last)];
        let baseline = mix_comp(&p, &workload::Source::default(), None, cid, Tick::ZERO, 128, 48000).unwrap();
        let mut mix = |suffix: &str| {
            p.comp_mut(cid).unwrap().layers[0].props.prop_mut(output).unwrap().expr =
                Some(effectcraft_project::Expression { text: format!("thisLayer.transform.{property}{suffix}"), enabled: true });
            mix_comp(&p, &workload::Source::default(), Some(&Expressions), cid, Tick::ZERO, 128, 48000)
                .unwrap_or_else(|error| panic!("{property}{suffix}: {error}"))
        };
        let expected = mix(".value");
        assert!(expected.iter().any(|&v| v != 0.0));
        assert_ne!(expected, baseline, "the expression changes the mix: {property}");
        assert_eq!(mix(""), expected, "{property}");
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn sampled_footage_pauses_expression_time_but_preserves_deadline_and_cancellation() {
    use effectcraft_project::{Footage, ItemId, ItemKind, LayerSource, build};
    use effectcraft_render::{FootageSource, Image};
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    struct Clip {
        audio: workload::Source,
        reads: AtomicUsize,
        delay: Duration,
        cancel: Option<Arc<AtomicBool>>,
        gray: f32,
    }
    impl FootageSource for Clip {
        fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(self.delay);
            if let Some(cancel) = &self.cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            let mut img = Image::new(f.width, f.height);
            img.data.fill([self.gray, self.gray, self.gray, 1.0]);
            Some(Arc::new(img))
        }
        fn audio(&self, item: ItemId, f: &Footage, t: Tick, n: usize, rate: u32) -> Option<Vec<f32>> {
            self.audio.audio(item, f, t, n, rate)
        }
    }

    let (mut p, cid) = workload::project(1, 0, Some("thisComp.layer('Clip').sampleImage([4,4], [0.5,0.5], false, 0)[0] + time"));
    let item = p.add_item(
        "Clip",
        effectcraft_color::Label::Aqua,
        None,
        ItemKind::Footage(Footage { width: 8, height: 8, has_video: true, kind: effectcraft_project::FootageKind::Still, ..Default::default() }),
    );
    let comp = p.comp(cid).unwrap().clone();
    let layer = build::layer(&mut p, &comp, "Clip", LayerSource::Footage { item }, (8, 8), None);
    p.comp_mut(cid).unwrap().layers.push(layer);
    let source = |gray, delay, cancel| Clip { audio: workload::Source::default(), reads: AtomicUsize::new(0), delay, cancel, gray };
    let mix = |clip: &Clip, control: &MixControl| {
        let sources = MixSources { project: &p, footage: clip, expr: Some(&Expressions) };
        mix_comp_with_control(&sources, cid, Tick::ZERO, 16, 48000, control)
    };

    let reference = source(0.5, Duration::ZERO, None);
    let expected = mix(&reference, &MixControl::default()).unwrap();
    assert_eq!(reference.reads.load(Ordering::Relaxed), 1, "one sampled frame per mix");
    let slow = source(0.5, Duration::from_millis(400), None);
    let control = MixControl { max_expression_time: Some(Duration::from_millis(200)), ..Default::default() };
    assert_eq!(mix(&slow, &control).unwrap(), expected);
    assert_eq!(slow.reads.load(Ordering::Relaxed), 1);
    let dark = source(0.0, Duration::ZERO, None);
    assert_ne!(mix(&dark, &control).unwrap(), expected, "sampling follows the current mix's footage");

    let deadline = std::time::Instant::now() + Duration::from_millis(250);
    let control = MixControl { deadline: Some(deadline), ..control };
    assert_eq!(mix(&slow, &control), Err(AudioError::DeadlineExceeded));
    assert_eq!(slow.reads.load(Ordering::Relaxed), 2, "the read began before the deadline");

    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled = source(0.5, Duration::ZERO, Some(cancel.clone()));
    let control = MixControl { cancel: Some(cancel), deadline: None, ..control };
    assert_eq!(mix(&cancelled, &control), Err(AudioError::Cancelled));
    assert_eq!(cancelled.audio.reads.load(Ordering::Relaxed), 0);
    assert_eq!(mix(&reference, &MixControl::default()).unwrap(), expected);
}
