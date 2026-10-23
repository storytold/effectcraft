// ---------------------------------------------------------------- Immersive Video (fx_vr.rs)
//
// Equirectangular frames: longitude across, latitude down, the front (+Z) at the centre, y up
// (effects::vr). Every function mirrors its CPU namesake.

const FXV_PI: f32 = 3.14159265358979;

fn fxv_unpremul(p: vec4<f32>) -> vec4<f32> {
    if (p.w > 1e-6) {
        return vec4<f32>(p.xyz / p.w, p.w);
    }
    return vec4<f32>(0.0, 0.0, 0.0, max(p.w, 0.0));
}

// vr::equi_dir.
fn fxv_equi_dir(x: f32, y: f32, w: f32, h: f32) -> vec3<f32> {
    let lon = (x / w - 0.5) * 2.0 * FXV_PI;
    let lat = (0.5 - y / h) * FXV_PI;
    return vec3<f32>(cos(lat) * sin(lon), sin(lat), cos(lat) * cos(lon));
}

// vr::equi_px.
fn fxv_equi_px(d: vec3<f32>, w: f32, h: f32) -> vec2<f32> {
    let lon = atan2(d.x, d.z);
    let lat = asin_p(clamp(d.y, -1.0, 1.0));
    return vec2<f32>((lon / (2.0 * FXV_PI) + 0.5) * w, (0.5 - lat / FXV_PI) * h);
}

// Angle between two unit directions (the CPU's acos of the dot, in its f32-stable form).
fn fxv_angle(a: vec3<f32>, b: vec3<f32>) -> f32 {
    return atan2(length(cross(a, b)), dot(a, b));
}

// vr::sample_equi's texel fetch: wraps horizontally, continues over the poles.
fn fxv_equi_at(xi0: i32, yi0: i32, w: i32, h: i32) -> vec4<f32> {
    var xi = xi0;
    var yi = yi0;
    if (yi < 0) {
        yi = -1 - yi;
        xi += w / 2;
    } else if (yi >= h) {
        yi = 2 * h - 1 - yi;
        xi += w / 2;
    }
    let xm = imod(xi, w);
    return textureLoad(src, vec2<i32>(xm, clamp(yi, 0, h - 1)), 0);
}

// vr::sample_equi (bilinear).
fn fxv_sample_equi(x: f32, y: f32) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(src));
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = fxv_equi_at(xi, yi, d.x, d.y);
    let b = fxv_equi_at(xi + 1, yi, d.x, d.y);
    let c = fxv_equi_at(xi, yi + 1, d.x, d.y);
    let e = fxv_equi_at(xi + 1, yi + 1, d.x, d.y);
    let top = a + (b - a) * tx;
    let bot = c + (e - c) * tx;
    return top + (bot - top) * ty;
}

// ---- VR Converter's projections (vr::Proj)

fn fxv_face_fw(f: u32) -> vec3<f32> {
    var v = array<vec3<f32>, 6>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(-1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, -1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -1.0));
    return v[f];
}

fn fxv_face_r(f: u32) -> vec3<f32> {
    var v = array<vec3<f32>, 6>(vec3<f32>(0.0, 0.0, -1.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(-1.0, 0.0, 0.0));
    return v[f];
}

fn fxv_face_u(f: u32) -> vec3<f32> {
    var v = array<vec3<f32>, 6>(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, -1.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 1.0, 0.0));
    return v[f];
}

// vr::cube_grid: the grid size.
fn fxv_grid(kind: u32) -> vec2<f32> {
    switch (kind) {
        case 1u: {
            return vec2<f32>(4.0, 3.0);
        }
        case 2u, 6u, 7u: {
            return vec2<f32>(3.0, 2.0);
        }
        default: {
            return vec2<f32>(6.0, 1.0);
        }
    }
}

// vr::cube_grid: (column, row) of face f.
fn fxv_cell(kind: u32, f: u32) -> vec2<f32> {
    switch (kind) {
        case 1u: {
            var c = array<vec2<f32>, 6>(vec2<f32>(2.0, 1.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 2.0), vec2<f32>(1.0, 1.0), vec2<f32>(3.0, 1.0));
            return c[f];
        }
        case 2u: {
            var c = array<vec2<f32>, 6>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(2.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0), vec2<f32>(2.0, 1.0));
            return c[f];
        }
        case 6u, 7u: {
            var c = array<vec2<f32>, 6>(vec2<f32>(2.0, 0.0), vec2<f32>(0.0, 0.0), vec2<f32>(2.0, 1.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0));
            return c[f];
        }
        default: {
            return vec2<f32>(f32(f), 0.0);
        }
    }
}

