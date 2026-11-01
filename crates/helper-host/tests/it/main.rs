//! The helper protocol end to end against a fake helper, written in safe Rust with
//! [`effectcraft_helper_host::serve`]. This binary is both sides: started with
//! `--effectcraft-helper` it is the helper, otherwise it runs the checks, starting itself as
//! the helper. (`harness = false`: libtest would reject the helper argument.)

use std::sync::Arc;
use std::time::Duration;

use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginHost, PluginImage, PluginManifest, PluginParam, PluginParamKind, PluginParams, plugin};
use effectcraft_helper_host::protocol::HELPER_ARG;
use effectcraft_helper_host::serve::{Provider, serve};

fn manifest(id: &str, params: Vec<PluginParam>) -> PluginManifest {
    PluginManifest {
        api: 2,
        id: id.into(),
        name: id.rsplit('.').next().unwrap_or(id).into(),
        category: "Helper Test".into(),
        version: "1".into(),
        author: String::new(),
        description: String::new(),
        params,
    }
}

fn amount() -> PluginParam {
    PluginParam {
        id: "amount".into(),
        name: "Amount".into(),
        group: String::new(),
        kind: PluginParamKind::Slider { default: 1.0, min: 0.0, max: 1.0, slider_min: None, slider_max: None, decimals: 2, integer: false },
    }
}

/// What each fake effect does.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// RGB → amount·(1 − RGB).
    Invert,
    /// Asks for 3 pixels of padding; fills the frame with its padding as red.
    Pad,
    /// Replaces the frame with the input one frame earlier, and puts `amount` from
    /// `params_at(0)` in alpha of pixel 0.
    Echo,
    /// Exits the process mid-render.
    Crash,
    /// Never answers.
    Hang,
}

struct Fake {
    m: PluginManifest,
    kind: Kind,
}

impl EffectPlugin for Fake {
    fn manifest(&self) -> &PluginManifest {
        &self.m
    }
    fn render(&self, _: &mut PluginFrame, _: &PluginParams, _: f64) -> Result<(), String> {
        Err("needs render_v2".into())
    }
    fn padding(&self, _: &PluginParams, _: f64, _: f64, _: [f64; 2], _: u64) -> u32 {
        if self.kind == Kind::Pad { 3 } else { 0 }
    }
    fn render_v2(&self, frame: &mut PluginFrame, params: &PluginParams, _time: f64, host: &dyn PluginHost) -> Result<(), String> {
        match self.kind {
            Kind::Invert => {
                let a = params.f("amount") as f32;
                for p in frame.pixels.iter_mut() {
                    for c in &mut p[..3] {
                        *c = a * (1.0 - *c);
                    }
                }
            }
            Kind::Pad => frame.pixels.iter_mut().for_each(|p| *p = [3.0, 0.0, 0.0, 1.0]),
            Kind::Echo => {
                let prev = host.input_at(host.time() - 1.0 / host.frame_rate()).ok_or("no input at the previous frame")?;
                if prev.pixels.len() != frame.pixels.len() {
                    return Err("previous frame has another size".into());
                }
                frame.pixels.copy_from_slice(&prev.pixels);
                let (flat, _) = host.params_at(0.0).ok_or("no params at 0")?;
                if let (Some(p), Some(a)) = (frame.pixels.first_mut(), flat.first()) {
                    p[3] = *a as f32;
                }
            }
            Kind::Crash => std::process::exit(3),
            Kind::Hang => std::thread::sleep(Duration::from_secs(3600)),
        }
        Ok(())
    }
}

struct FakeProvider;

impl Provider for FakeProvider {
    fn name(&self) -> String {
        "Fake".into()
    }
    fn effects(&mut self) -> (Vec<Arc<dyn EffectPlugin>>, Vec<String>) {
        let v: Vec<Arc<dyn EffectPlugin>> = [("invert", Kind::Invert), ("pad", Kind::Pad), ("echo", Kind::Echo), ("crash", Kind::Crash), ("hang", Kind::Hang)]
            .into_iter()
            .map(|(n, kind)| Arc::new(Fake { m: manifest(&format!("org.test.helper.{n}"), vec![amount()]), kind }) as Arc<dyn EffectPlugin>)
            .collect();
        (v, vec!["fake helper: a warning".into()])
    }
}

/// The host side of a render in these checks: the input at any time is a flat grey of that
/// time; with `nest`, asking for the input renders `invert` through the same helper first.
struct TestHost {
    w: u32,
    h: u32,
    nest: bool,
}

