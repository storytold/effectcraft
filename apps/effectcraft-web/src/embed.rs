//! Whether a page that frames EffectCraft with `?host=parent` gets the host bridge ([`crate`]'s
//! `host` module on wasm32).
//!
//! Host mode hands the page's files to the parent: the session isn't kept in browser storage, the
//! recovery offer doesn't open, and saves and renders are posted to the parent instead of
//! downloading. Those messages go to this page's own origin only, so for a parent on any other
//! origin the browser would drop every save without an error while the project showed as saved.
//! Host mode is therefore for a parent on the page's own origin only; framed by anything else,
//! EffectCraft runs as it does unframed.

/// Whether a parent whose origin reads as `parent` may run this page (origin `own`) in host mode:
/// only when it could be read (reading a cross-origin parent's location throws, which is `None`)
/// and is the very same origin. An opaque origin (`"null"`: a sandboxed frame, a `file:` page)
/// matches nothing, not even another opaque one.
pub fn host_allowed(own: &str, parent: Option<&str>) -> bool {
    !own.is_empty() && own != "null" && parent == Some(own)
}