fn fxv_is_cube(kind: u32) -> bool {
    return (kind >= 1u && kind <= 3u) || kind == 6u || kind == 7u;
}

// Proj::dir: the direction seen at frame position (x, y); w = 2 when outside the image.
fn fxv_dir(kind: u32, fov: f32, x: f32, y: f32, w: f32, h: f32) -> vec4<f32> {
    let none = vec4<f32>(0.0, 0.0, 0.0, 2.0);
    if (kind == 0u) {
        return vec4<f32>(fxv_equi_dir(x, y, w, h), 1.0);
    }
    if (fxv_is_cube(kind)) {
        let g = fxv_grid(kind);
        let cw = w / g.x;
        let ch = h / g.y;
        let cx = floor(x / cw);
        let cy = floor(y / ch);
        var f = 6u;
        for (var i = 0u; i < 6u; i++) {
            let c = fxv_cell(kind, i);
            if (f == 6u && c.x == cx && c.y == cy) {
                f = i;
            }
        }
        if (f == 6u) {
            return none;
        }
        var a = (x - cx * cw) / cw * 2.0 - 1.0;
        var b = (y - cy * ch) / ch * 2.0 - 1.0;
        if (kind == 7u) {
            a = tan(a * FXV_PI / 4.0);
            b = tan(b * FXV_PI / 4.0);
        }
        return vec4<f32>(normalize(fxv_face_fw(f) + a * fxv_face_r(f) - b * fxv_face_u(f)), 1.0);
    }
    if (kind == 8u) {
        let s = min(w, h) / 2.0;
        let u = (x - w / 2.0) / s;
        let v = (y - h / 2.0) / s;
        let r = sqrt(u * u + v * v);
        if (r > 1.0) {
            return none;
        }
        let th = r * FXV_PI / 2.0;
        var cu = 0.0;
        var cv = 0.0;
        if (r > 1e-12) {
            cu = u / r;
            cv = v / r;
        }
        return vec4<f32>(sin(th) * cu, cos(th), sin(th) * cv, 1.0);
    }
    if (kind == 4u) {
        let half = radians(clamp(fov, 1.0, 360.0) / 2.0);
        let s = min(w, h) / 2.0;
        let u = (x - w / 2.0) / s;
        let v = (h / 2.0 - y) / s;
        let r = sqrt(u * u + v * v);
        if (r > 1.0) {
            return none;
        }
        let th = r * half;
        var cu = 0.0;
        var cv = 0.0;
        if (r > 1e-12) {
            cu = u / r;
            cv = v / r;
        }
        return vec4<f32>(sin(th) * cu, sin(th) * cv, cos(th), 1.0);
    }
    let t = tan(radians(clamp(fov, 1.0, 179.0) / 2.0));
    let u = (x / w * 2.0 - 1.0) * t;
    let v = (1.0 - y / h * 2.0) * t * h / w;
    return vec4<f32>(normalize(vec3<f32>(u, v, 1.0)), 1.0);
}

