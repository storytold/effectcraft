//! Workload measurements are opt-in: no hardware-dependent timing assertion in ordinary CI.
#[path = "support/audio_workload.rs"]
mod workload;
use effectcraft_expr::Expressions;
use effectcraft_render::audio::mix_comp;
use effectcraft_time::Tick;
use std::{sync::atomic::Ordering, time::Instant};

/// Exercises medium/large projects using the production JavaScript evaluator, streaming blocks.
/// EC_AUDIO_CASE selects a case; EC_AUDIO_SECONDS and EC_AUDIO_RATE override duration/rate.
#[test]
#[ignore = "opt-in audio workload benchmark"]
fn audio_workload_matrix() {
    let cases = [
        ("keyframed", 8, 0, None, 4.0),
        ("typical", 4, 0, Some("0.25 + time * 0.5"), 4.0),
        ("heavy", 24, 0, Some("0.25 + time * time * 0.01"), 2.0),
        ("nested", 8, 2, Some("0.25 + time * 0.5"), 1.0),
        ("nonlinear", 4, 0, Some("0.25 + Math.sin(time) * Math.sin(time)"), 2.0),
        ("alternating", 1, 0, Some("Math.floor(time * 48000) % 2 ? 10 : 0"), 128.0 / 48000.0),
        ("wandering", 1, 0, Some("time * 100000"), 128.0 / 48000.0),
        ("long", 8, 0, Some("0.25 + time * 0.5"), 180.0),
    ];
    let selected = std::env::var("EC_AUDIO_CASE").unwrap_or_default();
    let rate = std::env::var("EC_AUDIO_RATE").ok().and_then(|s| s.parse().ok()).unwrap_or(48000u32);
    for (name, layers, depth, expression, seconds) in cases {
        if (!selected.is_empty() && selected != name) || (selected.is_empty() && name == "long") {
            continue;
        }
        let seconds = std::env::var("EC_AUDIO_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(seconds);
        let (p, cid) = workload::project(layers, depth, expression);
        let source = workload::Source::default();
        let host = Expressions;
        let total = (seconds * rate as f64).round() as usize;
        let mut hash = 0xcbf29ce484222325u64;
        let mut worst = 0.0_f64;
        let started = Instant::now();
        let mut failure = None;
        for first in (0..total).step_by(1024) {
            let t = Instant::now();
            let samples = mix_comp(&p, &source, Some(&host), cid, Tick::from_units(first as i64, rate as i64), 1024.min(total - first), rate);
            worst = worst.max(t.elapsed().as_secs_f64());
            let samples = match samples {
                Ok(s) => s,
                Err(e) => {
                    failure = Some(e.to_string());
                    break;
                }
            };
            for v in samples {
                assert!(v.is_finite());
                hash = (hash ^ v.to_bits() as u64).wrapping_mul(0x100000001b3);
            }
        }
        let elapsed = started.elapsed().as_secs_f64();
        if name != "wandering" {
            assert!(failure.is_none(), "{name}: {failure:?}");
        }
        println!(
            "{}",
            serde_json::json!({"error":failure,"case":name,"layers":layers,"depth":depth,"rate":rate,"seconds":seconds,"wall_seconds":elapsed,"realtime_factor":seconds/elapsed,"worst_block_ms":worst*1000.0,"reads":source.reads.load(Ordering::Relaxed),"source_frames":source.frames.load(Ordering::Relaxed),"hash":format!("{hash:016x}")})
        );
    }
}
