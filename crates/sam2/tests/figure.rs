use effectcraft_sam2::model::ImageModel;
use effectcraft_sam2::prompt::Prompt;
use effectcraft_sam2::weights::Weights;

fn figure(w: usize, h: usize) -> (Vec<[f32; 3]>, Vec<bool>) {
    let mut rgb = vec![[0f32; 3]; w * h];
    let mut gt = vec![false; w * h];
    let part = |x: f32, y: f32| -> Option<[f32; 3]> {
        let (cx, _cy) = (128.0, 0.0);
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| x >= x0 && x < x1 && y >= y0 && y < y1;
        if (x - cx).powi(2) + (y - 40.0).powi(2) < 18.0f32.powi(2) {
            return Some([0.8, 0.6, 0.5]);
        } // head
        if r(104.0, 60.0, 152.0, 130.0) {
            return Some([0.2, 0.3, 0.7 + 0.1 * (x / 7.0).sin()]);
        } // torso
        if r(76.0, 62.0, 104.0, 74.0) || r(152.0, 62.0, 180.0, 74.0) {
            return Some([0.8, 0.6, 0.5]);
        } // arms
        if r(108.0, 130.0, 124.0, 210.0) || r(132.0, 130.0, 148.0, 210.0) {
            return Some([0.15, 0.15, 0.2]);
        } // legs
        if r(102.0, 210.0, 126.0, 224.0) || r(130.0, 210.0, 154.0, 224.0) {
            return Some([0.95, 0.95, 0.95]);
        } // white shoes
        None
    };
    for y in 0..h {
        for x in 0..w {
            let bg = 0.45 + 0.1 * (x as f32 / 11.0).sin() * (y as f32 / 9.0).cos();
            let p = part(x as f32, y as f32);
            rgb[y * w + x] = p.unwrap_or([bg, bg * 0.9, bg * 0.8]);
            gt[y * w + x] = p.is_some();
        }
    }
    (rgb, gt)
}

fn iou(p: &[f32], gt: &[bool]) -> f64 {
    let (mut i, mut u) = (0usize, 0usize);
    for (a, b) in p.iter().zip(gt) {
        let a = *a > 0.0;
        i += usize::from(a && *b);
        u += usize::from(a || *b);
    }
    i as f64 / u.max(1) as f64
}

/// A multi-part figure (torso, head, legs, arms, white shoes in different colours): clicks alone make
/// SAM 2 pick the torso and head, a box around the figure makes it cover all of it. This is why Roto
/// Brush prompts the tracker with a box from the classic engine's matte (found on real footage, where
/// the stroke frame came out partial).
#[test]
fn a_box_makes_sam2_cover_every_part_of_the_object() {
    let wf = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sam2-tiny/model.safetensors");
    let wf = std::env::var_os("EFFECTCRAFT_SAM2_DIR").map_or(wf, |d| std::path::PathBuf::from(d).join("model.safetensors"));
    if !wf.exists() {
        eprintln!("skipped: no SAM 2 weights");
        return;
    }
    let model = ImageModel::load(&Weights::open(&wf).unwrap()).unwrap();
    let (w, h) = (256usize, 256usize);
    let (rgb, gt) = figure(w, h);
    let enc = model.encode(&effectcraft_sam2::mask::preprocess(&rgb, w, h).unwrap()).unwrap();
    let s = 4.0f32;
    let clicks = vec![([128.0 * s, 75.0 * s], true), ([128.0 * s, 100.0 * s], true), ([128.0 * s, 125.0 * s], true), ([20.0 * s, 20.0 * s], false)];
    let mask = |p: &Prompt| -> (f64, f32) {
        let out = model.segment(&enc, p, false).unwrap();
        let m = out.masks.narrow(1, 0, 1).unwrap().upsample_bilinear2d(h, w, false).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
        (iou(&m, &gt), out.iou.flatten_all().unwrap().to_vec1::<f32>().unwrap()[0])
    };
    let (points_only, _) = mask(&Prompt { points: clicks.clone(), bbox: None });
    let (boxed, predicted) = mask(&Prompt { points: clicks, bbox: Some([70.0 * s, 18.0 * s, 186.0 * s, 228.0 * s]) });
    eprintln!("clicks only: IoU {points_only:.2}; with the box: IoU {boxed:.2} (predicted quality {predicted:.2})");
    assert!(points_only < 0.6, "the clicks-only failure this test documents is gone ({points_only}): revisit the box prompt");
    assert!(boxed > 0.95 && predicted > 0.7, "boxed IoU {boxed}, predicted {predicted}");
}
