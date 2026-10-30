// ---------------------------------------------------------------- simulation render passes (fx_sim.rs)
//
// Items (sprites, CC Bubbles bubbles, Mr. Mercury blobs, textured pieces) are binned into 16 × 16
// tiles: data = tile offsets, item indices, then the item records (u[0].w floats each, the last
// four the item's integer pixel bounds). Every shading step mirrors effects::sim / sim2.

fn fxm_unpremul(p: vec4<f32>) -> vec4<f32> {
    if (p.w > 1e-6) {
        return vec4<f32>(p.xyz / p.w, p.w);
    }
    return vec4<f32>(0.0, 0.0, 0.0, max(p.w, 0.0));
}

fn fxm_aa(edge: f32) -> f32 {
    return clamp(edge + 0.5, 0.0, 1.0);
}

struct FxmShade {
    ok: bool,
    c: vec4<f32>,
}

fn fxm_none() -> FxmShade {
    return FxmShade(false, vec4<f32>(0.0));
}

// sim::sprite_shade, after resolving the sprite's tint (sim::SpritePlan::resolve) against `src`.
fn fxm_sprite(at: u32, x: f32, y: f32) -> FxmShade {
    let kind = u32(data[at]);
    let sx = data[at + 1u];
    let sy = data[at + 2u];
    let r0 = data[at + 3u];
    var c = vec4<f32>(data[at + 4u], data[at + 5u], data[at + 6u], data[at + 7u]);
    let tk = u32(data[at + 11u]);
    if (tk != 0u) {
        let smp = sample_bilinear(src, data[at + 12u], data[at + 13u]);
        let s = fxm_unpremul(smp);
        if (tk == 1u) {
            if (s.w > 0.0) {
                c = vec4<f32>((s.xyz * 0.6 + 0.5) * c.xyz, c.w);
            }
        } else if (tk == 2u) {
            if (s.w <= 0.0) {
                c = vec4<f32>(0.0);
            } else {
                c = vec4<f32>(s.xyz, s.w * c.w);
            }
        } else {
            if (smp.w < 0.5) {
                c = vec4<f32>(0.0);
            } else {
                let hair = vec3<f32>(data[at + 14u], data[at + 15u], data[at + 16u]);
                let inherit = data[at + 17u];
                let base = s.xyz + (hair - s.xyz) * (1.0 - inherit);
                c = vec4<f32>(base * data[at + 18u] + data[at + 19u], c.w);
            }
        }
    }
    if (c.w <= 1e-5) {
        return fxm_none();
    }
    var r = r0;
    var fade = 1.0;
    if (r0 < 0.5) {
        r = 0.5;
        fade = r0 / 0.5;
    }
    let dx = x - sx;
    let dy = y - sy;
    var rgb = c.xyz;
    var cov = 0.0;
    switch (kind) {
        case 0u: {
            let d2 = (dx * dx + dy * dy) / (r * r);
            if (d2 >= 1.0) {
                return fxm_none();
            }
            let q = 1.0 - d2;
            cov = q * q;
        }
        case 1u: {
            cov = fxm_aa(r - sqrt(dx * dx + dy * dy));
        }
        case 2u: {
            let d = sqrt(dx * dx + dy * dy);
            let cv = fxm_aa(r - d);
            if (cv <= 0.0) {
                return fxm_none();
            }
            let nx = dx / r;
            let ny = dy / r;
            let nz = sqrt(max(1.0 - nx * nx - ny * ny, 0.0));
            let diff = max(nx * -0.48 + ny * -0.58 + nz * 0.66, 0.0);
            let rz = 2.0 * diff * nz - 0.66;
            let spec = powz(max(rz, 0.0), 20.0) * 0.6;
            let k = 0.2 + 0.8 * diff;
            rgb = rgb * k + spec;
            cov = cv;
        }
        case 3u: {
            let d = sqrt(dx * dx + dy * dy) / r;
            cov = powz(max(1.0 - d, 0.0), 1.5);
        }
        case 4u: {
            let d = sqrt(dx * dx + dy * dy) / r;
            if (d > 1.0 + 1.0 / r) {
                return fxm_none();
            }
            let rr = (d - 0.9) / 0.12;
            let ring = max(1.0 - rr * rr, 0.0);
            let hx = dx / r + 0.4;
            let hy = dy / r + 0.4;
            let hl = max(1.0 - (hx * hx + hy * hy) / 0.04, 0.0);
            cov = min(0.12 * select(0.0, 1.0, d <= 1.0) + ring * 0.8 + hl, 1.0);
        }
        case 5u: {
            let sn = sin(data[at + 8u]);
            let cs = cos(data[at + 8u]);
            let ux = (dx * cs + dy * sn) / r;
            let uy = (-dx * sn + dy * cs) / r;
            let d = sqrt(ux * ux + uy * uy);
            let core = max(1.0 - d, 0.0) * max(1.0 - d, 0.0);
            let stx = max(1.0 - abs(ux) / 2.0, 0.0) * max(1.0 - abs(uy) * 6.0, 0.0);
            let sty = max(1.0 - abs(uy) / 2.0, 0.0) * max(1.0 - abs(ux) * 6.0, 0.0);
            cov = min(core + stx + sty, 1.0);
        }
        case 6u: {
            let sn = sin(data[at + 8u]);
            let cs = cos(data[at + 8u]);
            let ux = dx * cs + dy * sn;
            let uy = -dx * sn + dy * cs;
            let e = r * 0.75;
            cov = fxm_aa(e - abs(ux)) * fxm_aa(e - abs(uy));
        }
        case 7u: {
            let sn = sin(data[at + 8u]);
            let cs = cos(data[at + 8u]);
            let ux = dx * cs + dy * sn;
            let uy = -dx * sn + dy * cs;
            let e = r * 0.5;
            var m = 3.4e38;
            for (var k = 0; k < 3; k++) {
                let a = 1.5707964 + f32(k) * 6.2831855 / 3.0;
                m = min(m, e - (ux * cos(a) + uy * sin(a)));
            }
            cov = fxm_aa(m);
        }
        default: {
            let lx = data[at + 9u];
            let ly = data[at + 10u];
            let l2 = lx * lx + ly * ly;
            var t = 0.0;
            if (l2 > 0.0) {
                t = clamp((dx * lx + dy * ly) / l2, 0.0, 1.0);
            }
            let px = dx - lx * t;
            let py = dy - ly * t;
            cov = fxm_aa(r - sqrt(px * px + py * py));
        }
    }
    let a = cov * c.w * fade;
    if (a <= 1e-6) {
        return fxm_none();
    }
    return FxmShade(true, vec4<f32>(rgb * a, a));
}

