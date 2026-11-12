// SAM 2 compute kernels (f32). Original work for EffectCraft; the maths follows the Hugging Face
// `transformers` SAM 2 implementation (Apache-2.0, see NOTICE).
//
// Every kernel reads its parameters from one uniform: 24 u32 words (floats are bit-cast).

struct P { v: array<vec4<u32>, 6> }
@group(0) @binding(0) var<storage, read> in0: array<f32>;
@group(0) @binding(1) var<storage, read> in1: array<f32>;
@group(0) @binding(2) var<storage, read> in2: array<f32>;
@group(0) @binding(3) var<storage, read_write> out0: array<f32>;
@group(0) @binding(4) var<storage, read> in3: array<f32>;
@group(0) @binding(5) var<uniform> p: P;

fn u(i: u32) -> u32 { return p.v[i / 4u][i % 4u]; }
fn f(i: u32) -> f32 { return bitcast<f32>(u(i)); }

// erf, Abramowitz and Stegun 7.1.26 (abs error 1.5e-7).
fn erf_(x: f32) -> f32 {
    let s = select(1.0, -1.0, x < 0.0);
    let a = abs(x);
    let t = 1.0 / (1.0 + 0.3275911 * a);
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * exp(-a * a);
    return s * y;
}
fn gelu_(x: f32) -> f32 { return (erf_(x * 0.70710678) + 1.0) * 0.5 * x; }

// ---------------------------------------------------------------- strided batched matmul
// C[b] = act(alpha * A[b] (m x k) . B[b] (k x n) + bias) (+ residual), with free strides for A and B.
//  words: 0 m, 1 n, 2 k, 3 batch, 4 a_off, 5 a_bs, 6 a_rs, 7 a_cs, 8 b_off, 9 b_bs, 10 b_rs, 11 b_cs,
//         12 c_off, 13 c_bs, 14 c_rs, 15 flags (bit0 bias, bits 1-2 act: 1 gelu 2 relu, bit3 residual),
//         16 alpha (f32)
//  in0 = A, in1 = B, in2 = bias (n), out0 = C, in3 = residual (same layout as C)
var<workgroup> sa: array<f32, 1024>;
var<workgroup> sb: array<f32, 1024>;

@compute @workgroup_size(16, 16)
fn matmul(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let m = u(0u); let n = u(1u); let k = u(2u);
    let a_base = u(4u) + wg.z * u(5u);
    let b_base = u(8u) + wg.z * u(9u);
    let a_rs = u(6u); let a_cs = u(7u); let b_rs = u(10u); let b_cs = u(11u);
    let row0 = wg.y * 64u;
    let col0 = wg.x * 64u;
    let tid = li.y * 16u + li.x;
    let a_k_contig = a_cs == 1u;
    let b_n_contig = b_cs == 1u;
    var acc: array<f32, 16>;
    for (var i = 0u; i < 16u; i++) { acc[i] = 0.0; }
    for (var kt = 0u; kt < k; kt += 16u) {
        for (var i = 0u; i < 4u; i++) {
            let idx = tid + i * 256u;
            var r: u32; var c: u32;
            if (a_k_contig) { r = idx / 16u; c = idx % 16u; } else { r = idx % 64u; c = idx / 64u; }
            let gr = row0 + r; let gc = kt + c;
            var va = 0.0;
            if (gr < m && gc < k) { va = in0[a_base + gr * a_rs + gc * a_cs]; }
            sa[r * 16u + c] = va;
            var r2: u32; var c2: u32;
            if (b_n_contig) { r2 = idx / 64u; c2 = idx % 64u; } else { r2 = idx % 16u; c2 = idx / 16u; }
            let gr2 = kt + r2; let gc2 = col0 + c2;
            var vb = 0.0;
            if (gr2 < k && gc2 < n) { vb = in1[b_base + gr2 * b_rs + gc2 * b_cs]; }
            sb[r2 * 64u + c2] = vb;
        }
        workgroupBarrier();
        for (var kk = 0u; kk < 16u; kk++) {
            var ar: array<f32, 4>;
            var br: array<f32, 4>;
            for (var i = 0u; i < 4u; i++) {
                ar[i] = sa[(li.y * 4u + i) * 16u + kk];
                br[i] = sb[kk * 64u + li.x * 4u + i];
            }
            for (var i = 0u; i < 4u; i++) {
                for (var j = 0u; j < 4u; j++) { acc[i * 4u + j] += ar[i] * br[j]; }
            }
        }
        workgroupBarrier();
    }
    let flags = u(15u);
    let alpha = f(16u);
    let c_base = u(12u) + wg.z * u(13u);
    let c_rs = u(14u);
    let act = (flags >> 1u) & 3u;
    for (var i = 0u; i < 4u; i++) {
        for (var j = 0u; j < 4u; j++) {
            let gr = row0 + li.y * 4u + i;
            let gc = col0 + li.x * 4u + j;
            if (gr < m && gc < n) {
                var v = acc[i * 4u + j] * alpha;
                if ((flags & 1u) != 0u) { v += in2[gc]; }
                if (act == 1u) { v = gelu_(v); } else if (act == 2u) { v = max(v, 0.0); }
                let idx = c_base + gr * c_rs + gc;
                if ((flags & 8u) != 0u) { v += in3[idx]; }
                out0[idx] = v;
            }
        }
    }
}

