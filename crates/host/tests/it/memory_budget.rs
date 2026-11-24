//! #3298: every host session (the desktop app, the CLI, the MCP server) starts with the caches'
//! common budget: the caches have no limits of their own below it, so an unset budget would let
//! them grow until memory runs out.
//!
//! #3941: a headless session (the CLI, its MCP server) reads the system's memory once, so its
//! caches share the budget of the installed RAM, as in the desktop app.

use effectcraft_engine::render::memory::Accountant;

/// One test: the headless session sets the common budget to the installed RAM's, which the
/// first check (the budget for unknown RAM) would see if both ran at once.
#[test]
fn a_host_session_bounds_the_caches() {
    let s = effectcraft_host::session();
    let budget = Accountant::global().budget();
    assert!(budget < usize::MAX, "a finite budget");
    assert_eq!(budget, s.memory_budget().bytes);
    assert!(s.layer_cache.budget() <= budget);

    // (a sandbox that can't read the system's memory keeps the budget for unknown RAM)
    let Some(memory) = effectcraft_engine::sysinfo::memory() else {
        eprintln!("the system's memory can't be read here: the headless reading is not checked");
        return;
    };
    let headless = effectcraft_host::headless_session();
    let b = headless.memory_budget();
    assert_eq!(b.installed, Some(memory.total), "the budget of the installed RAM, not of unknown RAM");
    assert_eq!(b, effectcraft_engine::prefs::Prefs::default().memory_budget(Some(memory), b.reduced));
    assert_eq!(headless.layer_cache.budget(), b.bytes, "the caches follow it");
    assert_eq!(headless.export_cache_budget(), b.bytes, "exports and Render Queue runs follow it");
}
