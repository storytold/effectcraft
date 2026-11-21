//! Fonts a host page gives the engine (`effectcraft.addFont`, #3996).

use effectcraft_web::fonts::{PREFIX, add, all, receive};

/// A font is registered and kept for the workers under its own path; a file that is not a font
/// is refused and kept nowhere.
#[test]
fn a_font_is_added_once_and_a_non_font_is_refused() {
    let inter = include_bytes!("../../../../assets/fonts/Inter-Bold.ttf").to_vec();
    assert_eq!(add("Inter-Bold.ttf", inter.clone()), Ok(vec!["Inter".to_string()]));
    let e = add("notes.txt", b"not a font".to_vec()).unwrap_err();
    assert!(e.contains("notes.txt"), "{e}");
    let sent = all();
    assert_eq!(sent.len(), 1, "the refused file is not sent to workers");
    assert!(sent[0].0.starts_with(PREFIX));
    assert_eq!(&sent[0].1[..], &inter[..]);
}

/// A worker takes the files under the font folder as fonts and leaves footage alone.
#[test]
fn a_worker_takes_only_font_paths_as_fonts() {
    let inter = include_bytes!("../../../../assets/fonts/Inter-Bold.ttf");
    assert!(receive(&format!("{PREFIX}0"), inter));
    assert!(!receive("/footage/Inter-Bold.ttf", inter), "a footage file named like a font is footage");
}