// The same, with B (a weight matrix) stored as half precision, two values per word.
@group(0) @binding(1) var<storage, read> w16: array<u32>;

@compute @workgroup_size(16, 16)
fn matmul_w16(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let m = u(0u); let n = u(1u); let k = u(2u);
    let a_base = u(4u) + wg.z * u(5u);
    let b_base = u(8u) + wg.z * u(9u);
    let a_rs = u(6u); let a_cs = u(7u); let b_rs = u(10u); let b_cs = u(11u);
    let row0 = wg.y * 64u;
    let col0 = wg.x * 64u;
    let tid = li.y * 16u + li.x;
    let a_k_contig = a_cs == 1u;
    let b_n_contig = b_cs == 1u;
    var acc: array<f32, 16>;
    for (var i = 0u; i < 16u; i++) { acc[i] = 0.0; }
    for (var kt = 0u; kt < k; kt += 16u) {
        for (var i = 0u; i < 4u; i++) {
            let idx = tid + i * 256u;
            var r: u32; var c: u32;
            if (a_k_contig) { r = idx / 16u; c = idx % 16u; } else { r = idx % 64u; c = idx / 64u; }
            let gr = row0 + r; let gc = kt + c;
            var va = 0.0;
            if (gr < m && gc < k) { va = in0[a_base + gr * a_rs + gc * a_cs]; }
            sa[r * 16u + c] = va;
            var r2: u32; var c2: u32;
            if (b_n_contig) { r2 = idx / 64u; c2 = idx % 64u; } else { r2 = idx % 16u; c2 = idx / 16u; }
            let gr2 = kt + r2; let gc2 = col0 + c2;
            var vb = 0.0;
            if (gr2 < k && gc2 < n) { let bi = b_base + gr2 * b_rs + gc2 * b_cs; vb = unpack2x16float(w16[bi >> 1u])[bi & 1u]; }
            sb[r2 * 64u + c2] = vb;
        }
        workgroupBarrier();
        for (var kk = 0u; kk < 16u; kk++) {
            var ar: array<f32, 4>;
            var br: array<f32, 4>;
            for (var i = 0u; i < 4u; i++) {
                ar[i] = sa[(li.y * 4u + i) * 16u + kk];
                br[i] = sb[kk * 64u + li.x * 4u + i];
            }
            for (var i = 0u; i < 4u; i++) {
                for (var j = 0u; j < 4u; j++) { acc[i * 4u + j] += ar[i] * br[j]; }
            }
        }
        workgroupBarrier();
    }
    let flags = u(15u);
    let alpha = f(16u);
    let c_base = u(12u) + wg.z * u(13u);
    let c_rs = u(14u);
    let act = (flags >> 1u) & 3u;
    for (var i = 0u; i < 4u; i++) {
        for (var j = 0u; j < 4u; j++) {
            let gr = row0 + li.y * 4u + i;
            let gc = col0 + li.x * 4u + j;
            if (gr < m && gc < n) {
                var v = acc[i * 4u + j] * alpha;
                if ((flags & 1u) != 0u) { v += in2[gc]; }
                if (act == 1u) { v = gelu_(v); } else if (act == 2u) { v = max(v, 0.0); }
                let idx = c_base + gr * c_rs + gc;
                if ((flags & 8u) != 0u) { v += in3[idx]; }
                out0[idx] = v;
            }
        }
    }
}
