//! Deterministic rendering (better than After Effects): the same project renders bit-identical
//! frames every time and whatever the number of render threads. Renders the demo comps (and a
//! comp of seeded-random effects: particles, grain, noise, wiggle expressions, Shatter) twice
//! on one thread and on several, and compares every float of every pixel.

use effectcraft_engine::Session;
use effectcraft_engine::project::{ItemId, Project};
use effectcraft_engine::render::{RenderOpts, Renderer};
use effectcraft_engine::time::Tick;
use serde_json::json;

fn render(s: &Session, p: &Project, comp: ItemId, t: f64) -> Vec<u32> {
    let mut r = Renderer::new(p, s.footage.as_ref(), RenderOpts { scale: 0.25, ..Default::default() });
    r.expr = s.expr.as_deref();
    // No layer cache: every render computes everything.
    let img = r.comp_frame(comp, Tick::from_seconds_f64(t));
    img.data.iter().flat_map(|px| px.iter().map(|c| c.to_bits())).collect()
}

fn pool(n: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(n).build().expect("thread pool")
}

/// A comp of effects that draw random numbers (all seeded per instance).
fn random_comp(s: &mut Session) -> ItemId {
    s.execute("comp.new", json!({"name": "Random", "width": 480, "height": 270, "frameRate": 24, "duration": 4})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let solid = |s: &mut Session, name: &str, color: &str| {
        s.execute("layer.newSolid", json!({"name": name, "color": color, "width": 480, "height": 270})).unwrap()["layer"].as_u64().unwrap()
    };
    let a = solid(s, "Particles", "#000000");
    for fx in ["ec.sim.ccparticleworld", "ec.noise.addgrain"] {
        s.execute("effect.apply", json!({"layer": a, "effect": fx})).unwrap();
    }
    let b = solid(s, "Noise", "#336699");
    for fx in ["ec.noise.fractal", "ec.sim.ccsnowfall", "ec.noise.noisehls"] {
        s.execute("effect.apply", json!({"layer": b, "effect": fx})).unwrap();
    }
    s.execute("prop.setExpression", json!({"layer": b, "path": "transform/position", "expression": "wiggle(3, 40)"})).unwrap();
    s.execute("prop.setExpression", json!({"layer": b, "path": "transform/opacity", "expression": "random(40, 90)"})).unwrap();
    let c = solid(s, "Shatter", "#cc8844");
    s.execute("layer.setTransform", json!({"layer": c, "prop": "scale", "value": [50, 50]})).unwrap();
    s.execute("effect.apply", json!({"layer": c, "effect": "ec.sim.shatter"})).unwrap();
    cid
}

#[test]
fn renders_are_bit_identical_across_runs_and_thread_counts() {
    let mut s = effectcraft_host::session();
    s.replace_project(effectcraft_engine::demo::demo_project(), None);
    let random = random_comp(&mut s);
    let p = (*s.project).clone();
    let mut comps: Vec<(ItemId, f64)> = p.comps().map(|(id, c)| (*id, c.duration.seconds())).collect();
    comps.sort_by_key(|c| c.0);
    assert!(comps.len() >= 3, "demo comps + the random comp");
    assert!(comps.iter().any(|c| c.0 == random));
    let (one, many) = (pool(1), pool(6));
    let mut frames = 0;
    for (comp, dur) in comps {
        for t in [0.0, dur * 0.37, dur * 0.81] {
            let a = one.install(|| render(&s, &p, comp, t));
            let b = many.install(|| render(&s, &p, comp, t));
            let c = many.install(|| render(&s, &p, comp, t));
            let d = one.install(|| render(&s, &p, comp, t));
            let name = &p.item(comp).unwrap().name;
            assert!(!a.is_empty());
            assert!(a == b, "{name} @ {t:.2}s: 1 thread vs 6 threads differ");
            assert!(b == c, "{name} @ {t:.2}s: two 6-thread renders differ");
            assert!(a == d, "{name} @ {t:.2}s: two 1-thread renders differ");
            frames += 1;
        }
    }
    assert!(frames >= 9);
}