impl PluginHost for TestHost {
    fn input_at(&self, t: f64) -> Option<PluginImage> {
        let mut pixels = vec![[t as f32, t as f32, t as f32, 1.0]; (self.w * self.h) as usize];
        if self.nest {
            let inv = plugin("org.test.helper.invert")?;
            let m = inv.manifest().clone();
            let mut f =
                PluginFrame { width: self.w, height: self.h, pixels: &mut pixels, scale: 1.0, origin: [0.0; 2], layer_size: [self.w as f64, self.h as f64] };
            inv.render_v2(&mut f, &PluginParams { manifest: &m, flat: &[1.0], texts: &[] }, t, &TestHost { nest: false, ..*self }).ok()?;
        }
        Some(PluginImage { width: self.w, height: self.h, pixels, origin: [0.0; 2], scale: 1.0 })
    }
    fn layer_input(&self, _: &str, _: f64) -> Option<PluginImage> {
        None
    }
    fn params_at(&self, _: f64) -> Option<(Vec<f64>, Vec<String>)> {
        Some((vec![0.25], vec![]))
    }
    fn frame_rate(&self) -> f64 {
        10.0
    }
    fn time(&self) -> f64 {
        1.0
    }
    fn instance_key(&self) -> u64 {
        7
    }
}

fn render(id: &str, pixels: &mut Vec<[f32; 4]>, w: u32, h: u32, amount: f64, nest: bool) -> Result<(), String> {
    let p = plugin(id).ok_or_else(|| format!("{id} is not registered"))?;
    let m = p.manifest().clone();
    let params = PluginParams { manifest: &m, flat: &[amount], texts: &[] };
    let host = TestHost { w, h, nest };
    let pad = p.padding(&params, 1.0, 1.0, [w as f64, h as f64], 7);
    let (w, h) = (w + 2 * pad, h + 2 * pad);
    pixels.resize((w * h) as usize, [0.0; 4]);
    let mut f = PluginFrame { width: w, height: h, pixels, scale: 1.0, origin: [pad as f64; 2], layer_size: [w as f64, h as f64] };
    p.render_v2(&mut f, &params, 1.0, &host)
}

fn check(name: &str, ok: bool, detail: impl std::fmt::Debug) -> bool {
    println!("{} {name}{}", if ok { "ok  " } else { "FAIL" }, if ok { String::new() } else { format!(": {detail:?}") });
    ok
}

