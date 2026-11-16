//! The helper side of the protocol: [`serve`] answers EffectCraft's requests with any set of
//! [`EffectPlugin`]s, so a helper program only has to provide its effects ([`Provider`]).
//! The fake helper in the tests and the OpenFX helper both use it.
//!
//! During a render the plug-in's [`PluginHost`] calls become requests to EffectCraft
//! ([`Reply::InputAt`], ...). While a helper waits for the answer, EffectCraft may send a nested
//! [`Request::Render`] (an effect of this helper further up the same layer's stack); it is
//! served right away, on the same stack, before the wait goes on.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::sync::Arc;

use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginHost, PluginImage, PluginParamKind, PluginParams};

use crate::frames::Exchange;
use crate::protocol::{ImageRef, MAX_LINE, PROTOCOL_VERSION, RemoteEffect, RenderRequest, Reply, Request, reply_line};

/// How deep nested renders may go.
const MAX_DEPTH: u32 = 16;

/// What a helper program offers.
pub trait Provider {
    /// The helper's display name (`OpenFX`).
    fn name(&self) -> String;
    /// The helper's own version.
    fn version(&self) -> String {
        String::new()
    }
    /// File extensions [`Provider::load`] accepts (`["ofx"]`).
    fn loads(&self) -> Vec<String> {
        vec![]
    }
    /// The effects available at start-up, and errors worth reporting.
    fn effects(&mut self) -> (Vec<Arc<dyn EffectPlugin>>, Vec<String>);
    /// Load effects from a file or folder the user picked.
    fn load(&mut self, _path: &str) -> Result<(Vec<Arc<dyn EffectPlugin>>, Vec<String>), String> {
        Err("this helper does not load files".into())
    }
    /// Whether EffectCraft should ask for padding before each render of `effect`.
    fn wants_padding(&self, _effect: &dyn EffectPlugin) -> bool {
        true
    }
    /// The effect instance is gone.
    fn destroy(&mut self, _instance: u64) {}
}

/// Serve requests on stdin / stdout until EffectCraft says `shutdown` or closes the pipe.
pub fn serve(provider: &mut dyn Provider) -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve_io(provider, stdin.lock(), stdout.lock())
}

/// [`serve`] on any reader / writer.
pub fn serve_io(provider: &mut dyn Provider, input: impl BufRead, output: impl Write) -> Result<(), String> {
    let mut s = Server { provider, input: Box::new(input), output: Box::new(output), exchange: None, effects: HashMap::new(), depth: 0, features: vec![] };
    loop {
        let Some(req) = s.next_request()? else { return Ok(()) };
        if matches!(req, Request::Shutdown) {
            return Ok(());
        }
        s.handle(req)?;
    }
}

struct Server<'p> {
    provider: &'p mut dyn Provider,
    input: Box<dyn BufRead + 'p>,
    output: Box<dyn Write + 'p>,
    exchange: Option<Exchange>,
    effects: HashMap<String, Arc<dyn EffectPlugin>>,
    depth: u32,
    /// What EffectCraft listed in `hello`: optional callbacks and manifest kinds (`gradient_params`…).
    features: Vec<String>,
}

