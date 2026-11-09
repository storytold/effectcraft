use super::*;
use crate::{EffectEnv, MaskShape, run_fx};

fn run(vals: &[(&str, Value)], t: f64) -> Image {
    run_fx("ec.sim.particleplayground", vals, Image::filled(80, 60, [0.2, 0.2, 0.2, 1.0]), t, EffectEnv::default()).img
}

fn cannon(t: f64) -> Image {
    run(&[("cannon/cannonPosition", Value::Vec2([40.0, 50.0])), ("cannon/velocity", num(60.0)), ("gravity/gravityForce", num(30.0))], t)
}

#[test]
fn cannon_stream_is_seek_consistent() {
    let a = cannon(1.0);
    // Particles replace the layer: some coverage, mostly transparent, red.
    let cov: f32 = a.data.iter().map(|p| p[3]).sum();
    assert!(cov > 5.0, "{cov}");
    assert!(a.data.iter().any(|p| p[3] > 0.5 && p[0] > 0.5 && p[1] < 0.1));
    // Seek back and forth: the same frame every time.
    let _ = cannon(2.0);
    let b = cannon(1.0);
    assert_eq!(a.data, b.data);
    let c = cannon(0.5);
    assert_ne!(a.data, c.data);
    // Nothing has been fired at time 0.
    let z = cannon(0.0);
    assert!(z.data.iter().all(|p| p[3] == 0.0));
}

#[test]
fn grid_falls_with_gravity_and_bounces_off_a_wall() {
    let grid = |t: f64, env: EffectEnv| {
        run_fx(
            "ec.sim.particleplayground",
            &[
                ("cannonEnabled", Value::Bool(false)),
                ("grid/gridPosition", Value::Vec2([40.0, 10.0])),
                ("grid/gridWidth", num(40.0)),
                ("grid/gridHeight", num(0.0)),
                ("grid/particlesAcross", num(5.0)),
                ("grid/particlesDown", num(1.0)),
                ("gravity/gravityForce", num(100.0)),
                ("wall/wallBoundary", num(1.0)),
            ],
            Image::new(80, 60),
            t,
            env,
        )
        .img
    };
    // Alpha-weighted mean height of the particles.
    let row_of = |img: &Image| {
        let (mut s, mut n) = (0.0f64, 0.0f64);
        for y in 0..60 {
            for x in 0..80 {
                let a = img.get(x, y)[3] as f64;
                s += (y as f64 + 0.5) * a;
                n += a;
            }
        }
        (s / n.max(1e-9)).round() as i64
    };
    let t0 = grid(0.0, EffectEnv::default());
    let t1 = grid(0.5, EffectEnv::default());
    assert_eq!(row_of(&t0), 10);
    // ½ g t² = 12.5 px.
    assert!((row_of(&t1) - 22).abs() <= 2, "{}", row_of(&t1));
    // A wall (mask 1) whose floor is at y = 15 keeps them above it.
    let masks = [MaskShape { name: "w".into(), points: vec![[0.0, 0.0], [80.0, 0.0], [80.0, 15.0], [0.0, 15.0]], closed: true, inverted: false }];
    let env = EffectEnv { masks: &masks, ..Default::default() };
    let w = grid(1.5, env);
    assert!(row_of(&w) <= 16, "{}", row_of(&w));
}

#[test]
fn repel_spreads_particles() {
    let spread = |force: f64| {
        let img = run(
            &[
                ("cannonEnabled", Value::Bool(false)),
                ("grid/gridPosition", Value::Vec2([40.0, 30.0])),
                ("grid/gridWidth", num(6.0)),
                ("grid/gridHeight", num(6.0)),
                ("grid/particlesAcross", num(3.0)),
                ("grid/particlesDown", num(3.0)),
                ("gravity/gravityForce", num(0.0)),
                ("repel/repelForce", num(force)),
                ("repel/repelForceRadius", num(20.0)),
            ],
            0.5,
        );
        let (mut x0, mut x1) = (80, 0);
        for y in 0..60 {
            for x in 0..80 {
                if img.get(x, y)[3] > 0.3 {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                }
            }
        }
        x1 - x0
    };
    assert!(spread(0.2) > spread(0.0) + 4, "{} vs {}", spread(0.2), spread(0.0));
}

