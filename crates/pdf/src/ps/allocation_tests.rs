use super::exec::{MAX_NAME, MAX_STACK};
use super::{Font, Interp, Rc, V};

/// Repeated short array/procedure slices spend only their returned slots, not source snapshots
/// (and each `load` the scan of the name `a`).
#[test]
fn eps_getinterval_short_slices_charge_only_returned_slots() {
    let mut failures = vec![];
    for setup in ["/a 4096 array def", "/a 4096 array cvx def"] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(setup.as_bytes());
        assert_eq!(it.lookup("a").unwrap().item_count(), 4096);
        it.allocation_budget = 3 * (std::mem::size_of::<V>() + "a".len());
        it.run(b"3 { /a load 17 1 getinterval pop } repeat 0 0 10 10 rectfill");
        let left = it.allocation_budget;
        let (nodes, _, skipped) = it.b.finish();
        if left != 0 || nodes.len() != 1 || !skipped.is_empty() {
            failures.push((setup, left, nodes.len(), skipped));
        }
    }
    assert!(failures.is_empty(), "short intervals must fit their exact cumulative budget: {failures:?}");
}

/// A one-byte-short interval budget quits through `stopped` without losing earlier slices or art.
#[test]
fn eps_getinterval_refuses_only_the_slice_that_exceeds_the_budget() {
    for setup in ["/a 4096 array def", "/a 4096 array cvx def"] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(setup.as_bytes());
        it.run(b"0 0 10 10 rectfill");
        // Three slices and the three `load`s of `a` that fetch them, one byte short.
        it.allocation_budget = 3 * (std::mem::size_of::<V>() + "a".len()) - 1;
        it.run(b"{ 3 { /a load 17 1 getinterval } repeat } stopped 20 20 10 10 rectfill");
        assert_eq!(it.st.len(), 2, "{setup}: only two complete slices fit");
        assert!(it.st.iter().all(|v| matches!(v, V::Array(_)) && v.item_count() == 1));
        assert_eq!(it.allocation_budget, std::mem::size_of::<V>() - 1);
        let (nodes, _, skipped) = it.b.finish();
        assert_eq!(nodes.len(), 1, "{setup}: the resource quit must escape stopped and retain prior artwork");
        assert_eq!(skipped, ["PostScript (allocation limit)"]);
    }
}

/// Array/procedure intervals clamp empty, oversized and saturating ranges before charging.
#[test]
fn eps_getinterval_clamps_compound_bounds_before_charging() {
    for setup in ["/a [ 11 22 33 ] def", "/a { 11 22 33 } def"] {
        for (range, expected) in [
            ("1 0", &[][..]),
            ("3 1", &[][..]),
            ("99 99", &[][..]),
            ("9999999999999999999999999999999999999999 9999999999999999999999999999999999999999", &[][..]),
            ("2 9", &[33.0][..]),
            ("1 9999999999999999999999999999999999999999", &[22.0, 33.0][..]),
            ("0 3", &[11.0, 22.0, 33.0][..]),
        ] {
            let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
            it.run(setup.as_bytes());
            it.allocation_budget = expected.len() * std::mem::size_of::<V>() + "a".len();
            it.run(format!("/a load {range} getinterval").as_bytes());
            assert_eq!(it.st.len(), 1, "{setup}: {range}");
            let V::Array(part) = it.st.last().unwrap() else { panic!("intervals must still return arrays") };
            assert_eq!(part.borrow().iter().map(|v| v.num().unwrap()).collect::<Vec<_>>(), expected);
            assert_eq!(it.allocation_budget, 0);
            let (_, _, skipped) = it.b.finish();
            assert!(skipped.is_empty(), "{setup}: {range}: {skipped:?}");
        }
    }
}

/// Interval payloads retain slot order and shallow references to mutable compound values.
#[test]
fn eps_getinterval_preserves_order_and_shallow_identity() {
    for setup in ["/a [ 0 [ 11 ] (ab) /tag 9 ] def", "/a [ 0 [ 11 ] (ab) /tag 9 ] cvx def"] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(setup.as_bytes());
        let source = it.lookup("a").unwrap().items().unwrap();
        // The interval's four slots, the new entry `slice` the dictionary keeps, and three `load`s of
        // `a`.
        it.allocation_budget = 4 * std::mem::size_of::<V>() + "slice".len() + std::mem::size_of::<(String, V)>() + 3 * "a".len();
        it.run(b"/a load 1 4 getinterval /slice exch def /a load 1 get 0 77 put /a load 2 get 0 122 put slice");
        let V::Array(part) = it.st.last().unwrap() else { panic!("intervals must still return arrays") };
        let part = part.borrow();
        assert_eq!(part.len(), 4);
        assert!(matches!((&source[1], &part[0]), (V::Array(a), V::Array(b)) if Rc::ptr_eq(a, b) && b.borrow()[0].num() == Some(77.0)));
        assert!(matches!((&source[2], &part[1]), (V::Str(a), V::Str(b)) if Rc::ptr_eq(a, b) && *b.borrow() == b"zb"));
        assert!(matches!(&part[2], V::Lit(name) if name.as_ref() == "tag"));
        assert_eq!(part[3].num(), Some(9.0));
        assert_eq!(it.allocation_budget, 0);
        let (_, _, skipped) = it.b.finish();
        assert!(skipped.is_empty(), "{setup}: {skipped:?}");
    }
}

/// String intervals keep their existing byte-based bounds, exact charge and refusal behavior.
#[test]
fn eps_getinterval_preserves_string_budget_and_bounds() {
    for (range, expected, budget, allowed) in [
        ("1 2", &b"bc"[..], 2, true),
        ("1 2", &b"bc"[..], 1, false),
        ("1 0", &b""[..], 0, true),
        ("99 99", &b""[..], 0, true),
        ("9999999999999999999999999999999999999999 9999999999999999999999999999999999999999", &b""[..], 0, true),
        ("2 9999999999999999999999999999999999999999", &b"cd"[..], 2, true),
    ] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(b"/s (abcd) def");
        it.allocation_budget = budget;
        it.run(format!("s {range} getinterval").as_bytes());
        assert_eq!(it.st.len(), usize::from(allowed));
        if allowed {
            assert!(matches!(it.st.last(), Some(V::Str(s)) if *s.borrow() == expected));
            assert_eq!(it.allocation_budget, 0);
        } else {
            assert_eq!(it.allocation_budget, budget);
        }
        let (_, _, skipped) = it.b.finish();
        if allowed {
            assert!(skipped.is_empty(), "{range}: {skipped:?}");
        } else {
            assert_eq!(skipped, ["PostScript (allocation limit)"]);
        }
    }
}

/// Three depth-two dictionary snapshots spend exactly their six runtime value slots.
#[test]
fn eps_dictstack_exact_snapshot_budget_reaches_later_artwork() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"1 dict begin 0 0 10 10 rectfill");
    assert_eq!(it.dicts.len(), 2);
    it.allocation_budget = 3 * 2 * std::mem::size_of::<V>();
    it.run(b"{ 3 { 0 array dictstack } repeat } stopped 20 20 10 10 rectfill");
    assert_eq!(it.allocation_budget, 0, "every snapshot must spend its slots");
    assert_eq!(it.st.len(), 4);
    assert!(it.st[..3].iter().all(|v| matches!(v, V::Array(_)) && v.item_count() == 2));
    assert!(matches!(it.st.last(), Some(V::Bool(false))), "ordinary completion remains catchable by stopped");
    let (nodes, _, skipped) = it.b.finish();
    assert_eq!(nodes.len(), 2);
    assert!(skipped.is_empty(), "{skipped:?}");
}

