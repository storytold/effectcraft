//! The command line, as clap types (RFC-0012). Values are parsed into their types here, so a bad
//! one is a usage error naming its flag (exit 2) before any work starts. A flag a subcommand
//! doesn't take is a usage error too, so a typo such as `--saveas` can't be silently ignored, and
//! so is a repeated one: `--out a --out b` is more likely a mistake than a choice. A value that
//! starts with `-` is written `--out=-x.png`.
//!
//! The project file may be given as `--project F.ecproj` or as a positional ending in `.ecproj`,
//! anywhere among the positionals; [`parse`] settles which, and the handlers read `project` only.

use std::ffi::OsString;

use clap::builder::NonEmptyStringValueParser;
use clap::error::ErrorKind;
use clap::{ArgGroup, Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueHint};
use serde_json::Value;

use crate::error::UsageError;

/// effectcraft-cli: headless EffectCraft
///
/// `<comp>` is a comp id or name (`-` = the active comp); `<layer>` an id, `#n` or name; `<value>`
/// is JSON (`50`, `[960,540]`, `"#ff0000"`) or a bare string. Without a project the demo project
/// is opened (`mcp`: an empty one). See docs/agents.md.
#[derive(Parser)]
#[command(name = "effectcraft-cli", version, arg_required_else_help = true)]
pub struct Cli {
    /// Print one compact JSON document on stdout; an error is {"error": …} (exit 1)
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    Info(InfoArgs),
    Commands(CommandsArgs),
    Exec(ExecArgs),
    Run(RunArgs),
    Props(PropsArgs),
    Get(GetArgs),
    Set(SetArgs),
    #[command(alias = "frame")]
    RenderFrame(RenderFrameArgs),
    Render(Box<RenderArgs>),
    Bench(Box<BenchArgs>),
    Script(ScriptArgs),
    Mcp(McpArgs),
}

/// Which project a headless session opens.
#[derive(Args, Default)]
pub struct SourceArgs {
    /// The project to open (or give a *.ecproj as an argument); it wins over --demo and --empty
    #[arg(long, value_name = "F.ecproj", value_hint = ValueHint::FilePath, value_parser = NonEmptyStringValueParser::new())]
    pub project: Option<String>,
    /// Open the demo project (the default except for `mcp`)
    #[arg(long)]
    pub demo: bool,
    /// Start with an empty project (--demo wins when both are given)
    #[arg(long)]
    pub empty: bool,
}

/// `--bridge`: drive a running app instead of a headless session.
#[derive(Args)]
pub struct BridgeArg {
    /// Drive the running `effectcraft --control PORT` (PORT or HOST:PORT) instead of a headless session
    #[arg(long, value_name = "PORT", value_parser = NonEmptyStringValueParser::new(), conflicts_with_all = ["project", "demo", "empty"])]
    pub bridge: Option<String>,
}

/// `--gpu`.
#[derive(Args)]
pub struct GpuArg {
    /// Render on the GPU compositor (Mercury GPU Acceleration) when an adapter exists; the default is the CPU
    #[arg(long)]
    pub gpu: bool,
}

/// `--save` / `--save-as`, after a command that changes the project.
#[derive(Args)]
pub struct SaveArgs {
    /// Save back to the project (--project or the *.ecproj argument)
    #[arg(long, conflicts_with = "save_as")]
    pub save: bool,
    /// Save the project as this file
    #[arg(long, value_name = "F.ecproj", value_hint = ValueHint::FilePath, value_parser = NonEmptyStringValueParser::new())]
    pub save_as: Option<String>,
}

/// The one project argument of commands that take nothing else positional.
#[derive(Args)]
pub struct FileArg {
    /// The project to open
    #[arg(value_name = "F.ecproj", value_hint = ValueHint::FilePath, value_parser = ecproj)]
    pub file: Option<String>,
}

/// Project summary: version, commands, effects, items, the active comp's layers
#[derive(Args)]
pub struct InfoArgs {
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub bridge: BridgeArg,
    #[command(flatten)]
    pub gpu: GpuArg,
    #[command(flatten)]
    pub file: FileArg,
    /// The project, set by [`parse`]
    #[arg(skip)]
    pub project: Option<String>,
}