#[test]
fn property_mapper_and_layer_map_read_layers() {
    // Without a host, the layer-based features are inert (deterministic output).
    let a = run(
        &[
            ("persistentPropertyMapper/mapRedTo", Value::Enum(8)),
            ("persistentPropertyMapper/redMin", num(-50.0)),
            ("persistentPropertyMapper/redMax", num(50.0)),
        ],
        0.7,
    );
    let b = run(
        &[
            ("persistentPropertyMapper/mapRedTo", Value::Enum(8)),
            ("persistentPropertyMapper/redMin", num(-50.0)),
            ("persistentPropertyMapper/redMax", num(50.0)),
        ],
        0.7,
    );
    assert_eq!(a.data, b.data);
    let st = Pg { persistent: Some(mapper(Image::filled(4, 4, [1.0, 0.0, 0.5, 1.0]), [(15, 0, 0.0, 120.0), (0, 0, 0.0, 1.0), (9, 0, 0.0, 2.0)])), ..bare(1) };
    let mut s = PgState::default();
    s.parts.push(PgParticle::new([1.0, 1.0], [0.0; 2], [1.0; 4], 2.0, 0, SRC_GRID));
    st.step(&mut s);
    // Red 1 → X speed 120 px/s; blue 0.5 → scale 1.
    assert!((s.parts[0].v[0] - 120.0).abs() < 1e-3);
    assert!((s.parts[0].scale[0] - 1.0).abs() < 1e-3);
}

/// A Pg with nothing switched on.
fn bare(seed: u32) -> Pg {
    Pg {
        cannon: false,
        pos: [0.0; 2],
        barrel_angle: 0.0,
        barrel_radius: 0.0,
        rate: 0.0,
        dir_spread: 0.0,
        vel: 0.0,
        vel_spread: 0.0,
        color: [1.0; 4],
        radius: 2.0,
        cannon_text: vec![],
        pex_radius: 0.0,
        pex_dispersion: 0.0,
        pex_affects: Affects::default(),
        gravity: [0.0; 2],
        grav_spread: 0.0,
        grav_affects: Affects::default(),
        repel: 0.0,
        repel_radius: 0.0,
        repel_affects: Affects::default(),
        wall: None,
        wall_affects: Affects::default(),
        persistent: None,
        ephemeral: None,
        seed,
        bounds: [-1e6, -1e6, 1e6, 1e6],
    }
}

fn mapper(img: Image, chans: [(u32, u32, f32, f32); 3]) -> Mapper {
    Mapper { map: MapImg { img, k: [1.0; 2], o: [0.0; 2] }, chans, affects: Affects::default() }
}

#[test]
fn barrel_radius_square_or_disc() {
    let pg = |r: f32| Pg { cannon: true, barrel_radius: r, ..bare(3) };
    let (sq, disc) = (pg(10.0), pg(-10.0));
    let mut corner = false;
    for id in 0..400 {
        let a = sq.spawn(id).p;
        assert!(a[0].abs() <= 10.0 && a[1].abs() <= 10.0);
        // A square barrel fills its corners (both offsets in use, not a line).
        corner |= a[0].abs() > 7.5 && a[1].abs() > 7.5;
        let b = disc.spawn(id).p;
        assert!(b[0].hypot(b[1]) <= 10.0 + 1e-4);
    }
    assert!(corner);
    // Radius 0: every particle starts at the cannon position.
    assert_eq!(pg(0.0).spawn(5).p, [0.0, 0.0]);
}

#[test]
fn mapper_drives_position_and_opacity_in_ae_order() {
    let st = Pg { persistent: Some(mapper(Image::filled(4, 4, [1.0, 0.5, 0.25, 1.0]), [(12, 0, 0.0, 30.0), (20, 0, 0.0, 1.0), (4, 0, 0.0, 1.0)])), ..bare(1) };
    assert_eq!(MAP_TARGETS[12], "X");
    assert_eq!(MAP_TARGETS[20], "Opacity");
    assert_eq!(MAP_TARGETS[26], "Scale Speed");
    let mut s = PgState::default();
    s.parts.push(PgParticle::new([1.0, 1.0], [0.0; 2], [1.0; 4], 2.0, 0, SRC_GRID));
    st.step(&mut s);
    assert!((s.parts[0].p[0] - 30.0).abs() < 1e-3, "{:?}", s.parts[0].p);
    assert!((s.parts[0].c[3] - 0.5).abs() < 1e-3);
    assert!((s.parts[0].friction - 0.25).abs() < 1e-3);
}