/// A one-byte-short cumulative budget refuses only the third snapshot, before any partial result.
#[test]
fn eps_dictstack_one_byte_short_preserves_snapshots_and_prior_artwork() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"1 dict begin 0 0 10 10 rectfill");
    assert_eq!(it.dicts.len(), 2);
    let snapshot = 2 * std::mem::size_of::<V>();
    it.allocation_budget = 3 * snapshot - 1;
    it.run(b"{ 3 { 0 array dictstack } repeat } stopped 20 20 10 10 rectfill");
    assert_eq!(it.st.len(), 2, "only two complete snapshots fit; Quit must not push a stopped result");
    assert!(it.st.iter().all(|v| matches!(v, V::Array(_)) && v.item_count() == 2));
    assert_eq!(it.allocation_budget, snapshot - 1, "refusal must not spend a partial snapshot");
    let (nodes, _, skipped) = it.b.finish();
    assert_eq!(nodes.len(), 1, "resource Quit must escape stopped and retain prior artwork");
    assert_eq!(skipped, ["PostScript (allocation limit)"]);
}

/// A prepared destination reaches dictstack itself at zero budget, including through nested stopped.
#[test]
fn eps_dictstack_zero_budget_quit_escapes_nested_stopped() {
    for body in ["dictstack 20 20 10 10 rectfill", "{ { dictstack 20 20 10 10 rectfill } stopped 40 40 10 10 rectfill } stopped 60 60 10 10 rectfill"] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(b"0 0 10 10 rectfill 42 0 array");
        assert_eq!(it.dicts.len(), 1);
        it.allocation_budget = 0;
        it.run(body.as_bytes());
        assert_eq!(it.st.len(), 1, "{body}: consume the destination but push no refused snapshot or stopped result");
        assert_eq!(it.st[0].num(), Some(42.0));
        assert_eq!(it.allocation_budget, 0);
        let (nodes, _, skipped) = it.b.finish();
        assert_eq!(nodes.len(), 1, "{body}: keep prior artwork and suppress every later drawing");
        assert_eq!(skipped, ["PostScript (allocation limit)"]);
    }
}

/// Dropping a dictionary-stack snapshot never refunds its runtime slots.
#[test]
fn eps_dictstack_discarded_snapshots_still_spend_the_cumulative_budget() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"0 0 10 10 rectfill");
    it.allocation_budget = 2 * std::mem::size_of::<V>();
    it.run(b"3 { 0 array dictstack pop } repeat 20 20 10 10 rectfill");
    assert_eq!(it.allocation_budget, 0);
    assert!(it.st.is_empty(), "the denied third snapshot must not leave a partial result");
    let (nodes, _, skipped) = it.b.finish();
    assert_eq!(nodes.len(), 1);
    assert_eq!(skipped, ["PostScript (allocation limit)"]);
}

/// Keep the existing fresh-array result, untouched destination, stack order and shallow dictionaries.
#[test]
fn eps_dictstack_preserves_legacy_destination_and_shallow_dictionary_identity() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"7 /destination [ 11 22 33 44 ] def 1 dict begin /key 9 def");
    let V::Array(destination) = it.lookup("destination").unwrap() else { panic!("destination is an array") };
    let dictionaries = it.dicts.clone();
    it.run(b"destination dictstack end 0 0 10 10 rectfill");
    assert_eq!(it.st.len(), 2);
    assert_eq!(it.st[0].num(), Some(7.0));
    let V::Array(snapshot) = &it.st[1] else { panic!("dictstack returns an array") };
    assert!(!Rc::ptr_eq(snapshot, &destination), "legacy dictstack returns a fresh array rather than filling the destination");
    assert_eq!(destination.borrow().iter().map(|v| v.num().unwrap()).collect::<Vec<_>>(), [11.0, 22.0, 33.0, 44.0]);
    let snapshot = snapshot.borrow();
    assert_eq!(snapshot.len(), dictionaries.len());
    for (value, dictionary) in snapshot.iter().zip(&dictionaries) {
        assert!(matches!(value, V::Dict(d) if Rc::ptr_eq(d, dictionary)), "bottom-to-top dictionary identities must be shared, not copied");
    }
    dictionaries[1].borrow_mut().insert("key".to_owned(), V::Num(77.0));
    assert!(matches!(&snapshot[1], V::Dict(d) if d.borrow().get("key").and_then(V::num) == Some(77.0)));
    assert_eq!(it.dicts.len(), 1, "end must not invalidate the returned dictionary reference");
    let (nodes, _, skipped) = it.b.finish();
    assert_eq!(nodes.len(), 1);
    assert!(skipped.is_empty(), "{skipped:?}");
}

/// Exhausted string equality must stop execution before subsequent artwork is drawn.
#[test]
fn eps_string_comparison_eq_stops_at_exhausted_budget() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"/s 10 string def");
    it.allocation_budget = 0;
    it.run(b"s s eq pop 0 0 10 10 rectfill");
    let (nodes, _, skipped) = it.b.finish();
    assert!(nodes.is_empty(), "a denied comparison copy must stop execution before rectfill");
    assert_eq!(skipped, ["PostScript (allocation limit)"]);
}

/// String inequality must honor the same copy refusal as equality.
#[test]
fn eps_string_comparison_ne_stops_at_exhausted_budget() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"/s 10 string def");
    it.allocation_budget = 0;
    it.run(b"s s ne pop 0 0 10 10 rectfill");
    let (nodes, _, skipped) = it.b.finish();
    assert!(nodes.is_empty(), "a denied comparison copy must stop execution before rectfill");
    assert_eq!(skipped, ["PostScript (allocation limit)"]);
}

/// Discarding comparison results must not refund their temporary string-copy charges.
#[test]
fn eps_string_comparison_repeated_copies_are_cumulative() {
    for op in ["eq", "ne"] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(b"/s 10 string def");
        // Two worst-case UTF-8 conversion bounds fit exactly once, not twice.
        it.allocation_budget = 60;
        it.run(format!("2 {{ s s {op} pop }} repeat 0 0 10 10 rectfill").as_bytes());
        let (nodes, _, skipped) = it.b.finish();
        assert!(nodes.is_empty(), "{op}: discarded comparison strings must not refund the budget");
        assert_eq!(skipped, ["PostScript (allocation limit)"], "{op}");
    }
}

/// Preserve lenient cross-type, lossy UTF-8, empty-key and compound-identity comparisons.
#[test]
fn eps_string_comparison_preserves_existing_equality_semantics() {
    // These are compatibility controls, including lenient cross-type and lossy-UTF-8
    // comparisons and the existing rule that an empty key is never equal.
    for (operands, expected) in [
        ("(same) (same)", true),
        ("(same) (other)", false),
        ("() ()", false),
        ("() /same", false),
        ("/same ()", false),
        ("<ff> <fe>", true),
        ("<ff> <efbfbd>", true),
        ("<ff> (x)", false),
        ("(same) /same", true),
        ("/same (same)", true),
        ("/same /same", true),
        ("/same /other", false),
        ("(12) 12", true),
        ("12 (12)", true),
        ("(true) true", true),
        ("true (true)", true),
        ("1 1", true),
        ("1 2", false),
        ("true true", true),
        ("true false", false),
        ("1 true", false),
        ("null null", true),
        ("0 array dup", true),
        ("0 array 0 array", false),
        ("0 dict dup", true),
        ("0 dict 0 dict", false),
        ("{} dup", false),
    ] {
        for op in ["eq", "ne"] {
            let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
            it.run(format!("{operands} {op}").as_bytes());
            assert_eq!(it.st.len(), 1, "{operands} {op}");
            assert!(matches!(it.st.last(), Some(V::Bool(b)) if *b == (expected == (op == "eq"))), "{operands} {op}");
            let (_, _, skipped) = it.b.finish();
            assert!(skipped.is_empty(), "{operands} {op}: {skipped:?}");
        }
    }
}

