// GPU particles: effects::psim systems, one invocation per particle id. Mirrors the CPU step
// (effects::sim::Phys::spawn / step, effects::sim3::Pg::spawn / step) operation for operation.
//
// Concatenated after common.wgsl and kernels.wgsl (hash_noise, value_noise, powz); bindings in
// group 1 (group 0 is unused here).

struct PSim {
    // Particles, from step, to step, system (0 engine, 1 cannon).
    u: vec4<u32>,
    // Animation, flat, seed, particles in `prev`.
    u2: vec4<u32>,
    // Engine: f[0] = (life, speed, extra, cone), f[1] = producer, f[2] = radius,
    // f[3] = gravity, f[4] = axis, f[5] = (damp, dt).
    // Cannon: f[0] = (pos.x, pos.y, barrel angle, barrel radius), f[1] = (dir spread, vel,
    // vel spread, grav spread), f[2] = (gravity.x, gravity.y, dt), f[3] = bounds.
    f: array<vec4<f32>, 6>,
}

struct PState {
    // Position, age.
    p: vec4<f32>,
    // Velocity, life.
    v: vec4<f32>,
    // rnd, alive (1 / 0), id.
    m: vec4<f32>,
}

@group(1) @binding(0) var<uniform> S: PSim;
@group(1) @binding(1) var<storage, read> births: array<u32>;
@group(1) @binding(2) var<storage, read> prev: array<PState>;
@group(1) @binding(3) var<storage, read_write> next: array<PState>;

const PS_TAU: f32 = 6.2831855;

fn ps_h(i: u32, k: u32) -> f32 {
    return hash_noise(i, k, S.u2.z);
}

fn ps_hs(i: u32, k: u32) -> f32 {
    return hash_noise(i, k, S.u2.z) * 2.0 - 1.0;
}

// sim::norm3.
fn ps_norm3(v: vec3<f32>) -> vec3<f32> {
    let l = sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
    if (l > 1e-9) {
        return v / l;
    }
    return vec3<f32>(0.0, -1.0, 0.0);
}

// sim::rand_dir.
fn ps_rand_dir(i: u32, k: u32, flat: bool) -> vec3<f32> {
    if (flat) {
        let a = ps_h(i, k) * PS_TAU;
        return vec3<f32>(cos_p(a), sin_p(a), 0.0);
    }
    let z = ps_hs(i, k);
    let a = ps_h(i, k + 1u) * PS_TAU;
    let r = sqrt(max(1.0 - z * z, 0.0));
    return vec3<f32>(r * cos_p(a), r * sin_p(a), z);
}

struct Part {
    p: vec3<f32>,
    v: vec3<f32>,
    age: f32,
    life: f32,
    rnd: f32,
}

// sim::Phys::spawn (+ the step's sub-step birth offset).
fn ps_spawn_engine(id: u32) -> Part {
    let flat = S.u2.y != 0u;
    let anim = S.u2.x;
    let producer = S.f[1].xyz;
    let radius = S.f[2].xyz;
    let u = ps_rand_dir(id, 1u, flat);
    var e = 1.0 / 3.0;
    if (flat) {
        e = 0.5;
    }
    let rr = powz(ps_h(id, 3u), e);
    var p = producer + u * radius * rr;
    if (flat) {
        p.z = 0.0;
    }
    let ax = S.f[4].xyz;
    let jitter = ps_rand_dir(id, 5u, flat);
    var dir = jitter;
    switch anim {
        case 1u, 9u: {
            dir = ps_norm3(ax + jitter * 0.08);
        }
        case 2u: {
            let k = S.f[0].w;
            dir = ps_norm3(ax + jitter * k * ps_h(id, 8u));
        }
        case 5u: {
            var z = jitter.z;
            if (flat) {
                z = 0.0;
            }
            dir = vec3<f32>(jitter.x, 0.0, z);
        }
        case 6u: {
            dir = ps_norm3(vec3<f32>(jitter.x * 0.25, -1.0, jitter.z * 0.25));
        }
        case 7u: {
            dir = ps_norm3(vec3<f32>(1.0, jitter.y * 0.05, jitter.z * 0.05));
        }
        default: {}
    }
    let sp = S.f[0].y * (0.5 + 0.5 * ps_h(id, 9u));
    var v = dir * sp;
    if (flat) {
        v.z = 0.0;
    }
    let life = S.f[0].x * (0.8 + 0.4 * ps_h(id, 10u));
    let dt = S.f[5].y;
    let frac = ps_h(id, 12u) * dt;
    return Part(p + v * frac, v, frac, life, ps_h(id, 11u));
}

