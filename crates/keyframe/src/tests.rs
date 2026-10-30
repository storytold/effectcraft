use super::*;

fn s(x: f64) -> Tick {
    Tick::from_seconds_f64(x)
}

#[test]
fn missing_spatial_keys_have_no_tangents_or_length() {
    let keys = vec![Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0]))];
    for i in [1, usize::MAX] {
        assert_eq!(spatial_tangents(&keys, i), ([0.0; 3], [0.0; 3]));
        assert_eq!(spatial_segment_length(&keys, i), 0.0);
    }
    assert_eq!(spatial_tangents(&[], 0), ([0.0; 3], [0.0; 3]));
    assert_eq!(spatial_segment_length(&[], usize::MAX), 0.0);
}

#[test]
fn time_reverse_handles_deserialized_tick_extremes() {
    // Tick's public serde representation can contain values beyond from_seconds_f64's cap.
    for (first, last) in [(i64::MIN, i64::MIN + 10), (i64::MAX - 10, i64::MAX), (i64::MIN, i64::MAX)] {
        let mut keys = vec![Keyframe::new(Tick(first), Value::Scalar(1.0)), Keyframe::new(Tick(last), Value::Scalar(2.0))];
        time_reverse(&mut keys);
        assert_eq!(keys[0].time, Tick(first));
        assert_eq!(keys[0].value, Value::Scalar(2.0));
        assert_eq!(keys[1].time, Tick(last));
        assert_eq!(keys[1].value, Value::Scalar(1.0));
        time_reverse(&mut keys);
        assert_eq!(keys[0].value, Value::Scalar(1.0));
    }
}

#[test]
fn roving_handles_deserialized_tick_extremes() {
    let mut keys = vec![
        Keyframe::new(Tick(i64::MIN), Value::Vec2([0.0, 0.0])),
        Keyframe::new(Tick(10), Value::Vec2([50.0, 0.0])),
        Keyframe::new(Tick(i64::MAX), Value::Vec2([100.0, 0.0])),
    ];
    for key in &mut keys {
        key.spatial_auto = false;
    }
    keys[1].roving = true;
    assert!(retime_roving(&mut keys, true));
    assert_eq!(keys[1].time, Tick::ZERO);
}

#[test]
fn replacing_key_preserves_spatial_metadata() {
    let mut key = Keyframe::new(s(1.0), Value::Vec2([20.0, 30.0]));
    key.roving = true;
    key.spatial_continuous = true;
    let mut keys = vec![key];
    set_key(&mut keys, Keyframe::new(s(1.0), Value::Vec2([40.0, 50.0])));
    assert!(keys[0].roving);
    assert!(keys[0].spatial_continuous);
    assert_eq!(keys[0].value, Value::Vec2([40.0, 50.0]));
}

#[test]
fn temporal_ease_matches_live_ae_with_overlapping_influences() {
    // Original Rotation probe via AEsync 2.0.4, AE 26.3x87, 2026-10-05.
    // AE retains each handle's requested influence even when their sum exceeds 100%.
    for (from, to, out_speed, out_inf, in_speed, in_inf, samples) in [
        (0.0, 100.0, 0.0, 0.8, 0.0, 0.8, [(0.1, 0.59243939036327), (0.25, 4.76438030608764), (0.4, 18.4284394271562), (0.75, 95.2356196939123)]),
        (0.0, 100.0, 20.0, 0.8, 80.0, 0.6, [(0.1, 2.25754942621577), (0.25, 6.99841889412931), (0.4, 14.9334636959052), (0.75, 73.8196785378389)]),
        (100.0, 0.0, -20.0, 0.6, -80.0, 0.8, [(0.1, 97.6395466334594), (0.25, 91.597048571292), (0.4, 74.5367957474338), (0.75, 22.3789127753126)]),
    ] {
        let mut a = Keyframe::new(s(0.0), Value::Scalar(from));
        let mut b = Keyframe::new(s(1.0), Value::Scalar(to));
        a.out_interp = Interp::Bezier;
        b.in_interp = Interp::Bezier;
        a.out_ease = vec![Ease { speed: out_speed, influence: out_inf }];
        b.in_ease = vec![Ease { speed: in_speed, influence: in_inf }];
        for (time, expected) in samples {
            let value = evaluate(&[a.clone(), b.clone()], s(time), false).unwrap().as_f64();
            assert!((value - expected).abs() < 1e-7, "{time}: {value} != {expected}");
            let normalized = ease_progress(out_speed / (to - from), out_inf, in_speed / (to - from), in_inf, time);
            assert!((from + normalized * (to - from) - expected).abs() < 1e-7);
        }
    }
}

