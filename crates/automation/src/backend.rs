//! Where tools run: an in-process session, or the desktop app over its control channel.

use effectcraft_engine::Session;
use effectcraft_engine::time::Tick;
use serde_json::{Value, json};

use crate::confine::{ConfinedFootage, ConfinedServices, Confinement, Lookup};
use crate::{BridgeClient, Error, Result, base64, encode_png, recompress_png};

pub struct Backend {
    target: Target,
    /// File access confined to folders (`--automation-read-root` / `--automation-write-root`).
    confinement: Option<Confinement>,
}

enum Target {
    /// An in-process session (no window): engine commands, inspection and rendering.
    Headless(Box<Session>),
    /// A running desktop app reached over its control channel: everything, plus the live UI.
    Bridge(BridgeClient),
}

/// A rendered comp frame.
pub struct Frame {
    pub png: Vec<u8>,
    pub comp: Value,
    pub time: f64,
    pub width: u32,
    pub height: u32,
}

impl Backend {
    pub fn headless(session: Session) -> Self {
        Backend { target: Target::Headless(Box::new(session)), confinement: None }
    }

    /// Bridge to the app's control channel at `addr` (`9877` or `127.0.0.1:9877`).
    pub fn bridge(addr: &str) -> Result<Self> {
        Ok(Backend { target: Target::Bridge(BridgeClient::new(addr)?), confinement: None })
    }

    /// Confine the files the commands run here may read and write ([`Confinement`]). In-process,
    /// the session's own footage and file reads are confined too, so a project from a read root
    /// can't show footage from outside it.
    pub fn confine(&mut self, confinement: Confinement) {
        if let Target::Headless(s) = &mut self.target {
            s.footage = std::sync::Arc::new(ConfinedFootage::new(s.footage.clone(), confinement.clone()));
            s.services = std::sync::Arc::new(ConfinedServices::new(s.services.clone(), confinement.clone()));
        }
        self.confinement = Some(confinement);
    }

    pub fn confinement(&self) -> Option<&Confinement> {
        self.confinement.as_ref()
    }

    pub fn is_bridge(&self) -> bool {
        matches!(self.target, Target::Bridge(_))
    }

    pub fn session(&mut self) -> Option<&mut Session> {
        match &mut self.target {
            Target::Headless(s) => Some(s),
            Target::Bridge(_) => None,
        }
    }

    /// Run an engine command by id (checked against the [`Confinement`], if any).
    pub fn exec(&mut self, id: &str, params: Value) -> Result<Value> {
        let params = if params.is_null() { json!({}) } else { params };
        if let Some(c) = self.confinement.clone() {
            for lookup in c.check_command(id, &params)? {
                let found = match lookup {
                    Lookup::ProjectFile | Lookup::QueueDefault => self.run("project.summary", json!({}))?,
                    Lookup::RenderOutputs => self.run("renderQueue.list", json!({}))?,
                };
                c.check_lookup(&lookup, &found)?;
            }
        }
        self.run(id, params)
    }

    fn run(&mut self, id: &str, params: Value) -> Result<Value> {
        match &mut self.target {
            Target::Headless(s) => {
                let r = s.execute_checked(id, params);
                s.drain_events();
                Ok(r?)
            }
            Target::Bridge(b) => b.call("engine.execute", json!({"command": id, "params": params})),
        }
    }

    /// Query whether the current project protects `path`. This is an advisory snapshot, not a
    /// reservation: use [`Self::render_to_file`] to validate and write in the owning session.
    /// Relative paths are resolved against this process's folder, because the app has its own.
    pub fn check_output(&mut self, path: &str) -> Result<()> {
        let absolute = std::path::absolute(path).map_err(|e| Error::BadArgs(format!("cannot resolve {path}: {e}")))?;
        let found = self.exec("file.checkOutput", json!({"path": absolute.to_string_lossy()}))?;
        match found.get("blocked").and_then(Value::as_bool) {
            Some(false) => Ok(()),
            _ => Err(Error::BadArgs(found.get("reason").and_then(Value::as_str).unwrap_or("this file is one the project reads").to_string())),
        }
    }

