use super::*;

fn t(seconds: f64) -> Tick {
    Tick::from_seconds_f64(seconds)
}
fn scalar(keys: &[Keyframe], seconds: f64) -> f64 {
    evaluate(keys, t(seconds), false).unwrap().components()[0]
}

#[test]
fn ae_behavior_auto_bezier_uses_endpoint_slopes() {
    let mut keys: Vec<_> = [(0., 0.), (1., 30.), (2., 100.)].into_iter().map(|(time, value)| Keyframe::new(t(time), Value::Scalar(value)).eased()).collect();
    for k in &mut keys {
        k.auto_bezier = true;
    }
    assert_eq!(side_ease(&keys, 0, 0, false, true).unwrap(), Ease { speed: 30., influence: 1. / 6. });
    assert_eq!(side_ease(&keys, 2, 0, false, false).unwrap(), Ease { speed: 70., influence: 1. / 6. });
    assert!((scalar(&keys, 0.45) - 12.3575065960286).abs() < 0.002);
    assert!((scalar(&keys, 1.55) - 67.3575065960287).abs() < 0.002);
}

#[test]
fn ae_behavior_linear_side_of_mixed_segment_uses_one_sixth_influence() {
    let mut keys = vec![Keyframe::new(t(1.), Value::Scalar(0.)).eased(), Keyframe::new(t(3.), Value::Scalar(90.)).eased()];
    keys[0].out_interp = Interp::Linear;
    assert!((scalar(&keys, 2.) - 57.335700415487).abs() < 0.002);
}

#[test]
fn ae_behavior_inserting_numeric_key_preserves_curve_and_derivative() {
    let original = vec![Keyframe::new(t(0.), Value::Scalar(0.)).eased(), Keyframe::new(t(2.), Value::Scalar(100.)).eased()];
    for at in [0.3, 0.7, 1., 1.4] {
        let mut keys = original.clone();
        let value = scalar(&original, at);
        insert_key_preserving_ease(&mut keys, Keyframe::new(t(at), Value::Scalar(value)), false);
        for i in 0..=80 {
            let time = i as f64 / 40.;
            assert!((scalar(&keys, time) - scalar(&original, time)).abs() < 1e-8, "{at} {time}");
        }
    }
    let mut keys = original;
    insert_key_preserving_ease(&mut keys, Keyframe::new(t(1.), Value::Scalar(30.)), false);
    assert!((keys[1].in_ease[0].speed - 75.).abs() < 1e-8);
    assert!((scalar(&keys, 0.45) - 4.40437504331848).abs() < 0.002);
    assert!((scalar(&keys, 1.55) - 78.5856248435808).abs() < 0.002);
}

#[test]
fn ae_behavior_roving_preserves_endpoint_ease_across_the_whole_path() {
    let mut keys: Vec<_> = [(0., [0., 0.]), (0.4, [40., 100.]), (1.6, [180., 20.]), (3., [200., 180.])]
        .into_iter()
        .map(|(time, point)| {
            let mut k = Keyframe::new(t(time), Value::Vec2(point)).eased();
            k.spatial_auto = true;
            k.in_ease = vec![Ease { speed: 0., influence: 0.7 }];
            k.out_ease = k.in_ease.clone();
            k
        })
        .collect();
    keys[1].roving = true;
    keys[2].roving = true;
    retime_roving(&mut keys, true);
    assert!((keys[1].time.seconds() - 1.23776041666667).abs() < 0.001);
    assert!((keys[2].time.seconds() - 1.6203125).abs() < 0.001);
    assert_eq!(keys[1].out_interp, Interp::Bezier);
    for (at, expected) in
        [(0.75, [5.63379752962485, 26.7381308049077]), (1.5, [129.985235680704, 41.7494238406815]), (2.25, [199.200988445679, 152.668960987173])]
    {
        let actual = evaluate(&keys, t(at), true).unwrap().components();
        for (x, y) in actual.iter().zip(expected) {
            assert!((x - y).abs() < 0.05, "{at}: {actual:?} != {expected:?}");
        }
    }
    let times: Vec<_> = keys.iter().map(|k| k.time).collect();
    retime_roving(&mut keys, true);
    assert_eq!(times, keys.iter().map(|k| k.time).collect::<Vec<_>>());
}

#[test]
fn ae_behavior_auto_roving_is_constant_speed_and_retiming_is_stable() {
    let mut keys: Vec<_> = [(0., [0., 0.]), (0.4, [40., 100.]), (1.6, [180., 20.]), (3., [200., 180.])]
        .into_iter()
        .map(|(time, point)| {
            let mut k = Keyframe::new(t(time), Value::Vec2(point)).eased();
            k.spatial_auto = true;
            k.auto_bezier = true;
            k
        })
        .collect();
    keys[1].roving = true;
    keys[2].roving = true;
    retime_roving(&mut keys, true);
    let times: Vec<_> = keys.iter().map(|k| k.time).collect();
    for _ in 0..5 {
        retime_roving(&mut keys, true);
        assert_eq!(times, keys.iter().map(|k| k.time).collect::<Vec<_>>());
    }
    assert!((keys[1].time.seconds() - 0.76380208333333).abs() < 0.001);
    assert!((keys[2].time.seconds() - 1.883203125).abs() < 0.001);
    for k in 0..3 {
        assert!((side_ease(&keys, k, 0, true, true).unwrap().speed - 147.382655609551).abs() < 0.01);
    }
    let mut linear = keys.clone();
    for k in &mut linear {
        k.auto_bezier = false;
        k.in_interp = Interp::Linear;
        k.out_interp = Interp::Linear;
    }
    for i in 0..61 {
        assert_eq!(evaluate(&keys, t(i as f64 / 20.), true), evaluate(&linear, t(i as f64 / 20.), true));
    }
}

#[test]
fn ae_behavior_roving_invalid_geometry_preserves_key_times() {
    let mut keys =
        vec![Keyframe::new(t(0.), Value::Vec2([0., 0.])), Keyframe::new(t(0.4), Value::Vec2([1e308, 1e308])), Keyframe::new(t(2.), Value::Vec2([0., 0.]))];
    keys[1].roving = true;
    let before: Vec<_> = keys.iter().map(|k| k.time).collect();
    assert!(!retime_roving(&mut keys, true));
    assert_eq!(before, keys.iter().map(|k| k.time).collect::<Vec<_>>());
}
