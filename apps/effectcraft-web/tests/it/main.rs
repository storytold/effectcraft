// The pure remote-storage modules, included as the crate's own modules (the real crate is wasm32-only).
#[allow(dead_code)]
#[path = "../../src/remote_config.rs"]
mod remote_config;
mod remote_config_tests;
#[allow(dead_code)]
#[path = "../../src/remote_dav.rs"]
mod remote_dav;
mod remote_dav_tests;
#[allow(dead_code)]
#[path = "../../src/remote_roots.rs"]
mod remote_roots;
#[allow(dead_code)]
#[path = "../../src/remote_util.rs"]
mod remote_util;
mod remote_util_tests;
#[allow(dead_code)]
#[path = "../../src/remote_write.rs"]
mod remote_write;
mod remote_write_tests;