// CC Bubbles' bubble (sim::bubbles). u[1].y = metal, u[1].z = shading type, f[0] = the layer
// rectangle's first and last pixel centres (x0, y0, x1, y1), which refraction samples stay within.
fn fxm_bubble(at: u32, x: f32, y: f32) -> FxmShade {
    let qx = data[at + 1u];
    let qy = data[at + 2u];
    let qr = data[at + 3u];
    let dx = (x - qx) / max(qr, 0.5);
    let dy = (y - qy) / max(qr, 0.5);
    let d2 = dx * dx + dy * dy;
    let d = sqrt(d2);
    let cov = fxm_aa((1.0 - d) * qr);
    if (cov <= 0.0) {
        return fxm_none();
    }
    let k = select(-0.7, 1.6, P.u[1].y != 0u);
    let nz = sqrt(max(1.0 - d2, 0.0));
    let sx = min(max(data[at + 4u] + dx * qr * k, P.f[0].x), P.f[0].z);
    let sy = min(max(data[at + 5u] + dy * qr * k, P.f[0].y), P.f[0].w);
    let s = fxm_unpremul(sample_bilinear_clamped(src, sx, sy));
    var c = s.xyz;
    if (s.w <= 0.0) {
        c = vec3<f32>(0.6, 0.7, 0.8);
    }
    var a = cov;
    switch (P.u[1].z) {
        case 1u: {
            c = c * 0.7 + 0.3 * (1.0 - nz);
        }
        case 2u: {
            c = c * (0.5 + 0.5 * nz);
        }
        case 3u: {
            a *= powz(nz, 0.5);
        }
        case 4u: {
            a *= 1.0 - nz * 0.8;
        }
        default: {}
    }
    let hx = dx + 0.35;
    let hy = dy + 0.35;
    let hl = max(1.0 - (hx * hx + hy * hy) / 0.05, 0.0) * 0.8;
    c = c + hl;
    return FxmShade(true, vec4<f32>(c * a, a));
}

