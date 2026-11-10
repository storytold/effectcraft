//! Deep nesting the expression text check can't see (#2124, #2125): source handed to `eval` /
//! `Function` at run time, and deeply nested values that boa's natives (`join`, `JSON.stringify`,
//! proxies…) walk recursively. Each used to overflow the stack, which aborts the whole app
//! (`catch_unwind` can't catch it); now each is an expression error.

use super::*;

/// The render threads' stack size (rayon's default): what the app gets.
const RENDER_STACK: usize = 2 << 20;

/// Evaluate `text` on the title's source text (any result converts) on a render-sized stack.
fn eval_small_stack(text: &str) -> Result<Value, String> {
    eval_on_stack(text, RENDER_STACK)
}

/// Evaluate `text` on the title's source text on a thread with `size` bytes of stack.
fn eval_on_stack(text: &str, size: usize) -> Result<Value, String> {
    let text = text.to_string();
    std::thread::Builder::new()
        .stack_size(size)
        .spawn(move || {
            let f = fx();
            f.ev(f.title, "text/sourceText", &text, 0.0)
        })
        .unwrap()
        .join()
        .unwrap()
}

fn text_of(r: Result<Value, String>) -> String {
    match r {
        Ok(Value::Text(d)) => d.text,
        other => panic!("not text: {other:?}"),
    }
}

/// A 100 000-deep array: `var a = []; … a = [a]` in a loop (under the loop limit).
const DEEP_ARRAY: &str = "var a = []; for (var i = 0; i < 100000; i++) a = [a];";

#[test]
fn eval_and_function_constructors_check_nesting() {
    let deep = r#""(".repeat(1000) + "1" + ")".repeat(1000)"#;
    for call in [
        format!("eval({deep})"),
        format!("Function({deep})()"),
        format!("new Function('a', {deep})(1)"),
        format!("Function('a = ' + {deep}, 'return a')()"),
        "Function.apply(null, '('.repeat(1000).split('').concat(['1']))".to_string(),
        format!("(function () {{}}).constructor({deep})()"),
        format!("Object.getPrototypeOf(function* () {{}}).constructor({deep})"),
        format!("Object.getPrototypeOf(async function () {{}}).constructor({deep})"),
        format!("Object.getPrototypeOf(async function* () {{}}).constructor({deep})"),
    ] {
        let e = eval_small_stack(&call).expect_err(&call);
        assert!(e.contains("nested too deeply"), "{call}: {e}");
    }
}

#[test]
fn eval_and_function_still_work() {
    let f = fx();
    assert_eq!(f.num("eval('1 + 2')", 0.0), 3.0);
    assert_eq!(f.num("eval('[1, [2, (3)]]')[1][1]", 0.0), 3.0);
    assert_eq!(f.num("new Function('a', 'b', 'return a * b')(3, 4)", 0.0), 12.0);
    assert_eq!(f.num("Function('return 5')()", 0.0), 5.0);
    assert_eq!(f.num("(function () {}).constructor('return 6')()", 0.0), 6.0);
    assert_eq!(f.num("(function () {}) instanceof Function ? 1 : 0", 0.0), 1.0);
    assert_eq!(f.num("(function () {}).constructor === Function ? 1 : 0", 0.0), 1.0);
    assert_eq!(f.num("Object.getPrototypeOf(function* () { yield 7; }).constructor('yield 7')().next().value", 0.0), 7.0);
}

#[test]
fn deep_array_result_is_an_error_or_text_not_a_crash() {
    // The result itself: detaching it used to `toString` it past depth 4.
    let _ = eval_small_stack(&format!("{DEEP_ARRAY} a"));
    // Explicit conversions inside the script.
    for tail in ["a.join()", "a.toString()", "String(a)", "a + ''", "`${a}`", "a.toLocaleString()", "JSON.stringify(a)", "JSON.stringify({ a: a })"] {
        let text = format!("{DEEP_ARRAY} {tail}");
        let e = eval_small_stack(&text).expect_err(&text);
        assert!(e.contains("nested too deeply"), "{text}: {e}");
    }
    // A deep array as a text style value goes through JSON.stringify in the prelude.
    let _ = eval_small_stack(&format!("{DEEP_ARRAY} text.sourceText.style.setFontSize(a)"));
}

#[test]
fn deep_proxy_chain_is_an_error_not_a_crash() {
    let text = "var p = {}; for (var i = 0; i < 100000; i++) p = new Proxy(p, {}); p.x";
    let e = eval_small_stack(text).expect_err(text);
    assert!(e.contains("Proxy") && e.contains("not defined"), "{text}: {e}");
    // Neither is available to expressions; the object model's own proxies still work.
    let f = fx();
    assert_eq!(f.text("typeof Proxy + ' ' + typeof Reflect", 0.0), "undefined undefined");
    assert_eq!(f.num("thisComp.layer('A').effect('Slider Control')('Slider')", 0.0), 42.0);
}

#[test]
fn nested_values_still_convert() {
    let f = fx();
    assert_eq!(f.text("[[1, 2], [3, [4, 5]]].join(';')", 0.0), "1,2;3,4,5");
    assert_eq!(f.text("String([[1, 2], 3])", 0.0), "1,2,3");
    assert_eq!(f.text("JSON.stringify({ a: [1, { b: [2] }] })", 0.0), r#"{"a":[1,{"b":[2]}]}"#);
    assert_eq!(f.text("JSON.stringify({ a: 1, b: 2 }, ['b'])", 0.0), r#"{"b":2}"#);
    assert_eq!(f.text("JSON.stringify([1, [2]], null, 1)", 0.0), "[\n 1,\n [\n  2\n ]\n]");
    assert_eq!(f.text("JSON.stringify({ a: 1 }, function (k, v) { return k === 'a' ? v + 1 : v; })", 0.0), r#"{"a":2}"#);
    // Nested deeper than the detached levels: still converts to its text.
    assert_eq!(text_of(eval_small_stack("[[[[[[1, 2]]]]]]")), "");
    assert_eq!(f.text("String([[[[[[[[1]]]]]]]])", 0.0), "1");
    assert_eq!(f.num("[1, [2, [3, [4]]]].flat(Infinity).length", 0.0), 4.0);
}

/// The main thread runs commands and syntax checks: 1 MiB on Windows. Half of it, for the
/// callers above the evaluation, must still turn the nested-value triggers into errors.
#[test]
fn nested_values_are_an_error_on_half_the_main_thread() {
    for tail in ["a.join()", "String([a])", "JSON.stringify(a)", "a.toLocaleString()"] {
        let text = format!("{DEEP_ARRAY} {tail}");
        let e = eval_on_stack(&text, 512 << 10).expect_err(&text);
        assert!(e.contains("nested too deeply"), "{tail}: {e}");
    }
}

#[test]
fn stringify_with_a_key_list_reports_cycles_and_allows_shared_objects() {
    let f = fx();
    let cyclic = f.text("var o = {a: 1}; o.self = o; try { JSON.stringify(o, ['a', 'self']) } catch (e) { e.name }", 0.0);
    assert_eq!(cyclic, "TypeError");
    let shared = f.text("var x = {a: 1, b: 2}; JSON.stringify({p: x, q: x}, ['p', 'q', 'a'])", 0.0);
    assert_eq!(shared, r#"{"p":{"a":1},"q":{"a":1}}"#);
}