impl Server<'_> {
    /// The next request; `None` when the pipe closed.
    fn next_request(&mut self) -> Result<Option<Request>, String> {
        loop {
            let mut buf = vec![];
            let n = (&mut self.input).take(MAX_LINE).read_until(b'\n', &mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                return Ok(None);
            }
            let line = String::from_utf8_lossy(&buf);
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Request>(line.trim()) {
                Ok(r) => return Ok(Some(r)),
                Err(e) => self.send(&Reply::Error { message: format!("bad request: {e}") })?,
            }
        }
    }

    fn send(&mut self, r: &Reply) -> Result<(), String> {
        let line = reply_line(r)?;
        self.output.write_all(line.as_bytes()).and_then(|_| self.output.flush()).map_err(|e| e.to_string())
    }

    fn describe(&mut self, list: Vec<Arc<dyn EffectPlugin>>, errors: Vec<String>) -> Result<(), String> {
        let (mut effects, mut errors) = (vec![], errors);
        let gradients = self.features.iter().any(|f| f == "gradient_params");
        for e in list {
            let manifest = e.manifest().clone();
            // A host that didn't list `gradient_params` refuses such a manifest: say why here.
            if !gradients && manifest.params.iter().any(|p| matches!(p.kind, PluginParamKind::Gradient { .. })) {
                errors.push(format!("{}: needs a newer EffectCraft (gradient parameters)", manifest.id));
                continue;
            }
            effects.push(RemoteEffect { padding: self.provider.wants_padding(e.as_ref()), time_varying: e.reads_other_times(), manifest: manifest.clone() });
            self.effects.insert(manifest.id, e);
        }
        self.send(&Reply::Effects { effects, errors })
    }

    /// Answer one request (anything but `shutdown`).
    fn handle(&mut self, req: Request) -> Result<(), String> {
        match req {
            Request::Hello { version, frames, features } => {
                self.features = features;
                if version != PROTOCOL_VERSION {
                    return self.send(&Reply::Error { message: format!("this helper speaks protocol {PROTOCOL_VERSION}, not {version}") });
                }
                match Exchange::open(Path::new(&frames)) {
                    Ok(x) => self.exchange = Some(x),
                    Err(e) => return self.send(&Reply::Error { message: format!("can't open the frame exchange file: {e}") }),
                }
                let (name, helper_version, loads) = (self.provider.name(), self.provider.version(), self.provider.loads());
                self.send(&Reply::Hello { version: PROTOCOL_VERSION, name, helper_version, loads })
            }
            Request::Describe => {
                let (list, errors) = self.provider.effects();
                self.describe(list, errors)
            }
            Request::Load { path } => match self.provider.load(&path) {
                Ok((list, errors)) => self.describe(list, errors),
                Err(message) => self.send(&Reply::Error { message }),
            },
            Request::Padding { effect, instance, time, scale, layer_size, flat, texts } => {
                let Some(e) = self.effects.get(&effect).cloned() else {
                    return self.send(&Reply::Error { message: format!("unknown effect `{effect}`") });
                };
                let params = PluginParams { manifest: e.manifest(), flat: &flat, texts: &texts };
                let pixels = e.padding(&params, time, scale, layer_size, instance);
                self.send(&Reply::Padding { pixels })
            }
            Request::Render(r) => match self.render(&r) {
                Ok(()) => self.send(&Reply::Done),
                Err(message) => self.send(&Reply::Error { message }),
            },
            Request::Destroy { instance } => {
                self.provider.destroy(instance);
                Ok(())
            }
            Request::Ping => self.send(&Reply::Pong),
            Request::Image { .. } | Request::Params { .. } | Request::Transform { .. } | Request::Camera { .. } | Request::Lights { .. } => {
                self.send(&Reply::Error { message: "no callback is waiting for an answer".into() })
            }
            Request::Shutdown => Ok(()),
        }
    }

    fn render(&mut self, r: &RenderRequest) -> Result<(), String> {
        let e = self.effects.get(&r.effect).cloned().ok_or_else(|| format!("unknown effect `{}`", r.effect))?;
        if self.depth >= MAX_DEPTH {
            return Err("renders nested too deeply".into());
        }
        let exchange = self.exchange.as_mut().ok_or("no frame exchange file (hello first)")?;
        let mut pixels = exchange.read(&r.frame)?;
        let params = PluginParams { manifest: e.manifest(), flat: &r.flat, texts: &r.texts };
        let mut frame = PluginFrame {
            width: r.frame.width,
            height: r.frame.height,
            pixels: &mut pixels,
            scale: r.frame.scale,
            origin: r.frame.origin,
            layer_size: r.layer_size,
        };
        self.depth += 1;
        let result = {
            let host = Callbacks { server: RefCell::new(&mut *self), req: r };
            e.render_v2(&mut frame, &params, r.time, &host)
        };
        self.depth = self.depth.saturating_sub(1);
        result?;
        let exchange = self.exchange.as_mut().ok_or("no frame exchange file")?;
        exchange.write(&r.frame, &pixels)
    }

    /// Ask EffectCraft something during a render and wait for the answer, serving nested
    /// requests meanwhile.
    fn ask(&mut self, q: &Reply) -> Result<Request, String> {
        self.send(q)?;
        loop {
            let req = self.next_request()?.ok_or("EffectCraft closed the pipe")?;
            match req {
                Request::Image { .. } | Request::Params { .. } | Request::Transform { .. } | Request::Camera { .. } | Request::Lights { .. } => return Ok(req),
                Request::Shutdown => return Err("EffectCraft asked the helper to shut down".into()),
                other => self.handle(other)?,
            }
        }
    }

    fn image(&mut self, q: &Reply) -> Option<PluginImage> {
        let Ok(Request::Image { image: Some(at) }) = self.ask(q) else { return None };
        self.read_image(&at)
    }

    fn read_image(&mut self, at: &ImageRef) -> Option<PluginImage> {
        let pixels = self.exchange.as_mut()?.read(at).ok()?;
        Some(PluginImage { width: at.width, height: at.height, pixels, origin: at.origin, scale: at.scale })
    }
}

/// The [`PluginHost`] a helper's plug-in renders with: questions go back to EffectCraft.
struct Callbacks<'a, 'p> {
    server: RefCell<&'a mut Server<'p>>,
    req: &'a RenderRequest,
}

