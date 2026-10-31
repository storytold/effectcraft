//! Host mode (`?host=parent`) only for a parent page on the app's own origin.

use effectcraft_web::embed::host_allowed;

/// A parent on the same origin gets the bridge.
#[test]
fn a_same_origin_parent_gets_host_mode() {
    assert!(host_allowed("https://cloud.example.org", Some("https://cloud.example.org")));
    assert!(host_allowed("http://localhost:8080", Some("http://localhost:8080")));
}

/// A cross-origin parent, whose location can't be read, doesn't: its saves would be posted to an
/// origin it isn't on and dropped without an error.
#[test]
fn a_parent_whose_origin_cannot_be_read_does_not() {
    assert!(!host_allowed("https://cloud.example.org", None));
}

/// Any difference in scheme, host or port is another origin.
#[test]
fn another_origin_does_not() {
    let own = "https://cloud.example.org";
    for parent in ["http://cloud.example.org", "https://example.org", "https://cloud.example.org:8443", "https://evil.example.com", ""] {
        assert!(!host_allowed(own, Some(parent)), "{parent:?}");
    }
}

/// An opaque origin (a sandboxed frame, a `file:` page) matches nothing, itself included.
#[test]
fn an_opaque_origin_never_does() {
    assert!(!host_allowed("null", Some("null")));
    assert!(!host_allowed("null", Some("https://cloud.example.org")));
    assert!(!host_allowed("https://cloud.example.org", Some("null")));
    assert!(!host_allowed("", Some("")));
}
