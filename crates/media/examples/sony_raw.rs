//! Local sensor/footage verification. Never commit the input photographs or output previews.
//! `cargo run -p effectcraft-media --example sony_raw -- OUT_DIR PHOTO.ARW ...`

use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use effectcraft_media::{MediaPool, probe};
use effectcraft_time::Tick;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let out = PathBuf::from(args.next().ok_or("usage: sony_raw OUT_DIR PHOTO.ARW ...")?);
    std::fs::create_dir_all(&out)?;
    for arg in args {
        let path = Path::new(&arg);
        // Apply the production file, metadata, dimensions and sensor-range limits before
        // inspecting decoder facts, even in this local verification helper.
        probe(path)?;
        let bytes = std::fs::read(path)?;
        let header = lightcraft_raw::probe_info(&bytes)?;
        let sensor = lightcraft_raw::decode(&bytes)?;
        let mut camera_color = sensor.color.clone();
        let calibrated = sensor.metadata.model.as_deref().and_then(|m| lightcraft_raw::spectral::find(sensor.metadata.make.as_deref(), m));
        if let Some(camera) = calibrated {
            camera.fill(&mut camera_color);
        }
        let values: Vec<f32> = (0..sensor.data.len()).step_by(1024).map(|k| sensor.data.get(k)).collect();
        let facts = serde_json::json!({
            "file": path.file_name().map(|s| s.to_string_lossy()),
            "model": header.metadata.model,
            "sensor": [sensor.width, sensor.height], "developed": header.developed_size(),
            "bits": header.bits, "orientation": header.orientation.to_exif(),
            "cfa": sensor.cfa.as_ref().map(|c| c.name()), "samples": sensor.data.len(),
            "black": sensor.black.mean(), "white": sensor.white, "asShotWb": sensor.wb_multipliers,
            "sampleMin": values.iter().copied().fold(f32::INFINITY, f32::min),
            "sampleMax": values.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            "measuredCameraProfile": calibrated.is_some(),
        });
        drop(sensor);
        drop(bytes);
        let start = Instant::now();
        let footage = probe(path)?;
        let pool = MediaPool::new();
        let frame = pool.frame_at(&footage, Tick::ZERO)?;
        let again = pool.frame_at(&footage, Tick::ZERO)?;
        let finite = frame.data.iter().flatten().all(|v| v.is_finite());
        let thumb = pool.thumbnail(&footage, 1280)?;
        let rgba: Vec<u8> = thumb.data.iter().flatten().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
        let name = path.file_stem().ok_or("input has no filename")?.to_string_lossy();
        image::save_buffer(out.join(format!("{name}-sensor.png")), &rgba, thumb.width, thumb.height, image::ColorType::Rgba8)?;
        let mut project = effectcraft_project::Project::default();
        project.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
        let mut comp = effectcraft_project::Comp::new(footage.width, footage.height, effectcraft_time::FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
        let item = project.add_item("RAW", Default::default(), None, effectcraft_project::ItemKind::Footage(footage.clone()));
        let layer = effectcraft_project::build::layer(
            &mut project,
            &comp,
            "RAW",
            effectcraft_project::LayerSource::Footage { item },
            (footage.width, footage.height),
            None,
        );
        comp.layers.push(layer);
        let cid = project.add_item("RAW verification", Default::default(), None, effectcraft_project::ItemKind::Comp(comp.into()));
        let opts = effectcraft_render::RenderOpts { scale: 1280.0 / footage.width.max(footage.height) as f64, ..Default::default() };
        let rendered = effectcraft_render::Renderer::new(&project, &pool, opts).comp_frame(cid, Tick::ZERO);
        let comp_rgba: Vec<u8> = rendered.data.iter().flatten().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
        image::save_buffer(out.join(format!("{name}-comp.png")), &comp_rgba, rendered.width, rendered.height, image::ColorType::Rgba8)?;
        writeln!(
            std::io::stdout().lock(),
            "{}",
            serde_json::json!({"sensor": facts, "footage": [frame.width, frame.height], "finite": finite,
            "cacheReused": std::sync::Arc::ptr_eq(&frame, &again), "seconds": start.elapsed().as_secs_f64(),
            "composition": [rendered.width, rendered.height], "compositionFinite": rendered.data.iter().flatten().all(|v| v.is_finite()),
            "compositionVisible": rendered.data.iter().any(|c| c[3] > 0.9 && c[0] > 0.1),
            "floatMin": frame.data.iter().flatten().copied().fold(f32::INFINITY, f32::min),
            "floatMax": frame.data.iter().flatten().copied().fold(f32::NEG_INFINITY, f32::max)})
        )?;
    }
    Ok(())
}
