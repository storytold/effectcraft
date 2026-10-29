//! Helpers shared by the import tests.
#![allow(dead_code, unused_macros)]
use effectcraft_aep::import::{AepImportError, ImportOptions, ImportResult, import};
use effectcraft_project::{Comp, ItemId, ItemKind, Project};

/// Imports a fixture into an empty project, allowing losses, with no footage file present;
/// `None` when the fixture is missing.
pub fn try_load(name: &str) -> Option<(Project, ImportResult)> {
    let mut project = Project::default();
    let bytes = crate::common::try_fixture(name)?;
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let r = import(&mut project, &bytes, &opts).unwrap_or_else(|e| panic!("{name}: {e}"));
    Some((project, r))
}

/// Imports a fixture into `project`; `None` when the fixture is missing.
pub fn try_import_fixture(project: &mut Project, name: &str, allow: bool) -> Option<Result<ImportResult, AepImportError>> {
    let bytes = crate::common::try_fixture(name)?;
    let none = |_: &str| false;
    Some(import(project, &bytes, &ImportOptions { allow_losses: allow, ..ImportOptions::new(&none) }))
}

/// [`try_load`]; returns from the calling test when the fixture is missing.
macro_rules! load {
    ($name:expr) => {
        match $crate::import::helpers::try_load(&$name) {
            Some(loaded) => loaded,
            None => return,
        }
    };
}

/// [`try_import_fixture`]; returns from the calling test when the fixture is missing.
macro_rules! try_import {
    ($project:expr, $name:expr, $allow:expr) => {
        match $crate::import::helpers::try_import_fixture($project, &$name, $allow) {
            Some(result) => result,
            None => return,
        }
    };
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

/// The name, item id and layer ids (top first) of every composition of a case, from the
/// container; `None` when the case is missing. Ids differ between the single-case files and the
/// cases of a combined file, so tests that patch ids read them here.
pub fn comp_ids(name: &str) -> Option<Vec<(String, u32, Vec<u32>)>> {
    let bytes = crate::common::try_fixture(name)?;
    let file = effectcraft_aep::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    let view = effectcraft_aep::Project::new(&file).expect("a project");
    Some(
        view.all_items()
            .into_iter()
            .filter_map(|i| Some((i.name().unwrap_or_default(), i.id()?, i.comp()?.layers().filter_map(|l| l.id()).collect())))
            .collect(),
    )
}
