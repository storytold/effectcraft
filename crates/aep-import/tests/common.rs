#![allow(dead_code)]
use effectcraft_aep_import::{AepImportError, ImportOptions, ImportResult, import};
use effectcraft_project::{Comp, ItemId, ItemKind, Project};

pub fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Imports a fixture into an empty project, allowing losses, with no footage file present.
pub fn load(name: &str) -> (Project, ImportResult) {
    let mut project = Project::default();
    let bytes = fixture_bytes(name);
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let r = import(&mut project, &bytes, &opts).unwrap_or_else(|e| panic!("{name}: {e}"));
    (project, r)
}

pub fn try_import(project: &mut Project, name: &str, allow: bool) -> Result<ImportResult, AepImportError> {
    let bytes = fixture_bytes(name);
    let none = |_: &str| false;
    import(project, &bytes, &ImportOptions { allow_losses: allow, ..ImportOptions::new(&none) })
}

pub fn comp<'p>(project: &'p Project, r: &ImportResult, name: &str) -> (ItemId, &'p Comp) {
    for id in &r.comps {
        if let Some(item) = project.item(*id)
            && item.name == name
            && let ItemKind::Comp(c) = &item.kind
        {
            return (*id, c);
        }
    }
    panic!("no comp {name}")
}
