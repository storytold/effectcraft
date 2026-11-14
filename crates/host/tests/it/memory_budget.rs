//! #3298: every host session (the desktop app, the CLI, the MCP server) starts with the caches'
//! common budget: the caches have no limits of their own below it, so an unset budget would let
//! them grow until memory runs out.

use effectcraft_engine::render::memory::Accountant;

#[test]
fn a_host_session_bounds_the_caches() {
    let s = effectcraft_host::session();
    let budget = Accountant::global().budget();
    assert!(budget < usize::MAX, "a finite budget");
    assert_eq!(budget, s.memory_budget().bytes);
    assert!(s.layer_cache.budget() <= budget);
}
