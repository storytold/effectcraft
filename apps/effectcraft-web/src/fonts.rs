//! Fonts a host page gives the engine (`effectcraft.addFont`, #3996). The web build only has its
//! bundled fonts, and every render worker runs an engine instance of its own with its own font
//! list, so a font is registered on the page and sent to each worker before its next job or frame
//! (a worker started later gets them all). Fonts last for the session: the page adds them again
//! after a reload.

use std::cell::RefCell;
use std::sync::Arc;

/// Where fonts travel in the workers' `{type: "file"}` messages: a folder no project names.
pub const PREFIX: &str = "/.effectcraft-fonts/";

thread_local! {
    /// The fonts the page was given, in order: (path in [`PREFIX`], bytes).
    static FONTS: RefCell<Vec<(String, Arc<[u8]>)>> = const { RefCell::new(Vec::new()) };
}

/// Register font file `data` (TrueType or OpenType, a collection too) on the page; `name` only
/// names it in the error. Returns the families it added; a file without a face the engine can read
/// is refused.
pub fn add(name: &str, data: Vec<u8>) -> Result<Vec<String>, String> {
    let data: Arc<[u8]> = data.into();
    let families = register(&data);
    if families.is_empty() {
        return Err(format!("{name}: not a font EffectCraft can read (TrueType or OpenType)"));
    }
    FONTS.with(|f| {
        let mut f = f.borrow_mut();
        let path = format!("{PREFIX}{}", f.len());
        f.push((path, data));
    });
    Ok(families)
}

/// The fonts the page was given, for a worker: (path, bytes).
pub fn all() -> Vec<(String, Arc<[u8]>)> {
    FONTS.with(|f| f.borrow().clone())
}

/// A worker's side: register a font the page sent. Returns whether `path` was one.
pub fn receive(path: &str, data: &[u8]) -> bool {
    if !path.starts_with(PREFIX) {
        return false;
    }
    register(data);
    true
}

/// Add the faces of `data` to this engine instance's font list: their families, in order, once each.
fn register(data: &[u8]) -> Vec<String> {
    use effectcraft_engine::text::fonts;
    let mut families: Vec<String> = Vec::new();
    for id in fonts::add_font_data(data.to_vec()) {
        let family = fonts::face(id).info.family.clone();
        if !families.contains(&family) {
            families.push(family);
        }
    }
    families
}
