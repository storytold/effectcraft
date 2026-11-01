//! The `startup` key of the config file.

use effectcraft_web::startup::{CONFIG_MAX, Startup, parse};

/// The three accepted values parse.
#[test]
fn the_three_values() {
    assert_eq!(parse(br#"{"startup":"empty"}"#), Startup::Empty);
    assert_eq!(parse(br#"{"startup":"demo"}"#), Startup::Demo);
    assert_eq!(parse(br#"{"startup":"last"}"#), Startup::Last);
}

/// A missing key, a wrong type, an unknown value or a non-object file gives `Last`.
#[test]
fn absent_or_invalid_is_the_default() {
    for bytes in
        [&b"{}"[..], b"", b"not json", b"[]", b"null", br#"{"startup":1}"#, br#"{"startup":"Empty"}"#, br#"{"startup":null}"#, br#"{"startup":["empty"]}"#]
    {
        assert_eq!(parse(bytes), Startup::Last, "{:?}", String::from_utf8_lossy(bytes));
    }
}

/// Other keys do not hide `startup`.
#[test]
fn a_config_with_other_keys_still_has_its_startup() {
    assert_eq!(parse(br#"{"remote":{"mounts":[]},"startup":"empty"}"#), Startup::Empty);
    assert_eq!(parse(br#"{"startup":"demo","unknown":{"a":1}}"#), Startup::Demo);
}

/// A file over `CONFIG_MAX` is ignored.
#[test]
fn an_oversized_file_is_the_default() {
    let mut big = br#"{"startup":"empty","pad":""#.to_vec();
    big.extend(std::iter::repeat_n(b'x', CONFIG_MAX));
    big.extend_from_slice(br#""}"#);
    assert_eq!(parse(&big), Startup::Last);
}

/// Which project each value restores or opens.
#[test]
fn what_each_value_does_on_load() {
    assert!(Startup::Last.restores() && Startup::Last.demo_when_nothing_restored());
    assert!(Startup::Empty.restores() && !Startup::Empty.demo_when_nothing_restored());
    assert!(!Startup::Demo.restores() && Startup::Demo.demo_when_nothing_restored());
}

/// Only the explicit demo start stops saving the session.
#[test]
fn only_the_demo_start_leaves_the_saved_session_alone() {
    assert!(Startup::Last.keeps_session() && Startup::Empty.keeps_session());
    assert!(!Startup::Demo.keeps_session());
}
