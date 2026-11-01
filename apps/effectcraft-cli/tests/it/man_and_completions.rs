//! The clap definition is consistent, and every shell's completions and every man page
//! (`examples/man-and-completions.rs`) can be generated from it.

use clap::ValueEnum;
use clap_complete::Shell;

#[test]
fn command_is_consistent() {
    effectcraft_cli::command().debug_assert();
}

#[test]
fn completions_generate_for_every_shell() {
    for &shell in Shell::value_variants() {
        let mut out = Vec::new();
        clap_complete::generate(shell, &mut effectcraft_cli::command(), "effectcraft-cli", &mut out);
        assert!(!out.is_empty(), "{shell}");
    }
}

#[test]
fn man_pages_render_for_the_command_and_every_subcommand() {
    let cmd = effectcraft_cli::command();
    for page in std::iter::once(&cmd).chain(cmd.get_subcommands()) {
        let mut out = Vec::new();
        clap_mangen::Man::new(page.clone()).render(&mut out).unwrap();
        assert!(!out.is_empty(), "{}", page.get_name());
    }
}

/// The example's two `generate_to` calls, repeated here, write the five completion files under the
/// names the Linux and FreeBSD packages install, `effectcraft-cli.1` and a page per subcommand,
/// and no pages for the `help` subcommand.
#[test]
fn the_example_writes_the_files_the_packages_install() {
    let dir = std::env::temp_dir().join(format!("ec-cli-{}-man-and-completions", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cmd = effectcraft_cli::command();
    for &shell in Shell::value_variants() {
        clap_complete::generate_to(shell, &mut cmd, "effectcraft-cli", &dir).unwrap();
    }
    clap_mangen::generate_to(effectcraft_cli::command(), &dir).unwrap();

    let mut written: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    written.sort();
    let completions = ["effectcraft-cli.bash", "_effectcraft-cli", "effectcraft-cli.fish", "effectcraft-cli.elv", "_effectcraft-cli.ps1"];
    let subcommands = ["info", "commands", "exec", "run", "props", "get", "set", "render-frame", "render", "bench", "script", "mcp"];
    let mut expected: Vec<String> = completions.map(String::from).into();
    expected.push("effectcraft-cli.1".into());
    expected.extend(subcommands.iter().map(|s| format!("effectcraft-cli-{s}.1")));
    expected.sort();
    assert_eq!(written, expected);
    std::fs::remove_dir_all(&dir).unwrap();
}
