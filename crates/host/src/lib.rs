//! A fully wired [`Session`]: footage decoding through `effectcraft-media` (FilmCraft's codecs),
//! the media importer, the expression engine, JavaScript scripting (`effectcraft-script`) and
//! Render Queue export through `effectcraft-export` (FilmCraft's encoders). Frontends (desktop,
//! CLI, MCP, web) start here.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::Arc;

use effectcraft_engine::project::render_queue::OutputFormat;
use effectcraft_engine::{ExportJob, ExportResult, Exporter, Importer, Session};
use effectcraft_project::Footage;
pub use effectcraft_script as script;

pub mod app_dirs;

struct MediaImporter;

impl Importer for MediaImporter {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        effectcraft_media::probe(path).map_err(|e| e.to_string())
    }
}

/// Render Queue export via `effectcraft-export`: to the file system, or to `sink` (the web app
/// turns written files into downloads).
#[derive(Default)]
pub struct FileExporter {
    pub sink: Option<Arc<effectcraft_export::Sink>>,
}

impl FileExporter {
    /// The `effectcraft-export` job of a queue item's output module.
    fn job<'a>(&'a self, job: &'a ExportJob<'a>) -> effectcraft_export::Job<'a> {
        effectcraft_export::Job {
            project: job.project,
            footage: job.footage,
            expr: job.expr,
            accel: job.accel,
            comp: job.item.comp,
            settings: &job.item.settings,
            output: &job.item.output,
            path: job.path,
            sink: self.sink.as_deref(),
            options: effectcraft_export::JobOptions {
                log: job.item.log,
                label: job.label.clone(),
                storage: job.storage,
                overflow: job.project.render_prefs.overflow_folders.clone(),
                memory_budget: job.memory_budget,
            },
            nested_switches: job.nested_switches,
        }
    }
}

/// An export's result as the Render Queue reports it (a stop is [`CANCELLED`](effectcraft_engine::render_queue::CANCELLED)).
fn result(r: effectcraft_export::Result<effectcraft_export::Report>) -> Result<ExportResult, String> {
    match r {
        Ok(r) => Ok(ExportResult {
            path: r.path,
            frames: r.frames,
            width: r.width,
            height: r.height,
            bytes: r.bytes,
            seconds: r.seconds,
            audio: r.audio,
            log: r.log,
            overflow: r.overflow,
        }),
        Err(effectcraft_export::ExportError::Cancelled) => Err(effectcraft_engine::render_queue::CANCELLED.into()),
        Err(e) => Err(e.to_string()),
    }
}

impl Exporter for FileExporter {
    fn formats(&self) -> Vec<OutputFormat> {
        effectcraft_export::available_formats()
    }
    fn export(&self, job: &ExportJob, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
        effectcraft_engine::render::passes::block_on(self.export_async(job, progress))
    }

    fn export_async<'a>(
        &'a self,
        job: &'a ExportJob<'a>,
        progress: &'a mut dyn FnMut(u64, u64) -> bool,
    ) -> effectcraft_engine::render_queue::LocalFuture<'a, Result<ExportResult, String>> {
        Box::pin(async move { result(effectcraft_export::export_async(&self.job(job), &mut |p| progress(p.done, p.total)).await) })
    }

    /// Modules that render the same frames export at once from one render of each frame
    /// ([`effectcraft_export::export_modules_async`]).
    fn export_modules_async<'a>(
        &'a self,
        jobs: &'a [ExportJob<'a>],
        progress: &'a mut dyn FnMut(usize, u64, u64) -> bool,
    ) -> effectcraft_engine::render_queue::LocalFuture<'a, Vec<Result<ExportResult, String>>> {
        Box::pin(async move {
            let js: Vec<effectcraft_export::Job> = jobs.iter().map(|j| self.job(j)).collect();
            effectcraft_export::export_modules_async(&js, &mut |mi, p| progress(mi, p.done, p.total)).await.into_iter().map(result).collect()
        })
    }
}

/// A new session with media, import, expressions, scripting and export enabled.
pub fn session() -> Session {
    Session {
        // The desktop app, the CLI and the MCP server share installed Roto Brush models.
        models_dir: config_dir().map(|d| d.join("models")),
        exporter: Some(Arc::new(FileExporter::default())),
        footage: Arc::new(effectcraft_media::MediaPool::new()),
        importer: Some(Arc::new(MediaImporter)),
        expr: Some(Arc::new(effectcraft_expr::Expressions)),
        expr_check: Some(effectcraft_expr::check_syntax),
        script: Some(effectcraft_script::runner),
        script_ui: effectcraft_engine::scriptui::ScriptUi { dispatch: Some(effectcraft_script::dispatch_ui), ..Default::default() },
        plugin_loader: effectcraft_plugin::wasm_available().then_some(effectcraft_plugin::loader as effectcraft_engine::PluginLoader),
        ..Default::default()
    }
}