// Mr. Mercury's blob field (f, ∂f/∂x, ∂f/∂y). f[0].x = 1 + influence.
fn fxm_blob(at: u32, x: f32, y: f32) -> FxmShade {
    let dx = x - data[at + 1u];
    let dy = y - data[at + 2u];
    let rr = data[at + 3u] * 2.0 * P.f[0].x;
    let d2 = (dx * dx + dy * dy) / (rr * rr);
    if (d2 >= 1.0) {
        return fxm_none();
    }
    let k = 1.0 - d2;
    let g = -6.0 * k * k / (rr * rr);
    return FxmShade(true, vec4<f32>(k * k * k, g * dx, g * dy, 0.0));
}

// A textured piece (sim2::draw_pieces): src = front texture, aux = back (u[1].y = 1 when set).
fn fxm_piece(at: u32, x: f32, y: f32) -> FxmShade {
    let m0 = vec3<f32>(data[at + 1u], data[at + 2u], data[at + 3u]);
    let m1 = vec3<f32>(data[at + 4u], data[at + 5u], data[at + 6u]);
    let m2 = vec3<f32>(data[at + 7u], data[at + 8u], data[at + 9u]);
    let w = m2.x * x + m2.y * y + m2.z;
    if (abs(w) < 1e-12) {
        return fxm_none();
    }
    let c0 = vec2<f32>(data[at + 10u], data[at + 11u]);
    var u = (m0.x * x + m0.y * y + m0.z) / w + c0.x;
    var v = (m1.x * x + m1.y * y + m1.z) / w + c0.y;
    let n = u32(data[at + 24u]);
    var md = 3.4e38;
    for (var i = 0u; i < n; i++) {
        let j = (i + 1u) % n;
        let ax = data[at + 12u + i * 2u];
        let ay = data[at + 13u + i * 2u];
        let ex = data[at + 12u + j * 2u] - ax;
        let ey = data[at + 13u + j * 2u] - ay;
        let l = max(sqrt(ex * ex + ey * ey), 1e-6);
        md = min(md, (ex * (v - ay) - ey * (u - ax)) / l);
    }
    let cov = clamp(md + 1.0, 0.0, 1.0);
    if (cov <= 0.0) {
        return fxm_none();
    }
    var c: vec3<f32>;
    var a = 1.0;
    if (data[at + 33u] != 0.0) {
        c = vec3<f32>(data[at + 34u], data[at + 35u], data[at + 36u]);
    } else {
        let back = data[at + 37u] != 0.0;
        let mirror = u32(data[at + 38u]);
        if (back && mirror == 1u) {
            v = 2.0 * c0.y - v;
        } else if (back && mirror == 2u) {
            u = 2.0 * c0.x - u;
        }
        var t: vec4<f32>;
        if (back && P.u[1].y != 0u) {
            t = sample_bilinear(aux, u, v);
        } else {
            t = sample_bilinear(src, u, v);
        }
        let s = fxm_unpremul(t);
        c = s.xyz;
        a = s.w;
    }
    a = a * cov * data[at + 32u];
    if (a <= 1e-6) {
        return FxmShade(false, vec4<f32>(0.0));
    }
    let tint = vec3<f32>(data[at + 26u], data[at + 27u], data[at + 28u]);
    let spec = vec3<f32>(data[at + 29u], data[at + 30u], data[at + 31u]);
    let o = c * data[at + 25u] * tint + spec;
    return FxmShade(true, vec4<f32>(o * a, a));
}