// Proj::sample of `src` (in projection `kind`) in direction d.
fn fxv_proj_sample(kind: u32, fov: f32, d: vec3<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(src));
    let w = dims.x;
    let h = dims.y;
    if (kind == 0u) {
        let q = fxv_equi_px(d, w, h);
        return fxv_sample_equi(q.x, q.y);
    }
    if (fxv_is_cube(kind)) {
        // The face facing d most (the last one on ties, as max_by).
        var f = 0u;
        var best = dot(d, fxv_face_fw(0u));
        for (var i = 1u; i < 6u; i++) {
            let k = dot(d, fxv_face_fw(i));
            if (k >= best) {
                best = k;
                f = i;
            }
        }
        let fw = fxv_face_fw(f);
        let k = dot(d, fw);
        var a = dot(d, fxv_face_r(f)) / k;
        var b = -dot(d, fxv_face_u(f)) / k;
        if (kind == 7u) {
            a = atan(a) / (FXV_PI / 4.0);
            b = atan(b) / (FXV_PI / 4.0);
        }
        let g = fxv_grid(kind);
        let cw = w / g.x;
        let ch = h / g.y;
        let c = fxv_cell(kind, f);
        let x0 = c.x * cw;
        let y0 = c.y * ch;
        let x = clamp(x0 + (a + 1.0) * 0.5 * cw, x0 + 0.5, x0 + cw - 0.5);
        let y = clamp(y0 + (b + 1.0) * 0.5 * ch, y0 + 0.5, y0 + ch - 0.5);
        return sample_bilinear_clamped(src, x, y);
    }
    if (kind == 8u) {
        let th = acos_p(clamp(d.y, -1.0, 1.0));
        if (th > FXV_PI / 2.0 + 1e-6) {
            return vec4<f32>(0.0);
        }
        let s = min(w, h) / 2.0;
        let r = th / (FXV_PI / 2.0);
        let l = sqrt(d.x * d.x + d.z * d.z);
        var cu = 0.0;
        var cv = 0.0;
        if (l > 1e-12) {
            cu = d.x / l;
            cv = d.z / l;
        }
        return sample_bilinear_clamped(src, w / 2.0 + cu * r * s, h / 2.0 + cv * r * s);
    }
    if (kind == 4u) {
        let half = radians(clamp(fov, 1.0, 360.0) / 2.0);
        let th = acos_p(clamp(d.z, -1.0, 1.0));
        if (th > half + 1e-6) {
            return vec4<f32>(0.0);
        }
        let s = min(w, h) / 2.0;
        let r = th / half;
        let l = sqrt(d.x * d.x + d.y * d.y);
        var cu = 0.0;
        var cv = 0.0;
        if (l > 1e-12) {
            cu = d.x / l;
            cv = d.y / l;
        }
        return sample_bilinear_clamped(src, w / 2.0 + cu * r * s, h / 2.0 - cv * r * s);
    }
    if (d.z <= 1e-9) {
        return vec4<f32>(0.0);
    }
    let t = tan(radians(clamp(fov, 1.0, 179.0) / 2.0));
    let u = d.x / d.z / t;
    let v = d.y / d.z / (t * h / w);
    if (abs(u) > 1.0 || abs(v) > 1.0) {
        return vec4<f32>(0.0);
    }
    return sample_bilinear_clamped(src, (u + 1.0) * 0.5 * w, (1.0 - v) * 0.5 * h);
}

// mᵀ · d with m's rows in f[at..at + 3].
fn fxv_mtv(at: u32, d: vec3<f32>) -> vec3<f32> {
    let r0 = P.f[at].xyz;
    let r1 = P.f[at + 1u].xyz;
    let r2 = P.f[at + 2u].xyz;
    return r0 * d.x + r1 * d.y + r2 * d.z;
}

// m · d.
fn fxv_mv(at: u32, d: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(P.f[at].xyz, d), dot(P.f[at + 1u].xyz, d), dot(P.f[at + 2u].xyz, d));
}

// vr::convert. u[0] = (from kind, to kind, feather); f[0] = (from fov, to fov);
// f[1..4] = rotation rows; f[4] = (feather, tan(fov / 2)) (VR Plane to Sphere).
@compute @workgroup_size(16, 16)
fn fxv_convert(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let w = f32(dims.x);
    let h = f32(dims.y);
    let x = f32(p.x) + 0.5;
    let y = f32(p.y) + 0.5;
    let dd = fxv_dir(P.u[0].y, P.f[0].y, x, y, w, h);
    var o = vec4<f32>(0.0);
    if (dd.w < 1.5) {
        o = fxv_proj_sample(P.u[0].x, P.f[0].x, fxv_mtv(1u, dd.xyz));
    }
    if (P.u[0].z != 0u) {
        let d = fxv_mtv(1u, fxv_equi_dir(x, y, w, h));
        if (d.z > 0.0) {
            let t = P.f[4].y;
            let u = abs(d.x / d.z / t);
            let v = abs(d.y / d.z / (t * h / w));
            let edge = 1.0 - max(u, v);
            o = o * clamp(edge / P.f[4].x, 0.0, 1.0);
        }
    }
    textureStore(out, p, o);
}