#[test]
fn maximum_influence_resolves_flat_midpoint_time_curve() {
    for time in [0.5 - 1e-9, 0.5 - 1e-12, 0.5, 0.5 + 1e-12, 0.5 + 1e-9] {
        // At 100%/100%, x = 0.5 + 4*(u - 0.5)^3 exactly.
        let u = 0.5 + ((time - 0.5) / 4.0_f64).cbrt();
        let expected = bez(0.0, 0.0, 1.0, 1.0, u);
        let actual = ease_progress(0.0, 1.0, 0.0, 1.0, time);
        assert!((actual - expected).abs() < 1e-10, "{time}: {actual} != {expected}");
    }
}

/// M13.15: agents set gradients as plain JSON, not only the tagged form `get` returns.
#[test]
fn gradients_coerce_from_plain_json() {
    let g = Value::Gradient(Gradient::default());
    let plain = serde_json::json!({"colors": [[0, [0, 0.5, 1, 1]], [1, "#ffff00"]], "opacities": [[0, 1], [1, 0.5]]});
    let Some(Value::Gradient(a)) = g.coerce_json(&plain) else { panic!("object form") };
    assert_eq!(a.colors, vec![(0.0, [0.0, 0.5, 1.0, 1.0]), (1.0, [1.0, 1.0, 0.0, 1.0])]);
    assert_eq!(a.opacities, vec![(0.0, 1.0), (1.0, 0.5)]);
    let Some(Value::Gradient(b)) = g.coerce_json(&serde_json::json!(["#ff0000", "#00ff00", "#0000ff"])) else { panic!("list form") };
    assert_eq!(b.colors.iter().map(|c| c.0).collect::<Vec<_>>(), vec![0.0, 0.5, 1.0]);
    assert_eq!(b.opacities.len(), 3);
    // The tagged form (what prop.get returns) still round-trips.
    assert_eq!(g.coerce_json(&g.to_json()), Some(g.clone()));
    assert_eq!(g.coerce_json(&serde_json::json!(5)), None);
}

/// #612: the gradient editor samples a new stop from its own track only. `sample` multiplies
/// in the opacity stops, so feeding its alpha into a colour stop would apply 50% twice (25%).
#[test]
fn gradient_tracks_sample_separately() {
    let g = Gradient { colors: vec![(0.0, [1.0, 0.0, 0.0, 0.8]), (1.0, [0.0, 0.0, 1.0, 0.4])], opacities: vec![(0.0, 0.5), (1.0, 0.5)] };
    let c = g.color_at(0.5);
    assert!((c[0] - 0.5).abs() < 1e-6 && (c[2] - 0.5).abs() < 1e-6, "{c:?}");
    assert!((c[3] - 0.6).abs() < 1e-6, "the colour track's own alpha, without opacity: {c:?}");
    assert!((g.opacity_at(0.5) - 0.5).abs() < 1e-6);
    assert!((g.sample(0.5)[3] - 0.3).abs() < 1e-6, "sample still applies opacity once");
    // A stop added at 50% from the opacity track keeps 50%: sampled again, it is still 50%.
    let mut with_stop = g.clone();
    with_stop.opacities.insert(1, (0.5, g.opacity_at(0.5)));
    assert!((with_stop.opacity_at(0.5) - 0.5).abs() < 1e-6);
    assert!((with_stop.sample(0.5)[3] - 0.3).abs() < 1e-6, "adding a stop doesn't change the look");
    // Outside the stops, and with none: the ends hold, and an empty track is opaque white.
    assert_eq!(g.color_at(-1.0), [1.0, 0.0, 0.0, 0.8]);
    assert_eq!(g.color_at(2.0), [0.0, 0.0, 1.0, 0.4]);
    let empty = Gradient { colors: vec![], opacities: vec![] };
    assert_eq!((empty.color_at(0.3), empty.opacity_at(0.3)), ([1.0; 4], 1.0));
}