// sim::raster over a transparent frame. u[0] = (kind: 0 sprites, 1 bubbles, 2 blob field,
// 3 pieces; additive; items offset; item stride); u[1].x = tiles across.
@compute @workgroup_size(16, 16)
fn fxm_raster(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let tile = u32(p.y / 16) * P.u[1].x + u32(p.x / 16);
    let start = u32(data[tile]);
    let end = u32(data[tile + 1u]);
    let stride = P.u[0].w;
    let add = P.u[0].y != 0u;
    let x = f32(p.x) + 0.5;
    let y = f32(p.y) + 0.5;
    var d = vec4<f32>(0.0);
    for (var k = start; k < end; k++) {
        let at = P.u[0].z + u32(data[k]) * stride;
        let bb = at + stride - 4u;
        if (f32(p.x) < data[bb] || f32(p.y) < data[bb + 1u] || f32(p.x) > data[bb + 2u] || f32(p.y) > data[bb + 3u]) {
            continue;
        }
        var s: FxmShade;
        switch (P.u[0].x) {
            case 0u: {
                s = fxm_sprite(at, x, y);
            }
            case 1u: {
                s = fxm_bubble(at, x, y);
            }
            case 2u: {
                s = fxm_blob(at, x, y);
            }
            default: {
                s = fxm_piece(at, x, y);
            }
        }
        if (!s.ok) {
            continue;
        }
        let c = s.c;
        if (add) {
            d = vec4<f32>(d.xyz + c.xyz, min(d.w + c.w, 1.0));
        } else {
            d = c + d * (1.0 - c.w);
        }
    }
    textureStore(out, p, d);
}

// The sprite layer (aux) meets the original (src). u[0].x = 0: sim::combine with mode u[0].y;
// 1: mixed toward the original by f[0].x; 2: over opaque black.
@compute @workgroup_size(16, 16)
fn fxm_post(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let s = textureLoad(src, p, 0);
    let f = textureLoad(aux, p, 0);
    var o = f;
    if (P.u[0].x == 1u) {
        o = f + (s - f) * P.f[0].x;
    } else if (P.u[0].x == 2u) {
        o = vec4<f32>(f.xyz, f.w + (1.0 - f.w));
    } else {
        switch (P.u[0].y) {
            case 0u: {
                o = f + s * (1.0 - f.w);
            }
            case 1u: {
                o = s + f - s * f;
            }
            case 2u: {
                o = vec4<f32>(s.xyz + f.xyz, min(s.w + f.w, 1.0));
            }
            case 3u: {
                o = vec4<f32>(max(s.xyz, f.xyz), s.w + f.w - s.w * f.w);
            }
            case 4u: {
                let fu = fxm_unpremul(f);
                let m = s.xyz - (s.xyz - min(s.xyz, fu.xyz * s.w)) * fu.w;
                o = vec4<f32>(m, s.w);
            }
            default: {}
        }
    }
    textureStore(out, p, o);
}

fn fxm_norm3(v: vec3<f32>) -> vec3<f32> {
    let l = sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
    if (l > 1e-9) {
        return v / l;
    }
    return vec3<f32>(0.0, -1.0, 0.0);
}

// Mr. Mercury's shading (src = layer, aux = blob field). f[0] = (ambient, diffuse, specular,
// roughness); f[1] = (metal, light intensity, render scale); f[2] = light direction; f[3] = colour.
@compute @workgroup_size(16, 16)
fn fxm_mercury(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let fv = textureLoad(aux, p, 0);
    let f = fv.x;
    let thr = 0.35;
    if (f <= thr * 0.8) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let cov = clamp((f - thr * 0.8) / (thr * 0.4), 0.0, 1.0);
    let n = fxm_norm3(vec3<f32>(fv.y * 3.0, fv.z * 3.0, 1.0 / (1.0 + f)));
    let s = P.f[1].z;
    let smp = fxm_unpremul(sample_bilinear_clamped(src, f32(p.x) + 0.5 - n.x * 12.0 * s, f32(p.y) + 0.5 - n.y * 12.0 * s));
    var base = smp.xyz;
    if (smp.w <= 0.0) {
        base = vec3<f32>(0.7, 0.7, 0.75);
    }
    let l = P.f[2].xyz;
    let nl = max(dot(n, l), 0.0);
    let hv = fxm_norm3(vec3<f32>(l.x, l.y, l.z + 1.0));
    let li = P.f[1].y;
    let spec = powz(max(dot(n, hv), 0.0), 1.0 / P.f[0].w) * P.f[0].z * li;
    let shade = P.f[0].x + P.f[0].y * nl * li;
    let metal = P.f[1].x;
    let cc = base * shade + spec * (P.f[3].xyz * (1.0 - metal) + base * metal);
    textureStore(out, p, vec4<f32>(cc * cov, cov));
}