// sim::Phys::step for one particle at step `j`.
fn ps_step_engine(q: Part, j: u32) -> Part {
    let flat = S.u2.y != 0u;
    let anim = S.u2.x;
    let seed = S.u2.z;
    let dt = S.f[5].y;
    let t = f32(j) * dt;
    let producer = S.f[1].xyz;
    let gravity = S.f[3].xyz;
    let extra = S.f[0].z;
    var a = gravity;
    switch anim {
        case 4u: {
            let dx = q.p.x - producer.x;
            let dz = q.p.z - producer.z;
            let k = 4.0 * extra;
            a.x += -dz * k;
            a.z += dx * k;
            if (flat) {
                let dy = q.p.y - producer.y;
                a.x += -dy * k;
                a.y += dx * k;
            }
        }
        case 5u: {
            let dx = q.p.x - producer.x;
            let dy = q.p.y - producer.y;
            let k = 6.0 * extra;
            a.x += -dy * k - dx * 2.0;
            a.y += dx * k - dy * 2.0;
        }
        case 6u: {
            let n = value_noise(q.p.x * 8.0, q.p.y * 8.0, t * 3.0, seed) - 0.5;
            a.x += n * 4.0 * extra;
            a.y -= gravity.y * 1.5 + 0.3 * extra;
        }
        case 8u, 9u: {
            let f = 6.0 * extra;
            a.x += (value_noise(q.p.x * 5.0, q.p.y * 5.0, q.p.z * 5.0 + t, seed) - 0.5) * f;
            a.y += (value_noise(q.p.x * 5.0 + 17.0, q.p.y * 5.0, q.p.z * 5.0 + t, seed) - 0.5) * f;
            if (!flat) {
                a.z += (value_noise(q.p.x * 5.0, q.p.y * 5.0 + 31.0, q.p.z * 5.0 + t, seed) - 0.5) * f;
            }
        }
        default: {}
    }
    let damp = S.f[5].x;
    var o = q;
    o.v = (q.v + a * dt) * damp;
    o.p = q.p + o.v * dt;
    if (flat) {
        o.p.z = 0.0;
        o.v.z = 0.0;
    }
    o.age = q.age + dt;
    return o;
}

// sim3::Pg::spawn (+ the sub-step birth offset).
fn ps_spawn_cannon(id: u32) -> Part {
    let pos = S.f[0].xy;
    let angle = S.f[0].z;
    let r = S.f[0].w;
    let spread = S.f[1].x;
    let vel = S.f[1].y;
    let vel_spread = S.f[1].z;
    let a = radians(angle + ps_hs(id, 1u) * spread * 0.5);
    let d = vec2<f32>(sin_p(a), -cos_p(a));
    let speed = max(vel + ps_hs(id, 3u) * vel_spread, 0.0);
    var off: vec2<f32>;
    if (r >= 0.0) {
        off = vec2<f32>(ps_hs(id, 2u) * r, ps_hs(id, 4u) * r);
    } else {
        let aa = ps_h(id, 5u) * PS_TAU;
        let rr = sqrt(ps_h(id, 6u)) * -r;
        off = vec2<f32>(cos_p(aa) * rr, sin_p(aa) * rr);
    }
    let v = d * speed;
    let dt = S.f[2].z;
    let frac = ps_h(id, 9u) * dt;
    let p = pos + off + v * frac;
    return Part(vec3<f32>(p, 0.0), vec3<f32>(v, 0.0), 0.0, 0.0, 0.0);
}

// sim3::Pg::step for one particle (gravity only); returns false when it leaves the bounds.
fn ps_step_cannon(q: ptr<function, Part>, id: u32) -> bool {
    let dt = S.f[2].z;
    let spread = S.f[1].w;
    var g = 1.0;
    if (spread > 0.0) {
        g = 1.0 + (ps_h(id, 7u) * 2.0 - 1.0) * spread;
    }
    let a = S.f[2].xy * g;
    let v = ((*q).v.xy + a * dt) * 1.0;
    let p = (*q).p.xy + v * dt;
    (*q).v = vec3<f32>(v, 0.0);
    (*q).p = vec3<f32>(p, 0.0);
    let b = S.f[3];
    return p.x > b.x && p.x < b.z && p.y > b.y && p.y < b.w;
}

@compute @workgroup_size(64, 1)
fn particles(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = gid.x;
    if (id >= S.u.x) {
        return;
    }
    let s0 = S.u.y;
    let s1 = S.u.z;
    let cannon = S.u.w == 1u;
    let b = births[id];
    var q: Part;
    var j: u32;
    if (b < s0 && id < S.u2.w) {
        let o = prev[id];
        if (o.m.y == 0.0) {
            next[id] = o;
            return;
        }
        q = Part(o.p.xyz, o.v.xyz, o.p.w, o.v.w, o.m.x);
        j = s0;
    } else {
        if (cannon) {
            q = ps_spawn_cannon(id);
        } else {
            q = ps_spawn_engine(id);
        }
        j = b + 1u;
    }
    var alive = true;
    for (; j < s1; j++) {
        if (cannon) {
            if (!ps_step_cannon(&q, id)) {
                alive = false;
                break;
            }
        } else {
            q = ps_step_engine(q, j);
            if (q.age >= q.life) {
                alive = false;
                break;
            }
        }
    }
    next[id] = PState(vec4<f32>(q.p, q.age), vec4<f32>(q.v, q.life), vec4<f32>(q.rnd, select(0.0, 1.0, alive), 0.0, 0.0));
}
