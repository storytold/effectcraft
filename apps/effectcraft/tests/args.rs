//! The desktop binary rejects a bad command line before it starts anything (#997).
// Integration tests: unwrapping and panicking on failure is fine here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A settings folder of its own, so a launch that got past the arguments would show up as
/// preferences (Recent Footage) written there instead of in the user's settings.
fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ec-args-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Runs the app, which must exit at once with `effectcraft: <message>` and a failure status. A
/// regression fails instead of leaving a window open: the process is killed after 5 s.
fn rejects(name: &str, args: &[&str], env: &[(&str, &str)], message: &str) {
    let config = config_dir(name);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_effectcraft"));
    cmd.args(args).env_remove("EFFECTCRAFT_CONTROL_PORT").env_remove("RUST_LOG").env("EFFECTCRAFT_CONFIG_DIR", &config);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let timed_out = loop {
        if child.try_wait().unwrap().is_some() {
            break false;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            break true;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let out = child.wait_with_output().unwrap();
    assert!(!timed_out, "{args:?}: the app started or did not exit promptly");
    assert_eq!(out.status.code(), Some(1), "{args:?}");
    assert!(out.stdout.is_empty(), "{args:?}: {:?}", String::from_utf8_lossy(&out.stdout));
    assert_eq!(String::from_utf8(out.stderr).unwrap(), format!("effectcraft: {message}\n"), "{args:?}");
    let written: Vec<_> = std::fs::read_dir(&config).unwrap().collect();
    assert!(written.is_empty(), "{args:?}: wrote {written:?}");
    let _ = std::fs::remove_dir_all(&config);
}

#[test]
fn unknown_options_stop_the_launch_before_anything_is_imported() {
    let folder = config_dir("folder");
    rejects("read-root", &["--control", "47613", "--automation-read-root", folder.to_str().unwrap()], &[], "unknown option `--automation-read-root`");
    rejects("write-root", &["--automation-write-root=/tmp"], &[], "unknown option `--automation-write-root=/tmp`");
    rejects("short", &["-h"], &[], "unknown option `-h`");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_bad_control_port_stops_the_launch() {
    let need = "--control needs a port number from 1 to 65535";
    rejects("missing", &["--control"], &[], need);
    rejects("flag", &["--control", "--demo"], &[], &format!("{need}, not `--demo`"));
    rejects("text", &["--control", "abc"], &[], &format!("{need}, not `abc`"));
    rejects("zero", &["--control=0"], &[], &format!("{need}, not `0`"));
    rejects("env", &[], &[("EFFECTCRAFT_CONTROL_PORT", "abc")], "EFFECTCRAFT_CONTROL_PORT needs a port number from 1 to 65535, not `abc`");
}

#[test]
fn version_still_prints_with_a_bad_port_variable() {
    let out = Command::new(env!("CARGO_BIN_EXE_effectcraft")).arg("--version").env("EFFECTCRAFT_CONTROL_PORT", "abc").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("effectcraft {}\n", env!("CARGO_PKG_VERSION")));
}