// CC Drizzle's ripple height at (x, y): data = drops (x, y, ring radius, amplitude).
fn fxm_ripples(x: f32, y: f32) -> f32 {
    var v = 0.0;
    for (var i = 0u; i < P.u[0].x; i++) {
        let dx = data[i * 4u];
        let dy = data[i * 4u + 1u];
        let ex = x - dx;
        let ey = y - dy;
        let d = sqrt(ex * ex + ey * ey);
        let r = data[i * 4u + 2u];
        let u = (d - r) / P.f[0].y;
        if (abs(u) < 1.0) {
            let env = 0.5 + 0.5 * cos(u * 3.1415927);
            v += data[i * 4u + 3u] * env * cos((d - r) / P.f[0].x * 6.2831855);
        }
    }
    return v;
}

// CC Drizzle. u[0].x = drops; f[0] = (wavelength, ring width, displacement, roughness);
// f[1] = (light direction, intensity); f[2] = (half vector, specular); f[3] = light colour;
// f[4] = (ambient, diffuse).
@compute @workgroup_size(16, 16)
fn fxm_drizzle(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let fx = f32(p.x) + 0.5;
    let fy = f32(p.y) + 0.5;
    let gx = fxm_ripples(fx + 1.0, fy) - fxm_ripples(fx - 1.0, fy);
    let gy = fxm_ripples(fx, fy + 1.0) - fxm_ripples(fx, fy - 1.0);
    if (gx == 0.0 && gy == 0.0) {
        textureStore(out, p, textureLoad(src, p, 0));
        return;
    }
    let smp = sample_bilinear_clamped(src, fx - gx * P.f[0].z, fy - gy * P.f[0].z);
    let v = vec3<f32>(-gx * 6.0, -gy * 6.0, 1.0);
    let n = v / sqrt(v.x * v.x + v.y * v.y + 1.0);
    let l = P.f[1].xyz;
    let li = P.f[1].w;
    let nl = max(dot(n, l), 0.0);
    let sp = powz(max(dot(n, P.f[2].xyz), 0.0), 1.0 / P.f[0].w) * P.f[2].w * li;
    let shade = 1.0 + (P.f[4].x + P.f[4].y * nl - P.f[4].y * l.z) * li;
    let a = smp.w;
    textureStore(out, p, vec4<f32>(smp.xyz * shade + sp * P.f[3].xyz * a, a));
}