/// List the engine commands (id, label, shortcut, params)
#[derive(Args)]
pub struct CommandsArgs {
    /// Only the commands whose id or label contains this text
    #[arg(long, value_name = "TEXT")]
    pub filter: Option<String>,
    /// Only the commands that can run now
    #[arg(long)]
    pub enabled: bool,
    /// Include each command's params JSON Schema
    #[arg(long)]
    pub schemas: bool,
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub bridge: BridgeArg,
    #[command(flatten)]
    pub gpu: GpuArg,
    #[command(flatten)]
    pub file: FileArg,
    /// The project, set by [`parse`]
    #[arg(skip)]
    pub project: Option<String>,
}

/// Run one engine command, or list them
///
/// `exec <command-id> [PARAMS-JSON] [F.ecproj]`; `exec --list` lists the commands as `commands`
/// does.
#[derive(Args)]
#[command(group(ArgGroup::new("what").args(["list", "args"]).required(true).multiple(true)))]
pub struct ExecArgs {
    /// <command-id> [<params JSON>] [F.ecproj]
    #[arg(value_name = "ARGS")]
    pub args: Vec<String>,
    /// The command's parameters, a JSON object (default {})
    #[arg(long, value_name = "JSON", value_parser = json, conflicts_with = "list")]
    pub params: Option<Value>,
    /// List the engine commands instead of running one
    #[arg(long)]
    pub list: bool,
    /// With --list: only the commands whose id or label contains this text
    #[arg(long, value_name = "TEXT", requires = "list")]
    pub filter: Option<String>,
    /// With --list: only the commands that can run now
    #[arg(long, requires = "list")]
    pub enabled: bool,
    /// With --list: include each command's params JSON Schema
    #[arg(long, requires = "list")]
    pub schemas: bool,
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub bridge: BridgeArg,
    #[command(flatten)]
    pub gpu: GpuArg,
    #[command(flatten)]
    pub save: SaveArgs,
    /// The command id, the params and the project, set by [`parse`]
    #[arg(skip)]
    pub call: Option<ExecCall>,
    #[arg(skip)]
    pub project: Option<String>,
}

/// What `exec` runs.
pub struct ExecCall {
    pub id: String,
    pub params: Value,
}

/// Run several commands in order
///
/// `run <id> [<json>] [<id> [<json>] …] [F.ecproj]`: a command id, optionally followed by its
/// params (a JSON object). More steps come from --args-file or --commands.
#[derive(Args)]
pub struct RunArgs {
    /// <id> [<json>] pairs, and the project
    #[arg(value_name = "ARGS")]
    pub args: Vec<String>,
    /// The same, one argument per line of FILE (no command-line length limit)
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath, value_parser = NonEmptyStringValueParser::new())]
    pub args_file: Option<String>,
    /// Steps from FILE, or standard input for -: a JSON array of {"command": ID, "params": {...}}
    /// (or "id" for "command"), or one such object per line (JSON Lines); at most 64 MiB and
    /// 100000 steps. Repeatable; run after the <id> <json> pairs and --args-file
    #[arg(long, value_name = "FILE|-", value_hint = ValueHint::FilePath, action = clap::ArgAction::Append, value_parser = NonEmptyStringValueParser::new())]
    pub commands: Vec<String>,
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub bridge: BridgeArg,
    #[command(flatten)]
    pub gpu: GpuArg,
    #[command(flatten)]
    pub save: SaveArgs,
    /// The `<id> [<json>]` pairs, set by [`parse`]
    #[arg(skip)]
    pub pairs: Vec<String>,
    #[arg(skip)]
    pub project: Option<String>,
}

/// A layer's property tree, with paths
///
/// `props <comp> <layer> [F.ecproj]`
#[derive(Args)]
pub struct PropsArgs {
    /// <comp> <layer> [F.ecproj]
    #[arg(value_name = "ARGS")]
    pub args: Vec<String>,
    /// Seconds (layer time) to evaluate at
    #[arg(long, value_name = "S", value_parser = number)]
    pub time: Option<f64>,
    /// How many levels of the tree to list
    #[arg(long, value_name = "N")]
    pub depth: Option<u32>,
    /// Without --json the listing is flat; with it, the tree unless --flat
    #[arg(long)]
    pub flat: bool,
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub bridge: BridgeArg,
    #[command(flatten)]
    pub gpu: GpuArg,
    /// The comp, layer and project, set by [`parse`]
    #[arg(skip)]
    pub target: Vec<String>,
    #[arg(skip)]
    pub project: Option<String>,
}

