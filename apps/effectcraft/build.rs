//! Windows only: embed the app icon and version info (VERSIONINFO) into `effectcraft.exe`.
//!
//! On every other target this does nothing beyond recording the built channel/version the app
//! reports. A missing resource compiler is a warning, so a cross-compile from macOS or Linux still
//! links, unless `EFFECTCRAFT_REQUIRE_WINRES=1` (set by the release and nightly workflows) turns it
//! into an error.

#[path = "src/build_channel.rs"]
mod build_channel;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=EFFECTCRAFT_BUILD_CHANNEL");
    println!("cargo:rustc-check-cfg=cfg(effectcraft_nightly_v1)");
    // `EFFECTCRAFT_BUILD_CHANNEL=nightly-v1` compiles the isolated Nightly flavor; unset, empty or
    // `stable` compiles stable. Anything else is a build error rather than a silent fallback.
    let nightly = match std::env::var("EFFECTCRAFT_BUILD_CHANNEL").as_deref() {
        Ok("nightly-v1") => {
            println!("cargo:rustc-cfg=effectcraft_nightly_v1");
            true
        }
        Ok("stable" | "") | Err(std::env::VarError::NotPresent) => false,
        _ => {
            eprintln!("unsupported EFFECTCRAFT_BUILD_CHANNEL (expected stable or nightly-v1)");
            std::process::exit(1);
        }
    };
    println!("cargo:rerun-if-changed=../../assets/app-icon/effectcraft.ico");
    println!("cargo:rerun-if-changed=../../assets/app-icon/effectcraft-nightly.ico");
    println!("cargo:rerun-if-env-changed=EFFECTCRAFT_REQUIRE_WINRES");
    println!("cargo:rerun-if-env-changed=EFFECTCRAFT_VERSION");
    println!("cargo:rerun-if-env-changed=EFFECTCRAFT_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=EFFECTCRAFT_BUILD_DATE");
    // A nightly build refuses a version the feed would not have produced: the same identity the
    // Linux, macOS and Windows packaging checks require. Stable keeps its Cargo version.
    let cargo_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let built_version = if nightly {
        let version = match std::env::var("EFFECTCRAFT_VERSION") {
            Ok(version) if build_channel::is_nightly_version(&version) => version,
            _ => {
                eprintln!("nightly-v1 requires an explicit EFFECTCRAFT_VERSION feed identity");
                std::process::exit(1);
            }
        };
        match (std::env::var("EFFECTCRAFT_BUILD_SHA"), std::env::var("EFFECTCRAFT_BUILD_DATE")) {
            (Ok(sha), Ok(date)) if build_channel::identity_matches(&version, &sha, &date) => version,
            _ => {
                eprintln!("nightly-v1 requires a matching EFFECTCRAFT_BUILD_SHA (40 hex) and EFFECTCRAFT_BUILD_DATE");
                std::process::exit(1);
            }
        }
    } else {
        cargo_version
    };
    println!("cargo:rustc-env=EFFECTCRAFT_BUILT_VERSION={built_version}");
    let commit =
        std::env::var("EFFECTCRAFT_BUILD_SHA").ok().filter(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    println!("cargo:rustc-env=EFFECTCRAFT_BUILT_COMMIT={}", commit.as_deref().unwrap_or(""));

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(build_channel::windows_icon_path(nightly))
        .set("ProductName", if nightly { "EffectCraft Nightly" } else { "EffectCraft" })
        .set("FileDescription", if nightly { "EffectCraft Nightly motion graphics" } else { "EffectCraft motion graphics and visual effects" })
        .set("LegalCopyright", "Copyright (c) 2026 The EffectCraft contributors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "effectcraft.exe")
        .set("InternalName", "effectcraft");
    if nightly {
        // The Launcher checks the PE ProductVersion against the exact feed identity, not FileVersion.
        res.set("ProductVersion", &built_version);
    }
    if let Err(e) = res.compile() {
        if std::env::var_os("EFFECTCRAFT_REQUIRE_WINRES").is_some() {
            panic!("embedding Windows resources failed: {e}");
        }
        println!("cargo:warning=effectcraft.exe built without icon/version resources: {e}");
    }
}