/// Each UTF-8 conversion must fit before copying, while short-circuited operands cost nothing. A
/// name operand is charged its length first, so `/same` leaves 7 of 11 bytes, too few for the
/// string's 12.
#[test]
fn eps_string_comparison_charges_each_lossy_copy_before_conversion() {
    for op in ["eq", "ne"] {
        for (operands, budget, left, allowed) in [
            ("(0123456789) (0123456789)", 60, 0, true),
            ("(0123456789) (0123456789)", 59, 29, false),
            ("/same (same)", 11, 7, false),
            ("(same) /same", 11, 11, false),
            ("<fffe> <fffe>", 12, 0, true),
            ("<fffe> <fffe>", 11, 5, false),
            ("() ()", 0, 0, true),
            ("() (not copied)", 0, 0, true),
            ("1 1", 0, 0, true),
            ("true false", 0, 0, true),
        ] {
            let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
            it.allocation_budget = budget;
            it.run(format!("{operands} {op}").as_bytes());
            assert_eq!(it.allocation_budget, left, "{operands} {op}, budget {budget}");
            assert_eq!(it.st.len(), usize::from(allowed), "denied comparisons must not push a partial result");
            let (_, _, skipped) = it.b.finish();
            if allowed {
                assert!(skipped.is_empty(), "{operands} {op}: {skipped:?}");
            } else {
                assert_eq!(skipped, ["PostScript (allocation limit)"], "{operands} {op}");
            }
        }
    }
}

/// A comparison resource quit must escape `stopped` while retaining earlier artwork.
#[test]
fn eps_string_comparison_quit_inside_stopped_preserves_prior_artwork() {
    for op in ["eq", "ne"] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(b"0 0 10 10 rectfill /s 10 string def");
        it.allocation_budget = 0;
        it.run(format!("{{ s s {op} pop 20 20 10 10 rectfill }} stopped pop 40 40 10 10 rectfill").as_bytes());
        let (nodes, _, skipped) = it.b.finish();
        assert_eq!(nodes.len(), 1, "{op}: stopped must not swallow a resource quit or discard prior artwork");
        assert_eq!(skipped, ["PostScript (allocation limit)"]);
    }
}

/// The executable-string estimate must still admit nested procedures and ASCII85 zero groups.
#[test]
fn eps_cvx_accepts_nested_procedures_and_ascii85_zero_expansion() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"({ { 7 } exec } exec <~z~> length) cvx exec");
    assert_eq!(it.st.iter().filter_map(V::num).collect::<Vec<_>>(), [7.0, 4.0]);
    let (_, _, skipped) = it.b.finish();
    assert!(skipped.is_empty(), "{skipped:?}");
}

/// Constructors, intervals and conversions must each refuse an over-budget runtime payload.
#[test]
fn eps_runtime_copies_share_the_allocation_budget() {
    // A reduced private counter exercises the real operators without large allocations.
    // Setup allocates before the seam; each body tries to allocate more than 64 bytes.
    let mut missed = vec![];
    for (setup, body) in [
        ("", "10 array"),
        ("", "65 string"),
        ("/a 10 array def", "a 0 10 getinterval"),
        ("/s 65 string def", "s 0 65 getinterval"),
        ("/a 10 array def", "a cvx"),
        ("/p { 1 2 3 4 5 6 7 8 9 10 } def", "/p load cvlit"),
        ("/s (1 2 3 4 5 6 7 8 9 10) def", "s cvx"),
        ("", "[ 1 2 3 4 5 6 7 8 9 10 ]"),
        ("/a 10 array def", "1 2 3 4 5 6 7 8 9 10 a astore"),
        ("/a 10 array def", "a 10 array copy"),
        ("/a 10 array def", "a {} forall"),
        ("/s 65 string def", "s {} forall"),
    ] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(setup.as_bytes());
        it.allocation_budget = 64;
        it.run(format!("{body} clear 0 0 10 10 rectfill").as_bytes());
        let (nodes, _, skipped) = it.b.finish();
        if skipped != ["PostScript (allocation limit)"] || !nodes.is_empty() {
            missed.push(body);
        }
    }
    assert!(missed.is_empty(), "unbudgeted operator allocations: {missed:?}");
}

/// Read-side snapshots, matrix values and encodings must share the runtime payload cap.
#[test]
fn eps_runtime_read_snapshots_share_the_allocation_budget() {
    let mut missed = vec![];
    for (setup, body) in [
        ("/a 10 array def", "a 0 setdash"),
        ("/a 10 array def", "a rectfill"),
        ("/a 10 array def", "a 10 array def"),
        ("/a 10 array def /b 10 array def", "a b copy"),
        ("/s 65 string def", "s show"),
        ("/s 65 string def", "s 0 string cvs"),
        ("", "StandardEncoding"),
        ("", "matrix matrix"),
    ] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(setup.as_bytes());
        it.allocation_budget = 64;
        it.run(format!("{body} clear").as_bytes());
        let (_, _, skipped) = it.b.finish();
        if skipped != ["PostScript (allocation limit)"] {
            missed.push(body);
        }
    }
    assert!(missed.is_empty(), "unbudgeted payloads: {missed:?}");
}

/// Legacy EPS encoding choices and their existing lookup-scan costs, including lenient aliases.
fn encoding_cases() -> Vec<(String, crate::encoding::Base, usize)> {
    use crate::encoding::Base;

    let mut cases = vec![];
    for (name, base) in [
        ("StandardEncoding", Base::Standard),
        ("ISOLatin1Encoding", Base::WinAnsi),
        ("WinAnsiEncoding", Base::WinAnsi),
        ("MacRomanEncoding", Base::WinAnsi),
        ("UnknownEncoding", Base::WinAnsi),
    ] {
        if name == "StandardEncoding" || name == "ISOLatin1Encoding" {
            cases.push((name.to_owned(), base, 0));
        }
        cases.push((format!("/{name} findencoding"), base, name.len()));
        cases.push((format!("/{name} /Encoding findresource"), base, name.len() + "Encoding".len()));
        cases.push((format!("({name}) findencoding"), base, 3 * name.len()));
        cases.push((format!("({name}) (Encoding) findresource"), base, 3 * (name.len() + "Encoding".len())));
    }
    cases
}

/// The actual retained name text is an independent oracle, not a duplicated encoding table.
fn encoding_payload_bytes(base: crate::encoding::Base) -> usize {
    256 * std::mem::size_of::<V>() + (0..=u8::MAX).map(|c| base.name(c).unwrap_or_else(|| ".notdef".into()).len()).sum::<usize>()
}