/// Read a property
///
/// `get <comp> <layer> <path> [F.ecproj]`
#[derive(Args)]
pub struct GetArgs {
    /// <comp> <layer> <path> [F.ecproj]
    #[arg(value_name = "ARGS")]
    pub args: Vec<String>,
    /// Seconds (layer time) to read at
    #[arg(long, value_name = "S", value_parser = number)]
    pub time: Option<f64>,
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub bridge: BridgeArg,
    #[command(flatten)]
    pub gpu: GpuArg,
    /// The comp, layer and path, set by [`parse`]
    #[arg(skip)]
    pub target: Vec<String>,
    #[arg(skip)]
    pub project: Option<String>,
}

/// Set a property (a keyframe when --time is given and the property is animated)
///
/// `set <comp> <layer> <path> <value> [F.ecproj]`; with --expression there is no value.
#[derive(Args)]
pub struct SetArgs {
    /// <comp> <layer> <path> [<value>] [F.ecproj]; a value is JSON (`50`, `[960,540]`) or a bare string
    #[arg(value_name = "ARGS", allow_negative_numbers = true)]
    pub args: Vec<String>,
    /// Seconds (layer time) to set at
    #[arg(long, value_name = "S", value_parser = number)]
    pub time: Option<f64>,
    /// Set the property's expression instead of its value (empty removes it)
    #[arg(long, value_name = "E")]
    pub expression: Option<String>,
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub bridge: BridgeArg,
    #[command(flatten)]
    pub gpu: GpuArg,
    #[command(flatten)]
    pub save: SaveArgs,
    /// The comp, layer, path and value, set by [`parse`]
    #[arg(skip)]
    pub target: Vec<String>,
    #[arg(skip)]
    pub project: Option<String>,
}

/// Render one frame to a PNG
///
/// With --template, renders a new instance of an Essential Graphics template instead of a
/// project.
#[derive(Args)]
pub struct RenderFrameArgs {
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub bridge: BridgeArg,
    #[command(flatten)]
    pub gpu: GpuArg,
    #[command(flatten)]
    pub file: FileArg,
    /// The comp (an id or a name; default the active comp)
    #[arg(long, value_name = "C", value_parser = NonEmptyStringValueParser::new(), conflicts_with = "template")]
    pub comp: Option<String>,
    /// Seconds (comp time)
    #[arg(long, value_name = "S", value_parser = number)]
    pub time: Option<f64>,
    /// A frame number instead of --time
    #[arg(long, value_name = "N", value_parser = number, conflicts_with = "time")]
    pub frame: Option<f64>,
    /// Scale the longest side down to PX
    #[arg(long, value_name = "PX")]
    pub max_side: Option<u32>,
    /// Scale the frame by K (1 = full size) instead of --max-side
    #[arg(long, value_name = "K", value_parser = number, conflicts_with = "max_side")]
    pub scale: Option<f64>,
    /// The PNG to write
    #[arg(long, value_name = "F.png", value_hint = ValueHint::FilePath, value_parser = NonEmptyStringValueParser::new(), default_value = "frame.png")]
    pub out: String,
    /// Keep the frame's alpha
    #[arg(long)]
    pub transparent: bool,
    #[command(flatten)]
    pub template: TemplateArgs,
    /// The project, set by [`parse`]
    #[arg(skip)]
    pub project: Option<String>,
}

/// An Essential Graphics template instance to render.
#[derive(Args)]
pub struct TemplateArgs {
    /// Render a new instance of this template instead of a project
    #[arg(long, value_name = "T.ectemplate", value_hint = ValueHint::FilePath, value_parser = NonEmptyStringValueParser::new(), conflicts_with_all = ["project", "demo"])]
    pub template: Option<String>,
    /// With --template: a JSON object of control values
    #[arg(long, value_name = "V.json", value_hint = ValueHint::FilePath, value_parser = NonEmptyStringValueParser::new(), requires = "template")]
    pub values: Option<String>,
}

