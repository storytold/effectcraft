#![cfg(target_os = "android")]

//! Startup self test: run the embedded clip and still through EffectCraft's **own** media layer
//! (`effectcraft-media` → `filmcraft-codecs`), then import the clip into a composition and render a
//! frame, and report what came out.
//!
//! EffectCraft 0.4.0 has no hardware codec backend — there is no `crates/platform` and no
//! `register()` seam (unlike FilmCraft's MediaCodec backend) — so what is worth proving on a new
//! architecture is the software spine the whole app sits on: container probe, video decode, the
//! media pool's frame path, `file.import`, and the CPU renderer producing real pixels. Everything
//! below goes through public APIs the app itself uses; logcat carries the result.

use std::path::Path;
use std::sync::Arc;

use effectcraft_engine::project::ItemId;
use effectcraft_engine::render::RenderOpts;
use effectcraft_engine::time::Tick;
use effectcraft_media::MediaPool;
use serde_json::json;

/// ffmpeg-generated assets: 320x240, 25 fps, 1 s, H.264 (`test.mp4`) and a 64x64 PNG still.
const CLIP: &[u8] = include_bytes!("../assets/test.mp4");
const STILL: &[u8] = include_bytes!("../assets/test.png");

/// Decode at most this many frames of the clip (the clip has 25).
const MAX_FRAMES: usize = 40;

/// Decode the clip through the media pool — the same frame path the app's viewers use — and report
/// the picture count, the frame size and a luma sample (so a black or empty decode is visible).
fn decode_clip(name: &str, bytes: &[u8]) -> String {
    let pool = MediaPool::new();
    pool.add_bytes(name, Arc::from(bytes));
    let footage = match effectcraft_media::probe_bytes(name, Arc::from(bytes)) {
        Ok(f) => f,
        Err(e) => return format!("probe 失败: {e}"),
    };
    let (num, den) = (footage.frame_rate.num.max(1), footage.frame_rate.den.max(1));
    let mut decoded = 0usize;
    let mut size = String::new();
    let (mut luma, mut samples) = (0.0f64, 0usize);
    for i in 0..MAX_FRAMES {
        let t = Tick::from_seconds_f64(i as f64 * den as f64 / num as f64);
        if i > 0 && t >= footage.duration {
            break;
        }
        match pool.frame_at(&footage, t) {
            Ok(img) => {
                if decoded == 0 {
                    size = format!("{}x{}", img.width, img.height);
                }
                let step = (img.data.len() / 64).max(1);
                for px in img.data.iter().step_by(step) {
                    luma += (px[0] + px[1] + px[2]) as f64 / 3.0;
                    samples += 1;
                }
                decoded += 1;
            }
            Err(e) => return format!("解码第 {i} 帧失败: {e}"),
        }
    }
    let mean = if samples == 0 { 0.0 } else { luma / samples as f64 };
    format!(
        "{name} {} {} fps，时长 {:.2}s，编解码 {}{} → 解出 {decoded} 帧（{}），采样亮度 {mean:.3}",
        footage.codec,
        num as f64 / den as f64,
        footage.duration.seconds(),
        if footage.has_video { "，含视频" } else { "" },
        if footage.has_audio { "，含音频" } else { "" },
        if size.is_empty() { "无".to_string() } else { size },
    )
}

/// Import the clip with the *engine* command (`file.import`, what the desktop's File ▸ Import runs
/// once the paths are known), place it in a composition and render a frame: the whole spine —
/// probe → project item → layer → renderer — on this device.
fn render_report(path: &Path) -> String {
    let mut session = effectcraft_host::session();
    let path = path.to_string_lossy().to_string();
    let imported = match session.execute("file.import", json!({"paths": [path]})) {
        Ok(v) => v,
        Err(e) => return format!("file.import 失败: {e}"),
    };
    let Some(item) = imported.get("items").and_then(|v| v.as_array()).and_then(|a| a.first()).and_then(serde_json::Value::as_u64) else {
        return format!("file.import 没有返回素材: {imported}");
    };
    let comp = match session.execute("comp.new", json!({"name": "SelfTest", "width": 320, "height": 240, "duration": 1.0, "frameRate": 25})) {
        Ok(v) => v.get("comp").and_then(serde_json::Value::as_u64),
        Err(e) => return format!("comp.new 失败: {e}"),
    };
    let Some(comp) = comp else { return "comp.new 没有返回合成".to_string() };
    if let Err(e) = session.execute("layer.addItem", json!({"item": item})) {
        return format!("layer.addItem 失败: {e}");
    }
    let img = session.render(ItemId(comp), Tick::from_seconds_f64(0.5), RenderOpts::default());
    let covered = img.data.iter().filter(|p| p[3] > 0.5).count();
    format!("file.import → 素材 {item} → 合成 320x240 → 渲染 {}x{}，{covered} 个不透明像素", img.width, img.height)
}

/// A still through the same probe (the image path, not the video one).
fn still_report(bytes: &[u8]) -> String {
    match effectcraft_media::probe_bytes("test.png", Arc::from(bytes)) {
        Ok(f) => format!("静帧 test.png {}x{}（{}）", f.width, f.height, f.codec),
        Err(e) => format!("静帧 probe 失败: {e}"),
    }
}

/// Write the embedded assets into the app's own directory and run the checks; called once at
/// startup from `android_main`, on its own thread.
pub fn run(data_dir: &Path) -> String {
    let dir = data_dir.join("selftest");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return format!("自检: 无法创建 {}: {e}", dir.display());
    }
    let clip = dir.join("test.mp4");
    if let Err(e) = std::fs::write(&clip, CLIP) {
        return format!("自检: 无法写入 {}: {e}", clip.display());
    }
    let mut out = format!("素材自检: {}", decode_clip("test.mp4", CLIP));
    out.push_str(" ／ ");
    out.push_str(&still_report(STILL));
    out.push_str(" ／ 引擎自检: ");
    out.push_str(&render_report(&clip));
    out
}
