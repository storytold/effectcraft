// ---------------------------------------------------------------- 3D Channel family (fx_depth.rs)
//
// `aux` planes are buffer-sized resamplings of the layer's auxiliary channels (fxd_lookup); the
// alpha of a plane is 1 inside the aux image and 0 outside (where the effect's fill applies).

fn fxd_unpremul(p: vec4<f32>) -> vec4<f32> {
    if (p.w > 1e-6) {
        return vec4<f32>(p.xyz / p.w, p.w);
    }
    return vec4<f32>(0.0, 0.0, 0.0, max(p.w, 0.0));
}

// f32::signum (+0 → 1).
fn fxd_signum(v: f32) -> f32 {
    return select(-1.0, 1.0, v >= 0.0);
}

// Nearest aux texel under each buffer pixel (AuxChannels::index_at), `fill` outside.
// f[0] = (offset x, offset y, 1 / buffer scale, aux scale); f[1].xy = sub-pixel position; f[2] = fill.
@compute @workgroup_size(16, 16)
fn fxd_lookup(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let lx = (f32(p.x) + P.f[1].x - P.f[0].x) * P.f[0].z;
    let ly = (f32(p.y) + P.f[1].y - P.f[0].y) * P.f[0].z;
    let ax = floor(lx * P.f[0].w);
    let ay = floor(ly * P.f[0].w);
    let ad = vec2<f32>(textureDimensions(src));
    var v = P.f[2];
    if (ax >= 0.0 && ay >= 0.0 && ax < ad.x && ay < ad.y) {
        v = textureLoad(src, vec2<i32>(i32(ax), i32(ay)), 0);
    }
    textureStore(out, p, v);
}

// 3D Channel Extract's black / white point mapping.
fn fxd_lv(v: f32) -> f32 {
    let bp = P.f[0].x;
    let wp = P.f[0].y;
    let clampo = P.u[0].z != 0u;
    var o: f32;
    if (wp >= bp) {
        o = (v - bp) / max(abs(wp - bp), 1e-9) * fxd_signum(wp - bp);
        if (clampo) {
            o = clamp(o, 0.0, 1.0);
        }
    } else {
        var q = (v - wp) / max(bp - wp, 1e-9);
        if (clampo) {
            q = clamp(q, 0.0, 1.0);
        }
        o = 1.0 - q;
    }
    if (P.u[0].w != 0u) {
        o = 1.0 - o;
    }
    return o;
}

// channel3d::id_color.
fn fxd_id_color(id: f32) -> vec3<f32> {
    if (id == 0.0) {
        return vec3<f32>(0.0);
    }
    let h = hash_noise(bitcast<u32>(id), 0x1du, 0x5eedu);
    return hsl_to_rgb(h, 0.75, 0.5);
}