/// Render a comp to a movie, image sequence or audio file, or the project's Render Queue
///
/// `render [--comp C] --out FILE`: a relative --out is relative to the working directory. The
/// format comes from --format, or the --out extension. --profile, --level, --rate-control and
/// --keyint are for HEVC and AV1; --audio-bitrate and --opus-app for WebM Opus. With --queue the
/// outputs follow the project's own settings. With --template, renders a new instance of an
/// Essential Graphics template.
#[derive(Args)]
#[command(group(ArgGroup::new("settings").args([
    "comp", "out", "format", "start", "end", "work_area", "fps", "resolution", "scale", "quality", "channels", "jpeg_quality",
    "bitrate", "prores", "audio", "profile", "level", "rate_control", "video_quality", "keyint", "webm_codec", "audio_bitrate",
    "opus_app",
]).multiple(true).conflicts_with("queue")))]
#[command(group(ArgGroup::new("target").args(["out", "queue"]).required(true).multiple(true)))]
pub struct RenderArgs {
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub gpu: GpuArg,
    #[command(flatten)]
    pub file: FileArg,
    /// The comp (an id or a name; default the active comp)
    #[arg(long, value_name = "C", value_parser = NonEmptyStringValueParser::new(), conflicts_with = "template")]
    pub comp: Option<String>,
    /// The file to write
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath, value_parser = NonEmptyStringValueParser::new())]
    pub out: Option<String>,
    /// h264|hevc|av1|prores|webm|png|jpeg|tiff|exr|tga|gif|wav|aiff (default: the --out extension)
    #[arg(long, value_name = "FORMAT", value_parser = NonEmptyStringValueParser::new())]
    pub format: Option<String>,
    /// Start second
    #[arg(long, value_name = "S", value_parser = number)]
    pub start: Option<f64>,
    /// End second
    #[arg(long, value_name = "S", value_parser = number)]
    pub end: Option<f64>,
    /// Render the comp's work area
    #[arg(long, conflicts_with_all = ["start", "end"])]
    pub work_area: bool,
    /// Frame rate
    #[arg(long, value_name = "N", value_parser = number)]
    pub fps: Option<f64>,
    /// full|half|third|quarter|K
    #[arg(long, value_name = "RES", value_parser = NonEmptyStringValueParser::new())]
    pub resolution: Option<String>,
    /// The same as --resolution
    #[arg(long, hide = true, value_name = "K", value_parser = NonEmptyStringValueParser::new(), conflicts_with = "resolution")]
    pub scale: Option<String>,
    /// best|draft
    #[arg(long, value_name = "QUALITY", value_parser = NonEmptyStringValueParser::new())]
    pub quality: Option<String>,
    /// rgb|rgba
    #[arg(long, value_name = "CHANNELS", value_parser = NonEmptyStringValueParser::new())]
    pub channels: Option<String>,
    /// JPEG quality
    #[arg(long, value_name = "N", value_parser = number)]
    pub jpeg_quality: Option<f64>,
    /// Bitrate in kbit/s
    #[arg(long, value_name = "KBPS", value_parser = number)]
    pub bitrate: Option<f64>,
    /// proxy|lt|standard|hq|4444|4444xq
    #[arg(long, value_name = "PROFILE", value_parser = NonEmptyStringValueParser::new())]
    pub prores: Option<String>,
    /// auto|on|off
    #[arg(long, value_name = "AUDIO", value_parser = NonEmptyStringValueParser::new())]
    pub audio: Option<String>,
    /// main|main10
    #[arg(long, value_name = "PROFILE", value_parser = NonEmptyStringValueParser::new())]
    pub profile: Option<String>,
    /// auto|4.1
    #[arg(long, value_name = "LEVEL", value_parser = NonEmptyStringValueParser::new())]
    pub level: Option<String>,
    /// bitrate|quality
    #[arg(long, value_name = "MODE", value_parser = NonEmptyStringValueParser::new())]
    pub rate_control: Option<String>,
    /// Video quality, 1-100
    #[arg(long, value_name = "1-100", value_parser = number)]
    pub video_quality: Option<f64>,
    /// Keyframe interval in frames
    #[arg(long, value_name = "FRAMES", value_parser = number)]
    pub keyint: Option<f64>,
    /// vp9|av1
    #[arg(long, value_name = "CODEC", value_parser = NonEmptyStringValueParser::new())]
    pub webm_codec: Option<String>,
    /// WebM Opus bitrate in kbit/s
    #[arg(long, value_name = "KBPS", value_parser = number)]
    pub audio_bitrate: Option<f64>,
    /// audio|voice
    #[arg(long, value_name = "APP", value_parser = NonEmptyStringValueParser::new())]
    pub opus_app: Option<String>,
    /// Render the project's Render Queue instead of one comp
    #[arg(long, conflicts_with = "template")]
    pub queue: bool,
    /// Progress lines on stderr when it is not a terminal, up to four a second: `progress <frames
    /// done> <frames total> <seconds elapsed> <seconds left, or - when unknown>`
    #[arg(long)]
    pub progress: bool,
    #[command(flatten)]
    pub template: TemplateArgs,
    /// The project, set by [`parse`]
    #[arg(skip)]
    pub project: Option<String>,
}