/// #612: the editor saves the typed form, so one colour stop (which the plain JSON parser
/// rejects) and each stop's RGBA come back exactly.
#[test]
fn typed_gradient_round_trips_one_stop_and_rgba() {
    let one = Value::Gradient(Gradient { colors: vec![(0.25, [0.1, 0.2, 0.3, 0.7])], opacities: vec![(0.0, 1.0), (0.6, 0.35)] });
    let cur = Value::Gradient(Gradient::default());
    assert_eq!(cur.coerce_json(&one.to_json()), Some(one.clone()));
    assert_eq!(cur.coerce_json(&serde_json::json!({"colors": [[0.25, [0.1, 0.2, 0.3, 0.7]]]})), None, "plain JSON needs two colours");
}

#[test]
fn linear_and_hold() {
    let keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)), Keyframe::new(s(2.0), Value::Scalar(100.0))];
    assert_eq!(evaluate(&keys, s(1.0), false), Some(Value::Scalar(50.0)));
    assert_eq!(evaluate(&keys, s(-1.0), false), Some(Value::Scalar(0.0)));
    assert_eq!(evaluate(&keys, s(3.0), false), Some(Value::Scalar(100.0)));
    let held = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)).hold(), Keyframe::new(s(2.0), Value::Scalar(100.0))];
    assert_eq!(evaluate(&held, s(1.999), false), Some(Value::Scalar(0.0)));
}

#[test]
fn hold_key_keeps_its_incoming_segment_linear() {
    // Hold on the second key (in and out, as KeyframeInterpolationType.HOLD sets it) holds
    // what comes after it, not the linear motion into it.
    let keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)), Keyframe::new(s(2.0), Value::Scalar(100.0)).hold()];
    assert_eq!(evaluate(&keys, s(0.5), false), Some(Value::Scalar(25.0)));
    let keys = vec![Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0])), Keyframe::new(s(2.0), Value::Vec2([100.0, 0.0])).hold()];
    let p = evaluate(&keys, s(0.5), true).unwrap().as_vec2();
    assert!((p[0] - 25.0).abs() < 0.01, "{p:?}");
}

#[test]
fn easy_ease_is_symmetric_and_slow_at_ends() {
    let keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)).eased(), Keyframe::new(s(1.0), Value::Scalar(100.0)).eased()];
    let mid = evaluate(&keys, s(0.5), false).unwrap().as_f64();
    assert!((mid - 50.0).abs() < 1e-6, "{mid}");
    let early = evaluate(&keys, s(0.1), false).unwrap().as_f64();
    assert!(early < 10.0 && early > 0.0, "{early}");
    let v0 = velocity(&keys, s(0.01), false)[0];
    let vm = velocity(&keys, s(0.5), false)[0];
    assert!(v0 < vm);
}

#[test]
fn bezier_with_linear_speed_matches_linear() {
    let mut a = Keyframe::new(s(0.0), Value::Scalar(0.0));
    let mut b = Keyframe::new(s(2.0), Value::Scalar(100.0));
    a.out_interp = Interp::Bezier;
    b.in_interp = Interp::Bezier;
    a.out_ease = vec![Ease { speed: 50.0, influence: 1.0 / 3.0 }];
    b.in_ease = vec![Ease { speed: 50.0, influence: 1.0 / 3.0 }];
    let keys = vec![a, b];
    for i in 0..=20 {
        let t = i as f64 * 0.1;
        let v = evaluate(&keys, s(t), false).unwrap().as_f64();
        assert!((v - 50.0 * t).abs() < 1e-6, "{t} {v}");
    }
}

#[test]
fn spatial_linear_constant_speed() {
    let keys = vec![Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0])), Keyframe::new(s(1.0), Value::Vec2([300.0, 400.0]))];
    let p = evaluate(&keys, s(0.5), true).unwrap().as_vec2();
    assert!((p[0] - 150.0).abs() < 0.5 && (p[1] - 200.0).abs() < 0.5, "{p:?}");
}

/// #456: along a curved spatial path with linear (constant-speed) keys the speed graph is
/// flat, not a sawtooth from a piecewise-linear arc-length table.
#[test]
fn spatial_curved_path_has_smooth_constant_speed() {
    let mut a = Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0]));
    let mut b = Keyframe::new(s(2.0), Value::Vec2([400.0, 0.0]));
    a.spatial_auto = false;
    b.spatial_auto = false;
    a.spatial_out = [50.0, -300.0, 0.0];
    b.spatial_in = [80.0, 250.0, 0.0];
    let keys = vec![a, b];
    let len = spatial_segment_length(&keys, 0);
    let speeds: Vec<f64> = (1..400).map(|i| speed(&keys, s(i as f64 * 0.005), true)).collect();
    for v in &speeds {
        assert!((v - len / 2.0).abs() < 1e-3 * len, "speed {v} vs {}", len / 2.0);
    }
}

