//! GPU Numbers and Timecode vs the CPU effects (the oracle): direct on a buffer at full and half
//! resolution, as adjustment, and composited at 8 and 32 bpc.

use effectcraft_keyframe::Value;

use crate::tests::{c, effect_case, n};

fn e(v: u32) -> Value {
    Value::Enum(v)
}

fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

#[test]
fn numbers() {
    effect_case("ec.text.numbers", &[("format/value", n(42.5)), ("size", n(14.0))]);
    effect_case(
        "ec.text.numbers",
        &[("format/value", n(-7.0)), ("size", n(12.0)), ("fillAndStroke/displayOptions", e(2)), ("fillAndStroke/strokeWidth", n(1.5)), ("tracking", n(2.0))],
    );
    effect_case(
        "ec.text.numbers",
        &[
            ("format/value", n(3.25)),
            ("size", n(16.0)),
            ("fillAndStroke/displayOptions", e(3)),
            ("fillAndStroke/fillColor", c(0.2, 0.6, 1.0)),
            ("compositeOnOriginal", Value::Bool(false)),
        ],
    );
    effect_case("ec.text.numbers", &[("size", n(10.0)), ("fillAndStroke/displayOptions", e(1)), ("fillAndStroke/position", pt(20.0, 15.0))]);
}

#[test]
fn timecode() {
    effect_case("ec.text.timecode", &[("textSize", n(10.0)), ("textPosition", pt(35.0, 22.0))]);
    effect_case(
        "ec.text.timecode",
        &[("textSize", n(9.0)), ("displayFormat", e(1)), ("renderOnOriginal", Value::Bool(false)), ("textColor", c(1.0, 0.8, 0.1))],
    );
    effect_case("ec.text.timecode", &[("textSize", n(8.0)), ("showBox", Value::Bool(true)), ("boxColor", c(0.1, 0.1, 0.4)), ("opacity", n(70.0))]);
}