/// Time rendering, everyday operations, or the Roto Brush trackers
///
/// Plain `bench` renders one frame --n times and prints per-layer and per-effect timings; --play
/// renders consecutive frames with and without the layer cache; --gpu compares CPU and GPU
/// ms/frame for every comp at Full and Half. `--ops` times everyday operations on a large
/// generated project (open, save, auto-save, undo/redo, timeline, Project panel); `--roto` runs
/// the Roto Brush trackers (the classical engine, and MobileSAM and SAM 2.1 when installed) on
/// synthetic clips with known mattes, or on your own with --clip and --truth.
#[derive(Args)]
#[command(group(ArgGroup::new("mode").args(["ops", "roto"])))]
pub struct BenchArgs {
    #[command(flatten)]
    pub file: FileArg,
    /// The project to open (default: the demo)
    #[arg(long, value_name = "F.ecproj", value_hint = ValueHint::FilePath, value_parser = NonEmptyStringValueParser::new(), conflicts_with_all = ["ops", "roto"])]
    pub project: Option<String>,
    /// The comp to time
    #[arg(long, value_name = "C", value_parser = NonEmptyStringValueParser::new(), conflicts_with_all = ["ops", "roto"])]
    pub comp: Option<String>,
    /// Seconds to render at (default 3)
    #[arg(long, value_name = "S", value_parser = number, conflicts_with_all = ["ops", "roto"])]
    pub time: Option<f64>,
    /// Render scale (default 1)
    #[arg(long, value_name = "K", value_parser = number, conflicts_with_all = ["ops", "roto"])]
    pub scale: Option<f64>,
    /// Runs (default 10)
    #[arg(long, value_name = "N", conflicts_with_all = ["ops", "roto"])]
    pub n: Option<usize>,
    /// Render N consecutive frames with and without the layer cache
    #[arg(long, value_name = "N", conflicts_with_all = ["ops", "roto", "gpu"])]
    pub play: Option<usize>,
    /// CPU vs GPU ms/frame for every comp at Full and Half
    #[arg(long, conflicts_with_all = ["ops", "roto"])]
    pub gpu: bool,
    /// With --gpu: only the Advanced 3D comp
    #[arg(long, requires = "gpu")]
    pub adv3d: bool,
    /// Everyday operations on a large generated project
    #[arg(long)]
    pub ops: bool,
    /// With --ops: a smaller project, for a quick run
    #[arg(long, requires = "ops")]
    pub small: bool,
    /// With --ops: layers in the main comp
    #[arg(long, value_name = "N", requires = "ops")]
    pub layers: Option<usize>,
    /// With --ops: comps
    #[arg(long, value_name = "N", requires = "ops")]
    pub comps: Option<usize>,
    /// With --ops: footage items
    #[arg(long, value_name = "N", requires = "ops")]
    pub footage: Option<usize>,
    /// Roto Brush trackers on synthetic clips with known mattes
    #[arg(long)]
    pub roto: bool,
    /// With --roto: frames per clip (1-240, default 24)
    #[arg(long, value_name = "N", requires = "roto")]
    pub frames: Option<usize>,
    /// With --roto: SAM 2.1 weights to use instead of the installed ones
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath, value_parser = NonEmptyStringValueParser::new(), requires = "roto")]
    pub sam2: Option<String>,
    /// With --roto: MobileSAM weights to use instead of the installed ones
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath, value_parser = NonEmptyStringValueParser::new(), requires = "roto")]
    pub mobilesam: Option<String>,
    /// With --roto: count the correction rounds before every frame reaches IoU 0.9
    #[arg(long, requires = "roto")]
    pub fixes: bool,
    /// With --roto: your own frames (a folder of png/jpg), with --truth
    #[arg(long, value_name = "FRAMES_DIR", value_hint = ValueHint::DirPath, value_parser = NonEmptyStringValueParser::new(), requires_all = ["roto", "truth"])]
    pub clip: Option<String>,
    /// With --clip: the folder of mattes, one per frame with the same file stem
    #[arg(long, value_name = "MASKS_DIR", value_hint = ValueHint::DirPath, value_parser = NonEmptyStringValueParser::new(), requires = "clip")]
    pub truth: Option<String>,
    /// The project, set by [`parse`]
    #[arg(skip)]
    pub project_file: Option<String>,
}