#[test]
fn spatial_auto_bezier_passes_through_keys() {
    let keys = vec![
        Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0])),
        Keyframe::new(s(1.0), Value::Vec2([100.0, 100.0])),
        Keyframe::new(s(2.0), Value::Vec2([200.0, 0.0])),
    ];
    let p = evaluate(&keys, s(1.0), true).unwrap().as_vec2();
    assert_eq!(p, [100.0, 100.0]);
    // Curved: the midpoint of the first segment lies off the straight line.
    let m = evaluate(&keys, s(0.5), true).unwrap().as_vec2();
    assert!((m[0] - m[1]).abs() > 1.0, "{m:?}");
}

#[test]
fn straight_auto_bezier_path_is_exact() {
    // Auto-Bezier ends now have handles along the line; the chord is still hit exactly.
    let keys = vec![Keyframe::new(s(0.0), Value::Vec2([40.0, 50.0])), Keyframe::new(s(2.0), Value::Vec2([120.0, 50.0]))];
    assert_eq!(segment_value(&keys, 0, s(1.0), true), Value::Vec2([80.0, 50.0]));
}

/// #880: a motion path whose arc length overflows (huge coordinates, as the Position field
/// accepts) or is NaN evaluates without panicking: scrubbing over it crashed the app.
#[test]
fn overflowing_or_nan_motion_paths_evaluate() {
    let paths = [
        [[-1.7e308, 0.0], [1.7e308, 0.0], [-1.7e308, 5.0]],
        [[0.0, 0.0], [f64::NAN, 10.0], [20.0, 0.0]],
        [[f64::INFINITY, 0.0], [0.0, f64::NEG_INFINITY], [20.0, 0.0]],
    ];
    for path in paths {
        let mut keys: Vec<Keyframe> = path.iter().enumerate().map(|(i, &v)| Keyframe::new(s(i as f64 * 2.0), Value::Vec2(v))).collect();
        for eased in [false, true] {
            if eased {
                keys = keys.into_iter().map(Keyframe::eased).collect();
            }
            for f in 0..=40 {
                let t = s(f as f64 * 0.1);
                assert!(evaluate(&keys, t, true).is_some());
                let _ = speed(&keys, t, true);
            }
        }
    }
}

#[test]
fn spatial_auto_bezier_tangents_follow_after_effects() {
    // Measured on After Effects 26.3 with original paths (docs/fidelity/README.md): each
    // auto-Bezier key's out tangent is (next - previous) / 6 and its in tangent the mirror,
    // whatever the key times. End keys use their one neighbour: (neighbour - key) / 6.
    let p = [[40.0, 300.0, 0.0], [260.0, 80.0, 20.0], [700.0, 420.0, -40.0], [1100.0, 150.0, 0.0]];
    let mut keys: Vec<Keyframe> = [0.0, 0.2, 1.5, 1.8].iter().zip(p).map(|(&t, v)| Keyframe::new(s(t), Value::Vec3(v))).collect();
    let sixth = |a: [f64; 3], b: [f64; 3]| [(b[0] - a[0]) / 6.0, (b[1] - a[1]) / 6.0, (b[2] - a[2]) / 6.0];
    let neg = |v: [f64; 3]| [-v[0], -v[1], -v[2]];
    let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9);
    let want = [sixth(p[0], p[1]), sixth(p[0], p[2]), sixth(p[1], p[3]), sixth(p[2], p[3])];
    for (i, out) in want.into_iter().enumerate() {
        let (tin, tout) = spatial_tangents(&keys, i);
        assert!(close(tout, out) && close(tin, neg(out)), "key {i}: {tin:?} {tout:?}");
    }
    // Moving a key in time leaves every tangent as it was.
    keys[1].time = s(1.2);
    assert!(close(spatial_tangents(&keys, 1).1, sixth(p[0], p[2])));
    // A key between two coincident neighbours, and a lone key, have none.
    let still = [[5.0, 5.0], [9.0, 1.0], [5.0, 5.0]].iter().enumerate().map(|(i, &v)| Keyframe::new(s(i as f64), Value::Vec2(v))).collect::<Vec<_>>();
    assert_eq!(spatial_tangents(&still, 1), ([0.0; 3], [0.0; 3]));
    assert_eq!(spatial_tangents(&still[..1], 0), ([0.0; 3], [0.0; 3]));
}