// Wave World's Height Map view. data = grid heights (nx × ny), then water depths when
// u[0].z (dry areas transparent); f[0] = (brightness, contrast, gamma, alpha);
// f[1] = (buffer offset, render scale, layer width).
@compute @workgroup_size(16, 16)
fn fxm_wave(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let nx = P.u[0].x;
    let ny = P.u[0].y;
    let lw = P.f[1].w;
    let lx = (f32(p.x) + 0.5 - P.f[1].x) / P.f[1].z;
    let ly = (f32(p.y) + 0.5 - P.f[1].y) / P.f[1].z;
    let gx = clamp(lx / lw * f32(nx - 1u), 0.0, f32(nx - 1u));
    let gy = clamp(ly / lw * f32(nx - 1u), 0.0, f32(ny - 1u));
    let x0 = u32(floor(gx));
    let y0 = u32(floor(gy));
    let x1 = min(x0 + 1u, nx - 1u);
    let y1 = min(y0 + 1u, ny - 1u);
    let tx = gx - f32(x0);
    let ty = gy - f32(y0);
    let g00 = data[y0 * nx + x0];
    let g10 = data[y0 * nx + x1];
    let g01 = data[y1 * nx + x0];
    let g11 = data[y1 * nx + x1];
    let a = g00 + (g10 - g00) * tx;
    let c = g01 + (g11 - g01) * tx;
    let h = a + (c - a) * ty;
    let v = powz(clamp(P.f[0].x + P.f[0].y * h, 0.0, 1.0), 1.0 / P.f[0].z);
    var al = P.f[0].w;
    if (P.u[0].z != 0u) {
        let rx = u32(clamp(round_away(lx / lw * f32(nx - 1u)), 0.0, f32(nx - 1u)));
        let ry = u32(clamp(round_away(ly / lw * f32(nx - 1u)), 0.0, f32(ny - 1u)));
        if (data[nx * ny + ry * nx + rx] <= 0.0) {
            al = 0.0;
        }
    }
    textureStore(out, p, vec4<f32>(v * al, v * al, v * al, al));
}

// ---- Caustics

// The water surface's height gradient at (x, y) (aux = height plane, clamped; none = flat).
fn fxm_grad(x: i32, y: i32) -> vec2<f32> {
    if (P.u[0].x == 0u) {
        return vec2<f32>(0.0);
    }
    let gx = (tex_get_clamped(aux, x + 1, y).x - tex_get_clamped(aux, x - 1, y).x) * 0.5;
    let gy = (tex_get_clamped(aux, x, y + 1).x - tex_get_clamped(aux, x, y - 1).x) * 0.5;
    return vec2<f32>(gx, gy);
}

fn fxm_rem(v: f32, n: f32) -> f32 {
    return v - n * floor(v / n);
}

fn fxm_mirror(v: f32, n: f32) -> f32 {
    let r = fxm_rem(v, 2.0 * n);
    return select(r, 2.0 * n - r, r > n);
}

// sim2::sample_repeat of `src`.
fn fxm_sample_repeat(x: f32, y: f32, mode: u32) -> vec4<f32> {
    let d = vec2<f32>(textureDimensions(src));
    if (mode == 1u) {
        return sample_bilinear_clamped(src, fxm_rem(x, d.x), fxm_rem(y, d.y));
    }
    if (mode == 2u) {
        return sample_bilinear_clamped(src, fxm_mirror(x, d.x), fxm_mirror(y, d.y));
    }
    return sample_bilinear(src, x, y);
}

// A sky texel from `data` (RGBA f32 rows of u[1].y pixels; u[1].zw = size).
fn fxm_sky_texel(x: i32, y: i32, clamped: bool) -> vec4<f32> {
    let w = i32(P.u[1].z);
    let h = i32(P.u[1].w);
    var xi = x;
    var yi = y;
    if (clamped) {
        xi = clamp(x, 0, w - 1);
        yi = clamp(y, 0, h - 1);
    } else if (x < 0 || y < 0 || x >= w || y >= h) {
        return vec4<f32>(0.0);
    }
    let i = (u32(yi) * P.u[1].y + u32(xi)) * 4u;
    return vec4<f32>(data[i], data[i + 1u], data[i + 2u], data[i + 3u]);
}

fn fxm_sky_bilinear(x: f32, y: f32, clamped: bool) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = fxm_sky_texel(xi, yi, clamped);
    let b = fxm_sky_texel(xi + 1, yi, clamped);
    let c = fxm_sky_texel(xi, yi + 1, clamped);
    let d = fxm_sky_texel(xi + 1, yi + 1, clamped);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

fn fxm_sky_repeat(x: f32, y: f32, mode: u32) -> vec4<f32> {
    let w = f32(P.u[1].z);
    let h = f32(P.u[1].w);
    if (mode == 1u) {
        return fxm_sky_bilinear(fxm_rem(x, w), fxm_rem(y, h), true);
    }
    if (mode == 2u) {
        return fxm_sky_bilinear(fxm_mirror(x, w), fxm_mirror(y, h), true);
    }
    return fxm_sky_bilinear(x, y, false);
}