/// Every encoding keeps 256 slots and fresh literal-name text, including each `.notdef`.
#[test]
fn eps_encoding_names_exact_budget_reaches_later_artwork() {
    for (body, base, scan) in encoding_cases() {
        let names: Vec<_> = (0..=u8::MAX).map(|c| base.name(c).unwrap_or_else(|| ".notdef".into())).collect();
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.allocation_budget = encoding_payload_bytes(base) + scan;
        it.run(format!("{body} 0 0 10 10 rectfill").as_bytes());
        assert_eq!(it.st.len(), 1, "{body}: exactly one complete encoding");
        let V::Array(a) = &it.st[0] else { panic!("encodings remain arrays") };
        let a = a.borrow();
        assert_eq!(a.len(), 256);
        for (value, name) in a.iter().zip(&names) {
            assert!(matches!(value, V::Lit(n) if n.as_ref() == name), "{body}: literal glyph names must be unchanged");
        }
        assert_eq!(it.allocation_budget, 0, "{body}: every kept name byte must be charged");
        drop(a);
        let (nodes, _, skipped) = it.b.finish();
        assert_eq!(nodes.len(), 1, "{body}: the exact budget must reach later artwork");
        assert!(skipped.is_empty(), "{body}: {skipped:?}");
    }
}

/// A short payload budget refuses before publishing an encoding; Quit escapes nested `stopped`.
#[test]
fn eps_encoding_names_short_budget_preserves_prior_artwork() {
    for (body, base, scan) in encoding_cases() {
        let bytes = encoding_payload_bytes(base);
        for budget in [0, 256 * std::mem::size_of::<V>(), bytes - 1] {
            let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
            it.run(b"0 0 10 10 rectfill 42");
            it.allocation_budget = budget + scan;
            it.run(format!("{{ {{ {body} 20 20 10 10 rectfill }} stopped 40 40 10 10 rectfill }} stopped 60 60 10 10 rectfill").as_bytes());
            assert_eq!(it.st.len(), 1, "{body}, {budget}: no partial encoding or stopped result");
            assert_eq!(it.st[0].num(), Some(42.0));
            assert_eq!(it.allocation_budget, budget, "{body}: precharge the entire encoding atomically after lookup scans");
            let (nodes, _, skipped) = it.b.finish();
            assert_eq!(nodes.len(), 1, "{body}: Quit must keep prior artwork and suppress every later drawing");
            assert_eq!(skipped, ["PostScript (allocation limit)"], "{body}, {budget}");
        }
    }
}

/// Kept and discarded encodings spend the same cumulative payload budget, without refunds.
#[test]
fn eps_encoding_names_repeated_arrays_do_not_refund_budget() {
    for (body, base, scan) in encoding_cases() {
        let bytes = encoding_payload_bytes(base);
        for discard in ["", "pop"] {
            for short in [0, 1] {
                let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
                it.run(b"0 0 10 10 rectfill");
                // A prior string and the encodings must use the same counter.
                it.allocation_budget = 3 + 3 * (bytes + scan) - short;
                it.run(format!("3 string pop {{ 3 {{ {body} {discard} }} repeat }} stopped 20 20 10 10 rectfill").as_bytes());
                let fits = short == 0;
                let arrays = if discard.is_empty() { if fits { 3 } else { 2 } } else { 0 };
                assert_eq!(it.st.len(), arrays + usize::from(fits), "{body}, {discard}, short {short}");
                assert!(it.st.iter().take(arrays).all(|v| matches!(v, V::Array(_)) && v.item_count() == 256));
                if fits {
                    assert!(matches!(it.st.last(), Some(V::Bool(false))));
                }
                assert_eq!(it.allocation_budget, if fits { 0 } else { bytes - 1 }, "{body}: every completed array must stay charged");
                let (nodes, _, skipped) = it.b.finish();
                assert_eq!(nodes.len(), if fits { 2 } else { 1 }, "{body}: keep earlier artwork");
                if fits {
                    assert!(skipped.is_empty(), "{body}: {skipped:?}");
                } else {
                    assert_eq!(skipped, ["PostScript (allocation limit)"], "{body}");
                }
            }
        }
    }
}

/// Encoding results remain fresh mutable arrays of literal names with the legacy mappings.
#[test]
fn eps_encoding_names_preserve_fresh_arrays_and_legacy_mappings() {
    for (body, base, _) in encoding_cases() {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(format!("{body} {body}").as_bytes());
        assert_eq!(it.st.len(), 2, "{body}");
        let (V::Array(a), V::Array(b)) = (&it.st[0], &it.st[1]) else { panic!("encodings remain arrays") };
        assert!(!Rc::ptr_eq(a, b), "{body}: arrays must remain independently mutable");
        for (c, value) in b.borrow().iter().enumerate() {
            let expected = base.name(c as u8).unwrap_or_else(|| ".notdef".into());
            assert!(matches!(value, V::Lit(n) if n.as_ref() == expected), "{body}: code {c}");
        }
        a.borrow_mut()[65] = V::Lit("changed".into());
        assert!(matches!(&b.borrow()[65], V::Lit(n) if n.as_ref() == "A"));
        let (_, _, skipped) = it.b.finish();
        assert!(skipped.is_empty(), "{body}: {skipped:?}");
    }
}

/// Numeric `copy` must charge all new operand slots before extending the stack.
#[test]
fn eps_numeric_copy_charges_new_operand_slots() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"1 2 3 4");
    it.allocation_budget = 0;
    it.run(b"4 copy 0 0 10 10 rectfill");
    assert_eq!(it.st.len(), 4, "refuse before copying any operands");
    let (nodes, _, skipped) = it.b.finish();
    assert_eq!(skipped, ["PostScript (allocation limit)"]);
    assert!(nodes.is_empty());
}

/// Small built-in strings must accumulate charges just like explicit string constructors.
#[test]
fn eps_small_runtime_strings_share_the_allocation_budget() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.allocation_budget = 7;
    it.run(b"version product 0 0 10 10 rectfill");
    let (nodes, _, skipped) = it.b.finish();
    assert_eq!(skipped, ["PostScript (allocation limit)"]);
    assert!(nodes.is_empty());
}

/// Exact stack limits must preserve operand order, shared arrays and consume-first `astore`.
#[test]
fn eps_stack_boundaries_preserve_order_and_compound_identity() {
    let nums = |it: &Interp| it.st.iter().map(|v| v.num().unwrap()).collect::<Vec<_>>();
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"1 2 3 2 copy");
    assert_eq!(nums(&it), [1.0, 2.0, 3.0, 2.0, 3.0]);
    it.run(b"clear [ 4 5 6 ] 3 array copy aload pop");
    assert_eq!(nums(&it), [4.0, 5.0, 6.0]);
    it.run(b"clear 7 8 9 3 array astore aload pop");
    assert_eq!(nums(&it), [7.0, 8.0, 9.0]);
    it.run(b"clear /a [ 10 11 ] def a 1 copy 0 99 put clear a aload pop pop");
    assert_eq!(nums(&it), [99.0]);

    // Numeric copy and aload admit the exact final depth, including aload's array.
    it.st = vec![V::Num(1.0); MAX_STACK / 2];
    it.run(b"50000 copy");
    assert_eq!(it.st.len(), MAX_STACK);
    it.run(b"clear 99999 array aload");
    assert_eq!(it.st.len(), MAX_STACK);
    assert!(matches!(it.st.last(), Some(V::Array(_))));
    // astore consumes before returning its array; it needs no extra stack room.
    it.run(b"astore");
    assert_eq!(it.st.len(), 1);
    assert_eq!(it.st[0].item_count(), MAX_STACK - 1);
    let (_, _, skipped) = it.b.finish();
    assert!(skipped.is_empty(), "{skipped:?}");
}