#[test]
fn auto_spatial_holds_between_identical_keys() {
    let path = [(0.0, [0.0, 540.0]), (0.6, [400.0, 540.0]), (4.4, [400.0, 540.0]), (5.0, [0.0, 540.0])];
    for eased in [false, true] {
        let keys: Vec<Keyframe> = path
            .iter()
            .map(|&(t, v)| {
                let k = Keyframe::new(s(t), Value::Vec2(v));
                if eased { k.eased() } else { k }
            })
            .collect();
        assert_eq!(spatial_tangents(&keys, 1).1, [0.0; 3]);
        assert_eq!(spatial_tangents(&keys, 2).0, [0.0; 3]);
        for t in [0.7, 1.0, 2.5, 4.0, 4.3] {
            let p = evaluate(&keys, s(t), true).unwrap().as_vec3();
            assert!((p[0] - 400.0).abs() < 1e-6 && (p[1] - 540.0).abs() < 1e-6, "eased={eased} t={t} p={p:?}");
        }
    }
}

#[test]
fn set_key_replaces_same_time() {
    let mut keys = vec![];
    set_key(&mut keys, Keyframe::new(s(1.0), Value::Scalar(1.0)).eased());
    set_key(&mut keys, Keyframe::new(s(0.0), Value::Scalar(0.0)));
    set_key(&mut keys, Keyframe::new(s(1.0), Value::Scalar(5.0)));
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[1].value, Value::Scalar(5.0));
    assert_eq!(keys[1].in_interp, Interp::Bezier);
}

#[test]
fn colors_and_paths_interpolate() {
    let keys = vec![Keyframe::new(s(0.0), Value::Color([0.0, 0.0, 0.0, 1.0])), Keyframe::new(s(1.0), Value::Color([1.0, 0.5, 0.0, 1.0]))];
    assert_eq!(evaluate(&keys, s(0.5), false).unwrap().as_color(), [0.5, 0.25, 0.0, 1.0]);
    let a = ShapePath::rect([0.0, 0.0], 10.0, 10.0);
    let b = ShapePath::rect([0.0, 0.0], 20.0, 20.0);
    let keys = vec![Keyframe::new(s(0.0), Value::Path(a)), Keyframe::new(s(1.0), Value::Path(b))];
    let v = evaluate(&keys, s(0.5), false).unwrap();
    assert_eq!(v.as_path().unwrap().vertices[0], [-7.5, -7.5]);
}

#[test]
fn roving_keys_get_constant_speed_times() {
    let mut keys = vec![
        Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0])),
        Keyframe::new(s(0.2), Value::Vec2([300.0, 0.0])),
        Keyframe::new(s(2.0), Value::Vec2([400.0, 0.0])),
    ];
    for k in &mut keys {
        k.spatial_auto = false;
    }
    keys[1].roving = true;
    assert!(retime_roving(&mut keys, true));
    // 300 of 400 px → 75 % of the 2 s span.
    assert!((keys[1].time.seconds() - 1.5).abs() < 1e-6, "{}", keys[1].time.seconds());
    let v0 = speed(&keys, s(0.5), true);
    let v1 = speed(&keys, s(1.8), true);
    assert!((v0 - 200.0).abs() < 1.0 && (v1 - 200.0).abs() < 1.0, "{v0} {v1}");
    // First/last keys can't rove.
    keys[0].roving = true;
    retime_roving(&mut keys, true);
    assert!(!keys[0].roving);
}

#[test]
fn roving_keys_never_share_a_time_with_a_fixed_key() {
    // Zero-length last segment (issue #532) and zero-length first segment.
    for xs in [[0.0, 40.0, 120.0, 120.0], [0.0, 0.0, 40.0, 120.0]] {
        let mut keys: Vec<Keyframe> = xs.iter().enumerate().map(|(i, x)| Keyframe::new(s(i as f64), Value::Vec2([*x, 64.0]))).collect();
        for k in &mut keys {
            k.spatial_auto = false;
        }
        keys[1].roving = true;
        keys[2].roving = true;
        retime_roving(&mut keys, true);
        assert!(keys.windows(2).all(|w| w[0].time < w[1].time), "{xs:?}: {:?}", keys.iter().map(|k| k.time).collect::<Vec<_>>());
    }
}

