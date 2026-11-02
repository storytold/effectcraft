//! File ▸ Import ▸ Vanishing Point (.vpe): present in the menu like After Effects', but always
//! disabled. The `.vpe` exchange format Photoshop's Vanishing Point filter writes has no public
//! specification, and EffectCraft implements formats only from published specs (clean room), so
//! the entry explains that instead of importing. See docs/file-format-parity.md.

use serde_json::Value;

use super::{CommandSpec, bad};
use crate::{Result, Session, cmd};

/// Why the entry is disabled (shown as its tooltip).
pub const WHY: &str =
    "Vanishing Point Exchange (.vpe) files can't be imported: the format has no public specification, and EffectCraft only implements documented formats";

fn import(_: &mut Session, _: &Value) -> Result<Value> {
    Err(bad("file.importVanishingPoint", WHY))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "file.importVanishingPoint",
        "Vanishing Point (.vpe)...",
        ["File", "Import"],
        None,
        "{} (always disabled: undocumented format)",
        |_| Err(WHY.into()),
        import
    )]
}
