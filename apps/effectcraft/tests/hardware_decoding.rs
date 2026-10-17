//! Settings ▸ Import ▸ Enable Hardware Accelerated Decoding with the decoders the app registers
//! (`filmcraft-platform`): off decodes in software, codecs the hardware does not take keep the
//! software decoders, and where this machine has a hardware decoder its frames match software's.
//!
//! Fixtures come from ffmpeg (an external generator, never linked) into `target/fixtures/hw-decode`;
//! the tests skip when ffmpeg is missing, and the hardware smoke test skips without a hardware
//! decoder. The switch is process-wide, so the tests take turns (`SERIAL`) and live in their own
//! test binary.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, PoisonError};

use effectcraft_media::{MediaPool, probe};
use effectcraft_raster::Image;
use effectcraft_render::FootageSource;

static SERIAL: Mutex<()> = Mutex::new(());

fn fixture(name: &str, args: &[&str]) -> Option<PathBuf> {
    if !Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("SKIPPED: ffmpeg not found");
        return None;
    }
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/hw-decode").join(name);
    if out.exists() {
        return Some(out);
    }
    std::fs::create_dir_all(out.parent()?).ok()?;
    let tmp = out.with_file_name(format!("tmp-{}-{name}", std::process::id()));
    let ok = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).arg(&tmp).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("SKIPPED: ffmpeg could not make {name}");
        return None;
    }
    std::fs::rename(&tmp, &out).ok()?;
    Some(out)
}

/// H.264 High with B-frames and two GOPs (reordering and a seek).
fn h264() -> Option<PathBuf> {
    fixture(
        "h264_high.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=640x360:r=24:d=2,noise=alls=12:allf=t",
            "-c:v",
            "libx264",
            "-profile:v",
            "high",
            "-x264-params",
            "bframes=3:keyint=24:min-keyint=24:scenecut=0",
            "-pix_fmt",
            "yuv420p",
        ],
    )
}

/// VP9: no hardware decoder registered on Linux takes it.
fn vp9() -> Option<PathBuf> {
    fixture("vp9.webm", &["-f", "lavfi", "-i", "testsrc2=s=320x240:r=24:d=1", "-c:v", "libvpx-vp9", "-b:v", "500k", "-pix_fmt", "yuv420p"])
}

/// A pool without read-ahead: no decode may outlive its test (the hardware counters are
/// process-wide).
fn quiet_pool() -> MediaPool {
    let p = MediaPool::new();
    p.set_read_ahead(false);
    p
}

fn frames(pool: &MediaPool, path: &Path, n: i64) -> Vec<std::sync::Arc<Image>> {
    let f = probe(path.to_str().unwrap()).unwrap();
    (0..n).map(|i| pool.frame_at(&f, f.frame_rate.tick_of(i)).unwrap()).collect()
}

fn psnr(a: &Image, b: &Image) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let se: f64 = a.data.iter().zip(&b.data).flat_map(|(p, q)| (0..4).map(move |k| ((p[k] - q[k]) as f64).powi(2))).sum();
    let mse = se / (a.data.len() * 4) as f64;
    if mse == 0.0 { f64::INFINITY } else { 10.0 * (1.0 / mse).log10() }
}

fn hw_frames(pool: &MediaPool) -> u64 {
    pool.hardware_decoding().unwrap().frames
}

#[test]
fn off_decodes_everything_in_software() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(path) = h264() else { return };
    let pool = quiet_pool();
    pool.set_hardware_decoding(false);
    let available = filmcraft_platform::register();
    assert_eq!(filmcraft_platform::register(), available, "registering twice is harmless");
    let before = hw_frames(&pool);
    assert_eq!(frames(&pool, &path, 30).len(), 30);
    let status = pool.hardware_decoding().unwrap();
    assert!(!status.enabled);
    assert_eq!(status.frames, before, "off: no hardware frames ({available:?})");
    assert!(status.summary().starts_with("software"), "{}", status.summary());
    pool.set_hardware_decoding(true);
}

#[test]
fn codecs_the_hardware_does_not_take_stay_in_software() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(path) = vp9() else { return };
    filmcraft_platform::register();
    let (hw, sw) = (quiet_pool(), quiet_pool());
    hw.set_hardware_decoding(true);
    let before = hw_frames(&hw);
    let a = frames(&hw, &path, 12);
    assert_eq!(hw_frames(&hw), before, "VP9 is not decoded in hardware");
    sw.set_hardware_decoding(false);
    let b = frames(&sw, &path, 12);
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.data, y.data, "same software decoder, same pixels");
    }
    sw.set_hardware_decoding(true);
}

/// Where this machine has a hardware H.264 decoder: it takes the stream, its frames match the
/// software decoder's, and switching the setting off reopens the footage in software.
#[test]
fn hardware_decode_smoke() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(path) = h264() else { return };
    let available = filmcraft_platform::register();
    if let filmcraft_platform::Availability::Unavailable(why) = available {
        eprintln!("SKIPPED: no hardware decoder ({why})");
        return;
    }
    let pool = quiet_pool();
    pool.set_hardware_decoding(true);
    let before = pool.hardware_decoding().unwrap();
    let hw = frames(&pool, &path, 48);
    let after = pool.hardware_decoding().unwrap();
    if after.sessions == before.sessions {
        eprintln!("SKIPPED: {available:?} declined this H.264 stream (declined {} → {})", before.declined, after.declined);
        return;
    }
    assert!(after.frames >= before.frames + 48, "{before:?} → {after:?}");
    assert!(after.summary().contains("on"), "{}", after.summary());

    let sw_pool = quiet_pool();
    pool.set_hardware_decoding(false);
    let sw = frames(&sw_pool, &path, 48);
    for (i, (a, b)) in hw.iter().zip(&sw).enumerate() {
        let db = psnr(a, b);
        assert!(db > 60.0, "frame {i}: hardware vs software {db:.1} dB");
    }
    // Off closed the pool's decoders: what it decodes from now on comes from software.
    pool.clear_frames();
    let n0 = hw_frames(&pool);
    assert_eq!(frames(&pool, &path, 12).len(), 12);
    assert_eq!(hw_frames(&pool), n0, "off: no hardware frames after the switch");
    pool.set_hardware_decoding(true);
}