// ---- seam-aware box blurs

// Horizontal wrap-around box mean (vr::box_row_wrap); each row's radius in data[y].
// u[0].x = block; dispatched over (row, block).
@compute @workgroup_size(64, 1)
fn fxv_box_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let y = i32(gid.x);
    let block = i32(P.u[0].x);
    let b0 = i32(gid.y) * block;
    let w = dims.x;
    if (y >= dims.y || b0 >= w) {
        return;
    }
    let r = i32(data[y]);
    let b1 = min(b0 + block, w);
    if (r == 0) {
        for (var x = b0; x < b1; x++) {
            textureStore(out, vec2<i32>(x, y), textureLoad(src, vec2<i32>(x, y), 0));
        }
        return;
    }
    let norm = 1.0 / f32(2 * r + 1);
    var acc = vec4<f32>(0.0);
    for (var i = b0 - r; i <= b0 + r; i++) {
        acc += textureLoad(src, vec2<i32>(imod(i, w), y), 0);
    }
    for (var x = b0; x < b1; x++) {
        textureStore(out, vec2<i32>(x, y), acc * norm);
        let add = textureLoad(src, vec2<i32>((x + r + 1) % w, y), 0);
        let sub = textureLoad(src, vec2<i32>(imod(x - r, w), y), 0);
        acc += add - sub;
    }
}

// vr::box_cols_pole's fetch.
fn fxv_pole_at(x0: i32, y0: i32, w: i32, h: i32) -> vec4<f32> {
    var x = x0;
    var y = y0;
    if (y < 0) {
        y = -1 - y;
        x = imod(x + w / 2, w);
    } else if (y >= h) {
        y = 2 * h - 1 - y;
        x = imod(x + w / 2, w);
    }
    return textureLoad(src, vec2<i32>(x, clamp(y, 0, h - 1)), 0);
}

// Vertical box mean continuing over the poles. u[0] = (block, radius); dispatched over
// (column, block).
@compute @workgroup_size(64, 1)
fn fxv_box_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let x = i32(gid.x);
    let block = i32(P.u[0].x);
    let r = i32(P.u[0].y);
    let b0 = i32(gid.y) * block;
    if (x >= dims.x || b0 >= dims.y) {
        return;
    }
    let norm = 1.0 / f32(2 * r + 1);
    var acc = vec4<f32>(0.0);
    for (var j = b0 - r; j <= b0 + r; j++) {
        acc += fxv_pole_at(x, j, dims.x, dims.y);
    }
    let b1 = min(b0 + block, dims.y);
    for (var y = b0; y < b1; y++) {
        textureStore(out, vec2<i32>(x, y), acc * norm);
        acc += fxv_pole_at(x, y + r + 1, dims.x, dims.y) - fxv_pole_at(x, y - r, dims.x, dims.y);
    }
}

// vr::wrap_pad: u[0].x = pad.
@compute @workgroup_size(16, 16)
fn fxv_wrap_pad(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let s = vec2<i32>(textureDimensions(src));
    let pad = i32(P.u[0].x);
    var sx = p.x - pad;
    var sy = p.y - pad;
    if (sy < 0) {
        sy = -1 - sy;
        sx += s.x / 2;
    } else if (sy >= s.y) {
        sy = 2 * s.y - 1 - sy;
        sx += s.x / 2;
    }
    textureStore(out, p, textureLoad(src, vec2<i32>(imod(sx, s.x), clamp(sy, 0, s.y - 1)), 0));
}

