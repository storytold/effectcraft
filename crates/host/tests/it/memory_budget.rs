//! #3298: every host session (the desktop app, the CLI, the MCP server) starts with the caches'
//! common budget: the caches have no limits of their own below it, so an unset budget would let
//! them grow until memory runs out.
//!
//! #3941: a headless session (the CLI, its MCP server) reads the system's memory once, so its
//! caches share the budget of the installed RAM, as in the desktop app.
//!
//! #4157: but a headless session's footage frames keep less than that budget: its renders read
//! each frame about once, and frames kept up to the budget ran a render out of memory.

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
    let mut headless = effectcraft_host::headless_session();
    let b = headless.memory_budget();
    assert_eq!(b.installed, Some(memory.total), "the budget of the installed RAM, not of unknown RAM");
    assert_eq!(b, effectcraft_engine::prefs::Prefs::default().memory_budget(Some(memory), b.reduced));
    assert_eq!(headless.layer_cache.budget(), b.bytes, "the caches follow it");
    assert_eq!(headless.export_cache_budget(), b.bytes, "exports and Render Queue runs follow it");

    // #4157: the footage frames keep less than a large budget, also once it changes (a new
    // memory reading, then the settings)
    let footage_under_budget = |s: &effectcraft_engine::Session, when: &str| {
        let (footage, budget) = (s.footage.cache_budget(), s.memory_budget().bytes);
        assert!(footage.is_some_and(|f| f < budget), "{when}: footage frames keep {footage:?} of a {budget}-byte budget");
    };
    headless.take_memory_reading(Some(effectcraft_engine::sysinfo::SysMemory { total: 64 << 30, available: 48 << 30 }));
    footage_under_budget(&headless, "a new memory reading");
    headless.execute("prefs.set", serde_json::json!({"key": "memory.ramReservedGb", "value": 8})).unwrap();
    footage_under_budget(&headless, "a settings change");
}
