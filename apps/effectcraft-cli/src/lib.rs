//! EffectCraft headless CLI: one-shot agent commands, frame rendering and the MCP server.
//! `run` is the whole program, so it can be tested in-process as well as through the binary.
//!
//! ```text
//! effectcraft-cli info                                        project + engine summary
//! effectcraft-cli commands [--filter TEXT] [--enabled]        list engine commands
//! effectcraft-cli exec <command-id> [--params JSON]           run one command
//! effectcraft-cli exec --list [--filter TEXT] [--schemas]     list engine commands (as `commands`)
//! effectcraft-cli run <id> <json> [<id> <json> …]             run several commands in order
//! effectcraft-cli props <comp> <layer> [--flat] [--time S]    a layer's property tree (with paths)
//! effectcraft-cli get <comp> <layer> <path> [--time S]        read a property
//! effectcraft-cli set <comp> <layer> <path> <value> [--time S] [--expression E]
//! effectcraft-cli render-frame [--comp C] [--time S|--frame N] [--max-side PX|--scale K] [--out F.png]
//! effectcraft-cli render [--comp C] --out FILE [--format F] …  render a comp (or `--queue`)
//! effectcraft-cli bench [--ops | --roto | --gpu] …            render and operation timings
//! effectcraft-cli script FILE.jsx [F.ecproj] | --eval CODE    run an After Effects-style script
//! effectcraft-cli mcp [--autosave | --bridge PORT]            MCP server on stdio
//! ```
//!
//! `--help` (and `<subcommand> --help`) is the reference; this page follows RFC-0012 (exit codes:
//! 0 success, 1 failure, 2 usage error). `<comp>` is a comp id or name (`-` = the active comp);
//! `<layer>` an id, `#n` or name; `<value>` is JSON (`50`, `[960,540]`, `"#ff0000"`) or a bare
//! string. See `docs/agents.md`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::ffi::OsString;
use std::io::Write;

#[macro_use]
mod io;
mod args;
mod bench;
mod commands;
mod error;
mod render;
mod steps;
mod template;

pub use args::command;
use args::{Command, parse};
pub use error::{CliError, UsageError};
use io::Io;

/// Run the CLI on its arguments (without the program name). Returns the process exit code (0 ok,
/// 1 failure, 2 usage). clap reads the arguments as they are: text that is not valid Unicode is a
/// usage error naming it, never a panic as `std::env::args()` raises on one.
pub fn run<I, T>(args: I, out: &mut dyn Write, err: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    // Help and `--version` (`help`, `-h`, `<subcommand> --help`, `-V`) come back as errors too, for
    // `out` and exit 0: the subcommand never runs.
    let mut cli = match parse(&args) {
        Ok(cli) => cli,
        Err(e) => {
            let to: &mut dyn Write = if e.use_stderr() { err } else { out };
            let _ = write!(to, "{}", e.render());
            return e.exit_code();
        }
    };
    let mut io = Io::new(out, err);
    let json_out = cli.json;
    let result = match &mut cli.command {
        Command::Info(a) => commands::info(a, json_out, &mut io),
        Command::Commands(a) => commands::commands(a, json_out, &mut io),
        Command::Exec(a) => commands::exec(a, json_out, &mut io),
        Command::Run(a) => commands::run_cmds(a, json_out, &mut io),
        Command::Props(a) => commands::props(a, json_out, &mut io),
        Command::Get(a) => commands::get(a, json_out, &mut io),
        Command::Set(a) => commands::set(a, json_out, &mut io),
        Command::RenderFrame(a) => commands::render_frame(a, json_out, &mut io),
        Command::Render(a) => render::render(a, json_out, &mut io),
        Command::Bench(a) => bench::bench_cmd(a, json_out, &mut io),
        Command::Script(a) => commands::script(a, json_out, &mut io),
        Command::Mcp(a) => commands::mcp(a),
    };
    let code = match result {
        Ok(()) => 0,
        Err(CliError::Reported) => 1,
        Err(e) => {
            if json_out {
                say!(io, "{}", serde_json::json!({"error": e.to_string()}));
            }
            note!(io, "error: {e}");
            e.exit_code()
        }
    };
    io.flush();
    if code == 0 && io.stdout_broken() { 1 } else { code }
}