// Per-pixel steps. u[0].x = 0: VR Glow's highlights (u[0].y tint; f[0] = (threshold,
// saturation); f[1] = tint); 1: add the glow (aux) × f[0].x; 2: VR Sharpen (aux = blurred,
// f[0].x = amount); 3: VR De-Noise's detail mix (src = filtered, aux = original, f[0].x);
// 4: the guided filter's guide (straight luminance).
@compute @workgroup_size(16, 16)
fn fxv_point(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let q = tex_get(aux, p.x, p.y);
    var o = px;
    switch (P.u[0].x) {
        case 0u: {
            let s = fxv_unpremul(px);
            let th = P.f[0].x;
            let l = luminance(s.xyz);
            var k = 0.0;
            if (l > th) {
                k = min((l - th) / max(1.0 - th, 1e-3), 4.0);
            }
            var c = s.xyz * k;
            let lc = luminance(c);
            c = lc + (c - lc) * P.f[0].y;
            if (P.u[0].y != 0u) {
                c = lc * P.f[1].xyz;
            }
            o = vec4<f32>(c * s.w, s.w * min(k, 1.0));
        }
        case 1u: {
            let b = P.f[0].x;
            o = vec4<f32>(px.xyz + q.xyz * b, min(px.w + q.w * b, 1.0));
        }
        case 2u: {
            let hi = max(px.w, 0.0) * 4.0;
            let c = clamp(px.xyz + (px.xyz - q.xyz) * P.f[0].x, vec3<f32>(0.0), vec3<f32>(hi));
            o = vec4<f32>(c, px.w);
        }
        case 3u: {
            var m = px + (q - px) * P.f[0].x;
            m.w = clamp(m.w, 0.0, 1.0);
            o = vec4<f32>(max(m.xyz, vec3<f32>(0.0)), m.w);
        }
        case 4u: {
            o = vec4<f32>(luminance(fxv_unpremul(px).xyz));
        }
        default: {}
    }
    textureStore(out, p, o);
}

// vr::composite of a generated straight colour (alpha 1) over the pixel. u[0].z = 1: mode None
// (replace); u[0].w = blend mode id; f[0].y = opacity.
fn fxv_composite(d: vec4<f32>, c: vec3<f32>, opacity: f32) -> vec4<f32> {
    let s = vec4<f32>(c * opacity, opacity);
    if (P.u[0].z != 0u) {
        return s;
    }
    var r = blend_pixel(P.u[0].w, d, s, 0.5);
    let a = d.w;
    if (a < 1.0) {
        let k = a / max(r.w, 1e-6);
        if (k < 1.0) {
            r = r * k;
        }
    }
    return r;
}