#[test]
fn side_ease_reports_linear_and_eased_sides() {
    let keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)), Keyframe::new(s(2.0), Value::Scalar(100.0)).eased()];
    let out = side_ease(&keys, 0, 0, false, true).unwrap();
    assert!((out.speed - 50.0).abs() < 1e-9);
    let inn = side_ease(&keys, 1, 0, false, false).unwrap();
    assert_eq!(inn, Ease::EASY);
    assert!(side_ease(&keys, 0, 0, false, false).is_none());
}

#[test]
fn time_reverse_mirrors_keys() {
    let mut keys =
        vec![Keyframe::new(s(1.0), Value::Scalar(0.0)).eased(), Keyframe::new(s(2.0), Value::Scalar(5.0)), Keyframe::new(s(4.0), Value::Scalar(10.0))];
    keys[0].out_interp = Interp::Hold;
    time_reverse(&mut keys);
    assert_eq!(keys.iter().map(|k| k.time.seconds().round() as i64).collect::<Vec<_>>(), vec![1, 3, 4]);
    assert_eq!(keys[0].value, Value::Scalar(10.0));
    assert_eq!(keys[2].in_interp, Interp::Hold);
}

proptest::proptest! {
    #[test]
    fn eased_values_stay_in_range(t in 0.0f64..1.0, i0 in 0.01f64..1.0, i1 in 0.01f64..1.0) {
        let mut a = Keyframe::new(s(0.0), Value::Scalar(0.0)).eased();
        let mut b = Keyframe::new(s(1.0), Value::Scalar(10.0)).eased();
        a.out_ease = vec![Ease { speed: 0.0, influence: i0 }];
        b.in_ease = vec![Ease { speed: 0.0, influence: i1 }];
        let v = evaluate(&[a, b], s(t), false).unwrap().as_f64();
        proptest::prop_assert!((-1e-9..=10.0 + 1e-9).contains(&v));
    }
}

#[test]
fn muted_keys_are_skipped_by_evaluation() {
    let mut keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)), Keyframe::new(s(1.0), Value::Scalar(100.0)), Keyframe::new(s(2.0), Value::Scalar(300.0))];
    let at = |keys: &[Keyframe], t: f64| evaluate(keys, s(t), false).unwrap().as_f64();
    assert!((at(&keys, 0.5) - 50.0).abs() < 1e-9);
    assert!((at(&keys, 1.5) - 200.0).abs() < 1e-9);
    keys[1].muted = true;
    // The 0 → 300 segment now spans 0 → 2 s.
    assert!((at(&keys, 0.5) - 75.0).abs() < 1e-9);
    assert!((at(&keys, 1.0) - 150.0).abs() < 1e-9);
    assert!((at(&keys, 1.5) - 225.0).abs() < 1e-9);
    for k in &mut keys {
        k.muted = true;
    }
    assert!(evaluate(&keys, s(0.5), false).is_none());
}

#[test]
fn muted_key_automation_uses_the_active_neighbours() {
    let mut keys = vec![
        Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0])),
        Keyframe::new(s(1.0), Value::Vec2([50.0, 100.0])),
        Keyframe::new(s(2.0), Value::Vec2([100.0, 0.0])),
    ];
    for k in &mut keys {
        k.auto_bezier = true;
        k.in_interp = Interp::Bezier;
        k.out_interp = Interp::Bezier;
    }
    let all = keys.clone();
    let mut active = keys.clone();
    active.remove(1);
    keys[1].muted = true;
    for t in [0.0, 0.25, 0.5, 1.0, 1.5, 1.75, 2.0] {
        assert_eq!(evaluate(&keys, s(t), true), evaluate(&active, s(t), true), "{t}");
    }
    // The muted auto-Bezier key no longer bulges the motion path.
    assert_ne!(evaluate(&keys, s(0.5), true), evaluate(&all, s(0.5), true));
}

#[test]
fn mute_round_trips_and_survives_value_replacement() {
    let mut k = Keyframe::new(s(0.0), Value::Scalar(1.0));
    let json = serde_json::to_value(&k).unwrap();
    assert!(json.get("muted").is_none(), "false is the default and is skipped");
    k.muted = true;
    let json = serde_json::to_value(&k).unwrap();
    assert_eq!(json["muted"], serde_json::json!(true));
    let back: Keyframe = serde_json::from_value(json).unwrap();
    assert!(back.muted);
    let mut keys = vec![k];
    set_key(&mut keys, Keyframe::new(s(0.0), Value::Scalar(2.0)));
    assert!(keys[0].muted);
    assert_eq!(keys[0].value, Value::Scalar(2.0));
}