/// Run an After Effects-style script
///
/// `script FILE.jsx [F.ecproj]` or `script --eval CODE [F.ecproj]`: JavaScript with the After
/// Effects-style object model (app.project, comps, layers, properties…). Prints writeLn output
/// and the result; an error exits 1 with file:line:col. The project is empty unless one is given.
#[derive(Args)]
pub struct ScriptArgs {
    /// FILE.jsx [F.ecproj]
    #[arg(value_name = "ARGS")]
    pub args: Vec<String>,
    /// Run this code instead of a file
    #[arg(long, value_name = "CODE", value_parser = NonEmptyStringValueParser::new())]
    pub eval: Option<String>,
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub bridge: BridgeArg,
    #[command(flatten)]
    pub gpu: GpuArg,
    #[command(flatten)]
    pub save: SaveArgs,
    /// The script file, set by [`parse`]
    #[arg(skip)]
    pub script: Option<String>,
    #[arg(skip)]
    pub project: Option<String>,
}

/// Run the MCP server on stdio (JSON-RPC)
///
/// Headless (an empty project unless one is given), or bridged to a running `effectcraft --control
/// PORT`. The automation roots confine the agent's file access: reads under a read or write root,
/// writes under a write root; scripts are refused.
#[derive(Args)]
pub struct McpArgs {
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub bridge: BridgeArg,
    #[command(flatten)]
    pub gpu: GpuArg,
    #[command(flatten)]
    pub file: FileArg,
    /// Keep unsaved headless work in the config directory's auto-save folder
    #[arg(long, conflicts_with = "bridge")]
    pub autosave: bool,
    /// A folder the agent may read (and write); repeatable
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath, action = clap::ArgAction::Append, value_parser = NonEmptyStringValueParser::new())]
    pub automation_read_root: Vec<String>,
    /// A folder the agent may write (and read); repeatable
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath, action = clap::ArgAction::Append, value_parser = NonEmptyStringValueParser::new())]
    pub automation_write_root: Vec<String>,
    /// The project, set by [`parse`]
    #[arg(skip)]
    pub project: Option<String>,
}

/// The command line's clap definition, with the `--help` text of each subcommand.
pub fn command() -> clap::Command {
    Cli::command()
}

/// A project argument: a path ending in `.ecproj`.
fn ecproj(s: &str) -> Result<String, UsageError> {
    if s.ends_with(".ecproj") { Ok(s.to_owned()) } else { Err(UsageError::NotAProject(s.to_owned())) }
}

/// `--time`, `--fps` and the like: a finite number.
fn number(s: &str) -> Result<f64, UsageError> {
    s.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| UsageError::NotANumber(s.to_owned()))
}

/// `--params`: bad JSON is a usage error, before any work. Whether the value is an object is the
/// command's to say.
fn json(s: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(s)
}

/// Take the project out of the positionals: the one ending `.ecproj` (the first). With `--project`
/// the positionals are left alone, so a value that ends `.ecproj` (a text layer's caption) stays one.
fn take_project(pos: &mut Vec<String>, flag: Option<&String>) -> Result<Option<String>, UsageError> {
    if let Some(flag) = flag {
        return Ok(Some(flag.clone()));
    }
    let mut found = pos.iter().enumerate().filter(|(_, p)| p.ends_with(".ecproj")).map(|(i, _)| i);
    let first = found.next();
    if let (Some(first), Some(second)) = (first, found.next()) {
        return Err(UsageError::TwoProjects(pos.get(first).cloned().unwrap_or_default(), pos.get(second).cloned().unwrap_or_default()));
    }
    Ok(first.map(|i| pos.remove(i)))
}

/// The project of a command with at most one positional, which is the project.
fn single_project(flag: Option<&String>, file: &FileArg) -> Result<Option<String>, UsageError> {
    match (flag, &file.file) {
        (Some(a), Some(b)) => Err(UsageError::TwoProjects(a.clone(), b.clone())),
        (Some(p), None) | (None, Some(p)) => Ok(Some(p.clone())),
        (None, None) => Ok(None),
    }
}

