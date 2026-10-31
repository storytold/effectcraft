//! Why a subcommand failed. A [`UsageError`] is the command line's fault (exit 2); every other
//! [`CliError`] is a failure while running it (exit 1).

use std::path::PathBuf;

/// What the command line got wrong that clap can't say itself: printed as clap prints its own
/// errors, exit 2.
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("'{0}' is not a project: expected a path ending in .ecproj")]
    NotAProject(String),
    #[error("'{0}' is not a finite number")]
    NotANumber(String),
    #[error("missing arguments: usage `{0}`")]
    Missing(&'static str),
    #[error("too many arguments: usage `{0}`")]
    Extra(&'static str),
    #[error("two projects given, '{0}' and '{1}': name one")]
    TwoProjects(String, String),
    #[error("--bridge drives the running app's project; use `exec file.open` to open another")]
    BridgeWithProject,
    #[error("--template renders a new instance; it cannot be combined with a project, --demo, --bridge, --queue or --comp")]
    TemplateWithProject,
    #[error("params are given twice, as --params and as an argument: use one")]
    TwoParams,
    #[error("params are not valid JSON: {0}")]
    BadParams(#[source] serde_json::Error),
    #[error("--commands - is given twice: standard input is read once")]
    StdinTwice,
    #[error("run needs <command-id> [<json>] pairs, --args-file or --commands FILE|-")]
    NoSteps,
    /// A `run` pair whose params are not JSON.
    #[error("{id}: params are not valid JSON: {source}")]
    BadValue { id: String, source: serde_json::Error },
    /// `--args-file` or `--commands` could not be read or has no steps.
    #[error("{0}")]
    BadCommands(CommandsFileError),
}

/// Why `run --commands` (or `--args-file`) refused its input.
#[derive(Debug, thiserror::Error)]
pub enum CommandsFileError {
    #[error("--args-file {path}: {source}")]
    ArgsFile { path: String, source: std::io::Error },
    #[error("--commands {source_name}: {source}")]
    Read { source_name: String, source: std::io::Error },
    #[error("--commands {0}: more than {1} MiB of steps")]
    TooBig(String, u64),
    #[error("--commands {0}: not UTF-8 text")]
    NotUtf8(String),
    #[error("--commands {0}: {1}")]
    Json(String, serde_json::Error),
    #[error("--commands {0}: not a JSON array")]
    NotAnArray(String),
    #[error("--commands {0}: more than {1} steps")]
    TooMany(String, usize),
    #[error("--commands {source_name}: step {step} (line {line}): {error}")]
    Line { source_name: String, step: usize, line: usize, error: serde_json::Error },
    #[error("--commands {source_name}: step {step} ({place}): {error}")]
    Step { source_name: String, step: usize, place: String, error: StepError },
    #[error("--commands {0}: no commands")]
    Empty(String),
}

/// What is wrong with one `--commands` entry.
#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error("a step must be an object such as {{\"command\": \"comp.new\", \"params\": {{}}}}")]
    NotAnObject,
    #[error("give `command` or `id`, not both")]
    BothIds,
    #[error("no `command`")]
    NoCommand,
    #[error("`command` must be a string, a command id")]
    IdNotAString,
    #[error("`params` of {0} must be a JSON object")]
    ParamsNotAnObject(String),
    #[error("unknown key `{0}`: a step has `command` (or `id`) and `params`")]
    UnknownKey(String),
}

/// Why a template instance could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("--out {out} would overwrite {input} {path}")]
    OutIsInput { out: String, input: &'static str, path: String },
    #[error("cannot read values {path}: {source}")]
    ReadValues { path: String, source: std::io::Error },
    #[error("values {0}: JSON exceeds 1 MiB")]
    ValuesTooBig(String),
    #[error("values {path}: invalid JSON: {source}")]
    ValuesJson { path: String, source: serde_json::Error },
    #[error("values {0}: expected a JSON object mapping control names to values")]
    ValuesNotAnObject(String),
    #[error("cannot read {path}: {message}")]
    ReadTemplate { path: String, message: String },
    #[error("{0}")]
    Engine(#[from] effectcraft_engine::EngineError),
    #[error("template import did not return a composition")]
    NoImportedComp,
    #[error("cannot create template render composition")]
    NoRenderComp,
    #[error("template render composition is missing")]
    RenderCompMissing,
    #[error("cannot create template instance")]
    NoInstance,
    #[error("control `{0}` is a media control: give it a file path")]
    MediaNeedsPath(String),
    #[error("control `{control}`: cannot import {path}: {error}")]
    MediaImport { control: String, path: String, error: serde_json::Value },
    #[error("control `{control}`: {path} did not import as footage")]
    MediaNotFootage { control: String, path: String },
}

/// Why a `bench` could not run.
#[derive(Debug, thiserror::Error)]
pub enum BenchError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{flag} {path}: {message}")]
    Weights { flag: &'static str, path: String, message: String },
    #[error("cannot read {0}: {1}")]
    ReadDir(String, std::io::Error),
    #[error("{0}: {1}")]
    Image(PathBuf, image::ImageError),
    #[error("{frames} frames in {frames_dir} but {masks} masks in {truth_dir}: they must match one to one")]
    CountMismatch { frames: usize, frames_dir: String, masks: usize, truth_dir: String },
    #[error("frame and truth file names must match one to one, with unique stems (extensions may differ)")]
    NameMismatch,
    #[error("{0}")]
    Scenario(String),
    #[error("--gpu: no usable GPU adapter ({0})")]
    NoGpu(String),
    #[error("no composition")]
    NoComp,
}

/// Why a subcommand failed: printed as `error: {e}`.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Usage(#[from] UsageError),
    #[error(transparent)]
    Automation(#[from] effectcraft_automation::Error),
    #[error(transparent)]
    Engine(#[from] effectcraft_engine::EngineError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Template(#[from] TemplateError),
    #[error(transparent)]
    Bench(#[from] BenchError),
    #[error("--autosave: no config directory; set EFFECTCRAFT_CONFIG_DIR")]
    NoConfigDir,
    #[error("--gpu: no usable GPU adapter ({0})")]
    NoGpu(String),
    #[error("{source} (use --project F to save in place, or --save-as F)")]
    SaveWithoutProject { source: effectcraft_automation::Error },
    #[error("cannot read {path}: {source}")]
    ReadScript { path: String, source: std::io::Error },
    /// A script's error, as `file:line:col: message`.
    #[error("{0}")]
    Script(String),
    /// The failure is already in the JSON result on stdout.
    #[error("")]
    Reported,
    #[error("{0}")]
    RenderFailed(String),
    #[error("{id}: {source} (step {step})")]
    Step { id: String, step: usize, source: effectcraft_automation::Error },
    #[error("{id}: {message} (step {step})")]
    StepRender { id: String, step: usize, message: String },
    #[error("render: cannot resolve --out {path}: {source}")]
    ResolveOut { path: String, source: std::io::Error },
    #[error("{0}")]
    Items(String),
    #[error("render: no composition")]
    NoComp,
}

impl CliError {
    /// The process exit code: 2 for a usage error, else 1.
    pub fn exit_code(&self) -> i32 {
        if matches!(self, Self::Usage(_)) { 2 } else { 1 }
    }
}

pub type Result<T = (), E = CliError> = std::result::Result<T, E>;
