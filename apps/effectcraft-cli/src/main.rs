//! The `effectcraft-cli` binary: `run` in the library is the whole program.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

fn main() {
    // Warnings and errors to standard error (`RUST_LOG` overrides), so a GPU that declines
    // frames or runs out of memory says so instead of silently rendering on the CPU.
    effectcraft_engine::logging::install_with("warn");
    // `args_os`, not `args`: a non-Unicode argument (a Latin-1 file name on Linux) must reach clap,
    // not panic before anything is printed.
    let code = effectcraft_cli::run(std::env::args_os().skip(1), &mut std::io::stdout(), &mut std::io::stderr());
    std::process::exit(code);
}