fn run_checks() -> bool {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return check("current exe", false, e),
    };
    let state = std::env::temp_dir().join(format!("ec-fake-helper-state-{}", std::process::id()));
    effectcraft_helper_host::blocklist::set_state_dir(state.clone());
    let mut all = true;

    // Discovery + describe.
    let r = effectcraft_helper_host::scan_paths(std::slice::from_ref(&exe), false);
    let loaded = r["loaded"].as_array().map_or(0, Vec::len);
    all &= check("scan registers the helper's effects", loaded == 5, &r);
    all &= check("helper warnings are reported", r["errors"].to_string().contains("a warning"), &r);
    all &= check(
        "a second scan doesn't start it twice",
        effectcraft_helper_host::scan_paths(std::slice::from_ref(&exe), false)["helpers"].as_array().is_some_and(Vec::is_empty),
        "",
    );
    let list = effectcraft_helper_host::list();
    all &= check("list names the helper", list["helpers"][0]["name"] == "Fake" && list["helpers"][0]["running"] == true, &list);

    // Plain render through the exchange file.
    let mut px = vec![[0.25, 0.5, 1.0, 1.0]; 6];
    let r = render("org.test.helper.invert", &mut px, 3, 2, 1.0, false);
    all &= check("invert renders", r.is_ok() && px.iter().all(|p| (p[0] - 0.75).abs() < 1e-6 && p[2].abs() < 1e-6 && p[3] == 1.0), (&r, &px));

    // Padding.
    let mut px = vec![[0.0; 4]; 4];
    let r = render("org.test.helper.pad", &mut px, 2, 2, 1.0, false);
    all &= check("padding grows the frame", r.is_ok() && px.len() == 64 && px[0][0] == 3.0, (&r, px.len()));

    // Callbacks: the input at another time and parameters at another time.
    let mut px = vec![[0.0; 4]; 4];
    let r = render("org.test.helper.echo", &mut px, 2, 2, 1.0, false);
    all &= check("input_at / params_at round-trip", r.is_ok() && (px[1][0] - 0.9).abs() < 1e-6 && px[0][3] == 0.25, (&r, &px));

    // Nested: answering input_at renders another effect of the same helper.
    let mut px = vec![[0.0; 4]; 4];
    let r = render("org.test.helper.echo", &mut px, 2, 2, 1.0, true);
    all &= check("nested renders through one helper", r.is_ok() && (px[1][0] - 0.1).abs() < 1e-5, (&r, &px));

    // Several threads at once share the helper.
    let threads: Vec<_> = (0..4)
        .map(|i| {
            std::thread::spawn(move || {
                let mut px = vec![[0.0, 0.0, 0.0, 1.0]; 16];
                render("org.test.helper.invert", &mut px, 4, 4, 0.5, false).map(|_| (i, px[0][0]))
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().map_err(|_| "thread panicked".to_string()).and_then(|r| r)).collect();
    all &= check("concurrent renders", results.iter().all(|r| matches!(r, Ok((_, v)) if (*v - 0.5).abs() < 1e-6)), &results);

    // A crash mid-render is an error, the frame is untouched, and the helper comes back.
    let mut px = vec![[0.5; 4]; 4];
    let r = render("org.test.helper.crash", &mut px, 2, 2, 1.0, false);
    all &= check("a crashing helper is an error", r.as_ref().is_err_and(|e| e.contains("helper")) && px.iter().all(|p| *p == [0.5; 4]), &r);
    let mut px = vec![[0.0, 0.0, 0.0, 1.0]; 4];
    let r = render("org.test.helper.invert", &mut px, 2, 2, 1.0, false);
    all &= check("the helper restarts for the next render", r.is_ok() && px[0][0] == 1.0, &r);
    // A render that starts the stopped helper itself (no padding request first) succeeds.
    effectcraft_helper_host::stop_all();
    let r = plugin("org.test.helper.invert").ok_or("invert is not registered").and_then(|p| {
        let m = p.manifest().clone();
        let mut px = vec![[0.0, 0.0, 0.0, 1.0]; 4];
        let mut f = PluginFrame { width: 2, height: 2, pixels: &mut px, scale: 1.0, origin: [0.0; 2], layer_size: [2.0; 2] };
        let host = TestHost { w: 2, h: 2, nest: false };
        p.render_v2(&mut f, &PluginParams { manifest: &m, flat: &[1.0], texts: &[] }, 1.0, &host).map_err(|_| "render failed")?;
        Ok(px[0][0])
    });
    all &= check("a stopped helper restarts for a render without padding", r == Ok(1.0), r);

    // Effects that keep crashing it are turned off (and the list survives in the state dir).
    for _ in 0..3 {
        let _ = render("org.test.helper.crash", &mut vec![[0.0; 4]; 4], 2, 2, 1.0, false);
    }
    let r = render("org.test.helper.crash", &mut vec![[0.0; 4]; 4], 2, 2, 1.0, false);
    all &= check("repeat offenders are blocked", r.as_ref().is_err_and(|e| e.contains("turned off")), &r);
    all &= check("the block list is saved", std::fs::read_to_string(state.join("helper_blocklist.txt")).is_ok_and(|t| t.contains("org.test.helper.crash")), "");
    let r = effectcraft_helper_host::scan_paths(&[], true);
    all &= check("retry clears the block list", !effectcraft_helper_host::blocklist::is_blocked("org.test.helper.crash"), &r);

    // A hang is cut off by the timeout.
    let t0 = std::time::Instant::now();
    let r = render("org.test.helper.hang", &mut vec![[0.0; 4]; 4], 2, 2, 1.0, false);
    all &= check(
        "a hanging helper times out",
        r.as_ref().is_err_and(|e| e.contains("did not answer")) && t0.elapsed() < Duration::from_secs(30),
        (&r, t0.elapsed()),
    );

    // The restarts above use up the helper's restarts for this minute, so it can't start again.
    // That is not the fault of the effect being rendered: it fails, but is not charged and never
    // turned off (it would be after CRASH_LIMIT charged failures).
    let starts: Vec<_> = (0..4).map(|_| render("org.test.helper.invert", &mut vec![[0.0; 4]; 4], 2, 2, 1.0, false)).collect();
    all &= check(
        "a helper that can't start doesn't block the effect",
        starts.iter().all(|r| r.as_ref().is_err_and(|e| e.contains("keeps stopping")))
            && !effectcraft_helper_host::blocklist::is_blocked("org.test.helper.invert"),
        &starts,
    );

    effectcraft_helper_host::stop_all();
    let _ = std::fs::remove_dir_all(&state);
    all
}

/// The one test this binary reports to test runners.
const TEST_NAME: &str = "helper_protocol";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == HELPER_ARG) {
        if let Err(e) = serve(&mut FakeProvider) {
            eprintln!("fake helper: {e}");
            std::process::exit(1);
        }
        return;
    }
    // The libtest listing protocol cargo-nextest uses (`--list --format terse`, with and without
    // `--ignored`): one `name: test` line, then nextest runs `<binary> helper_protocol --exact`.
    if args.iter().any(|a| a == "--list") {
        if !args.iter().any(|a| a == "--ignored") {
            println!("{TEST_NAME}: test");
        }
        return;
    }
    // Keep the hang check short.
    effectcraft_helper_host::client::set_render_timeout(Duration::from_secs(3));
    if !run_checks() {
        std::process::exit(1);
    }
}