    /// Call a raw control-channel method (bridge only).
    pub fn control(&mut self, method: &str, params: Value) -> Result<Value> {
        match &mut self.target {
            Target::Bridge(b) => b.call(method, if params.is_null() { json!({}) } else { params }),
            Target::Headless(_) => Err(Error::BadArgs(NEED_BRIDGE.into())),
        }
    }

    /// Render a comp frame (comp time `time` seconds, default the CTI) as PNG, longest side at most
    /// `max_side` (0 = full size).
    pub fn render(&mut self, comp: Option<&Value>, time: Option<f64>, max_side: u32) -> Result<Frame> {
        self.render_with(comp, time, max_side, false)
    }

    /// [`Backend::render`]; `transparent` keeps the frame's alpha instead of compositing it over
    /// the comp's background colour.
    pub fn render_with(&mut self, comp: Option<&Value>, time: Option<f64>, max_side: u32, transparent: bool) -> Result<Frame> {
        self.render_output(comp, time, max_side, transparent, None)
    }

    /// Render and save in the owning session. In bridge mode, checking and writing are one
    /// desktop operation, so another command cannot make the destination protected in between.
    pub fn render_to_file(&mut self, comp: Option<&Value>, time: Option<f64>, max_side: u32, transparent: bool, path: &str) -> Result<Frame> {
        if let Some(confinement) = self.confinement() {
            confinement.check(path, crate::confine::Access::Write)?;
        }
        let absolute = std::path::absolute(path).map_err(|e| Error::BadArgs(format!("cannot resolve {path}: {e}")))?;
        self.render_output(comp, time, max_side, transparent, Some(&absolute.to_string_lossy()))
    }

    fn render_output(&mut self, comp: Option<&Value>, time: Option<f64>, max_side: u32, transparent: bool, path: Option<&str>) -> Result<Frame> {
        match &mut self.target {
            Target::Headless(s) => {
                if let Some(path) = path {
                    s.refuse_export(path)?;
                }
                let cid = s.resolve_comp(comp).map_err(|_| no_comp(comp))?;
                let t = time.map(Tick::from_seconds_f64).unwrap_or_else(|| s.time());
                let (w, h, rgba) = s.render_rgba8_alpha(cid, t, max_side, transparent)?;
                let png = encode_png(w, h, rgba, max_side)?;
                if let Some(path) = path {
                    s.write_output(path, &png)?;
                }
                Ok(Frame { png, comp: json!(cid.0), time: t.seconds(), width: w, height: h })
            }
            Target::Bridge(b) => {
                let mut p = json!({"max_side": max_side, "base64": true});
                if let Some(path) = path {
                    p["path"] = json!(path);
                    p["write"] = json!(true);
                }
                if transparent {
                    p["transparent"] = json!(true);
                }
                if let Some(t) = time {
                    p["time"] = json!(t);
                }
                if let Some(c) = comp.filter(|c| !c.is_null()) {
                    p["comp"] = c.clone();
                }
                let r = b.call("render.frame", p)?;
                if let Some(path) = path
                    && r.get("path").and_then(Value::as_str) != Some(path)
                {
                    return Err(Error::App("render.frame did not confirm the saved path; update the connected app before saving frames".into()));
                }
                let data = r.get("png").and_then(Value::as_str).ok_or_else(|| Error::App("render.frame returned no `png`".into()))?;
                let raw = base64::decode(data).ok_or_else(|| Error::App("render.frame: bad base64".into()))?;
                let (png, width, height) = recompress_png(&raw, max_side)?;
                Ok(Frame { png, comp: r["comp"].clone(), time: r["time"].as_f64().unwrap_or(0.0), width, height })
            }
        }
    }
}

fn no_comp(comp: Option<&Value>) -> Error {
    match comp.filter(|c| !c.is_null()) {
        Some(c) => Error::BadArgs(format!("no composition {c} (see get_project)")),
        None => Error::BadArgs("no active composition: create one (execute_command comp.new) or open a project".into()),
    }
}

pub const NEED_BRIDGE: &str =
    "this tool drives the live app: start it with `effectcraft --control 9877` and run the MCP server as `effectcraft-cli mcp --bridge 9877`";