/// Array slots, string bytes and dictionary keys must consume one counter without refunds on
/// overwrite or drop; overwriting a key's value keeps nothing new and charges only the scan that
/// finds the key.
#[test]
fn eps_mixed_payloads_exhaust_one_budget_without_refunds() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    let slots = 2 * std::mem::size_of::<V>();
    // Two slots, three string bytes, the entry `a` once, and the scan of `a` when its second `def`
    // overwrites it: exactly the budget, so the program runs to its end.
    it.allocation_budget = slots + 3 + "a".len() + std::mem::size_of::<(String, V)>() + "a".len();
    it.run(b"/a 2 array def 3 string pop /a null def");
    assert_eq!(it.allocation_budget, 0);
    assert!(it.b.skipped.is_empty(), "the exact budget fits: {:?}", it.b.skipped);
    it.run(b"0 string pop 1 string");
    assert_eq!(it.allocation_budget, 0);
    assert!(it.st.is_empty(), "refused allocation must not push a partial result");
    let (_, _, skipped) = it.b.finish();
    assert_eq!(skipped, ["PostScript (allocation limit)"]);
}

/// `aload` must check its whole result, including the array, before cloning or charging slots.
#[test]
fn eps_aload_refuses_before_cloning_or_pushing_items() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"42 /a 100000 array def");
    let left = it.allocation_budget;
    it.run(b"a aload");
    assert_eq!(it.st.len(), 1);
    assert_eq!(it.st[0].num(), Some(42.0));
    assert_eq!(it.allocation_budget, left);
    let (_, _, skipped) = it.b.finish();
    assert_eq!(skipped, ["PostScript (operand stack limit)"]);
}

/// An overflowing numeric `copy` must leave both stack contents and allocation budget intact.
#[test]
fn eps_copy_refuses_before_extending_the_stack() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.st = vec![V::Num(1.0); MAX_STACK / 2 + 1];
    let left = it.allocation_budget;
    it.run(b"50000 copy");
    assert_eq!(it.st.len(), MAX_STACK / 2 + 1);
    assert_eq!(it.allocation_budget, left);
    let (_, _, skipped) = it.b.finish();
    assert_eq!(skipped, ["PostScript (operand stack limit)"]);
}

/// Repeated discarded payloads must exhaust the cumulative cap despite bounded live storage.
#[test]
fn eps_runtime_payload_allocations_are_cumulative() {
    // Only one 1 MB string is live at a time. The reduced counter exercises repeated
    // allocation even when each object is discarded before the next one is created.
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.allocation_budget = 2_500_000;
    it.run(b"3 { 1000000 string pop } repeat 0 0 10 10 rectfill");
    let (nodes, _, skipped) = it.b.finish();
    assert_eq!(skipped, ["PostScript (allocation limit)"]);
    assert!(nodes.is_empty(), "execution must stop, not merely discard an allocation");
}

/// Length, element and matrix reads must still work when no payload-copy budget remains.
#[test]
fn eps_reads_work_with_no_allocation_budget_left() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"/a [ 1 2 3 ] def /m [ 2 0 0 3 0 0 ] def");
    it.allocation_budget = 0;
    it.run(b"a length a 1 get m setmatrix 1 1 transform");
    assert_eq!(it.st.iter().filter_map(V::num).collect::<Vec<_>>(), [3.0, 2.0, 2.0, 3.0]);
    let (_, _, skipped) = it.b.finish();
    assert!(skipped.is_empty(), "{skipped:?}");
}

/// A fresh interpreter has the 256 MiB runtime allocation budget the README documents: strings
/// totalling exactly 256 MiB fit, one byte more quits with the allocation-limit note and draws
/// nothing after.
#[test]
fn eps_interpreters_start_with_a_256_mib_allocation_budget() {
    // 26 strings of 10 000 000 bytes, then the rest of 268 435 456 (or one byte past it).
    let rest = (256 << 20) - 26 * 10_000_000;
    for (last, fits) in [(rest, true), (rest + 1, false)] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(format!("26 {{ 10000000 string pop }} repeat {last} string pop 0 0 10 10 rectfill").as_bytes());
        let (nodes, _, skipped) = it.b.finish();
        if fits {
            assert!(skipped.is_empty() && nodes.len() == 1, "256 MiB fits: {skipped:?}");
        } else {
            assert!(nodes.is_empty() && skipped == ["PostScript (allocation limit)"], "one byte more doesn't: {skipped:?}");
        }
    }
}

/// Text bytes a value holds: its string or name, and those of the arrays and dictionaries
/// (keys included) inside it.
fn held_text(v: &V) -> usize {
    match v {
        V::Str(s) => s.borrow().len(),
        V::Name(n) | V::Lit(n) | V::Op(n) => n.len(),
        V::Array(a) => a.borrow().iter().map(held_text).sum(),
        V::Proc(p) => p.iter().map(held_text).sum(),
        V::Dict(d) => d.borrow().iter().map(|(k, v)| k.len() + held_text(v)).sum(),
        _ => 0,
    }
}

/// `load` of an undefined name keeps the name in the operator it makes, `cvs` and `cvn` keep the
/// text they convert to, `forall` over a dictionary hands out a fresh copy of each key, and a
/// dictionary keeps its own copy of every key added to it (by `def`, `store`, `put`, `>>`, a
/// font's `scalefont` copy, or `findfont` and `definefont` naming a new font), so each charges
/// that text's full length first: made, converted, handed out or added over and over and kept in
/// an array, it never holds more than the budget.
#[test]
fn eps_conversions_charge_the_text_they_keep() {
    // The longest name there can be, kept 50 times against room for ten.
    let long = "a".repeat(MAX_NAME);
    let budget = 10 * MAX_NAME;
    let mut over = vec![];
    let dict = format!("/d 1 dict def d /{long} 0 put");
    let string = format!("/s {MAX_NAME} string def /i 0 def");
    // A new font name each time, from one string: its first byte counts up.
    let next = "s 0 i put /i i 1 add def";
    let font = format!("/s {MAX_NAME} string def /f 1 dict def f s 0 put");
    for (what, setup, body) in [
        ("load of an undefined name", "", format!("/{long} load")),
        ("load of an undefined string", "", format!("({long}) load")),
        ("cvs of a loaded operator", "", format!("/{long} load 0 string cvs")),
        ("cvs of a name", "", format!("/{long} 0 string cvs")),
        ("cvs of a string", "", format!("({long}) 0 string cvs")),
        ("cvn of a loaded operator", "", format!("/{long} load cvn")),
        ("cvn of a string", "", format!("({long}) cvn")),
        ("forall over a dictionary's key", dict.as_str(), "d { pop } forall".to_string()),
        ("put of a string key into new dictionaries", string.as_str(), "1 dict dup s 0 put".to_string()),
        ("def of a string key into new dictionaries", string.as_str(), "1 dict begin s 0 def currentdict end".to_string()),
        ("def of a name key into new dictionaries", "", format!("1 dict begin /{long} 0 def currentdict end")),
        ("store of a string key into new dictionaries", string.as_str(), "1 dict begin s 0 store currentdict end".to_string()),
        ("<< >> with a string key", string.as_str(), "<< s 0 >>".to_string()),
        ("scalefont copies of a font with a long key", font.as_str(), "f 10 scalefont".to_string()),
        ("findfont of new font names", string.as_str(), format!("{next} s findfont")),
        ("definefont of new font names", string.as_str(), format!("{next} s 1 dict definefont")),
    ] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(setup.as_bytes());
        it.allocation_budget = budget;
        it.run(format!("[ 50 {{ {body} }} repeat ] 0 0 10 10 rectfill").as_bytes());
        let held: usize = it.st.iter().map(held_text).sum();
        let (nodes, _, skipped) = it.b.finish();
        if held > budget || !nodes.is_empty() || skipped != ["PostScript (allocation limit)"] {
            over.push(format!("{what}: {held} bytes kept, {skipped:?}"));
        }
    }
    assert!(over.is_empty(), "conversions past a {budget}-byte budget: {over:#?}");
}

