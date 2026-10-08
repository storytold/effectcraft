//! Windows only: embed the app icon and version info (VERSIONINFO) into `effectcraft.exe`.
//!
//! On every other target this does nothing. Development builds may warn when a resource
//! compiler is unavailable, but release builds must fail rather than ship without the icon.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/effectcraft.ico");
    println!("cargo:rerun-if-env-changed=EFFECTCRAFT_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/effectcraft.ico")
        .set("ProductName", "EffectCraft")
        .set("FileDescription", "EffectCraft motion graphics and visual effects")
        .set("LegalCopyright", "Copyright (c) 2026 The EffectCraft contributors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "effectcraft.exe")
        .set("InternalName", "effectcraft");
    if let Err(e) = res.compile() {
        let release = std::env::var("PROFILE").as_deref() == Ok("release");
        let required = release || std::env::var_os("EFFECTCRAFT_REQUIRE_WINRES").is_some();
        if required {
            panic!("embedding Windows resources failed: {e}");
        }
        println!("cargo:warning=effectcraft.exe built without icon/version resources: {e}");
    }
}
