//! Verify the real frontend command path without opening a window or modifying photographs.
//! `cargo run -p effectcraft-host --example sony_raw_import -- OUT_DIR PHOTO.ARW ...`

use std::error::Error;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use effectcraft_engine::project::{FootageKind, ItemId, ItemKind};
use effectcraft_time::Tick;
use serde_json::json;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let out = PathBuf::from(args.next().ok_or("usage: sony_raw_import OUT_DIR PHOTO.ARW ...")?);
    std::fs::create_dir_all(&out)?;
    for arg in args {
        // Release the previous photograph's large cache before verifying another one.
        let mut s = effectcraft_host::session();
        let start = Instant::now();
        let path = arg.to_str().ok_or("input path is not UTF-8")?;
        let imported = s.execute("file.import", json!({"paths": [path], "background": false}))?;
        let id = imported.get("items").and_then(|v| v.as_array()).and_then(|v| v.first()).and_then(|v| v.as_u64()).ok_or("no imported item")?;
        let item = s.project.item(ItemId(id)).ok_or("imported item missing")?;
        let ItemKind::Footage(f) = &item.kind else { return Err("not footage".into()) };
        if f.kind != FootageKind::Still || !f.codec.starts_with("Sony ARW sensor") {
            return Err("not sensor RAW still footage".into());
        }
        let (w, h) = (f.width, f.height);
        // A visible composition alone could also be a missing-footage placeholder. Require
        // the session's real footage source to return the finite, full-resolution RAW frame.
        let frame = s.footage.frame(ItemId(id), f, Tick::ZERO).ok_or("session could not decode the sensor frame")?;
        if (frame.width, frame.height) != (w, h) || frame.data.iter().flatten().any(|v| !v.is_finite()) {
            return Err("invalid session sensor frame".into());
        }
        let again = s.footage.frame(ItemId(id), f, Tick::ZERO).ok_or("session could not reuse the sensor frame")?;
        let cache_reused = Arc::ptr_eq(&frame, &again);
        if !cache_reused {
            return Err("session did not reuse the decoded sensor frame".into());
        }
        let comp = s.execute("comp.new", json!({"name": "RAW verification", "width": w, "height": h, "frameRate": 30, "duration": 1}))?;
        let cid = comp.get("comp").and_then(|v| v.as_u64()).ok_or("no composition")?;
        s.execute("layer.addItem", json!({"comp": cid, "item": id}))?;
        let (rw, rh, rgba) = s.render_rgba8(ItemId(cid), Tick::ZERO, 1280)?;
        let visible = rgba.as_chunks::<4>().0.iter().any(|c| c[0] > 16 || c[1] > 16 || c[2] > 16);
        if !visible {
            return Err("RAW layer rendered an empty/black image".into());
        }
        let name = PathBuf::from(path).file_stem().ok_or("no filename")?.to_string_lossy().to_string();
        image::save_buffer(out.join(format!("{name}-engine.png")), &rgba, rw, rh, image::ColorType::Rgba8)?;
        writeln!(
            std::io::stdout().lock(),
            "{}",
            json!({"file": name, "import": imported, "footage": [w, h], "composition": [rw, rh],
                "visible": visible, "finite": true, "cacheReused": cache_reused, "seconds": start.elapsed().as_secs_f64()})
        )?;
    }
    Ok(())
}