/// A dictionary charges an entry once, when it is added: defining, storing or putting a new value
/// under the same key again keeps nothing new and charges only the scan that finds the key, once
/// for `def` and `put` and twice for `store`, which searches the dictionary stack first.
#[test]
fn eps_dictionary_keys_are_charged_once() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.allocation_budget = "key".len() + std::mem::size_of::<(String, V)>() + 100 * 4 * "key".len();
    it.run(b"/key 0 def 100 { /key 1 def /key 2 store currentdict /key 3 put } repeat 0 0 10 10 rectfill");
    assert_eq!(it.allocation_budget, 0, "the entry, charged once, and each overwrite's scan");
    let (nodes, _, skipped) = it.b.finish();
    assert!(skipped.is_empty() && nodes.len() == 1, "overwrites keep nothing new: {skipped:?}");
}

/// `forall` over a dictionary snapshots its entries before running the procedure, each as a key-
/// value pair whose key is a literal name of its own: the whole snapshot is charged before it is
/// taken, however early the procedure exits. Five keys of the longest length a name can have need
/// their bytes and the snapshot's five pairs of slots.
#[test]
fn eps_forall_charges_the_dictionary_snapshot_first() {
    let a = "a".repeat(MAX_NAME - 1);
    let setup = format!("/d 5 dict def d /{a}0 0 put d /{a}1 0 put d /{a}2 0 put d /{a}3 0 put d /{a}4 0 put");
    let need = 5 * MAX_NAME + 5 * 2 * std::mem::size_of::<V>();
    for (budget, fits) in [(need, true), (need - 1, false)] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(setup.as_bytes());
        it.allocation_budget = budget;
        it.run(b"d { pop pop exit } forall 0 0 10 10 rectfill");
        let left = it.allocation_budget;
        let (nodes, _, skipped) = it.b.finish();
        if fits {
            assert!(left == 0 && nodes.len() == 1 && skipped.is_empty(), "{budget}: {left} left, {skipped:?}");
        } else {
            assert!(nodes.is_empty() && skipped == ["PostScript (allocation limit)"], "{budget}: {skipped:?}");
        }
    }
}

/// A new font keeps its name, six entries and two six-slot matrices (FontMatrix and its own), plus
/// a Font Directory entry; a `scalefont` copy keeps an entry for every key and a matrix of its own;
/// a font `definefont` completes keeps the program's name, two entries, a matrix and a Font
/// Directory entry: all charged, entries by their key text and a fixed slot, matrices by their
/// slots.
#[test]
fn eps_fonts_charge_their_keys_and_matrices() {
    let slot = std::mem::size_of::<V>();
    let entry = std::mem::size_of::<(String, V)>();
    let keys: usize = ["FontName", "FontType", "FontMatrix", "FID", "__base", "__m"].iter().map(|k| k.len()).sum();
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    let spent = |it: &mut Interp, ps: &[u8]| {
        let before = it.allocation_budget;
        it.run(ps);
        before - it.allocation_budget
    };
    // Every `findfont` scans the name to look it up. A new font keeps the name, once for the font
    // and once as its Font Directory key, its six dictionary entries and the Font Directory entry.
    let scan = "F".len();
    assert_eq!(spent(&mut it, b"/F findfont pop"), scan + 2 + keys + 12 * slot + 7 * entry, "findfont of a new font");
    assert_eq!(spent(&mut it, b"/F findfont pop"), scan, "findfont of a known font");
    // The copy replaces its own `__m`, scanning that key.
    assert_eq!(spent(&mut it, b"/F findfont 10 scalefont pop"), scan + keys + 6 * entry + 6 * slot + "__m".len(), "scalefont");
    assert_eq!(spent(&mut it, b"/G 1 dict definefont pop"), 2 + "__base".len() + "__m".len() + 6 * slot + 3 * entry, "definefont");
}

/// `glyphshow` draws through a cached font that names just that glyph: the first use of a glyph
/// charges what that font keeps, and drawing it again charges nothing more to the allocation
/// budget, however many times (its outline's elements are spent as operations).
#[test]
fn eps_glyphshow_charges_a_new_glyph_once() {
    use std::mem::size_of;
    let entry = |glyph: &str| size_of::<Font>() + 256 * size_of::<Option<String>>() + "F".len() + glyph.len() + 255 * ".notdef".len();
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"/F findfont 10 scalefont setfont 0 0 moveto");
    let mut spent = |ps: &[u8]| {
        let before = it.allocation_budget;
        it.run(ps);
        before - it.allocation_budget
    };
    assert_eq!(spent(b"/A glyphshow"), entry("A"), "a new glyph");
    assert_eq!(spent(b"1000 { /A glyphshow } repeat"), 0, "the same glyph again");
    assert_eq!(spent(b"/Bee glyphshow"), entry("Bee"), "another glyph");
}

/// `gsave` and `save` keep a copy of the graphics state, dash array included: each copy's dash
/// is charged, and at most `MAX_SAVE_DEPTH` copies are kept at once.
#[test]
fn eps_gsave_charges_the_dash_it_copies() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"/a 1000 array def 0 1 999 { a exch 1 put } for a 0 setdash");
    let dash = 1000 * std::mem::size_of::<f64>();
    for op in ["gsave", "save pop"] {
        let before = it.allocation_budget;
        it.run(op.as_bytes());
        assert_eq!(before - it.allocation_budget, dash, "{op}");
    }
    it.allocation_budget = 3 * dash - 1;
    it.run(b"3 { gsave } repeat 0 0 10 10 rectfill");
    let (nodes, _, skipped) = it.b.finish();
    assert!(nodes.is_empty() && skipped == ["PostScript (allocation limit)"], "three copies past the budget: {skipped:?}");
}

/// Graphics-state copies stop at `MAX_SAVE_DEPTH`: a runaway `gsave` loop ends there, with a note,
/// instead of growing the state stack until the operation limit.
#[test]
fn eps_gsave_stops_at_the_depth_limit() {
    use super::exec::MAX_SAVE_DEPTH;
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"0 0 10 10 rectfill { { gsave } loop } loop");
    assert_eq!(it.saved.len(), MAX_SAVE_DEPTH);
    let (nodes, _, skipped) = it.b.finish();
    assert!(nodes.len() == 1 && skipped == ["PostScript (graphics state depth limit)"], "{skipped:?}");
}

