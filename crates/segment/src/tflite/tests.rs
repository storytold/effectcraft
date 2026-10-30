use super::*;

#[test]
fn same_padding_matches_tensorflow() {
    // TensorFlow pads the extra row after: 128 → 64 with a 3×3 stride-2 window pads 0 before.
    assert_eq!(same(128, 3, 2), (64, 0));
    assert_eq!(same(128, 5, 2), (64, 1));
    assert_eq!(same(16, 3, 1), (16, 1));
    assert_eq!(same(7, 2, 2), (4, 0));
    assert_eq!(window(1, 4, 2, 2), Ok((2, 0)));
    assert!(window(1, 1, 2, 2).is_err());
}

#[test]
fn asymmetric_conv_padding() {
    // A 3×3 stride-2 "same" conv on 4×4: pads nothing before, one after.
    let x: Vec<f32> = (0..16).map(|v| v as f32).collect();
    let ones = vec![1.0f32; 9];
    let c = Conv { wt: ones.clone(), b: vec![0.0], inp: 1, out: 1, ks: 3, stride: 2, pad: 0 };
    let y = c.forward_sized(&x, 4, 4, 2, 2, [0, 0]);
    // Top-left window rows 0–2, cols 0–2; the bottom-right one is cut by the edge.
    assert_eq!(y, vec![45.0, 39.0, 66.0, 50.0]);
    let d = Depthwise { w: ones, b: vec![0.0], c: 1, ks: 3, stride: 2 };
    assert_eq!(d.forward_sized(&x, 4, 4, 2, 2, [0, 0]), y);
}

