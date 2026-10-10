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
        Box::pin(async move {
            let j = effectcraft_export::Job {
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
                },
                nested_switches: job.nested_switches,
            };
            match effectcraft_export::export_async(&j, &mut |p| progress(p.done, p.total)).await {
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
        })
    }
}

/// A new session with media, import, expressions, scripting and export enabled.
pub fn session() -> Session {
    #[allow(unused_mut)]
    let mut s = Session {
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
    };
    // OpenFX plug-ins are native code run without a sandbox: only with the `openfx` feature, never
    // on the web build.
    #[cfg(all(feature = "openfx", not(target_arch = "wasm32")))]
    {
        s.ofx_loader = Some(effectcraft_ofx::loader);
        s.ofx_search_paths = effectcraft_ofx::default_search_paths();
        s.ofx_clear_blocklist = Some(effectcraft_ofx::blocklist::clear);
        // A plug-in that crashed the app while loading is remembered here and skipped next time.
        if let Some(dir) = config_dir() {
            effectcraft_ofx::blocklist::set_state_dir(dir);
        }
    }
    s
}

/// Call when the app or tool quits: stops helper programs OpenFX plug-ins started and would leave
/// running (the `openfx` feature; nothing to do otherwise).
pub fn shutdown() {
    #[cfg(all(feature = "openfx", not(target_arch = "wasm32")))]
    effectcraft_ofx::shutdown();
}

/// The platform config directory for EffectCraft (`EFFECTCRAFT_CONFIG_DIR` overrides):
/// `~/Library/Application Support/EffectCraft` (macOS), `%APPDATA%\EffectCraft` (Windows),
/// `$XDG_CONFIG_HOME/effectcraft` or `~/.config/effectcraft` (Linux and others).
pub fn config_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Some(d) = std::env::var_os("EFFECTCRAFT_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home().map(|h| h.join("Library/Application Support/EffectCraft"));
    }
    if cfg!(target_os = "windows") {
        return std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("EffectCraft"));
    }
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".config"))).map(|c| c.join("effectcraft"))
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
}