#[test]
fn ephemeral_mapper_holds_for_one_step_with_operators() {
    // Ephemeral X Speed = 60 px/s (Set): the particle moves, but keeps no velocity.
    let st = Pg { ephemeral: Some(mapper(Image::filled(4, 4, [1.0, 1.0, 1.0, 1.0]), [(15, 0, 0.0, 60.0), (0, 0, 0.0, 1.0), (0, 0, 0.0, 1.0)])), ..bare(1) };
    let mut s = PgState::default();
    s.parts.push(PgParticle::new([1.0, 1.0], [0.0; 2], [1.0; 4], 2.0, 0, SRC_GRID));
    st.step(&mut s);
    assert!((s.parts[0].p[0] - 2.0).abs() < 1e-4, "{:?}", s.parts[0].p);
    assert!(s.parts[0].v[0].abs() < 1e-4);
    // Operators: Add / Multiply / Max on the particle's own value.
    let op = |o: u32, cur: f32| {
        let m = mapper(Image::filled(4, 4, [1.0, 1.0, 1.0, 1.0]), [(21, o, 0.0, 3.0), (0, 0, 0.0, 1.0), (0, 0, 0.0, 1.0)]);
        let mut q = PgParticle::new([1.0, 1.0], [0.0; 2], [1.0; 4], 2.0, 0, SRC_GRID);
        q.mass = cur;
        m.apply(&mut q);
        q.mass
    };
    assert_eq!(MAP_OPERATORS[1], "Add");
    assert!((op(1, 2.0) - 5.0).abs() < 1e-5);
    assert!((op(4, 2.0) - 6.0).abs() < 1e-5);
    assert!((op(6, 5.0) - 5.0).abs() < 1e-5);
    assert!((op(2, 5.0) - 2.0).abs() < 1e-5, "Difference");
}

#[test]
fn affects_select_by_source_age_and_characters() {
    let mut q = PgParticle::new([0.0; 2], [0.0; 2], [1.0; 4], 2.0, 0, SRC_CANNON);
    let a = Affects { from: 2, ..Default::default() };
    assert_eq!(a.weight(&q), 0.0, "grid only");
    q.src = SRC_GRID;
    assert_eq!(a.weight(&q), 1.0);
    let older = Affects { age: 1.0, ..Default::default() };
    q.age = 0.5;
    assert_eq!(older.weight(&q), 0.0);
    q.age = 1.5;
    assert_eq!(older.weight(&q), 1.0);
    let younger = Affects { age: -1.0, feather: 1.0, ..Default::default() };
    q.age = 1.0;
    assert!((younger.weight(&q) - 0.5).abs() < 1e-5, "feathered");
    let chars = Affects { chars: vec!['A', 'B'], ..Default::default() };
    q.ch = 'B' as u32;
    assert_eq!(chars.weight(&q), 1.0);
    q.ch = 'C' as u32;
    assert_eq!(chars.weight(&q), 0.0);
    // Gravity restricted to the cannon leaves grid particles in place.
    let st = Pg { gravity: [0.0, 100.0], grav_affects: Affects { from: 1, ..Default::default() }, ..bare(1) };
    let mut s = PgState::default();
    s.parts.push(PgParticle::new([5.0, 5.0], [0.0; 2], [1.0; 4], 2.0, 0, SRC_GRID));
    s.parts.push(PgParticle::new([5.0, 5.0], [0.0; 2], [1.0; 4], 2.0, 1, SRC_CANNON));
    for _ in 0..10 {
        st.step(&mut s);
    }
    assert_eq!(s.parts[0].p, [5.0, 5.0]);
    assert!(s.parts[1].p[1] > 5.0);
}

#[test]
fn particle_exploder_bursts_big_particles() {
    let st = Pg { pex_radius: 2.0, pex_dispersion: 30.0, ..bare(1) };
    let mut s = PgState::default();
    s.parts.push(PgParticle::new([50.0, 50.0], [0.0; 2], [1.0; 4], 6.0, 0, SRC_GRID));
    s.next_id = 1;
    st.step(&mut s);
    assert_eq!(s.parts.len(), 9, "(6/2)² new particles");
    assert!(s.parts.iter().all(|q| q.r == 2.0 && q.src == SRC_PARTICLE_EXPLODER));
    st.step(&mut s);
    assert_eq!(s.parts.len(), 9, "new particles do not burst again");
    // Lifespan from a mapper removes particles once they are older.
    let life = Pg { persistent: Some(mapper(Image::filled(4, 4, [1.0; 4]), [(22, 0, 0.0, 0.05), (0, 0, 0.0, 1.0), (0, 0, 0.0, 1.0)])), ..bare(1) };
    let mut s = PgState::default();
    s.parts.push(PgParticle::new([1.0, 1.0], [0.0; 2], [1.0; 4], 2.0, 0, SRC_GRID));
    for _ in 0..2 {
        life.step(&mut s);
    }
    assert_eq!(s.parts.len(), 1);
    for _ in 0..3 {
        life.step(&mut s);
    }
    assert!(s.parts.is_empty());
}