// Caustics (sim2::caustics; src = the blurred bottom, aux = height plane, data = sky rows).
// u[0] = (height, repeat, point light, sky repeat); u[1] = (sky, row, width, height);
// f[0] = (scaling, wave height, displacement k, layer width); f[1] = (centre, light position);
// f[2] = (surface colour, opacity); f[3] = (light colour, intensity); f[4] = (light height,
// caustics strength, ambient, diffuse); f[5] = (specular, sharpness, sky scaling, sky
// intensity); f[6] = (distant light direction, convergence).
@compute @workgroup_size(16, 16)
fn fxm_caustics(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let k = P.f[0].z;
    let x = p.x;
    let y = p.y;
    let dd = -fxm_grad(x, y) * k;
    let dxr = -fxm_grad(min(x + 1, dims.x - 1), y).x * k;
    let dyd = -fxm_grad(x, min(y + 1, dims.y - 1)).y * k;
    let dxl = -fxm_grad(max(x - 1, 0), y).x * k;
    let dyu = -fxm_grad(x, max(y - 1, 0)).y * k;
    let jac = (1.0 + (dxr - dxl) * 0.5) * (1.0 + (dyd - dyu) * 0.5);
    let focus = clamp(1.0 / max(abs(jac), 0.2) - 1.0, -1.0, 4.0);
    let cx = P.f[1].x;
    let cy = P.f[1].y;
    let fx = f32(x) + 0.5;
    let fy = f32(y) + 0.5;
    let sx = cx + (fx + dd.x - cx) / P.f[0].x;
    let sy = cy + (fy + dd.y - cy) / P.f[0].x;
    let bot = fxm_unpremul(fxm_sample_repeat(sx, sy, P.u[0].y));
    let g = fxm_grad(x, y);
    let wh = P.f[0].y;
    let v = vec3<f32>(-g.x * wh * 20.0, -g.y * wh * 20.0, 1.0);
    let n = v / sqrt(v.x * v.x + v.y * v.y + 1.0);
    var lv = P.f[6].xyz;
    if (P.u[0].z != 0u) {
        let lw = max(P.f[0].w, 1.0);
        let q = vec3<f32>((P.f[1].z - fx) / lw, (P.f[1].w - fy) / lw, P.f[4].x);
        lv = q / max(sqrt(q.x * q.x + q.y * q.y + q.z * q.z), 1e-12);
    }
    let li = P.f[3].w;
    let nl = max(dot(n, lv), 0.0);
    let hq = vec3<f32>(lv.x, lv.y, lv.z + 1.0);
    let hv = hq / sqrt(hq.x * hq.x + hq.y * hq.y + hq.z * hq.z);
    let spec = powz(max(dot(n, hv), 0.0), P.f[5].y) * P.f[5].x * li;
    let ambient = P.f[4].z;
    let diffuse = P.f[4].w;
    let light = ambient + diffuse * li * nl + P.f[4].y * focus * li;
    let surf = P.f[2].xyz;
    let surf_op = P.f[2].w;
    var surface: vec3<f32>;
    if (P.u[1].x != 0u) {
        let reach = (1.0 - P.f[6].w) * P.f[0].w * 0.5;
        let rx = cx + (fx + n.x * reach - cx) / P.f[5].z;
        let ry = cy + (fy + n.y * reach - cy) / P.f[5].z;
        let sc = fxm_unpremul(fxm_sky_repeat(rx, ry, P.u[0].w)).xyz;
        surface = sc * P.f[5].w * 3.0 * surf;
    } else {
        surface = surf * (ambient + diffuse * li * nl);
    }
    let lc = P.f[3].xyz;
    let cc = bot.xyz * light * lc * (1.0 - surf_op) + surface * surf_op + spec * lc;
    let a = min(bot.w + surf_op * (1.0 - bot.w), 1.0);
    textureStore(out, p, vec4<f32>(cc * a, a));
}
