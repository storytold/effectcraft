//! `format` contributions: a still in a file type EffectCraft does not know imports through a plug-in.
//!
//! The seam is `effectcraft_media::foreign`: its probe and its still decoder ask the registered
//! readers before they give up on a file. [`attach`] registers one reader over the host's
//! [`FormatBook`]. The reader holds the book weakly, so a dropped host stops answering.

use std::sync::{Arc, Weak};

use craft_plugin_abi::format::extension_of;
use craft_plugin_host::contrib::format::FormatBook;
use effectcraft_media::foreign::{self, Raster};

/// Make footage import and decoding open the files that `book` handles. Call once per host.
pub fn attach(book: &Arc<FormatBook>) {
    let book: Weak<FormatBook> = Arc::downgrade(book);
    foreign::register(Arc::new(move |name, bytes| {
        let book = book.upgrade()?;
        if !book.handles(&extension_of(name)?) {
            return None;
        }
        Some(book.read(name, bytes).map_err(|e| e.to_string()).map(|doc| {
            let has_alpha = doc.rgba8.chunks_exact(4).any(|p| p[3] != 255);
            Raster { width: doc.width, height: doc.height, rgba8: doc.rgba8, has_alpha }
        }))
    }));
}
