//! Request building and answer handling of a save to a remote mount.

use crate::remote_config::{Config, parse};
use crate::remote_write::{
    Known, Outcome, Precondition, check_writable, classify_move, classify_put, delete_part, failure_message, head_target, move_part, part_path, precondition,
    put_part, valid_token,
};

const ORIGIN: &str = "https://app.example";

/// A config with one writable and one read-only mount.
fn cfg() -> Config {
    parse(br#"{"remote":{"mounts":[{"path":"/rw","url":"/dav/rw/","writable":true,"name":"Work"},{"path":"/ro","url":"/dav/ro/"}]}}"#, ORIGIN, "/").unwrap()
}

/// Part file is beside the target.
#[test]
fn part_file_is_beside_the_target() {
    assert_eq!(part_path("/rw/a/x.ecproj", "a1b2").unwrap(), "/rw/a/x.ecproj.a1b2.part");
    assert!(part_path("/rw/a/", "a1b2").is_err());
    assert!(part_path("", "a1b2").is_err());
}

/// Only writable mounts take writes.
#[test]
fn only_writable_mounts_take_writes() {
    let c = cfg();
    assert!(check_writable(&c, "/rw/x.ecproj").is_ok());
    let e = check_writable(&c, "/ro/x.ecproj").unwrap_err();
    assert!(e.contains("read-only"), "{e}");
    assert!(check_writable(&c, "/elsewhere/x").is_err());
    assert!(put_part(&c, "/ro/x.ecproj", "a1b2").is_err());
    assert!(move_part(&c, ORIGIN, "/ro/x.ecproj", "a1b2", true, None).is_err());
    assert!(delete_part(&c, "/ro/x.ecproj", "a1b2").is_err());
}

/// The upload goes to the part file with an encoded url.
#[test]
fn the_upload_goes_to_the_part_file_with_an_encoded_url() {
    let r = put_part(&cfg(), "/rw/My Films/a#b.ecproj", "a1b2").unwrap();
    assert_eq!((r.method, r.url.as_str()), ("PUT", "/dav/rw/My%20Films/a%23b.ecproj.a1b2.part"));
    let h = head_target(&cfg(), "/rw/My Films/a#b.ecproj").unwrap();
    assert_eq!((h.method, h.url.as_str()), ("HEAD", "/dav/rw/My%20Films/a%23b.ecproj"));
}

/// The move names the destination and overwrite.
#[test]
fn the_move_names_the_destination_and_overwrite() {
    let m = move_part(&cfg(), ORIGIN, "/rw/x.ecproj", "a1b2", true, None).unwrap();
    assert_eq!((m.method, m.url.as_str()), ("MOVE", "/dav/rw/x.ecproj.a1b2.part"));
    assert!(m.headers.contains(&("Destination", "https://app.example/dav/rw/x.ecproj".to_string())));
    assert!(m.headers.contains(&("Overwrite", "T".to_string())));
    let m = move_part(&cfg(), ORIGIN, "/rw/x.ecproj", "a1b2", false, None).unwrap();
    assert!(m.headers.contains(&("Overwrite", "F".to_string())));
    // a conditional header on a MOVE would be tested against the .part file: never sent
    assert!(m.headers.iter().all(|(k, _)| !k.eq_ignore_ascii_case("if-match") && !k.eq_ignore_ascii_case("if-none-match")));
    let d = delete_part(&cfg(), "/rw/x.ecproj", "a1b2").unwrap();
    assert_eq!((d.method, d.url.as_str()), ("DELETE", "/dav/rw/x.ecproj.a1b2.part"));
}

/// Hostile paths build no request.
#[test]
fn hostile_paths_build_no_request() {
    for p in ["/rw/../ro/x", "/rw/a//b", "/rw/a\\b", "/rw/"] {
        assert!(put_part(&cfg(), p, "a1b2").is_err(), "{p}");
        assert!(move_part(&cfg(), ORIGIN, p, "a1b2", true, None).is_err(), "{p}");
    }
}

/// A changed target is a conflict not an overwrite.
#[test]
fn a_changed_target_is_a_conflict_not_an_overwrite() {
    let was = Known::Etag("\"v1\"".into());
    assert_eq!(precondition(&was, 200, Some("\"v1\"")), Precondition::Proceed { overwrite: true });
    assert_eq!(precondition(&was, 200, Some("W/\"v1\"")), Precondition::Proceed { overwrite: true });
    assert_eq!(precondition(&was, 200, Some("\"v2\"")), Precondition::Conflict);
    // no ETag from the server now: it cannot be checked
    assert_eq!(precondition(&was, 200, None), Precondition::Proceed { overwrite: true });
    assert_eq!(precondition(&Known::NoEtag, 200, Some("\"x\"")), Precondition::Proceed { overwrite: true });
    // a file this session never saw must not exist
    assert_eq!(precondition(&Known::Unknown, 200, Some("\"x\"")), Precondition::Exists);
    assert_eq!(precondition(&Known::Unknown, 404, None), Precondition::Proceed { overwrite: false });
    assert_eq!(precondition(&was, 404, None), Precondition::Proceed { overwrite: false });
    for s in [401, 403, 500, 503, 301] {
        assert!(matches!(precondition(&was, s, None), Precondition::Failed(_)), "{s}");
    }
}

/// Answers are classified exactly.
#[test]
fn answers_are_classified_exactly() {
    for s in [200, 201, 204] {
        assert!(classify_put(s).is_ok());
    }
    for s in [202, 207, 400, 403, 409, 412, 413, 507] {
        assert!(classify_put(s).is_err(), "{s}");
    }
    assert_eq!(classify_move(201), Outcome::Saved);
    assert_eq!(classify_move(204), Outcome::Saved);
    assert_eq!(classify_move(412), Outcome::Conflict);
    for s in [200, 207, 403, 404, 409, 423, 500] {
        assert!(matches!(classify_move(s), Outcome::Failed(_)), "{s}");
    }
}

/// Failures say what to do.
#[test]
fn failures_say_what_to_do() {
    assert_eq!(failure_message(&Outcome::Saved), None);
    assert!(failure_message(&Outcome::Conflict).unwrap().contains("Save As"));
    assert!(failure_message(&Outcome::Exists).unwrap().contains("already exists"));
    assert!(failure_message(&Outcome::Failed("x".into())).unwrap().contains("x"));
}

/// Every upload has a part file of its own.
#[test]
fn every_upload_has_a_part_file_of_its_own() {
    let a = put_part(&cfg(), "/rw/x.ecproj", "aaaa").unwrap();
    let b = put_part(&cfg(), "/rw/x.ecproj", "bbbb").unwrap();
    assert_ne!(a.url, b.url);
    // the move and the cleanup name the same part file as the upload
    assert_eq!(move_part(&cfg(), ORIGIN, "/rw/x.ecproj", "aaaa", true, None).unwrap().url, a.url);
    assert_eq!(delete_part(&cfg(), "/rw/x.ecproj", "aaaa").unwrap().url, a.url);
    for bad in ["", "a/b", "a.b", "..", "a b", "é"] {
        assert!(put_part(&cfg(), "/rw/x.ecproj", bad).is_err(), "{bad:?}");
    }
    assert!(!valid_token(&"a".repeat(65)));
}

/// A replacing move carries the target's seen strong ETag in an `If` header tagged with the
/// destination; weak, malformed or absent ETags and a creating move send none.
#[test]
fn a_replacing_move_is_conditional_on_the_seen_etag() {
    let h = |m: &crate::remote_write::Req| m.headers.iter().find(|(k, _)| *k == "If").map(|(_, v)| v.clone());
    let m = move_part(&cfg(), ORIGIN, "/rw/x.ecproj", "a1b2", true, Some("\"v7\"")).unwrap();
    let dest = m.headers.iter().find(|(k, _)| *k == "Destination").unwrap().1.clone();
    assert_eq!(h(&m), Some(format!("<{dest}> ([\"v7\"])")));
    for seen in [Some("W/\"v7\""), Some("v7"), Some("\"a\"b\""), Some("\""), None] {
        assert_eq!(h(&move_part(&cfg(), ORIGIN, "/rw/x.ecproj", "a1b2", true, seen).unwrap()), None, "{seen:?}");
    }
    assert_eq!(h(&move_part(&cfg(), ORIGIN, "/rw/x.ecproj", "a1b2", false, Some("\"v7\"")).unwrap()), None);
}

/// The version a save leaves is the move's own, or the target's only when it is the upload's.
#[test]
fn a_saved_version_is_tied_to_the_upload() {
    use crate::remote_write::{SavedVersion as V, head_part, saved_version};
    let v = |s: &str| V::Etag(s.to_string());
    assert_eq!(saved_version(Some("\"m\""), Some("\"p\""), Some(Some("\"x\""))), v("\"m\""));
    // the move's own version needs no HEAD
    assert_eq!(saved_version(Some("\"m\""), None, None), v("\"m\""));
    assert_eq!(saved_version(None, Some("\"p\""), Some(Some("\"p\""))), v("\"p\""));
    // someone wrote in between, or the upload's version is unknown or weak: not tied
    assert_eq!(saved_version(None, Some("\"p\""), Some(Some("\"x\""))), V::Unbound);
    assert_eq!(saved_version(None, None, Some(Some("\"x\""))), V::Unbound);
    assert_eq!(saved_version(Some("W/\"m\""), Some("W/\"p\""), Some(Some("W/\"p\""))), V::Unbound);
    // a failed HEAD shows nothing: never "no ETag"
    assert_eq!(saved_version(None, Some("\"p\""), None), V::Unbound);
    assert_eq!(saved_version(None, None, None), V::Unbound);
    // a server that answers without ETags is as when the file was opened
    assert_eq!(saved_version(None, None, Some(None)), V::NoEtag);
    assert_eq!(saved_version(None, Some("\"p\""), Some(Some(" "))), V::NoEtag);
    let h = head_part(&cfg(), "/rw/x.ecproj", "a1b2").unwrap();
    assert_eq!((h.method, h.url.as_str()), ("HEAD", "/dav/rw/x.ecproj.a1b2.part"));
}