// Generated looks. u[0].x = 0: VR Chromatic Aberrations; 1: VR Color Gradients; 2: VR Fractal
// Noise; 3: VR Digital Glitch (parameter layouts in fx_vr.rs).
@compute @workgroup_size(16, 16)
fn fxv_gen(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let w = f32(dims.x);
    let h = f32(dims.y);
    let fx = f32(p.x) + 0.5;
    let fy = f32(p.y) + 0.5;
    let px = textureLoad(src, p, 0);
    var o = px;
    switch (P.u[0].x) {
        // f[0] = (aberration R, G, B, falloff); f[1] = centre; u[0].y = invert.
        case 0u: {
            let d = fxv_equi_dir(fx, fy, w, h);
            let center = P.f[1].xyz;
            let ax = cross(center, d);
            let al = length(ax);
            let th = atan2(al, dot(center, d));
            var weight = powz(th / FXV_PI, 1.0 + P.f[0].w * 3.0);
            if (P.u[0].y != 0u) {
                weight = 1.0 - weight;
            }
            var res = vec4<f32>(0.0);
            for (var c = 0; c < 3; c++) {
                var s = px;
                if (al >= 1e-9) {
                    let new_th = th * (1.0 - P.f[0][c] * weight);
                    let perp = normalize(d - center * cos(th));
                    let nd = center * cos(new_th) + perp * sin(new_th);
                    let q = fxv_equi_px(nd, w, h);
                    s = fxv_sample_equi(q.x, q.y);
                }
                res = vset4f(res, u32(c), s[c]);
                res.w = max(res.w, s.w);
            }
            o = res;
        }
        // data: points (direction, colour); u[0].y = count; f[0] = (power, opacity).
        case 1u: {
            let d = fxv_equi_dir(fx, fy, w, h);
            let n = P.u[0].y;
            var amin = 10.0;
            var exact = -1;
            for (var i = 0u; i < n; i++) {
                let a = fxv_angle(d, vec3<f32>(data[i * 6u], data[i * 6u + 1u], data[i * 6u + 2u]));
                if (a < 1e-6 && exact < 0) {
                    exact = i32(i);
                }
                amin = min(amin, a);
            }
            var c: vec3<f32>;
            if (exact >= 0) {
                let e = u32(exact);
                c = vec3<f32>(data[e * 6u + 3u], data[e * 6u + 4u], data[e * 6u + 5u]);
            } else {
                var acc = vec3<f32>(0.0);
                var ws = 0.0;
                for (var i = 0u; i < n; i++) {
                    let a = fxv_angle(d, vec3<f32>(data[i * 6u], data[i * 6u + 1u], data[i * 6u + 2u]));
                    let wgt = pow(amin / a, P.f[0].x);
                    acc += vec3<f32>(data[i * 6u + 3u], data[i * 6u + 4u], data[i * 6u + 5u]) * wgt;
                    ws += wgt;
                }
                c = acc / ws;
            }
            o = fxv_composite(px, c, P.f[0].y);
        }
        // u[0].y = type; u[1] = (octaves, seed, invert); f[0] = (contrast, brightness, sub
        // influence, sub scaling); f[1] = (base frequency, evolution, opacity, octave fraction);
        // f[2..5] = rotation rows.
        case 2u: {
            let d = fxv_mv(2u, fxv_equi_dir(fx, fy, w, h));
            let kind = P.u[0].y;
            let n_oct = P.u[1].x;
            let evo = P.f[1].y;
            var v = 0.0;
            var best = 0.0;
            var amp = 1.0;
            var freq = P.f[1].x;
            var norm = 0.0;
            for (var oc = 0u; oc < n_oct; oc++) {
                var wgt = 1.0;
                if (oc + 1u == n_oct && P.f[1].w > 0.0) {
                    wgt = P.f[1].w;
                }
                let q = vec3<f32>(d.x * freq + evo * 0.37, d.y * freq + evo * 0.71, d.z * freq + evo * 0.59);
                var nv = value_noise(q.x + f32(oc) * 17.3, q.y, q.z, P.u[1].y + oc);
                if (kind == 1u) {
                    nv = 1.0 - abs(nv * 2.0 - 1.0);
                } else if (kind == 3u) {
                    let t = clamp(abs(nv * 2.0 - 1.0) / 0.12, 0.0, 1.0);
                    nv = 1.0 - t * t;
                }
                best = max(best, nv * amp);
                v += nv * amp * wgt;
                norm += amp * wgt;
                amp *= P.f[0].z;
                freq /= P.f[0].w;
            }
            if (kind == 2u) {
                v = best;
            } else {
                v = v / max(norm, 1e-6);
            }
            v = (v - 0.5) * P.f[0].x + 0.5 + P.f[0].y;
            if (P.u[1].z != 0u) {
                v = 1.0 - v;
            }
            v = clamp(v, 0.0, 1.0);
            o = fxv_composite(px, vec3<f32>(v), P.f[1].z);
        }
        // data: per row (dx, dy, split, dark); f[0] = channel offsets; f[1] = target direction;
        // f[2] = (radius, feather) in radians.
        case 3u: {
            let y = u32(p.y);
            let dx = data[y * 4u];
            let dy = data[y * 4u + 1u];
            let split = data[y * 4u + 2u];
            let dark = data[y * 4u + 3u];
            let radius = P.f[2].x;
            let feather = P.f[2].y;
            var k = 1.0;
            if (radius > 0.0) {
                let a = fxv_angle(fxv_equi_dir(fx, fy, w, h), P.f[1].xyz);
                if (a <= radius) {
                    k = 1.0;
                } else if (feather > 0.0) {
                    k = clamp(1.0 - (a - radius) / feather, 0.0, 1.0);
                } else {
                    k = 0.0;
                }
            }
            if (k > 0.0) {
                let sx = fx - dx * k;
                let sy = clamp(fy - dy * k, 0.5, h - 0.5);
                let c0 = fxv_sample_equi(sx + P.f[0].x * split * k, sy);
                let c1 = fxv_sample_equi(sx + P.f[0].y * split * k, sy);
                let c2 = fxv_sample_equi(sx + P.f[0].z * split * k, sy);
                let dk = 1.0 - (1.0 - dark) * k;
                o = vec4<f32>(c0.x * dk, c1.y * dk, c2.z * dk, max(max(c0.w, c1.w), c2.w));
            }
        }
        default: {}
    }
    textureStore(out, p, o);
}
