//! The font menus list the installed fonts (issue: on Windows the Properties panel's font menu
//! offered only Inter, JetBrains Mono and Noto Serif). Its own test binary, so the system font
//! scan has not run yet when it starts.

use effectcraft_text::fonts::{DirectorySource, FontSource, families, system_scanned};

#[test]
fn families_include_installed_fonts_without_a_prior_scan() {
    assert!(!system_scanned(), "nothing has scanned yet in this process");
    let listed: Vec<String> = families().into_iter().map(|(f, _)| f).collect();
    assert!(system_scanned() || cfg!(target_arch = "wasm32"), "listing the families scans the system fonts");
    for b in ["Inter", "JetBrains Mono", "Noto Serif"] {
        assert!(listed.iter().any(|f| f == b), "bundled {b} listed");
    }
    // Every family found in the platform's font folders is in the menu.
    for face in DirectorySource::system().faces() {
        assert!(listed.iter().any(|f| f.eq_ignore_ascii_case(&face.family)), "installed family {} missing from {listed:?}", face.family);
    }
}