/// The platform config directory for EffectCraft (`EFFECTCRAFT_CONFIG_DIR` overrides):
/// `~/Library/Application Support/EffectCraft` (macOS), `%APPDATA%\EffectCraft` (Windows),
/// `$XDG_CONFIG_HOME/effectcraft` or `~/.config/effectcraft` (Linux and others). A `portable.txt`
/// beside the executable puts it in `EffectCraftData` there instead ([`app_dirs`]).
pub fn config_dir() -> Option<std::path::PathBuf> {
    app_dirs::current().dir.clone()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// Layer ▸ Camera ▸ Link Focus Distance to Point of Interest / to Layer: the expressions
    /// evaluate to the camera's focus on the target.
    #[test]
    fn focus_link_expressions_track_the_target() {
        use effectcraft_engine::geom::vec3;
        let mut s = super::session();
        s.execute("comp.new", json!({"name": "F", "width": 400, "height": 300, "duration": 2})).unwrap();
        let t = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 50, "height": 50})).unwrap()["layer"].as_u64().unwrap();
        s.execute("layer.setSwitch", json!({"layers": [t], "switch": "threeD", "value": true})).unwrap();
        s.execute("layer.setTransform", json!({"layer": t, "prop": "position", "value": [260, 120, 400]})).unwrap();
        let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
        let cid = s.active_comp_id().unwrap();
        let focus = |s: &effectcraft_engine::Session| {
            let comp = s.project.comp(cid).unwrap();
            let mut ctx = effectcraft_engine::render::EvalCtx::new(&s.project, cid, comp, s.time());
            ctx.expr = s.expr.as_deref();
            let l = comp.layer(effectcraft_engine::project::LayerId(cam)).unwrap();
            ctx.value(l, l.props.prop("cameraOptions/focusDistance").unwrap()).as_f64()
        };
        let depth = |s: &effectcraft_engine::Session| {
            let comp = s.project.comp(cid).unwrap();
            let ctx = effectcraft_engine::render::EvalCtx::new(&s.project, cid, comp, s.time());
            let c = comp.layer(effectcraft_engine::project::LayerId(cam)).unwrap();
            let tl = comp.layer(effectcraft_engine::project::LayerId(t)).unwrap();
            let cs = effectcraft_engine::render::three_d::camera::layer_camera(&ctx, c);
            cs.depth(ctx.world_matrix(tl).apply(vec3(25.0, 25.0, 0.0)))
        };
        s.execute("layer.select", json!({"layers": [cam, t]})).unwrap();
        let set = s.execute("camera.setFocusToLayer", json!({})).unwrap()["focusDistance"].as_f64().unwrap();
        assert!((set - depth(&s)).abs() < 1e-6 && (focus(&s) - set).abs() < 1e-6);
        s.execute("camera.linkFocusToLayer", json!({})).unwrap();
        // Move the target: the linked focus follows.
        s.execute("layer.setTransform", json!({"layer": t, "prop": "position", "value": [200, 150, 900]})).unwrap();
        assert!((focus(&s) - depth(&s)).abs() < 1e-3, "{} vs {}", focus(&s), depth(&s));
        s.execute("camera.linkFocusToPoi", json!({"camera": cam})).unwrap();
        let poi = {
            let l = s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(cam)).unwrap();
            let p = l.props.prop("transform/position").unwrap().value.as_vec3();
            let q = l.props.prop("transform/poi").unwrap().value.as_vec3();
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
        };
        assert!((focus(&s) - poi).abs() < 1e-6, "{} vs {poi}", focus(&s));
    }

    #[test]
    fn wired_session_renders_demo_with_expressions() {
        let mut s = super::session();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let cid = s.active_comp_id().unwrap();
        let lid = s.active_comp().unwrap().layers[1].id.0;
        s.execute("prop.setExpression", json!({"layer": lid, "path": "transform/rotation", "expression": "time * 90"})).unwrap();
        let img = s.render(cid, s.time(), effectcraft_engine::render::RenderOpts { scale: 0.25, ..Default::default() });
        assert!(img.data.iter().any(|p| p[3] > 0.5));
    }

    /// File ▸ Import of glTF/GLB/OBJ models through the media layer, placed as model layers and
    /// drawn by the Advanced 3D renderer.
    #[test]
    fn imported_models_render_in_advanced_3d() {
        let fx = |n: &str| format!("{}/../model/tests/fixtures/{n}", env!("CARGO_MANIFEST_DIR"));
        let mut s = super::session();
        s.execute("comp.new", json!({"name": "M", "width": 160, "height": 120, "renderer": "advanced3d"})).unwrap();
        let r = s.execute("file.import", json!({"paths": [fx("cube.obj"), fx("quad.glb"), fx("quad.gltf")]})).unwrap();
        assert_eq!(r["errors"], json!([]), "{r}");
        let items: Vec<u64> = r["items"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
        assert_eq!(items.len(), 3);
        let cube = s.execute("layer.addItem", json!({"item": items[0]})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.set", json!({"layer": cube, "path": "transform/rotationY", "value": 30})).unwrap();
        s.execute("prop.set", json!({"layer": cube, "path": "transform/rotationX", "value": 20})).unwrap();
        let cid = s.active_comp_id().unwrap();
        let opts = effectcraft_engine::render::RenderOpts::default();
        let img = s.render(cid, s.time(), opts);
        let covered = img.data.iter().filter(|p| p[3] > 0.99).count();
        assert!(covered > 1000, "the cube covers part of the frame: {covered}");
        // Red faces (material Red) are visible.
        assert!(img.data.iter().any(|p| p[3] > 0.99 && p[0] > 0.5 && p[1] < 0.2));
        // The textured binary glTF quad.
        for _ in 0..3 {
            s.execute("edit.undo", json!({})).unwrap();
        }
        s.execute("layer.addItem", json!({"item": items[1]})).unwrap();
        let img = s.render(cid, s.time(), opts);
        assert!(img.data.iter().filter(|p| p[3] > 0.99).count() > 1000);
        // Classic 3D doesn't draw models.
        s.execute("comp.renderer", json!({"renderer": "classic3d"})).unwrap();
        assert!(s.render(cid, s.time(), opts).data.iter().all(|p| p[3] == 0.0));
    }

    /// A binary glTF of a red cube 2 units wide (positions and indices only).
    fn glb_cube() -> Vec<u8> {
        let mut bin: Vec<u8> = (0..8u32).flat_map(|i| [i & 1, (i >> 1) & 1, (i >> 2) & 1].map(|b| b as f32 * 2.0 - 1.0)).flat_map(f32::to_le_bytes).collect();
        let faces: [[u16; 4]; 6] = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        bin.extend(faces.iter().flat_map(|f| [f[0], f[1], f[2], f[0], f[2], f[3]]).flat_map(u16::to_le_bytes));
        let gltf = json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "material": 0}]}],
            "materials": [{"pbrMetallicRoughness": {"baseColorFactor": [1, 0, 0, 1], "metallicFactor": 0, "roughnessFactor": 1}}],
            "buffers": [{"byteLength": bin.len()}],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 96}, {"buffer": 0, "byteOffset": 96, "byteLength": 72}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 8, "type": "VEC3", "min": [-1, -1, -1], "max": [1, 1, 1]},
                {"bufferView": 1, "componentType": 5123, "count": 36, "type": "SCALAR"}
            ]
        });
        let mut text = gltf.to_string().into_bytes();
        text.resize(text.len().div_ceil(4) * 4, b' ');
        let chunk = |kind: &[u8; 4], data: &[u8]| [&(data.len() as u32).to_le_bytes()[..], kind, data].concat();
        let body = [chunk(b"JSON", &text), chunk(b"BIN\0", &bin)].concat();
        [&b"glTF"[..], &2u32.to_le_bytes(), &(12 + body.len() as u32).to_le_bytes(), &body].concat()
    }

    /// #264: a model imported into a new composition (Classic 3D by default) was invisible on
    /// every renderer, because only Advanced 3D draws models. Adding it now switches the comp,
    /// and the software renderer (no GPU, as on the reporter's Intel HD Graphics 5500) draws it.
    #[test]
    fn imported_glb_shows_in_a_new_composition() {
        let path = std::env::temp_dir().join(format!("effectcraft-264-cube-{}.glb", std::process::id()));
        std::fs::write(&path, glb_cube()).unwrap();
        let mut s = super::session();
        assert!(s.accel.is_none(), "renders in software");
        let r = s.execute("file.import", json!({"paths": [path.to_string_lossy()]})).unwrap();
        assert_eq!(r["errors"], json!([]), "{r}");
        s.state.project_selection = vec![effectcraft_project::ItemId(r["items"][0].as_u64().unwrap())];
        s.execute("file.newCompFromSelection", json!({})).unwrap();
        let comp = s.active_comp().unwrap();
        assert_eq!(comp.renderer, effectcraft_project::Renderer::Advanced3D);
        let layer = comp.layers[0].id.0;
        s.execute("prop.set", json!({"layer": layer, "path": "transform/rotationY", "value": 30})).unwrap();
        s.execute("prop.set", json!({"layer": layer, "path": "transform/rotationX", "value": 20})).unwrap();
        let cid = s.active_comp_id().unwrap();
        let img = s.render(cid, s.time(), effectcraft_engine::render::RenderOpts { scale: 0.25, ..Default::default() });
        let red = img.data.iter().filter(|p| p[3] > 0.99 && p[0] > 0.2 && p[1] < 0.05 && p[2] < 0.05).count();
        let _ = std::fs::remove_file(&path);
        // Half the comp height wide, seen at an angle: well over 5% of the frame.
        assert!(red * 20 > img.data.len(), "red cube pixels: {red} of {}", img.data.len());
    }

    /// #651: an animated GIF imported from disk plays: the layer shows each frame at its time,
    /// in the viewer's renders and in exports alike.
    #[test]
    fn an_imported_animated_gif_plays() {
        let dir = std::env::temp_dir().join(format!("ec-gif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("anim.gif");
        let mut bytes = Vec::new();
        {
            let mut enc = gif::Encoder::new(&mut bytes, 4, 4, &[]).unwrap();
            for rgb in [[255u8, 0, 0], [0, 255, 0], [0, 0, 255]] {
                let mut f = gif::Frame::from_rgb(4, 4, &rgb.repeat(16));
                f.delay = 10;
                enc.write_frame(&f).unwrap();
            }
        }
        std::fs::write(&path, bytes).unwrap();
        let mut s = super::session();
        s.execute("comp.new", json!({"name": "G", "width": 4, "height": 4, "frameRate": 30, "duration": 1})).unwrap();
        s.execute("file.import", json!({"paths": [path.to_string_lossy()], "addToComp": true})).unwrap();
        let cid = s.active_comp_id().unwrap();
        let rgb = |t: f64| {
            let img = s.render(cid, effectcraft_time::Tick::from_seconds_f64(t), effectcraft_engine::render::RenderOpts::default());
            let p = img.data[5];
            [p[0].round(), p[1].round(), p[2].round()]
        };
        assert_eq!(rgb(0.05), [1.0, 0.0, 0.0]);
        assert_eq!(rgb(0.15), [0.0, 1.0, 0.0]);
        assert_eq!(rgb(0.25), [0.0, 0.0, 1.0]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The web app's path: outputs go to a sink (downloads), never to the file system.
    #[test]
    fn render_queue_exports_to_a_sink() {
        use std::sync::{Arc, Mutex};
        let got: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Arc::default();
        let g = got.clone();
        let mut s = super::session();
        s.exporter = Some(Arc::new(super::FileExporter { sink: Some(Arc::new(move |p: &str, d: Vec<u8>| g.lock().unwrap().push((p.to_string(), d)))) }));
        s.execute("file.openDemoProject", json!({})).unwrap();
        let dir = "/no-such-dir-effectcraft-sink-test";
        s.execute(
            "renderQueue.add",
            json!({"format": "gif", "output": format!("{dir}/a.gif"), "resolution": 0.0625, "timeSpan": "custom", "start": 0.0, "end": 0.2}),
        )
        .unwrap();
        s.execute(
            "renderQueue.add",
            json!({"format": "png", "output": format!("{dir}/seq_[##].png"), "resolution": 0.0625, "timeSpan": "custom", "start": 0.0, "end": 0.1}),
        )
        .unwrap();
        s.execute("renderQueue.render", json!({"wait": true})).unwrap();
        s.poll_render();
        let mut got = got.lock().unwrap().clone();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        let names: Vec<std::path::PathBuf> = got.iter().map(|(p, _)| std::path::PathBuf::from(p)).collect();
        let expected = [format!("{dir}/a.gif"), format!("{dir}/seq_00.png"), format!("{dir}/seq_01.png"), format!("{dir}/seq_02.png")]
            .map(|p| std::path::absolute(p).unwrap());
        assert_eq!(names, expected);
        assert!(got[0].1.starts_with(b"GIF89a"));
        assert!(got[1].1.starts_with(b"\x89PNG"));
        assert!(!std::path::Path::new(dir).exists());
    }

    #[test]
    fn render_queue_exports_demo_comp() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/host-rq");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = super::session();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let out = dir.join("[compName]_[width]x[height].[fileExtension]");
        let r = s
            .execute(
                "renderQueue.add",
                json!({"format": "h264", "output": out.to_string_lossy(), "resolution": 0.125, "timeSpan": "custom", "start": 0.0, "end": 0.4}),
            )
            .unwrap();
        let path = r["outputPath"].as_str().unwrap().to_string();
        assert!(path.ends_with("_240x134.mp4"), "{path}");
        // A second item: PNG sequence, rendered in the background.
        s.execute("renderQueue.add", json!({"format": "png", "output": dir.join("seq_[###].png").to_string_lossy(), "resolution": 0.0625, "timeSpan": "custom", "start": 0.0, "end": 0.2})).unwrap();
        let r = s.execute("renderQueue.render", json!({"wait": false})).unwrap();
        assert_eq!(r["items"].as_array().unwrap().len(), 2);
        let t0 = std::time::Instant::now();
        while s.is_rendering() {
            assert!(t0.elapsed().as_secs() < 300, "render timed out");
            std::thread::sleep(std::time::Duration::from_millis(20));
            s.poll_render();
        }
        s.poll_render();
        let list = s.execute("renderQueue.list", json!({})).unwrap();
        for it in list["items"].as_array().unwrap() {
            assert_eq!(it["statusLabel"], "Done", "{it}");
            assert!(it["render_time"].as_f64().is_some());
            assert!(std::path::Path::new(it["last_output"].as_str().unwrap()).exists(), "{it}");
        }
        assert!(std::path::Path::new(&path).metadata().unwrap().len() > 500);
        assert!(dir.join("seq_000.png").exists() && dir.join("seq_005.png").exists() && !dir.join("seq_006.png").exists());
        // Rendering again needs a re-queue.
        assert!(s.execute("renderQueue.render", json!({})).is_err());
        s.execute("renderQueue.setRender", json!({"index": 1, "render": true})).unwrap();
        assert_eq!(s.project.render_queue[0].status, effectcraft_engine::project::render_queue::RenderStatus::Queued);
    }

    /// Value of a property of layer `l` at time 0 (keys + expression).
    fn value(s: &effectcraft_engine::Session, l: u64, path: &str) -> effectcraft_engine::keyframe::Value {
        let cid = s.active_comp_id().unwrap();
        let comp = s.project.comp(cid).unwrap();
        let layer = comp.layer(effectcraft_engine::project::LayerId(l)).unwrap();
        let ctx =
            effectcraft_engine::render::EvalCtx { project: &s.project, comp_id: cid, comp, time: Default::default(), expr: s.expr.as_deref(), footage: None };
        ctx.value(layer, layer.props.prop(path).unwrap())
    }

    #[test]
    fn pick_whip_expressions_evaluate() {
        let mut s = super::session();
        s.execute("comp.new", json!({"name": "C", "width": 200, "height": 200, "frameRate": 30, "duration": 2})).unwrap();
        let a = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
        let b = s.execute("layer.newSolid", json!({"name": "Src", "color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.set", json!({"layer": b, "path": "transform/rotation", "value": 33})).unwrap();
        s.execute("prop.set", json!({"layer": b, "path": "transform/position", "value": [12, 34]})).unwrap();
        s.execute("effect.apply", json!({"layer": b, "effect": "Gaussian Blur"})).unwrap();
        s.execute("prop.set", json!({"layer": b, "path": "effects/#1/blurriness", "value": 21})).unwrap();
        s.execute("layer.addMask", json!({"layer": b})).unwrap();
        s.execute("prop.set", json!({"layer": b, "path": "masks/#1/feather", "value": [7, 7]})).unwrap();
        let link = |s: &mut effectcraft_engine::Session, from: &str, to: &str| {
            s.execute("prop.pickWhip", json!({"layer": a, "path": from, "target": {"layer": b, "path": to}})).unwrap();
        };
        link(&mut s, "transform/opacity", "transform/rotation");
        assert_eq!(value(&s, a, "transform/opacity").as_f64(), 33.0);
        link(&mut s, "transform/scale", "transform/rotation");
        let cid = s.active_comp_id().unwrap();
        let ep = effectcraft_expr::eval_property(&s.project, cid, effectcraft_engine::project::LayerId(a), "transform/scale", 0.0);
        assert!(ep.is_ok(), "{ep:?}");
        assert_eq!(value(&s, a, "transform/scale").as_vec2(), [33.0, 33.0]);
        link(&mut s, "transform/position", "transform/position");
        assert_eq!(value(&s, a, "transform/position").as_vec2(), [12.0, 34.0]);
        link(&mut s, "transform/rotation", "effects/#1/blurriness");
        assert_eq!(value(&s, a, "transform/rotation").as_f64(), 21.0);
        link(&mut s, "transform/opacity", "masks/#1/feather");
        assert_eq!(value(&s, a, "transform/opacity").as_f64(), 7.0);
        // Separated dimensions: X Position reference.
        s.execute("prop.separateDimensions", json!({"layer": b})).unwrap();
        link(&mut s, "transform/rotation", "transform/positionX");
        assert_eq!(value(&s, a, "transform/rotation").as_f64(), 12.0);
    }

    /// The expression error bar's list: evaluation errors and syntax errors, with the layer and
    /// property they belong to.
    #[test]
    fn expression_errors_are_collected() {
        let mut s = super::session();
        s.execute("comp.new", json!({"name": "C", "width": 100, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
        let a = s.execute("layer.newSolid", json!({"name": "Box", "color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.setExpression", json!({"layer": a, "path": "transform/opacity", "expression": "50"})).unwrap();
        assert_eq!(s.execute("expr.errors", json!({})).unwrap()["count"], 0);
        s.execute("prop.setExpression", json!({"layer": a, "path": "transform/rotation", "expression": "thisComp.layer(\"Nope\").rotation"})).unwrap();
        s.execute("prop.setExpression", json!({"layer": a, "path": "transform/opacity", "expression": "1 +* ("})).unwrap();
        let r = s.execute("expr.errors", json!({})).unwrap();
        assert_eq!(r["count"], 2, "{r}");
        let errs = r["errors"].as_array().unwrap();
        let rot = errs.iter().find(|e| e["path"] == "transform/rotation").unwrap();
        assert!(rot["message"].as_str().unwrap().contains("Nope"), "{rot}");
        assert_eq!(rot["layerIndex"], 1);
        assert_eq!(rot["layerName"], "Box");
        assert_eq!(rot["disabled"], false);
        let op = errs.iter().find(|e| e["path"] == "transform/opacity").unwrap();
        assert_eq!(op["disabled"], true);
        assert!(r["text"][0].as_str().unwrap().contains("of layer 1 ('Box') in comp 'C'"), "{r}");
        // A user-disabled (but valid) expression is not an error.
        s.execute("prop.setExpression", json!({"layer": a, "path": "transform/rotation", "expression": "45"})).unwrap();
        s.execute("prop.setExpression", json!({"layer": a, "path": "transform/rotation", "enabled": false})).unwrap();
        assert_eq!(s.execute("expr.errors", json!({})).unwrap()["count"], 1);
        // The Expression Language menu is categorised.
        let m = s.execute("expr.languageMenu", json!({})).unwrap();
        assert!(m.as_array().unwrap().iter().any(|c| c["category"] == "Footage" && c["items"].as_array().unwrap().iter().any(|i| i["text"] == "sourceData")));
    }

    /// File ▸ Import of JSON/CSV data files as data footage, read by expressions.
    #[test]
    fn imported_data_files_drive_expressions() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/host-data");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("levels.csv"), "label,value\nlow,10\nhigh,90\n").unwrap();
        std::fs::write(dir.join("cfg.json"), r#"{"angle": 33}"#).unwrap();
        std::fs::write(dir.join("broken.json"), "{nope").unwrap();
        let mut s = super::session();
        s.execute("comp.new", json!({"name": "C", "width": 100, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
        let paths: Vec<String> = ["levels.csv", "cfg.json", "broken.json"].iter().map(|n| dir.join(n).to_string_lossy().to_string()).collect();
        let r = s.execute("file.import", json!({"paths": paths})).unwrap();
        assert_eq!(r["items"].as_array().unwrap().len(), 2, "{r}");
        assert_eq!(r["errors"].as_array().unwrap().len(), 1, "{r}");
        let csv = r["items"][0].as_u64().unwrap();
        assert_eq!(s.project.item(effectcraft_engine::project::ItemId(csv)).unwrap().type_name(), "Data");
        assert!(s.execute("layer.addItem", json!({"item": csv})).is_err(), "data files are not layers");
        let a = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.setExpression", json!({"layer": a, "path": "transform/opacity", "expression": "footage(\"levels.csv\").sourceData[1].value"})).unwrap();
        s.execute("prop.setExpression", json!({"layer": a, "path": "transform/rotation", "expression": "footage(\"cfg.json\").sourceData.angle"})).unwrap();
        assert_eq!(value(&s, a, "transform/opacity").as_f64(), 90.0);
        assert_eq!(value(&s, a, "transform/rotation").as_f64(), 33.0);
    }

    /// sampleImage through the renderer: a box's Fill colour follows the solid under it.
    #[test]
    fn sample_image_drives_a_colour() {
        let mut s = super::session();
        s.execute("comp.new", json!({"name": "C", "width": 100, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
        s.execute("layer.newSolid", json!({"name": "Bg", "color": "#00ff00"})).unwrap();
        let a = s.execute("layer.newSolid", json!({"name": "Fg", "color": "#ffffff", "width": 20, "height": 20})).unwrap()["layer"].as_u64().unwrap();
        s.execute("effect.apply", json!({"layer": a, "effect": "Fill"})).unwrap();
        s.execute(
            "prop.setExpression",
            json!({"layer": a, "path": "effects/#1/color", "expression": "thisComp.layer(\"Bg\").sampleImage(thisComp.layer(\"Bg\").fromComp(toComp(anchorPoint)))"}),
        )
        .unwrap();
        let cid = s.active_comp_id().unwrap();
        let img = s.render(cid, s.time(), effectcraft_engine::render::RenderOpts::default());
        let px = img.get(50, 50);
        assert!(px[1] > 0.99 && px[0] < 0.01, "the box is filled with the sampled green: {px:?}");
    }

    /// Proxies through the real media and export layers: Render Settings ▸ Proxy Use decides
    /// whether an export uses the proxy, and Create Proxy renders and attaches one.
    #[test]
    fn proxies_in_exports_and_create_proxy() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/host-proxy");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = |n: &str| dir.join(n).to_string_lossy().to_string();
        let mut s = super::session();
        // Source and proxy stills, written by the app itself.
        s.execute("comp.new", json!({"name": "Red", "width": 16, "height": 16, "frameRate": 10, "duration": 1})).unwrap();
        let red = s.active_comp_id().unwrap();
        s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
        s.execute("comp.saveFrameAs", json!({"path": p("red.png")})).unwrap();
        s.execute("comp.new", json!({"name": "Blue", "width": 8, "height": 8, "frameRate": 10, "duration": 1})).unwrap();
        s.execute("layer.newSolid", json!({"color": "#0000ff"})).unwrap();
        s.execute("comp.saveFrameAs", json!({"path": p("blue.png")})).unwrap();
        let foot = s.execute("file.import", json!({"paths": [p("red.png")]})).unwrap()["items"][0].as_u64().unwrap();
        s.execute("file.setProxy", json!({"item": foot, "path": p("blue.png")})).unwrap();
        s.execute("comp.new", json!({"name": "Out", "width": 16, "height": 16, "frameRate": 10, "duration": 1})).unwrap();
        let out = s.active_comp_id().unwrap();
        s.execute("layer.addItem", json!({"item": foot})).unwrap();
        // Export one frame with each Proxy Use.
        for (name, using) in [("all", "use_all_[#].png"), ("none", "use_none_[#].png")] {
            s.execute(
                "renderQueue.add",
                json!({"comp": out.0, "format": "png", "output": p(using), "timeSpan": "custom", "start": 0.0, "end": 0.1, "proxyUse": name}),
            )
            .unwrap();
        }
        s.execute("renderQueue.render", json!({"wait": true})).unwrap();
        s.poll_render();
        let centre = |s: &mut effectcraft_engine::Session, path: &str| {
            let id = s.execute("file.import", json!({"paths": [path]})).unwrap()["items"][0].as_u64().unwrap();
            s.execute("comp.new", json!({"name": "Probe", "width": 16, "height": 16, "frameRate": 10, "duration": 1})).unwrap();
            s.execute("layer.addItem", json!({"item": id})).unwrap();
            let c = s.active_comp_id().unwrap();
            s.render(c, Default::default(), Default::default()).get(8, 8)
        };
        let all = centre(&mut s, &p("use_all_0.png"));
        assert!(all[2] > 0.99 && all[0] < 0.01, "Use All Proxies renders the blue proxy: {all:?}");
        let none = centre(&mut s, &p("use_none_0.png"));
        assert!(none[0] > 0.99 && none[2] < 0.01, "Use No Proxies renders the red source: {none:?}");
        // Create Proxy ▸ Still: rendered at half size and attached to the comp.
        s.execute("file.createProxy", json!({"kind": "still", "comp": red.0, "path": p("red_proxy_[#####].png")})).unwrap();
        s.execute("renderQueue.render", json!({"wait": true})).unwrap();
        s.poll_render();
        let px = s.project.item(red).unwrap().proxy.clone().expect("Create Proxy attached the rendered still");
        assert!(px.enabled);
        assert_eq!((px.footage.width, px.footage.height), (8, 8));
        assert!(std::path::Path::new(&px.footage.path).exists(), "{}", px.footage.path);
    }

    /// A script builds a comp, queues it and renders it through the Render Queue.
    #[test]
    fn script_renders_through_the_render_queue() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/host-script");
        let _ = std::fs::remove_dir_all(&dir);
        let out = dir.join("scripted.gif");
        let mut s = super::session();
        let code = format!(
            r#"
            var comp = app.project.items.addComp("Scripted", 64, 48, 1, 0.2, 10);
            var sq = comp.layers.addSolid([1, 0.5, 0], "Square", 16, 16, 1);
            sq.transform.position.setValueAtTime(0, [8, 24]);
            sq.transform.position.setValueAtTime(0.1, [56, 24]);
            var item = app.project.renderQueue.items.add(comp);
            item.outputModule(1).applyTemplate("GIF");
            item.outputModule(1).file = new File({});
            app.project.renderQueue.render();
            [item.status == RQItemStatus.DONE, item.outputModule(1).file.fsName, app.project.renderQueue.numItems]
            "#,
            serde_json::to_string(&out.to_string_lossy()).unwrap()
        );
        let r = s.execute("script.run", json!({"code": code})).unwrap();
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(r["result"][0], json!(true), "{r}");
        assert_eq!(r["result"][2], json!(1));
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(b"GIF89a"));
    }

    /// importFile, layers.add(footage) and project save/open from a script.
    #[test]
    fn script_imports_footage_and_saves() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/host-script-import");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let q = |n: &str| serde_json::to_string(&dir.join(n).to_string_lossy()).unwrap();
        let mut s = super::session();
        let code = format!(
            r#"
            var c = app.project.items.addComp("Src", 32, 32, 1, 1, 10);
            c.layers.addSolid([0, 1, 0], "G", 32, 32, 1);
            app.run("comp.saveFrameAs", {{comp: c.id, path: {png}}});
            var still = app.project.importFile(new ImportOptions(new File({png})));
            var main = app.project.items.addComp("Main", 64, 64, 1, 1, 10);
            var l = main.layers.add(still);
            app.project.save(new File({proj}));
            [still instanceof FootageItem, still.width, still.mainSource.isStill, l.source.name, app.project.file.name]
            "#,
            png = q("frame.png"),
            proj = q("p.ecproj")
        );
        let r = s.execute("script.run", json!({"code": code})).unwrap();
        assert_eq!(r["ok"], json!(true), "{r}");
        assert_eq!(r["result"], json!([true, 32, true, "frame.png", "p.ecproj"]), "{r}");
        let r = s.execute("script.run", json!({"code": format!("app.open(new File({})); app.project.numItems", q("p.ecproj"))})).unwrap();
        assert_eq!(r["result"], json!(5), "{r}"); // Src, Solids, G, frame.png, Main
    }

    #[test]
    fn expression_syntax_errors_disable_the_expression() {
        let mut s = super::session();
        s.execute("comp.new", json!({"name": "C", "width": 100, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
        let a = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
        let r = s.execute("prop.setExpression", json!({"layer": a, "path": "transform/opacity", "expression": "1 +* ("})).unwrap();
        assert!(r["error"].is_string(), "{r}");
        let pr = s.active_comp().unwrap().layers[0].props.prop("transform/opacity").unwrap().clone();
        assert!(!pr.expr.unwrap().enabled);
        assert_eq!(value(&s, a, "transform/opacity").as_f64(), 100.0);
        s.execute("prop.setExpression", json!({"layer": a, "path": "transform/opacity", "expression": "50"})).unwrap();
        assert_eq!(value(&s, a, "transform/opacity").as_f64(), 50.0);
    }
    /// Create Stereo 3D Rig: the eye cameras' expressions follow the master camera and the
    /// Stereo 3D Controls (and agree with the values the command wrote).
    #[test]
    fn stereo_rig_expressions_follow_the_master_camera() {
        let mut s = super::session();
        s.execute("comp.new", json!({"name": "Shot", "width": 640, "height": 360, "frameRate": 30, "duration": 2})).unwrap();
        let src = s.active_comp_id().unwrap();
        let cam = s.execute("layer.newCamera", json!({"name": "Main", "position": [320, 180, -800], "poi": [320, 180, 0], "zoom": 900})).unwrap()["layer"]
            .as_u64()
            .unwrap();
        let r = s.execute("camera.stereoRig", json!({"sceneDepth": 4})).unwrap();
        let ctl = r["controls"].as_u64().unwrap();
        let left = effectcraft_engine::project::ItemId(r["leftComp"].as_u64().unwrap());
        // The left eye camera's evaluated value (expressions on).
        let v3 = |s: &effectcraft_engine::Session, path: &str| {
            let comp = s.project.comp(left).unwrap();
            let layer = &comp.layers[0];
            let ctx = effectcraft_engine::render::EvalCtx {
                project: &s.project,
                comp_id: left,
                comp,
                time: Default::default(),
                expr: s.expr.as_deref(),
                footage: None,
            };
            ctx.value(layer, layer.props.prop(path).unwrap()).as_vec3()
        };
        let close = |a: [f64; 3], b: [f64; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6);
        let d = 0.04 * 640.0;
        assert!(close(v3(&s, "transform/position"), [320.0 - d / 2.0, 180.0, -800.0]), "{:?}", v3(&s, "transform/position"));
        assert!(close(v3(&s, "transform/poi"), [320.0 - d / 2.0, 180.0, 0.0]), "{:?}", v3(&s, "transform/poi"));
        // Moving the master moves the eyes.
        s.execute("comp.open", json!({"comp": src.0})).unwrap();
        s.execute("prop.set", json!({"layer": cam, "path": "transform/position", "value": [300, 180, -800]})).unwrap();
        s.execute("prop.set", json!({"layer": cam, "path": "transform/poi", "value": [300, 180, 0]})).unwrap();
        assert!(close(v3(&s, "transform/position"), [300.0 - d / 2.0, 180.0, -800.0]), "{:?}", v3(&s, "transform/position"));
        // Controls: Center & Left puts the left eye a full separation left; convergence toes in.
        s.execute("prop.set", json!({"layer": ctl, "path": "effects/#1/configuration", "value": 2})).unwrap();
        s.execute("prop.set", json!({"layer": ctl, "path": "effects/#1/convergence", "value": true})).unwrap();
        assert!(close(v3(&s, "transform/position"), [300.0 - d, 180.0, -800.0]), "{:?}", v3(&s, "transform/position"));
        assert!(close(v3(&s, "transform/poi"), [300.0, 180.0, 0.0]), "{:?}", v3(&s, "transform/poi"));
    }

    /// Footage drawn per frame that counts the frames the renderer asks for; a `gate` makes
    /// the first request at frame 3 or later wait until the test lets it go.
    #[derive(Default)]
    struct Counted {
        asked: std::sync::Mutex<std::collections::BTreeMap<u32, u32>>,
        gate: Option<(std::sync::mpsc::Sender<()>, std::sync::Mutex<std::sync::mpsc::Receiver<()>>)>,
        gated: std::sync::atomic::AtomicBool,
    }

    impl effectcraft_engine::render::FootageSource for Counted {
        fn frame(
            &self,
            _: effectcraft_project::ItemId,
            _: &effectcraft_project::Footage,
            t: effectcraft_time::Tick,
        ) -> Option<std::sync::Arc<effectcraft_raster::Image>> {
            let f = (t.seconds() * 10.0).round().max(0.0) as u32;
            *self.asked.lock().unwrap().entry(f).or_default() += 1;
            if let Some((reached, go)) = &self.gate
                && f >= 3
                && !self.gated.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                reached.send(()).unwrap();
                go.lock().unwrap().recv().unwrap();
            }
            let mut img = effectcraft_raster::Image::new(64, 36);
            for y in 0..36 {
                for x in 0..64 {
                    let v = ((x + y * 2 + f * 7) % 64) as f32 / 63.0;
                    img.set(x, y, [v, 1.0 - v, (f as f32 / 12.0).min(1.0), 1.0]);
                }
            }
            Some(std::sync::Arc::new(img))
        }
    }

    /// A host session with a 64×36, 10 fps, 1.2 s comp showing `footage` and a queue item writing
    /// `modules` (format, path; the first is the item's own output module).
    fn modules_session(footage: std::sync::Arc<Counted>, modules: &[(&str, String)]) -> effectcraft_engine::Session {
        let mut s = super::session();
        s.execute("comp.new", json!({"name": "Modules", "width": 64, "height": 36, "frameRate": 10, "duration": 1.2})).unwrap();
        let cid = s.active_comp_id().unwrap();
        s.edit("clip", None, |p, _| {
            let f = effectcraft_project::Footage {
                path: "counted.mov".into(),
                kind: effectcraft_project::FootageKind::Video,
                width: 64,
                height: 36,
                frame_rate: effectcraft_time::FrameRate::new(10, 1),
                duration: effectcraft_time::Tick::from_seconds_f64(1.2),
                has_video: true,
                codec: "SYN".into(),
                ..Default::default()
            };
            let item = p.add_item("clip", effectcraft_engine::color::Label::Aqua, None, effectcraft_project::ItemKind::Footage(f));
            let comp = p.comp(cid).unwrap().clone();
            let l = effectcraft_project::build::layer(p, &comp, "Clip", effectcraft_project::LayerSource::Footage { item }, (64, 36), None);
            p.comp_mut(cid).unwrap().layers.insert(0, l);
            Ok(())
        })
        .unwrap();
        s.footage = footage;
        let (fmt, path) = &modules[0];
        s.execute("renderQueue.add", json!({"comp": cid.0, "format": fmt, "output": path})).unwrap();
        for (fmt, path) in &modules[1..] {
            s.execute("render.addOutputModule", json!({"index": 1, "format": fmt, "output": path})).unwrap();
        }
        s
    }

    fn modules_dir(name: &str) -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/host-modules").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The files in `dir` by name.
    fn files(dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap()).map(|e| (e.file_name().into_string().unwrap(), std::fs::read(e.path()).unwrap())).collect()
    }

    const MODULES: [(&str, &str); 3] = [("h264", "m.mp4"), ("prores", "m.mov"), ("png", "m_[##].png")];

    /// G5: the host exporter hands an item's output modules to `export_modules_async`: every
    /// frame renders once for all of them, and each file holds the bytes of its module exported
    /// alone.
    #[test]
    fn output_modules_render_each_frame_once_and_write_the_bytes_they_write_alone() {
        for n in [2, 3] {
            let mut alone = std::collections::BTreeMap::new();
            let mut asked_alone = 0;
            for (fmt, name) in &MODULES[..n] {
                let dir = modules_dir(&format!("alone-{n}-{fmt}"));
                let footage = std::sync::Arc::new(Counted::default());
                let mut s = modules_session(footage.clone(), &[(fmt, dir.join(name).to_string_lossy().to_string())]);
                s.execute("renderQueue.render", json!({"wait": true})).unwrap();
                s.poll_render();
                assert_eq!(s.project.render_queue[0].status.label(), "Done", "{fmt}");
                asked_alone += footage.asked.lock().unwrap().values().sum::<u32>();
                alone.extend(files(&dir));
            }
            let dir = modules_dir(&format!("together-{n}"));
            let footage = std::sync::Arc::new(Counted::default());
            let modules: Vec<(&str, String)> = MODULES[..n].iter().map(|(f, name)| (*f, dir.join(name).to_string_lossy().to_string())).collect();
            let mut s = modules_session(footage.clone(), &modules);
            s.execute("renderQueue.render", json!({"wait": true})).unwrap();
            s.poll_render();
            let it = &s.project.render_queue[0];
            assert_eq!(it.status.label(), "Done", "{n} modules");
            assert!(it.last_output.as_deref().is_some_and(|o| o.ends_with("m.mp4")), "{:?}", it.last_output);
            let asked = footage.asked.lock().unwrap().clone();
            assert_eq!(asked.keys().copied().collect::<Vec<_>>(), (0..12).collect::<Vec<_>>(), "{n} modules: {asked:?}");
            assert!(asked.values().all(|c| *c == 1), "{n} modules: every frame renders once: {asked:?}");
            assert_eq!(asked_alone, 12 * n as u32, "each module alone renders every frame");
            let together = files(&dir);
            assert_eq!(together.keys().collect::<Vec<_>>(), alone.keys().collect::<Vec<_>>());
            for (f, bytes) in &alone {
                assert!(together.get(f) == Some(bytes), "{n} modules: {f} differs from the module exported alone");
            }
        }
    }

    /// A stop mid-way cancels the modules being written: the item is User Stopped and none of
    /// its modules leave files.
    #[test]
    fn stopping_output_modules_removes_their_files() {
        let dir = modules_dir("stop");
        let (reached, wait) = std::sync::mpsc::channel();
        let (go, gate) = std::sync::mpsc::channel();
        let footage = std::sync::Arc::new(Counted { gate: Some((reached, std::sync::Mutex::new(gate))), ..Default::default() });
        let modules: Vec<(&str, String)> = MODULES.iter().map(|(f, name)| (*f, dir.join(name).to_string_lossy().to_string())).collect();
        let mut s = modules_session(footage.clone(), &modules);
        s.execute("renderQueue.render", json!({"wait": false})).unwrap();
        wait.recv_timeout(std::time::Duration::from_secs(120)).unwrap();
        s.execute("renderQueue.stop", json!({})).unwrap();
        go.send(()).unwrap();
        let t0 = std::time::Instant::now();
        while s.is_rendering() {
            assert!(t0.elapsed().as_secs() < 120, "render did not stop");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        s.poll_render();
        assert_eq!(s.project.render_queue[0].status.label(), "User Stopped");
        assert!(footage.asked.lock().unwrap().len() < 12, "the render stopped mid-way");
        assert!(files(&dir).is_empty(), "stopped modules left files: {:?}", files(&dir).keys());
    }

    /// A module that cannot write fails the item: the modules before it finish, the ones after
    /// it leave no files.
    #[test]
    fn a_failing_output_module_fails_the_item() {
        let dir = modules_dir("fail");
        std::fs::write(dir.join("file"), b"x").unwrap();
        let modules: Vec<(&str, String)> = [("png", "a_[##].png"), ("h264", "file/b.mp4"), ("prores", "c.mov")]
            .iter()
            .map(|(f, name)| (*f, dir.join(name).to_string_lossy().to_string()))
            .collect();
        let mut s = modules_session(std::sync::Arc::new(Counted::default()), &modules);
        s.execute("renderQueue.render", json!({"wait": true})).unwrap();
        s.poll_render();
        let status = s.project.render_queue[0].status.clone();
        assert!(matches!(status, effectcraft_engine::project::render_queue::RenderStatus::Failed(_)), "{status:?}");
        let left = files(&dir);
        assert!(left.contains_key("a_00.png") && left.contains_key("a_11.png"), "{:?}", left.keys());
        assert!(!left.contains_key("c.mov"), "the module after the failing one left its file");
    }

    /// The default `Exporter::export_modules_async`: modules one after another.
    struct OneByOne(super::FileExporter);

    impl effectcraft_engine::Exporter for OneByOne {
        fn formats(&self) -> Vec<effectcraft_engine::project::render_queue::OutputFormat> {
            self.0.formats()
        }
        fn export(&self, job: &effectcraft_engine::ExportJob, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<effectcraft_engine::ExportResult, String> {
            self.0.export(job, progress)
        }
    }

    /// Seconds to render a 2- and 3-module item of the demo comp at half resolution with the
    /// modules one after another (the default) and with the host exporter. Run with `--ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn time_output_modules() {
        let dir = modules_dir("time");
        for n in [2, 3] {
            for round in 0..2 {
                let mut t = [0.0; 2];
                for (k, together) in [false, true].into_iter().enumerate() {
                    let mut s = super::session();
                    if !together {
                        s.exporter = Some(std::sync::Arc::new(OneByOne(super::FileExporter::default())));
                    }
                    s.execute("file.openDemoProject", json!({})).unwrap();
                    let (fmt, name) = MODULES[0];
                    let span = json!({"timeSpan": "custom", "start": 0.0, "end": 2.0, "resolution": 0.5});
                    let mut p = json!({"format": fmt, "output": dir.join(name).to_string_lossy()});
                    p.as_object_mut().unwrap().extend(span.as_object().unwrap().clone());
                    s.execute("renderQueue.add", p).unwrap();
                    for (fmt, name) in &MODULES[1..n] {
                        s.execute("render.addOutputModule", json!({"index": 1, "format": fmt, "output": dir.join(name).to_string_lossy()})).unwrap();
                    }
                    let t0 = std::time::Instant::now();
                    s.execute("renderQueue.render", json!({"wait": true})).unwrap();
                    t[k] = t0.elapsed().as_secs_f64();
                    s.poll_render();
                    assert_eq!(s.project.render_queue[0].status.label(), "Done");
                }
                println!("{n} modules (round {round}): one after another {:.3} s, host exporter {:.3} s ({:.2}x)", t[0], t[1], t[0] / t[1]);
            }
        }
    }
}
