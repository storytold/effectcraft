//! Engine data (the text engine document of type layers, `TySh`): `craft-psd`'s
//! `engine_data`, re-exported. [`text_style`] is its typed view of the first style run.

pub use craft_psd::engine_data::{TextStyle, Value, minimal, parse, parse_lossy, write};

/// The first style run's character and paragraph attributes of a parsed document.
pub fn text_style(ed: &Value) -> TextStyle {
    craft_psd::engine_data::first_style(ed)
}