// u[0].x = op; the effects' per-pixel steps (src = layer, aux = plane unless noted).
@compute @workgroup_size(16, 16)
fn fxd_point(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let q = tex_get(aux, p.x, p.y);
    var o = px;
    switch (P.u[0].x) {
        // 3D Channel Extract. u[0] = (op, mode, clamp, invert); f[0] = (black, white).
        case 0u: {
            if (q.w == 0.0) {
                o = vec4<f32>(0.0);
            } else {
                var c = q.xyz;
                switch (P.u[0].y) {
                    case 1u: {
                        c = vec3<f32>(fxd_lv(c.x), fxd_lv(c.y), 0.0);
                    }
                    case 2u: {
                        c = vec3<f32>(fxd_lv(c.x * 0.5 + 0.5), fxd_lv(c.y * 0.5 + 0.5), fxd_lv(c.z * 0.5 + 0.5));
                    }
                    case 4u: {}
                    default: {
                        c = vec3<f32>(fxd_lv(c.x), fxd_lv(c.y), fxd_lv(c.z));
                    }
                }
                o = vec4<f32>(c, 1.0);
            }
        }
        // Depth Matte. u[0].y = invert; f[0] = (depth, feather).
        case 1u: {
            let zv = q.x;
            let depth = P.f[0].x;
            let feather = P.f[0].y;
            var k: f32;
            if (feather > 0.0) {
                k = clamp((zv - depth) / feather + 0.5, 0.0, 1.0);
            } else {
                k = select(0.0, 1.0, zv >= depth);
            }
            if (P.u[0].y != 0u) {
                k = 1.0 - k;
            }
            o = px * k;
        }
        // Fog 3D. u[0] = (op, foggy background, gradient in aux.y); f[0] = (start, end, opacity,
        // density); f[1] = (fog colour, contribution); f[2].x = background depth.
        case 2u: {
            let zv = q.x;
            let bg = zv >= P.f[2].x * 0.5;
            let foggy = P.u[0].y != 0u;
            var f: f32;
            if (bg) {
                f = select(0.0, 1.0, foggy);
            } else {
                let t = clamp((zv - P.f[0].x) / max(abs(P.f[0].y - P.f[0].x), 1e-6), 0.0, 1.0);
                f = 1.0 - powz(1.0 - t, 1.0 + P.f[0].w * 3.0);
            }
            if (P.u[0].z != 0u) {
                f *= q.y;
            }
            f = clamp(f * P.f[0].z, 0.0, 1.0);
            let s = fxd_unpremul(px);
            var al = s.w;
            if (bg && foggy) {
                al = s.w + (1.0 - s.w) * f;
            }
            let c = s.xyz + (P.f[1].xyz - s.xyz) * f;
            o = vec4<f32>(c * al, al);
        }
        // Fog 3D gradient factor (src = fitted gradient layer, aux = depth plane). f[0].x = contribution.
        case 3u: {
            let g = fxd_unpremul(px);
            let l = luminance(g.xyz) * g.w;
            o = vec4<f32>(q.x, 1.0 - P.f[0].x + P.f[0].x * l, 0.0, 0.0);
        }
        // ID Matte's matte (src = plane). u[0].y = use coverage; f[0].x = ID.
        case 4u: {
            var m = 0.0;
            if (px.w != 0.0 && round_away(px.x) == P.f[0].x) {
                m = select(1.0, clamp(px.y, 0.0, 1.0), P.u[0].y != 0u);
            }
            o = vec4<f32>(m);
        }
        // ID Matte applied (aux = blurred matte). u[0].y = invert.
        case 5u: {
            var k = q.x;
            if (P.u[0].y != 0u) {
                k = 1.0 - k;
            }
            o = px * k;
        }
        // IDentifier. u[0].y = display; f[0] = (ID, largest ID).
        case 6u: {
            let display = P.u[0].y;
            if (q.w == 0.0) {
                if (display != 2u) {
                    o = vec4<f32>(0.0);
                }
            } else {
                let id = round_away(q.x);
                let sel = P.f[0].x;
                let hit = id == sel;
                switch (display) {
                    case 0u: {
                        var c = fxd_id_color(id);
                        if (hit && sel != 0.0) {
                            c = vec3<f32>(1.0);
                        }
                        o = vec4<f32>(c, 1.0);
                    }
                    case 1u: {
                        let v = select(0.0, 1.0, hit);
                        o = vec4<f32>(v, v, v, 1.0);
                    }
                    case 2u: {
                        o = select(vec4<f32>(0.0), px, hit);
                    }
                    default: {
                        let v = id / P.f[0].y;
                        o = vec4<f32>(v, v, v, 1.0);
                    }
                }
            }
        }
        // Cryptomatte (aux = (ID colours, selection coverage)). u[0] = (op, display, matte only).
        case 7u: {
            let m = q.w;
            if (P.u[0].z != 0u) {
                o = vec4<f32>(m);
            } else if (P.u[0].y == 0u) {
                o = vec4<f32>(q.xyz * (1.0 - 0.5 * m) + 0.5 * m, 1.0);
            } else if (P.u[0].y == 1u) {
                o = px * m;
            } else {
                o = vec4<f32>(px.x * (1.0 - 0.5 * m) + 0.5 * m * px.w, px.y * (1.0 - 0.5 * m) + 0.5 * m * px.w, px.z * (1.0 - 0.5 * m), px.w);
            }
        }
        // EXtractoR. u[0] = (op, unmult, clip); u[1] = per-channel source (0..3 own, 4 aux, 5 default);
        // f[0] = (black, white).
        case 8u: {
            let s = fxd_unpremul(px);
            let straight = vec4<f32>(s.xyz, s.w);
            var v = vec4<f32>(0.0);
            for (var k = 0; k < 4; k++) {
                let kind = P.u[1][k];
                var x: f32;
                if (kind == 4u) {
                    x = q[k];
                } else if (kind < 4u) {
                    x = straight[kind];
                } else {
                    x = select(0.0, 1.0, k == 3);
                }
                if (k < 3) {
                    let bp = P.f[0].x;
                    let wp = P.f[0].y;
                    x = (x - bp) / max(abs(wp - bp), 1e-9) * fxd_signum(wp - bp);
                    if (P.u[0].z != 0u) {
                        x = clamp(x, 0.0, 1.0);
                    }
                }
                v = vset4f(v, u32(k), x);
            }
            let al = clamp(v.w, 0.0, 1.0);
            var c = v.xyz;
            if (P.u[0].y != 0u && al > 1e-6) {
                c = v.xyz / al;
            }
            o = vec4<f32>(c * al, al);
        }
        default: {}
    }
    textureStore(out, p, o);
}

// Depth of Field: each pixel's position in the blur stack (src = depth plane).
// u[0].x = levels; f[0] = (focal plane, thickness, maximum radius, bias exponent).
@compute @workgroup_size(16, 16)
fn fxd_dof_f(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let z = textureLoad(src, p, 0).x;
    let focal = P.f[0].x;
    let max_r = P.f[0].z;
    let d = max(abs(z - focal) - P.f[0].y * 0.5, 0.0);
    let range = max(abs(focal), 1.0);
    let n = min(d / range, 1.0);
    let r = max_r * powz(n, P.f[0].w);
    let top = f32(P.u[0].x - 1u);
    let f = clamp(r / max_r * top, 0.0, top);
    textureStore(out, p, vec4<f32>(f, 0.0, 0.0, 0.0));
}

// Depth of Field: pixels between stack levels k (src) and k + 1 (aux); the positions come
// through `data` (rows of RGBA f32). u[0] = (k, row length, levels).
@compute @workgroup_size(16, 16)
fn fxd_dof_level(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let f = data[(u32(p.y) * P.u[0].y + u32(p.x)) * 4u];
    let k0 = min(u32(floor(f)), P.u[0].z - 2u);
    if (k0 != P.u[0].x) {
        return;
    }
    let t = f - f32(k0);
    let a0 = textureLoad(src, p, 0);
    let a1 = textureLoad(aux, p, 0);
    textureStore(out, p, a0 + (a1 - a0) * t);
}

// u[0].x = 0: src + aux; 1: src × 0.25.
@compute @workgroup_size(16, 16)
fn fxd_avg(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let a = textureLoad(src, p, 0);
    if (P.u[0].x == 0u) {
        textureStore(out, p, a + textureLoad(aux, p, 0));
    } else {
        textureStore(out, p, a * 0.25);
    }
}
