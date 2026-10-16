//! Launch the explicitly configured converter or the macOS app's bundled exporter.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bundled_exporter(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    if macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" || app.extension()? != "app" {
        return None;
    }
    Some(contents.join("Resources/aep_ir_export.py"))
}

pub(super) fn convert(path: &str) -> io::Result<Vec<u8>> {
    let script = std::env::var_os("EFFECTCRAFT_AEP_EXPORTER")
        .map(PathBuf::from)
        .or_else(|| {
            if !cfg!(target_os = "macos") {
                return None;
            }
            bundled_exporter(&std::env::current_exe().ok()?).filter(|p| p.is_file())
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "AEP exporter script missing; set EFFECTCRAFT_AEP_EXPORTER"))?;
    let python = std::env::var_os("EFFECTCRAFT_AEP_PYTHON").unwrap_or_else(|| "python3".into());
    let mut command = Command::new(python);
    command.arg(script).arg(path).arg("--stdout").env("PYTHONIOENCODING", "utf-8");
    capture(&mut command, 512 * 1024 * 1024)
}

fn capture(command: &mut Command, limit: u64) -> io::Result<Vec<u8>> {
    // Converted projects never enter shared temporary storage. Drain diagnostics concurrently
    // so a noisy converter cannot fill its stderr pipe and deadlock the stdout reader.
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let Some(mut stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("AEP converter has no diagnostic pipe"));
    };
    let diagnostics = std::thread::Builder::new().name("aep-diagnostics".into()).spawn(move || -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        stderr.by_ref().take(64 * 1024).read_to_end(&mut bytes)?;
        io::copy(&mut stderr, &mut io::sink())?;
        Ok(bytes)
    });
    let diagnostics = match diagnostics {
        Ok(thread) => thread,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let read = (|| {
        let stdout = child.stdout.take().ok_or_else(|| io::Error::other("AEP converter has no output pipe"))?;
        let mut bytes = Vec::new();
        stdout.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(io::Error::other(format!("AEP conversion exceeds {limit} bytes")));
        }
        Ok(bytes)
    })();
    if read.is_err() {
        let _ = child.kill();
    }
    let status = child.wait();
    let diagnostics = diagnostics.join().map_err(|_| io::Error::other("AEP diagnostic reader failed"))?;
    let bytes = read?;
    if !status?.success() {
        return Err(io::Error::other(format!(
            "AEP export failed (set EFFECTCRAFT_AEP_PYTHON to an interpreter with py-aep installed): {}",
            String::from_utf8_lossy(&diagnostics?).trim()
        )));
    }
    diagnostics?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_app_bundles_supply_an_automatic_exporter() {
        assert_eq!(
            bundled_exporter(Path::new("/Applications/EffectCraft.app/Contents/MacOS/effectcraft")),
            Some(PathBuf::from("/Applications/EffectCraft.app/Contents/Resources/aep_ir_export.py"))
        );
        for exe in [
            "/Downloads/bin/effectcraft",
            "/Downloads/MacOS/effectcraft",
            "/Downloads/Contents/MacOS/effectcraft",
            "/Downloads/Fake/Contents/MacOS/effectcraft",
        ] {
            assert_eq!(bundled_exporter(Path::new(exe)), None, "{exe}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn converter_output_is_bounded_and_failures_keep_diagnostics() {
        let run = |script: &str, limit| capture(Command::new("sh").args(["-c", script]), limit);
        assert_eq!(run("printf '{\"items\":[]}'", 12).unwrap(), b"{\"items\":[]}");
        assert_eq!(run("printf 'é π'", 5).unwrap(), "é π".as_bytes());
        assert!(run("printf '12345'", 4).unwrap_err().to_string().contains("exceeds"));
        assert!(run("printf 'bad project' >&2; exit 1", 32).unwrap_err().to_string().contains("bad project"));
        assert_eq!(run("head -c 100000 /dev/zero >&2; printf ok", 2).unwrap(), b"ok");
    }
}