/// `--bridge` drives the running app's project: no other project, and no demo or empty one.
fn bridge_has_no_project(bridge: &BridgeArg, project: Option<&String>) -> Result<(), UsageError> {
    if bridge.bridge.is_some() && project.is_some() {
        return Err(UsageError::BridgeWithProject);
    }
    Ok(())
}

/// Exactly `N` positionals, with the project already taken out.
fn exact<const N: usize>(pos: Vec<String>, usage: &'static str) -> Result<[String; N], UsageError> {
    let len = pos.len();
    <[String; N]>::try_from(pos).map_err(|_| if len < N { UsageError::Missing(usage) } else { UsageError::Extra(usage) })
}

impl Cli {
    /// Settle what clap leaves to the commands: which positional is the project, how many
    /// positionals a command takes, and the JSON in them. Each failure is a usage error.
    fn settle(&mut self) -> Result<(), UsageError> {
        match &mut self.command {
            Command::Info(a) => {
                a.project = single_project(a.source.project.as_ref(), &a.file)?;
                bridge_has_no_project(&a.bridge, a.project.as_ref())?;
            }
            Command::Commands(a) => {
                a.project = single_project(a.source.project.as_ref(), &a.file)?;
                bridge_has_no_project(&a.bridge, a.project.as_ref())?;
            }
            Command::Exec(a) => {
                let mut pos = std::mem::take(&mut a.args);
                a.project = take_project(&mut pos, a.source.project.as_ref())?;
                bridge_has_no_project(&a.bridge, a.project.as_ref())?;
                if !a.list {
                    let mut it = pos.into_iter();
                    let id = it.next().ok_or(UsageError::Missing("exec <command-id> [<params JSON>]"))?;
                    let positional = it.next();
                    if it.next().is_some() {
                        return Err(UsageError::Extra("exec <command-id> [<params JSON>]"));
                    }
                    let params = match (a.params.take(), positional) {
                        (Some(_), Some(_)) => return Err(UsageError::TwoParams),
                        (Some(p), None) => p,
                        (None, Some(p)) => serde_json::from_str(&p).map_err(UsageError::BadParams)?,
                        (None, None) => serde_json::json!({}),
                    };
                    a.call = Some(ExecCall { id, params });
                } else if !pos.is_empty() {
                    return Err(UsageError::Extra("exec --list [F.ecproj]"));
                }
            }
            Command::Run(a) => {
                let mut pos = std::mem::take(&mut a.args);
                a.project = take_project(&mut pos, a.source.project.as_ref())?;
                bridge_has_no_project(&a.bridge, a.project.as_ref())?;
                a.pairs = pos;
                if a.commands.iter().filter(|s| *s == "-").count() > 1 {
                    return Err(UsageError::StdinTwice);
                }
            }
            Command::Props(a) => {
                let mut pos = std::mem::take(&mut a.args);
                a.project = take_project(&mut pos, a.source.project.as_ref())?;
                bridge_has_no_project(&a.bridge, a.project.as_ref())?;
                a.target = exact::<2>(pos, "props <comp> <layer>")?.into();
            }
            Command::Get(a) => {
                let mut pos = std::mem::take(&mut a.args);
                a.project = take_project(&mut pos, a.source.project.as_ref())?;
                bridge_has_no_project(&a.bridge, a.project.as_ref())?;
                a.target = exact::<3>(pos, "get <comp> <layer> <path>")?.into();
            }
            Command::Set(a) => {
                let mut pos = std::mem::take(&mut a.args);
                a.project = take_project(&mut pos, a.source.project.as_ref())?;
                bridge_has_no_project(&a.bridge, a.project.as_ref())?;
                // A value is not given with --expression.
                let usage = "set <comp> <layer> <path> <value> [--time S] [--expression E]";
                a.target = if a.expression.is_some() { exact::<3>(pos, usage)?.into() } else { exact::<4>(pos, usage)?.into() };
            }
            Command::RenderFrame(a) => {
                a.project = single_project(a.source.project.as_ref(), &a.file)?;
                bridge_has_no_project(&a.bridge, a.project.as_ref())?;
                if a.template.template.is_some() && (a.project.is_some() || a.bridge.bridge.is_some()) {
                    return Err(UsageError::TemplateWithProject);
                }
            }
            Command::Render(a) => {
                a.project = single_project(a.source.project.as_ref(), &a.file)?;
                if a.template.template.is_some() && (a.project.is_some() || a.source.demo) {
                    return Err(UsageError::TemplateWithProject);
                }
            }
            Command::Bench(a) => {
                a.project_file = single_project(a.project.as_ref(), &a.file)?;
                if (a.ops || a.roto) && a.project_file.is_some() {
                    return Err(UsageError::BenchModeWithProject);
                }
            }
            Command::Script(a) => {
                let mut pos = std::mem::take(&mut a.args);
                a.project = take_project(&mut pos, a.source.project.as_ref())?;
                bridge_has_no_project(&a.bridge, a.project.as_ref())?;
                let usage = "script FILE.jsx [F.ecproj] | --eval CODE";
                if a.eval.is_some() {
                    if !pos.is_empty() {
                        return Err(UsageError::Extra(usage));
                    }
                } else {
                    let [file] = exact::<1>(pos, usage)?;
                    a.script = Some(file);
                }
            }
            Command::Mcp(a) => {
                a.project = single_project(a.source.project.as_ref(), &a.file)?;
                bridge_has_no_project(&a.bridge, a.project.as_ref())?;
            }
        }
        Ok(())
    }

