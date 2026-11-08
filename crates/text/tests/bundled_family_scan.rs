//! An installed copy of a bundled family takes the bundled faces' place, so resolving Inter reads
//! the installed fonts first: the same text must not change face when a later scan finishes. Its
//! own test binary, so the system font scan has not run yet when it starts.

use effectcraft_text::fonts::{resolve, system_scanned};

#[test]
fn resolving_a_bundled_family_scans_the_installed_fonts_first() {
    assert!(!system_scanned(), "nothing has scanned yet in this process");
    let _ = resolve("Inter", "Regular");
    assert!(system_scanned() || cfg!(target_arch = "wasm32"), "the bundled Inter is the answer only once the installed fonts are known");
}