/// A font's encoding names are matched by identity (or, when new, by their text) to find its
/// cached font, so showing text with a font already built charges only the string shown; building
/// it charges what it keeps first: the font, its 256-name glyph table, its base name and the first
/// 256 encoding entries it is cached by.
#[test]
fn eps_text_charges_a_new_fonts_encoding_names_once() {
    use std::mem::size_of;
    let entry = |name: &str, slots: usize| {
        size_of::<Font>() + 256 * size_of::<Option<String>>() + "Helvetica".len() + slots * size_of::<Option<Rc<str>>>() + 256 * name.len()
    };
    let font = |name: &str, size: usize| {
        format!(
            "/n /{name} def /e {size} array def 0 1 {} {{ e exch n put }} for /f 3 dict def f /FontName /Helvetica put f /Encoding e put f setfont 0 0 moveto",
            size - 1
        )
    };
    let long = "n".repeat(MAX_NAME);
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(font(&long, 256).as_bytes());
    it.allocation_budget = 100 * MAX_NAME;
    it.run(b"(A) show 0 0 10 10 rectfill");
    let (nodes, _, skipped) = it.b.finish();
    assert!(nodes.is_empty() && skipped == ["PostScript (allocation limit)"], "256 copies of the longest name: {skipped:?}");

    // An encoding longer than 256 entries is cached by its first 256: what a font reads.
    for size in [256, 1_000_000] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(font("A", size).as_bytes());
        let before = it.allocation_budget;
        it.run(b"(A) show");
        assert_eq!(before - it.allocation_budget, 1 + entry("A", 256), "{size} entries: the shown string and the new font");
        let before = it.allocation_budget;
        it.run(b"(A) show");
        assert_eq!(before - it.allocation_budget, 1, "{size} entries: the second show charges only its string");
    }
}

/// Hashing or comparing a key reads every byte, so each operation that looks one up, overwrites
/// it or compares it charges its scan: a string at its worst-case lossy UTF-8 length, a name at
/// its own length. A loop over one long key does bounded work.
#[test]
fn eps_key_scans_are_charged() {
    let name = "k".repeat(MAX_NAME);
    let string = format!("/s {MAX_NAME} string def");
    for (key, setup, scan) in [("s", string.as_str(), 3 * MAX_NAME), (&*format!("/{name}"), "", MAX_NAME)] {
        for (what, define, op, scans) in [
            ("known", format!("/d 1 dict def d {key} 0 put"), format!("d {key} known pop"), 1),
            ("get", format!("/d 1 dict def d {key} 0 put"), format!("d {key} get pop"), 1),
            ("put overwrite", format!("/d 1 dict def d {key} 0 put"), format!("d {key} 1 put"), 1),
            ("where", format!("{key} 0 def"), format!("{key} where pop"), 1),
            ("load", format!("{key} 0 def"), format!("{key} load pop"), 1),
            ("findfont hit", format!("{key} findfont pop"), format!("{key} findfont pop"), 1),
            ("eq", String::new(), format!("{key} {key} eq pop"), 2),
        ] {
            let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
            it.run(format!("{setup} {define}").as_bytes());
            let before = it.allocation_budget;
            it.run(op.as_bytes());
            assert_eq!(before - it.allocation_budget, scans * scan, "{what} of {key:.8} charges its scan");
        }
    }
}

/// `glyphshow` with no current font drops its operand and copies nothing. With a font set, a
/// string glyph name, and a string font name on every `show`, is copied each time it is used: up
/// to the longest name, each copy is charged, so a loop runs out of budget; one byte longer, the
/// first use stops at the name length limit.
#[test]
fn eps_glyphshow_and_string_font_names_are_bounded() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"/s 1000 string def");
    let before = it.allocation_budget;
    it.run(b"100000 { s glyphshow } repeat");
    assert_eq!(before - it.allocation_budget, 0, "glyphshow with no font copies nothing");

    for (what, setup, body) in [
        ("a string glyph name", "/F findfont 10 scalefont setfont 0 0 moveto", "s glyphshow"),
        ("a string font name", "<< /FontName s /FontType 1 >> setfont 0 0 moveto", "(A) show"),
    ] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        // The first use builds the font; the loop's copies then have room for ten.
        it.run(format!("/s {MAX_NAME} string def {setup} {body}").as_bytes());
        it.allocation_budget = 10 * MAX_NAME;
        it.run(format!("{{ {{ {body} }} loop }} loop").as_bytes());
        let (_, _, skipped) = it.b.finish();
        assert_eq!(skipped, ["PostScript (allocation limit)"], "{what}: each copy is charged");

        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(format!("/s {} string def {setup}", MAX_NAME + 1).as_bytes());
        it.allocation_budget = 10 * MAX_NAME;
        it.run(format!("{body} {{ {{ {body} }} loop }} loop").as_bytes());
        let (_, _, skipped) = it.b.finish();
        assert_eq!(skipped, ["PostScript (name length limit)"], "{what}: one byte too long, at its first use");
    }
}

/// Names and dictionary keys are at most `MAX_NAME` bytes however they are made: read from the
/// file (a literal, an executable name, `//name`, inside a procedure or an executable string),
/// converted by `cvn`, made by `load`, `findfont` or `definefont` from a string, or a string
/// dictionary key. At the limit the program goes on; one byte longer stops it with a note, keeping
/// what it drew before.
#[test]
fn eps_names_stop_at_the_name_length_limit() {
    // The documented limit, written out rather than read from the code, so changing it fails here.
    let limit = 255;
    // A name made or kept at runtime over the limit stops the program, keeping what it drew.
    for len in [limit, limit + 1] {
        let n = "x".repeat(len);
        for (what, make) in [
            ("cvn", format!("({n}) cvn pop")),
            ("load of an undefined string", format!("({n}) load pop")),
            ("findfont of a string", format!("({n}) findfont pop")),
            ("definefont of a string", format!("({n}) 1 dict definefont pop")),
            ("a string dictionary key", format!("1 dict ({n}) 0 put")),
            ("a literal name used as a key", format!("1 dict /{n} 0 put")),
        ] {
            let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
            it.run(format!("0 0 10 10 rectfill {make} 20 20 10 10 rectfill").as_bytes());
            let (nodes, _, skipped) = it.b.finish();
            if len == limit {
                assert!(nodes.len() == 2 && skipped.is_empty(), "{what} at the limit: {skipped:?}");
            } else {
                assert!(nodes.len() == 1 && skipped == ["PostScript (name length limit)"], "{what} past the limit: {skipped:?}");
            }
        }
    }
    // A name read from the file, however long, never stops the program: a literal is just pushed,
    // and an executable name too long to be any defined name or operator is noted as unknown (it is
    // not looked up, so it does no work that scales with its length). Image data that reaches the
    // scanner (an indirect data source, an unknown image operator) depends on this.
    let n = "x".repeat(limit + 1);
    for (what, make) in [
        ("a literal name", format!("/{n} pop")),
        ("an executable name", n.clone()),
        ("an immediate name", format!("//{n} pop")),
        ("a name in a procedure", format!("{{ {n} }} exec")),
        ("a name in an executable string", format!("({n}) cvx exec")),
    ] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(format!("0 0 10 10 rectfill {make} 20 20 10 10 rectfill").as_bytes());
        let (nodes, _, skipped) = it.b.finish();
        assert!(nodes.len() == 2 && !skipped.iter().any(|s| s.contains("name length")), "{what} must not stop: {nodes:?} {skipped:?}");
    }
}