    /// The subcommand's name, for finding its clap definition.
    fn name(&self) -> &'static str {
        match self.command {
            Command::Info(_) => "info",
            Command::Commands(_) => "commands",
            Command::Exec(_) => "exec",
            Command::Run(_) => "run",
            Command::Props(_) => "props",
            Command::Get(_) => "get",
            Command::Set(_) => "set",
            Command::RenderFrame(_) => "render-frame",
            Command::Render(_) => "render",
            Command::Bench(_) => "bench",
            Command::Script(_) => "script",
            Command::Mcp(_) => "mcp",
        }
    }
}

/// Parse the arguments (without the program name). clap reads them as `OsString`s: text must be
/// valid Unicode (a usage error naming the argument otherwise), so nothing panics on a Latin-1
/// file name. A bare `--help` or `-h` is help even after a mistake on the same line (`--out
/// --help`), where clap would report the mistake first.
pub fn parse(args: &[OsString]) -> Result<Cli, clap::Error> {
    let mut cmd = Cli::command();
    let argv = std::iter::once(OsString::from("effectcraft-cli")).chain(args.iter().cloned());
    let matches = match cmd.try_get_matches_from_mut(argv) {
        Err(e) if args.iter().take_while(|a| *a != "--").any(|a| *a == "--help" || *a == "-h") => {
            // The help of the subcommand named first among the arguments (global flags may come
            // before it), else the command's.
            let sub = args.iter().take_while(|a| *a != "--").find(|a| a.to_str().is_some_and(|a| cmd.find_subcommand(a).is_some())).cloned();
            let help: Vec<OsString> = ["effectcraft-cli"].map(OsString::from).into_iter().chain(sub).chain([OsString::from("--help")]).collect();
            return Err(cmd.try_get_matches_from_mut(help).err().unwrap_or(e));
        }
        matches => matches?,
    };
    let mut cli = Cli::from_arg_matches(&matches)?;
    if let Err(e) = cli.settle() {
        let name = cli.name();
        return Err(match cmd.find_subcommand_mut(name) {
            Some(sub) => e.into_clap(sub),
            None => e.into_clap(&mut cmd),
        });
    }
    Ok(cli)
}

impl UsageError {
    /// The clap error for this one, printed with the subcommand's usage (exit 2).
    pub fn into_clap(self, cmd: &mut clap::Command) -> clap::Error {
        let kind = match self {
            Self::NotAProject(_) | Self::NotANumber(_) | Self::BadParams(_) | Self::BadCommands(_) | Self::BadValue { .. } => ErrorKind::ValueValidation,
            Self::Missing(_) | Self::NoSteps => ErrorKind::MissingRequiredArgument,
            Self::Extra(_) => ErrorKind::TooManyValues,
            Self::TwoProjects(..) | Self::TwoParams | Self::StdinTwice | Self::BridgeWithProject | Self::TemplateWithProject | Self::BenchModeWithProject => {
                ErrorKind::ArgumentConflict
            }
        };
        cmd.error(kind, self)
    }
}