#[test]
fn pool_pad_and_concat() {
    let x: Vec<f32> = (0..16).map(|v| v as f32).collect();
    assert_eq!(max_pool(&x, [4, 4], 1, [2, 2], [2, 2], [2, 2], [0, 0]), vec![5.0, 7.0, 13.0, 15.0]);
    // Same padding on 3×3 → 2×2: the last window is cut by the edge.
    let x3: Vec<f32> = (0..9).map(|v| v as f32).collect();
    assert_eq!(max_pool(&x3, [3, 3], 1, [2, 2], [2, 2], [2, 2], [0, 0]), vec![4.0, 5.0, 7.0, 8.0]);
    // Channel padding (residual connections widen the channels with zeros).
    let p = pad(&[1.0, 2.0, 3.0, 4.0], &[1, 1, 2, 2], &[[0, 0], [0, 0], [0, 0], [0, 1]]).unwrap();
    assert_eq!(p, vec![1.0, 2.0, 0.0, 3.0, 4.0, 0.0]);
    let p = pad(&[1.0, 2.0], &[1, 2], &[[0, 0], [1, 0]]).unwrap();
    assert_eq!(p, vec![0.0, 1.0, 2.0]);
    let a = [1.0, 2.0, 3.0, 4.0];
    let b = [5.0, 6.0];
    let (sa, sb) = (vec![1, 2, 2], vec![1, 1, 2]);
    assert_eq!(concat(&[(&a, &sa), (&b, &sb)], 1).unwrap(), vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    let (sa, sb) = (vec![2, 2], vec![2, 1]);
    assert_eq!(concat(&[(&a, &sa), (&b, &sb)], 1).unwrap(), vec![1.0, 2.0, 5.0, 3.0, 4.0, 6.0]);
}

#[test]
fn damaged_files_are_errors_not_panics() {
    assert!(Model::read(b"").is_err());
    assert!(Model::read(&[0xff; 64]).is_err());
    // Every prefix and a few corruptions of a hand-made buffer.
    let mut fb = vec![12, 0, 0, 0, 0, 0, 6, 0, 8, 0, 4, 0, 6, 0, 0, 0, 0, 0, 0, 0];
    for n in 0..fb.len() {
        let _ = Model::read(&fb[..n]);
    }
    for i in 0..fb.len() {
        fb[i] ^= 0x5a;
        let _ = Model::read(&fb);
    }
}

/// A FlatBuffer value, to build small models in tests.
enum V {
    Int(i32),
    Byte(u8),
    Table(Vec<(usize, V)>),
    Ints(Vec<i32>),
    Bytes(Vec<u8>),
    Tables(Vec<V>),
}

fn put_u32(b: &mut [u8], at: usize, v: usize) {
    b[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes());
}

/// Append `v` (children after their parent, so every offset points forward); where it starts.
fn put(b: &mut Vec<u8>, v: &V) -> usize {
    match v {
        V::Table(fields) => {
            let slots = fields.iter().map(|f| f.0 + 1).max().unwrap_or(0);
            let vt = b.len();
            let vt_len = 4 + 2 * slots;
            b.resize(vt + vt_len, 0);
            let t = b.len();
            // Every field gets 8 bytes (the reader does not need alignment).
            let t_len = 4 + 8 * fields.len();
            b.resize(t + t_len, 0);
            b[vt..vt + 2].copy_from_slice(&(vt_len as u16).to_le_bytes());
            b[vt + 2..vt + 4].copy_from_slice(&(t_len as u16).to_le_bytes());
            b[t..t + 4].copy_from_slice(&((t - vt) as i32).to_le_bytes());
            for (k, (id, f)) in fields.iter().enumerate() {
                let at = t + 4 + 8 * k;
                b[vt + 4 + 2 * id..vt + 6 + 2 * id].copy_from_slice(&((at - t) as u16).to_le_bytes());
                match f {
                    V::Int(i) => b[at..at + 4].copy_from_slice(&i.to_le_bytes()),
                    V::Byte(x) => b[at] = *x,
                    _ => {
                        let child = put(b, f);
                        put_u32(b, at, child - at);
                    }
                }
            }
            t
        }
        V::Ints(xs) => {
            let p = b.len();
            b.extend((xs.len() as u32).to_le_bytes());
            xs.iter().for_each(|x| b.extend(x.to_le_bytes()));
            p
        }
        V::Bytes(xs) => {
            let p = b.len();
            b.extend((xs.len() as u32).to_le_bytes());
            b.extend(xs);
            p
        }
        V::Tables(ts) => {
            let p = b.len();
            b.extend((ts.len() as u32).to_le_bytes());
            let slots = b.len();
            b.resize(slots + 4 * ts.len(), 0);
            for (i, t) in ts.iter().enumerate() {
                let child = put(b, t);
                put_u32(b, slots + 4 * i, child - (slots + 4 * i));
            }
            p
        }
        V::Int(_) | V::Byte(_) => b.len(),
    }
}

/// A tensor: shape, type and its constant data (`Some(k)`: buffer `k` of the model's).
type T = (Vec<i32>, u8, Option<usize>);
/// An operator: code, inputs, outputs and its options' fields.
type O = (i32, Vec<i32>, Vec<i32>, Vec<(usize, V)>);

/// A `.tflite` file with one graph.
fn model(bufs: Vec<Vec<u8>>, tensors: Vec<T>, ops: Vec<O>, inputs: Vec<i32>, outputs: Vec<i32>) -> Vec<u8> {
    let ts = tensors
        .into_iter()
        .enumerate()
        .map(|(i, (shape, ty, buf))| {
            let mut f = vec![(0, V::Ints(shape)), (1, V::Byte(ty)), (3, V::Bytes(format!("t{i}").into_bytes()))];
            if let Some(k) = buf {
                f.push((2, V::Int(k as i32 + 1)));
            }
            V::Table(f)
        })
        .collect();
    let mut codes = vec![];
    let os = ops
        .into_iter()
        .map(|(code, ins, outs, opts)| {
            let idx = codes.iter().position(|&c| c == code).unwrap_or_else(|| {
                codes.push(code);
                codes.len() - 1
            });
            V::Table(vec![(0, V::Int(idx as i32)), (1, V::Ints(ins)), (2, V::Ints(outs)), (4, V::Table(opts))])
        })
        .collect();
    let g = V::Table(vec![(0, V::Tables(ts)), (1, V::Ints(inputs)), (2, V::Ints(outputs)), (3, V::Tables(os))]);
    let buffers = std::iter::once(V::Table(vec![])).chain(bufs.into_iter().map(|d| V::Table(vec![(0, V::Bytes(d))]))).collect();
    let codes = codes.into_iter().map(|c| V::Table(vec![(3, V::Int(c))])).collect();
    let root = V::Table(vec![(1, V::Tables(codes)), (2, V::Tables(vec![g])), (4, V::Tables(buffers))]);
    let mut b = vec![0u8; 4];
    let r = put(&mut b, &root);
    put_u32(&mut b, 0, r);
    b
}

fn f32s(xs: &[f32]) -> Vec<u8> {
    xs.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Conv2DOptions / Pool2DOptions: padding (0 same, 1 valid) and stride.
fn window_opts(padding: u8, stride: i32) -> Vec<(usize, V)> {
    vec![(0, V::Byte(padding)), (1, V::Int(stride)), (2, V::Int(stride))]
}

fn refused(bytes: &[u8], why: &str) {
    let e = Model::read(bytes).err().unwrap_or_default();
    assert!(e.contains(why), "refused for `{e}`, expected `{why}`");
}

#[test]
fn built_models_load_and_run() {
    // A 3×3 stride-2 "same" conv on 4×4, then a RELU: as `asymmetric_conv_padding`.
    let m = model(
        vec![f32s(&[1.0; 9])],
        vec![(vec![1, 4, 4, 1], FLOAT32, None), (vec![1, 3, 3, 1], FLOAT32, Some(0)), (vec![1, 2, 2, 1], FLOAT32, None), (vec![1, 2, 2, 1], FLOAT32, None)],
        vec![(op::CONV_2D, vec![0, 1, -1], vec![2], window_opts(0, 2)), (op::RELU, vec![2], vec![3], vec![])],
        vec![0],
        vec![3],
    );
    let m = Model::read(&m).unwrap();
    let c = m.cost().unwrap();
    assert_eq!((c.macs, c.values, c.scratch, c.kernel), (2 * 2 * 9, 8, 2 * 9, 3));
    // The input, the conv's output and its patch row, alive together.
    assert_eq!(c.live, 16 + 4 + 2 * 9);
    let x: Vec<f32> = (0..16).map(|v| v as f32).collect();
    assert_eq!(m.run(&[&x]).unwrap(), vec![vec![45.0, 39.0, 66.0, 50.0]]);
}

/// #368: an 8 KB model whose CONV_2D needed a 512 GB scratch buffer aborted `run`.
#[test]
fn convolution_scratch_is_bounded() {
    let w = 1 << 25;
    let bomb = model(
        vec![vec![0; 2 * 64 * 64]],
        vec![(vec![1, 1, w, 1], FLOAT32, None), (vec![1, 64, 64, 1], FLOAT16, Some(0)), (vec![1, 1, w, 1], FLOAT32, None)],
        vec![(op::CONV_2D, vec![0, 1, -1], vec![2], window_opts(0, 1))],
        vec![0],
        vec![2],
    );
    refused(&bomb, "kernel is too large");
    // A small kernel over many channels: the patch row is `ow·3·3·cin`.
    let (w, cin) = (1 << 20, 256);
    let wide = model(
        vec![vec![0; 2 * 9 * 256]],
        vec![(vec![1, 1, w, cin], FLOAT32, None), (vec![1, 3, 3, cin], FLOAT16, Some(0)), (vec![1, 1, w, 1], FLOAT32, None)],
        vec![(op::CONV_2D, vec![0, 1, -1], vec![2], window_opts(0, 1))],
        vec![0],
        vec![2],
    );
    refused(&wide, "working space is too large");
}

#[test]
fn kernel_sides_are_bounded() {
    let k = MAX_KERNEL as i32 + 1;
    let conv = model(
        vec![vec![0; 2 * (k * k) as usize]],
        vec![(vec![1, 40, 40, 1], FLOAT32, None), (vec![1, k, k, 1], FLOAT16, Some(0)), (vec![1, 40, 40, 1], FLOAT32, None)],
        vec![(op::CONV_2D, vec![0, 1, -1], vec![2], window_opts(0, 1))],
        vec![0],
        vec![2],
    );
    refused(&conv, "kernel is too large");
    let depthwise = model(
        vec![vec![0; 2 * (k * k) as usize]],
        vec![(vec![1, 40, 40, 1], FLOAT32, None), (vec![1, k, k, 1], FLOAT16, Some(0)), (vec![1, 40, 40, 1], FLOAT32, None)],
        vec![(op::DEPTHWISE_CONV_2D, vec![0, 1, -1], vec![2], window_opts(0, 1))],
        vec![0],
        vec![2],
    );
    refused(&depthwise, "kernel is too large");
    // A "same" pool takes any window: a huge one looped for billions of steps per output.
    let mut opts = window_opts(0, 1);
    opts.extend([(3, V::Int(i32::MAX)), (4, V::Int(i32::MAX))]);
    let pool = model(
        vec![],
        vec![(vec![1, 8, 8, 1], FLOAT32, None), (vec![1, 8, 8, 1], FLOAT32, None)],
        vec![(op::MAX_POOL_2D, vec![0], vec![1], opts)],
        vec![0],
        vec![1],
    );
    refused(&pool, "pooling window is too large");
}

#[test]
fn multiply_adds_are_bounded() {
    // Three 16 → 16 pointwise convolutions over 4096×4096: 3·2³² multiply-adds.
    let s = vec![1, 4096, 4096, 16];
    let t = |buf| (s.clone(), FLOAT32, buf);
    let m = model(
        vec![vec![0; 2 * 16 * 16]],
        vec![t(None), (vec![16, 1, 1, 16], FLOAT16, Some(0)), t(None), t(None), t(None)],
        (0..3).map(|i| (op::CONV_2D, vec![if i == 0 { 0 } else { i + 1 }, 1, -1], vec![i + 2], window_opts(0, 1))).collect(),
        vec![0],
        vec![4],
    );
    refused(&m, "too many multiply-adds");
}

/// #368: 2000 RELUs over 2²⁵ values loaded fine, and one run took minutes.
#[test]
fn elementwise_work_is_bounded() {
    let n = 2000;
    let m = model(
        vec![],
        (0..=n).map(|_| (vec![1, 1 << 25], FLOAT32, None)).collect(),
        (0..n).map(|i| (op::RELU, vec![i], vec![i + 1], vec![])).collect(),
        vec![0],
        vec![n],
    );
    refused(&m, "too much work");
}

#[test]
fn memory_alive_at_once_is_bounded() {
    // Five outputs of 2²⁷ values each, all kept, besides the input.
    let m = model(
        vec![],
        (0..6).map(|_| (vec![1, 1 << 27], FLOAT32, None)).collect(),
        (1..6).map(|i| (op::RELU, vec![0], vec![i], vec![])).collect(),
        vec![0],
        (1..6).collect(),
    );
    refused(&m, "too much memory");
}

#[test]
fn shapes_computed_at_run_time_must_match_the_declared_ones() {
    // PAD to 2³⁰ × 2³⁰ while declaring a 1×1 output.
    let pads = vec![0, 1 << 30, 0, 1 << 30].into_iter().flat_map(i32::to_le_bytes).collect();
    let pad = model(
        vec![pads],
        vec![(vec![1, 1], FLOAT32, None), (vec![2, 2], INT32, Some(0)), (vec![1, 1], FLOAT32, None)],
        vec![(op::PAD, vec![0, 1], vec![2], vec![])],
        vec![0],
        vec![2],
    );
    refused(&pad, "padding disagrees");
    // Many large parts concatenated into a small declared output.
    let concat = model(
        vec![],
        vec![(vec![1, 1 << 27], FLOAT32, None), (vec![1, 2], FLOAT32, None)],
        vec![(op::CONCATENATION, vec![0; 64], vec![1], vec![(0, V::Int(1))])],
        vec![0],
        vec![1],
    );
    refused(&concat, "parts disagree");
    // A pool whose output claims more channels than its input has.
    let pool = model(
        vec![],
        vec![(vec![1, 2, 2, 1], FLOAT32, None), (vec![1, 1, 1, 1 << 20], FLOAT32, None)],
        vec![(op::MAX_POOL_2D, vec![0], vec![1], window_opts(1, 2).into_iter().chain([(3, V::Int(2)), (4, V::Int(2))]).collect())],
        vec![0],
        vec![1],
    );
    refused(&pool, "changes the channels");
    // Dimensions may not hide behind a zero.
    let zero = model(vec![], vec![(vec![0, 1 << 30, 1 << 30], FLOAT32, None)], vec![], vec![0], vec![0]);
    refused(&zero, "tensor is too large");
}

#[test]
fn shared_weights_count_once_per_use() {
    // One 128 KB buffer decoded for 2100 tensors would be 2100 copies of 2¹⁶ floats.
    let m = model(vec![vec![0; 2 << 16]], (0..2100).map(|_| (vec![1, 1 << 16], FLOAT16, Some(0))).collect(), vec![], vec![], vec![]);
    refused(&m, "weights are too large");
}

/// The official face models (`EFFECTCRAFT_FACE_LANDMARKER` = path to `face_landmarker.task`)
/// fit every budget with wide headroom.
#[test]
fn official_face_models_fit_the_budgets() {
    let Some(path) = std::env::var_os("EFFECTCRAFT_FACE_LANDMARKER") else { return };
    let bytes = std::fs::read(path).unwrap();
    for name in [crate::mediapipe::DETECTOR, crate::mediapipe::MESH] {
        let c = Model::read(&crate::pt::zip_file(&bytes, name).unwrap()).unwrap().cost().unwrap();
        println!("{name}: {c:?}");
        assert!(c.macs <= MAX_MACS / 8 && c.values <= MAX_VALUES / 8 && c.live <= MAX_LIVE / 8 && c.scratch <= MAX_SCRATCH / 8, "{name}: {c:?}");
        assert!(c.kernel <= MAX_KERNEL as u64 / 4, "{name}: {c:?}");
    }
}