impl PluginHost for Callbacks<'_, '_> {
    fn input_at(&self, layer_time: f64) -> Option<PluginImage> {
        self.server.try_borrow_mut().ok()?.image(&Reply::InputAt { time: layer_time })
    }
    fn layer_input(&self, param_id: &str, layer_time: f64) -> Option<PluginImage> {
        self.server.try_borrow_mut().ok()?.image(&Reply::LayerInput { param: param_id.to_string(), time: layer_time })
    }
    fn layer_transform(&self, param_id: &str, layer_time: f64) -> Option<effectcraft_effects::plugin::LayerTransform> {
        let mut s = self.server.try_borrow_mut().ok()?;
        if !s.features.iter().any(|f| f == "layer_transform") {
            return None;
        }
        match s.ask(&Reply::LayerTransform { param: param_id.to_string(), time: layer_time }) {
            Ok(Request::Transform { transform }) => transform,
            _ => None,
        }
    }
    fn comp_camera(&self) -> Option<effectcraft_effects::plugin::CompCamera> {
        let mut s = self.server.try_borrow_mut().ok()?;
        if !s.features.iter().any(|f| f == "comp_camera") {
            return None;
        }
        match s.ask(&Reply::CompCamera { time: None }) {
            Ok(Request::Camera { camera }) => camera,
            _ => None,
        }
    }
    fn comp_lights(&self, layer_time: f64) -> Option<Vec<effectcraft_effects::plugin::CompLightInfo>> {
        let mut s = self.server.try_borrow_mut().ok()?;
        if !s.features.iter().any(|f| f == "comp_lights") {
            return None;
        }
        match s.ask(&Reply::CompLights { time: layer_time }) {
            Ok(Request::Lights { lights }) => lights,
            _ => None,
        }
    }
    fn comp_camera_at(&self, layer_time: f64) -> Option<effectcraft_effects::plugin::CompCamera> {
        let mut s = self.server.try_borrow_mut().ok()?;
        if !s.features.iter().any(|f| f == "comp_camera_time") {
            return None;
        }
        match s.ask(&Reply::CompCamera { time: Some(layer_time) }) {
            Ok(Request::Camera { camera }) => camera,
            _ => None,
        }
    }
    fn params_at(&self, layer_time: f64) -> Option<(Vec<f64>, Vec<String>)> {
        match self.server.try_borrow_mut().ok()?.ask(&Reply::ParamsAt { time: layer_time }) {
            Ok(Request::Params { values }) => values,
            _ => None,
        }
    }
    fn frame_rate(&self) -> f64 {
        self.req.frame_rate
    }
    fn time(&self) -> f64 {
        self.req.time
    }
    fn instance_key(&self) -> u64 {
        self.req.instance
    }
    fn frame_range(&self) -> Option<(f64, f64)> {
        self.req.frame_range
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_effects::plugin::{PluginManifest, PluginParam};

    struct Ramp(PluginManifest);

    impl EffectPlugin for Ramp {
        fn manifest(&self) -> &PluginManifest {
            &self.0
        }
        fn render(&self, _frame: &mut PluginFrame, _params: &PluginParams, _time: f64) -> Result<(), String> {
            Ok(())
        }
    }

    struct OneRamp;

    impl Provider for OneRamp {
        fn name(&self) -> String {
            "Ramps".into()
        }
        fn effects(&mut self) -> (Vec<Arc<dyn EffectPlugin>>, Vec<String>) {
            let ramp = PluginParam { id: "ramp".into(), name: "Ramp".into(), kind: PluginParamKind::Gradient { default: None }, group: String::new() };
            let m = PluginManifest {
                api: 2,
                id: "org.test.ramp".into(),
                name: "Ramp".into(),
                category: "Test".into(),
                version: "1".into(),
                author: String::new(),
                description: String::new(),
                params: vec![ramp],
            };
            (vec![Arc::new(Ramp(m))], vec![])
        }
    }

    /// The `effects` reply to `describe` after `hello`.
    fn described(hello: &str) -> String {
        let mut out = vec![];
        serve_io(&mut OneRamp, format!("{hello}\n{{\"op\":\"describe\"}}\n").as_bytes(), &mut out).unwrap();
        String::from_utf8(out).unwrap().lines().find(|l| l.contains(r#""op":"effects""#)).unwrap_or_default().to_string()
    }

    /// A manifest with a gradient parameter goes only to hosts that listed `gradient_params`
    /// (older hosts refuse it); the others get an error saying why.
    #[test]
    fn gradient_effects_are_described_only_to_hosts_that_take_them() {
        let old = described(r#"{"op":"hello","version":1,"frames":"no-such-file"}"#);
        assert!(old.contains(r#""effects":[]"#) && old.contains("needs a newer EffectCraft"), "{old}");
        let new = described(r#"{"op":"hello","version":1,"frames":"no-such-file","features":["gradient_params"]}"#);
        assert!(new.contains(r#""id":"org.test.ramp""#) && !new.contains("needs a newer"), "{new}");
    }
}