/// An over-long executable name read from the file is not looked up (that would hash it against
/// every dictionary on the stack), so running one in a loop is bounded, not unbounded work.
#[test]
fn eps_a_long_executable_name_runs_bounded() {
    let n = "x".repeat(100_000);
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    // Run the long name in a nested loop: each pass must spend only a constant, so the operation
    // budget — not the name's length — ends it, and the square drawn first is kept.
    it.run(format!("0 0 10 10 rectfill /p {{ {n} }} def {{ {{ p }} loop }} loop 20 20 10 10 rectfill").as_bytes());
    let (nodes, _, skipped) = it.b.finish();
    assert_eq!(skipped, ["PostScript (operation limit)"], "{skipped:?}");
    assert_eq!(nodes.len(), 1, "the square before the loop is kept");
}

/// A matrix operand is exactly six numbers: a longer array is not read as a matrix, so a huge one
/// (`concat`, `setmatrix`, `makefont`, a font's stored matrix) does constant work, not work that
/// scales with its length.
#[test]
fn eps_matrix_requires_exactly_six_numbers() {
    // `transform` through a six-number matrix moves the point; a seven-number array is not a
    // matrix, so `transform` uses the identity CTM and leaves the point unchanged.
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"[ 1 0 0 1 10 20 ] 3 4 3 -1 roll transform");
    assert_eq!((it.popn(), it.popn()), (24.0, 13.0), "six numbers translate the point");
    // Six numbers and a seventh non-numeric entry: a matrix is exactly six numbers, so this is not
    // one, and `transform` uses the identity CTM — reading the six and ignoring the rest would scan
    // the whole array for an operand a program controls the length of.
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"[ 1 0 0 1 10 20 /x ] 3 4 3 -1 roll transform");
    assert_eq!((it.popn(), it.popn()), (4.0, 3.0), "six numbers plus an extra entry is not a matrix");
}

/// Each stroke keeps its own copy of the dash array in the output, so it is charged: a normal dash
/// costs its few numbers, and a large one painted in a loop stops at the allocation limit.
#[test]
fn eps_stroke_charges_the_dash_it_keeps() {
    let f8 = std::mem::size_of::<f64>();
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"/a 1000 array def 0 1 999 { a exch 1 put } for a 0 setdash 0 0 moveto 10 10 lineto");
    let before = it.allocation_budget;
    it.run(b"stroke");
    assert_eq!(before - it.allocation_budget, 1000 * f8, "the stroke's dash copy");
    it.allocation_budget = 3 * 1000 * f8 - 1;
    it.run(b"{ 0 0 moveto 10 10 lineto stroke } loop 50 50 10 10 rectfill");
    let (nodes, _, skipped) = it.b.finish();
    assert!(skipped == ["PostScript (allocation limit)"] && nodes.len() == 3, "a big dash in a loop is bounded: {nodes:?} {skipped:?}");
}

/// A `SubFileDecode` end-of-data marker longer than a name can be is not searched for (the search
/// costs its length at each file position); the data after the filter is then read normally, so a
/// trailing square draws. A short marker is still found and its data skipped.
#[test]
fn eps_subfiledecode_ignores_a_long_end_of_data_marker() {
    // A short marker that never appears skips to the end of file, so the square after is lost.
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"0 0 10 10 rectfill currentfile 0 (%%ZZ) /SubFileDecode filter pop 20 20 10 10 rectfill");
    assert_eq!(it.b.shapes, 1, "a short marker skips the rest of the file");
    // A marker longer than the limit is not searched for, so the square after the filter draws.
    let long = "Z".repeat(256);
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(format!("0 0 10 10 rectfill currentfile 0 ({long}) /SubFileDecode filter pop 20 20 10 10 rectfill").as_bytes());
    assert_eq!(it.b.shapes, 2, "an over-long marker is ignored and the program goes on");
}

/// An unknown operator is noted for the report by the start of its name only, however long the
/// name the file made.
#[test]
fn eps_unknown_operators_keep_a_short_name() {
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(format!("({}) cvn cvx exec", "x".repeat(MAX_NAME)).as_bytes());
    assert!(!it.unknown.is_empty() && it.unknown.iter().all(|u| u.chars().count() <= 64), "{:?}", it.unknown.iter().map(String::len).collect::<Vec<_>>());
}

/// A conversion charges the exact length of the key text it copies, and a new dictionary entry
/// that length and a fixed entry slot: a number's formatted length, a UTF-8 string's own length, a
/// string that isn't UTF-8 by its replacement. Overwriting an existing key keeps nothing new.
#[test]
fn eps_keys_and_conversions_charge_exact_lengths() {
    let entry = std::mem::size_of::<(String, V)>();
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(b"/d 3 dict def");
    for (ps, bytes) in [
        ("d 12345 0 put", 5 + entry),
        ("d (abc) 0 put", 3 + entry),
        ("d <ff> 0 put", "\u{fffd}".len() + entry),
        ("d 12345 1 put", 0),
        ("(abc) cvn pop", 3),
        ("12345 0 string cvs pop", 5),
        ("true 0 string cvs pop", 4),
    ] {
        let before = it.allocation_budget;
        it.run(ps.as_bytes());
        assert_eq!(before - it.allocation_budget, bytes, "{ps}");
    }
}

/// Building a font parses and keeps a result sized by its embedded program, so the program's
/// length is charged for the new font: a big embedded program cannot be parsed into a kept result
/// for a few operations without paying for it.
#[test]
fn eps_a_big_embedded_font_is_charged_per_new_font() {
    // A minimal embedded Type 1 program padded to about 4 MiB, then drawn once.
    let mut blob = b"%!FontType1-1.0: Big 001\n/FontName /Big def\ncurrentfile eexec\n".to_vec();
    let pad = 4usize << 20;
    blob.extend(std::iter::repeat_n(b'a', pad));
    blob.extend_from_slice(b"\ncleartomark\n");
    blob.extend_from_slice(b"/Big findfont 12 scalefont setfont 0 0 moveto ( ) stringwidth pop pop");
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    let before = it.allocation_budget;
    it.run(&blob);
    let spent = before - it.allocation_budget;
    // The whole run — building the font dictionary and the drawable font — is charged at least the
    // embedded program's size; a bundled stand-in's entry alone is a few kilobytes.
    assert!(spent >= pad, "a new font is charged its embedded program: {spent} < {pad}");
}

/// A string's glyph outlines count against the operation budget and the path limit as they are
/// built, element by element, so one `show` of a long string in a dense font cannot build an
/// unbounded path for a single operation: it stops at the path limit.
#[test]
fn eps_text_spends_and_limits_each_glyph_outline() {
    let dense = "/Helvetica findfont 12 scalefont dup /Encoding [ 256 { /H } repeat ] put setfont";
    for op in ["show", "false charpath"] {
        let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
        it.run(format!("{dense} 0 0 10 10 rectfill /s 100000 string def 0 0 moveto s {op} 20 20 10 10 rectfill").as_bytes());
        let (nodes, _, skipped) = it.b.finish();
        assert_eq!(skipped, ["PostScript (path limit)"], "{op}: one show of 100000 dense glyphs: {skipped:?}");
        assert_eq!(nodes.len(), 1, "{op}: the square before it is kept");
    }
    // A short show in the same font draws and spends per glyph without hitting the limit.
    let mut it = Interp::new([0.0, 0.0, 100.0, 100.0]);
    it.run(format!("{dense} 0 0 moveto (HHHH) show").as_bytes());
    let before = it.budget;
    it.run(b"0 0 moveto (HHHH) show");
    assert!(it.budget < before.saturating_sub(4), "four glyphs spend more than four operations");
    let (nodes, _, skipped) = it.b.finish();
    assert!(skipped.is_empty() && nodes.len() == 2, "short shows draw: {skipped:?}");
}
