//! Fail when `Cargo.lock` records a `[patch]` entry nothing uses.
//!
//! An unused patch only makes cargo warn, but the Nix package builds offline from vendored
//! sources, which hold only the packages the build uses: it cannot find the patched package and
//! refuses to build (#3403). Patch a crate together with the dependency that needs it.

use std::path::Path;

pub fn run(root: &Path) -> Result<(), String> {
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?;
    let unused = unused_patches(&lock);
    if unused.is_empty() {
        println!("Cargo.lock: every [patch] entry is used");
        return Ok(());
    }
    Err(format!(
        "Cargo.lock has [[patch.unused]] entries: {}. Remove them from [patch] in Cargo.toml (add a patch together with the dependency that needs it); the Nix build, which uses vendored sources, fails on them (#3403)",
        unused.join(", ")
    ))
}

/// The package names of the `[[patch.unused]]` entries in the text of a `Cargo.lock`.
fn unused_patches(lock: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_unused = false;
    for line in lock.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_unused = line == "[[patch.unused]]";
        } else if let Some(name) = line.strip_prefix("name = ").filter(|_| in_unused) {
            names.push(name.trim_matches('"').to_string());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"
[[package]]
name = "egui"
version = "0.37.0"

[[patch.unused]]
name = "egui_demo_lib"
version = "0.37.0"
source = "git+https://github.com/storytold/egui.git?rev=6d518fc#6d518fc"

[[patch.unused]]
name = "egui_inspection"
version = "0.37.0"
"#;

    #[test]
    fn lists_the_unused_patches_and_only_those() {
        assert_eq!(unused_patches(LOCK), ["egui_demo_lib", "egui_inspection"]);
    }

    #[test]
    fn a_lock_without_unused_patches_is_accepted() {
        assert!(unused_patches("[[package]]\nname = \"egui\"\nversion = \"0.37.0\"\n").is_empty());
    }

    #[test]
    fn the_repository_lock_has_no_unused_patches() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert!(run(&root).is_ok(), "{:?}", run(&root));
    }
}
