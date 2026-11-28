//! EffectCraft's side of hardware video helpers (`docs/video-helper-protocol.md`, RFC-0031): the
//! messages ([`wire`]), the rules for each reply ([`jobs`]), the running helper and its encode
//! jobs ([`client`]), and here, finding the program and checking its `hello`.

pub mod client;
pub mod jobs;
pub mod wire;

use std::path::{Path, PathBuf};

use crate::protocol::PROTOCOL_VERSION;
use wire::{VIDEO_VERSION, VideoHello, VideoReply};

pub const PROGRAM_NAME: &str = "effectcraft-helper-video";

pub fn program_file_name() -> String {
    if cfg!(windows) { format!("{PROGRAM_NAME}.exe") } else { PROGRAM_NAME.to_string() }
}

pub fn is_video_program(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return false };
    let want = program_file_name();
    if cfg!(windows) { name.eq_ignore_ascii_case(&want) } else { name == want }
}

/// Only `helpers/` next to the executable (`Contents/Helpers` in a macOS bundle), never the
/// settings folder, `EFFECTCRAFT_HELPER_PATH`, `PATH` or the current folder: a user-writable
/// folder must not be able to supply the program that gets the GPU.
pub fn find_video_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    find_video_helper_in(exe.parent()?)
}

pub fn find_video_helper_in(exe_dir: &Path) -> Option<PathBuf> {
    let name = program_file_name();
    let mut dirs = vec![exe_dir.join("helpers")];
    if cfg!(target_os = "macos")
        && let Some(helpers) = bundle_helpers(exe_dir)
    {
        dirs.push(helpers);
    }
    dirs.into_iter().map(|d| d.join(&name)).find(|p| p.is_file())
}

/// `Contents/Helpers` for an executable in `Contents/MacOS`, and nothing for any other folder.
fn bundle_helpers(exe_dir: &Path) -> Option<PathBuf> {
    let contents = exe_dir.parent()?;
    (exe_dir.file_name()? == "MacOS" && contents.file_name()? == "Contents").then(|| contents.join("Helpers"))
}

/// Nothing else is sent to a helper before this passes. A refusal names the installed helper
/// version and what to install or update, for the render log and the export error.
pub fn check_hello(reply: VideoReply, app_version: &str) -> Result<VideoHello, String> {
    let VideoReply::Hello { version, name, helper_version, video, .. } = reply else {
        return Err("the video helper did not start with a hello".into());
    };
    let who = format!("{name} {helper_version}");
    if version != PROTOCOL_VERSION {
        return Err(format!(
            "the video helper {who} speaks helper protocol {version}, this EffectCraft {app_version} speaks {PROTOCOL_VERSION}: install the video helper built for EffectCraft {app_version}"
        ));
    }
    let Some(video) = video else {
        return Err(format!("{who} is not a video helper: its hello has no video part"));
    };
    if video.version != VIDEO_VERSION {
        return Err(format!(
            "the video helper {who} speaks video protocol {}, this EffectCraft {app_version} speaks {VIDEO_VERSION}: install the video helper built for EffectCraft {app_version}",
            video.version
        ));
    }
    let (Some(min), Some(app)) = (parse_version(&video.app_min), parse_version(app_version)) else {
        return Err(format!("the video helper {who} gave an app_min that can't be read ({:?})", video.app_min));
    };
    if app < min {
        return Err(format!(
            "the video helper {who} needs EffectCraft {} or newer, this is {app_version}: update EffectCraft or install an older video helper",
            video.app_min
        ));
    }
    Ok(video)
}

/// Numbers, so that 0.9 is older than 0.25; a `-pre` or `+build` suffix is ignored.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let mut next = || parts.next()?.parse().ok();
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(version: u32, video: Option<(u32, &str)>) -> VideoReply {
        VideoReply::Hello {
            version,
            name: "Hardware video".into(),
            helper_version: "0.1.0".into(),
            loads: vec![],
            video: video.map(|(version, app_min)| VideoHello { version, app_min: app_min.into(), devices: vec![] }),
        }
    }

    #[test]
    fn a_hello_is_accepted_only_when_the_versions_fit_and_a_refusal_names_the_helper_version() {
        assert!(check_hello(hello(1, Some((1, "0.25.0"))), "0.25.1").is_ok());
        assert!(check_hello(hello(1, Some((1, "0.25.1"))), "0.25.1").is_ok(), "app_min equal to this version");
        assert!(check_hello(hello(1, Some((1, "0.25.1-rc.1"))), "0.25.1").is_ok());

        let refused = [
            check_hello(hello(2, Some((1, "0.1.0"))), "0.25.1"),
            check_hello(hello(1, None), "0.25.1"),
            check_hello(hello(1, Some((2, "0.1.0"))), "0.25.1"),
            check_hello(hello(1, Some((1, "0.26.0"))), "0.25.1"),
            check_hello(hello(1, Some((1, "soon"))), "0.25.1"),
        ];
        for r in &refused {
            assert!(r.is_err());
        }
        let newer = check_hello(hello(1, Some((1, "0.26.0"))), "0.25.1").unwrap_err();
        assert!(newer.contains("0.1.0") && newer.contains("0.26.0") && newer.contains("0.25.1"), "{newer}");
        let other_video = check_hello(hello(1, Some((2, "0.1.0"))), "0.25.1").unwrap_err();
        assert!(other_video.contains("0.1.0") && other_video.contains("install"), "{other_video}");
        assert!(check_hello(hello(1, Some((1, "0.9.0"))), "0.25.1").is_ok());
        assert!(check_hello(VideoReply::Cancelled { job: 1 }, "0.25.1").is_err());
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ec-video-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_video_helper_is_found_only_in_helpers_next_to_the_executable() {
        let exe_dir = scratch("find");
        assert_eq!(find_video_helper_in(&exe_dir), None);
        // Not beside the executable, and not in a sibling folder an environment variable could name.
        std::fs::write(exe_dir.join(program_file_name()), b"").unwrap();
        std::fs::create_dir_all(exe_dir.join("other")).unwrap();
        std::fs::write(exe_dir.join("other").join(program_file_name()), b"").unwrap();
        assert_eq!(find_video_helper_in(&exe_dir), None);
        std::fs::create_dir_all(exe_dir.join("helpers")).unwrap();
        let installed = exe_dir.join("helpers").join(program_file_name());
        std::fs::write(&installed, b"").unwrap();
        assert_eq!(find_video_helper_in(&exe_dir), Some(installed));
        let _ = std::fs::remove_dir_all(&exe_dir);
    }

    #[test]
    fn only_an_app_bundle_has_a_helpers_folder_beside_its_executable_folder() {
        assert_eq!(bundle_helpers(Path::new("/Apps/EffectCraft.app/Contents/MacOS")), Some(PathBuf::from("/Apps/EffectCraft.app/Contents/Helpers")));
        assert_eq!(bundle_helpers(Path::new("/opt/effectcraft/bin")), None);
        assert_eq!(bundle_helpers(Path::new("/opt/MacOS")), None);
    }

    #[test]
    fn effect_helper_scans_skip_the_video_helper() {
        let dir = scratch("scan");
        let effect = dir.join(if cfg!(windows) { "effectcraft-helper-ofx.exe" } else { "effectcraft-helper-ofx" });
        std::fs::write(&effect, b"").unwrap();
        std::fs::write(dir.join(program_file_name()), b"").unwrap();
        assert_eq!(crate::find_helpers(std::slice::from_ref(&dir)), vec![effect.clone()]);
        assert!(crate::find_helpers(&[dir.join(program_file_name())]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