#[test]
fn text_particles_draw_characters() {
    let grid = |text: &str| {
        run(
            &[
                ("cannonEnabled", Value::Bool(false)),
                ("grid/gridPosition", Value::Vec2([40.0, 30.0])),
                ("grid/gridWidth", num(40.0)),
                ("grid/gridHeight", num(0.0)),
                ("grid/particlesAcross", num(3.0)),
                ("grid/particlesDown", num(1.0)),
                ("grid/gridParticleRadius", num(16.0)),
                ("gravity/gravityForce", num(0.0)),
                ("options/gridText", Value::Str(text.into())),
            ],
            0.1,
        )
    };
    let dots = grid("");
    let text = grid("I-O");
    assert_ne!(dots.data, text.data);
    // "I" is a vertical bar: ink above and below the grid point, not a round dot.
    assert!(text.get(20, 26)[3] > 0.3 && text.get(20, 34)[3] > 0.3, "{:?}", text.get(20, 26));
    // A space leaves its grid point empty.
    let gap = grid("I O");
    assert!(gap.get(40, 30)[3] < 0.05);
    // Cannon text: particles carry the characters in order.
    let pg = Pg { cannon_text: vec!['A', 'B'], ..bare(1) };
    assert_eq!(pg.spawn(0).ch, 'A' as u32);
    assert_eq!(pg.spawn(3).ch, 'B' as u32);
}

#[test]
fn layer_map_shows_the_map_layer_at_its_time_offset() {
    /// A 6×6 map layer whose colour encodes the comp time it was fetched at.
    struct Clock;
    impl crate::EffectHost for Clock {
        fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn layer_at(&self, _: u64, t: f64, _: bool) -> Option<crate::LayerPixels> {
            let v = (t as f32 / 4.0).clamp(0.0, 1.0);
            Some(crate::LayerPixels { buf: Buf { img: Image::filled(6, 6, [v, 0.0, 1.0 - v, 1.0]), offset: [0.0; 2], scale: 1.0 }, size: [6.0, 6.0] })
        }
    }
    let render = |kind: u32, offset: f64| {
        let vals = [
            ("cannonEnabled", Value::Bool(false)),
            ("grid/gridPosition", Value::Vec2([40.0, 30.0])),
            ("grid/particlesAcross", num(1.0)),
            ("grid/particlesDown", num(1.0)),
            ("gravity/gravityForce", num(0.0)),
            ("layerMap/layerMapLayer", Value::Layer(Some(9))),
            ("layerMap/timeOffsetType", Value::Enum(kind)),
            ("layerMap/timeOffset", num(offset)),
        ];
        let env = EffectEnv { host: Some(&Clock), comp_time: 1.0, frame_rate: 10.0, ..Default::default() };
        run_fx("ec.sim.particleplayground", &vals, Image::filled(80, 60, [0.2, 0.2, 0.2, 1.0]), 1.0, env).img
    };
    // The particle is a 6×6 copy of the map layer.
    let rel = render(0, 1.0);
    assert!(rel.get(40, 30)[3] > 0.99 && rel.get(46, 30)[3] < 0.01);
    assert!((rel.get(40, 30)[0] - 0.5).abs() < 1e-3, "relative: now + 1 s = 2 s");
    let abs = render(1, 3.0);
    assert!((abs.get(40, 30)[0] - 0.75).abs() < 1e-3, "absolute: 3 s");
}

#[test]
fn cc_hair_grows_from_opaque_pixels() {
    let mut img = Image::new(60, 60);
    for y in 20..40 {
        for x in 20..40 {
            img.set(x, y, [0.8, 0.6, 0.4, 1.0]);
        }
    }
    let out = run_fx("ec.sim.cchair", &[("length", num(12.0)), ("density", num(300.0))], img.clone(), 0.0, EffectEnv::default());
    // Hair reaches beyond the square (above it: hair leans up, then droops).
    let outside: f32 = (0..60).flat_map(|x| (0..20).map(move |y| (x, y))).map(|(x, y)| out.img.get(x, y)[3]).sum();
    assert!(outside > 1.0, "{outside}");
    // Nothing grows from transparent areas far away.
    assert_eq!(out.img.get(2, 58)[3], 0.0);
    let again = run_fx("ec.sim.cchair", &[("length", num(12.0)), ("density", num(300.0))], img.clone(), 3.0, EffectEnv::default());
    assert_eq!(out.img.data, again.img.data);
    let none = run_fx("ec.sim.cchair", &[("length", num(0.0))], img.clone(), 0.0, EffectEnv::default());
    assert_eq!(none.img.data, img.data);
}
