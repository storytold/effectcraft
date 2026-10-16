//! AE Color keyframe speed is a positive magnitude in 8-bit RGBA vector units per second.
//! Native keyframes store signed speed per normalized (0–1) channel.

use effectcraft_keyframe::{Ease, Keyframe, Value};

pub(crate) fn normalize(keys: &mut [Keyframe]) {
    for i in 0..keys.len().saturating_sub(1) {
        let (Value::Color(a), Value::Color(b)) = (&keys[i].value, &keys[i + 1].value) else { continue };
        let delta: [f64; 4] = std::array::from_fn(|d| b[d] - a[d]);
        let len = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
        let (before, after) = keys.split_at_mut(i + 1);
        let (Some(left), Some(right)) = (before.last_mut(), after.first_mut()) else { continue };
        for eases in [&mut left.out_ease, &mut right.in_ease] {
            if let [ease] = eases.as_slice() {
                let ease = *ease;
                *eases =
                    delta.iter().map(|d| Ease { speed: if len > 1e-12 { ease.speed * d / (255.0 * len) } else { 0.0 }, influence: ease.influence }).collect();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_keyframe::{Interp, evaluate};
    use effectcraft_time::Tick;

    #[test]
    fn ui_claude_fill_matches_ae_midpoint() {
        let s = Tick::from_seconds_f64;
        let mut a = Keyframe::new(s(29.16), Value::Color([1.0; 4]));
        let mut b = Keyframe::new(s(29.48), Value::Color([0.0, 0.0, 0.0, 1.0]));
        a.out_interp = Interp::Bezier;
        b.in_interp = Interp::Bezier;
        a.out_ease = vec![Ease { speed: 0.0, influence: 1.0 / 3.0 }];
        b.in_ease = vec![Ease { speed: 1380.2279514757106, influence: 1.0 }];
        let mut keys = [a, b];
        normalize(&mut keys);
        let Value::Color(mid) = evaluate(&keys, s(29.32), false).unwrap() else { panic!("expected Color") };
        for channel in &mid[..3] {
            assert!((channel - 0.54017830013534).abs() < 1e-6, "{mid:?}");
        }
        assert!((mid[3] - 1.0).abs() < 1e-9);
    }
}
