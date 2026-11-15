//! `effectcraft-cli` stops its effect helpers when it exits, so their frame exchange files don't
//! pile up in the temp folder. This binary is both sides: started with `--effectcraft-helper` it
//! is a one-effect helper, otherwise it runs the CLI with itself on `EFFECTCRAFT_HELPER_PATH` and
//! a temp folder of its own, and checks the folder afterwards. (`harness = false`: libtest would
//! reject the helper argument.)

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use effectcraft_engine::effects::plugin::{EffectPlugin, PluginFrame, PluginManifest, PluginParams};
use effectcraft_helper_host::protocol::HELPER_ARG;
use effectcraft_helper_host::serve::{Provider, serve};

struct Keep(PluginManifest);

impl EffectPlugin for Keep {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    fn render(&self, _frame: &mut PluginFrame, _params: &PluginParams, _time: f64) -> Result<(), String> {
        Ok(())
    }
}

struct One;

impl Provider for One {
    fn name(&self) -> String {
        "Exit Test".into()
    }
    fn effects(&mut self) -> (Vec<Arc<dyn EffectPlugin>>, Vec<String>) {
        let m = PluginManifest {
            api: 2,
            id: "org.test.cli.keep".into(),
            name: "Keep".into(),
            category: "Helper Test".into(),
            version: "1".into(),
            author: String::new(),
            description: String::new(),
            params: vec![],
        };
        (vec![Arc::new(Keep(m))], vec![])
    }
}

fn exchange_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("effectcraft-helper-")).collect())
        .unwrap_or_default()
}

fn check() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let cli = std::env::var_os("NEXTEST_BIN_EXE_effectcraft_cli").unwrap_or_else(|| env!("CARGO_BIN_EXE_effectcraft-cli").into());
    // A new folder of our own (never an earlier run's), removed when `dir` drops, on every return.
    let dir = tempfile::Builder::new().prefix("ec-cli-helper-exit-").tempdir().map_err(|e| e.to_string())?;
    let tmp = dir.path();
    std::fs::create_dir(tmp.join("config")).map_err(|e| e.to_string())?;
    let out = Command::new(cli)
        .args(["run", "--empty", "effect.helpers.scan", "{}", "comp.new", r#"{"name":"C","width":8,"height":8}"#])
        .args(["layer.newSolid", "{}", "effect.apply", r#"{"effect":"org.test.cli.keep"}"#])
        .env("EFFECTCRAFT_HELPER_PATH", &exe)
        .env("EFFECTCRAFT_CONFIG_DIR", tmp.join("config"))
        .env("TMP", tmp)
        .env("TEMP", tmp)
        .env("TMPDIR", tmp)
        .output()
        .map_err(|e| e.to_string())?;
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let left = exchange_files(tmp);
    if !out.status.success() || !said.contains("org.test.cli.keep") {
        return Err(format!("the CLI didn't load the helper's effect: {said}"));
    }
    // (on Linux the files are in /dev/shm: nothing to see here)
    if !left.is_empty() {
        return Err(format!("left in the temp folder: {left:?}"));
    }
    Ok(())
}

/// The one test this binary reports to test runners.
const TEST_NAME: &str = "cli_exit_removes_helper_exchange_files";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == HELPER_ARG) {
        if let Err(e) = serve(&mut One) {
            eprintln!("exit test helper: {e}");
            std::process::exit(1);
        }
        return;
    }
    // The libtest listing protocol cargo-nextest uses (see crates/helper-host/tests/it).
    if args.iter().any(|a| a == "--list") {
        if !args.iter().any(|a| a == "--ignored") {
            println!("{TEST_NAME}: test");
        }
        return;
    }
    match check() {
        Ok(()) => println!("ok   {TEST_NAME}"),
        Err(e) => {
            println!("FAIL {TEST_NAME}: {e}");
            std::process::exit(1);
        }
    }
}
