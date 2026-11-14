//! Basic arithmetic typed into a number field (#3195): `960/2`, `(3+4)*2`, `100*2`, and, in a
//! field that knows the value it showed, a leading operator that applies to it: `*2` doubles,
//! `/2` halves, `+50` adds 50, as the issue's reporter describes for After Effects (not confirmed
//! in public docs). A leading `-` is a negative number (`-50`), not a subtraction. (Hot text used
//! to read `+50` as 50; it now adds 50.)
//!
//! This is a small parser for numbers and `+ - * / ( )` only: no names, no functions, nothing is
//! run as code (expressions, `crates/expr`, are a different thing). The text is bounded in
//! length and nesting, every step must stay finite, and a division by zero is no value. Text it
//! can't read is `None`, so the field keeps its value.

/// The longest text read as arithmetic (characters, after removing spaces).
const MAX_LEN: usize = 256;
/// How deeply brackets and signs (`-(-(…))`, `--5`) may nest.
const MAX_DEPTH: usize = 32;

/// A typed leading operator: what it does to the value the field showed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// `+n`
    Add(f64),
    /// `*n`
    Mul(f64),
    /// `/n` (never zero)
    Div(f64),
}

impl Step {
    /// `current` changed by the step; `None` when that isn't a finite number.
    pub fn apply(self, current: f64) -> Option<f64> {
        match self {
            Step::Add(n) => finite(current + n),
            Step::Mul(k) => finite(current * k),
            Step::Div(n) => divide(current, n),
        }
    }

    /// The factor a `*` or `/` step multiplies by (`None` for `+`), for other layers that scale
    /// their own value by it.
    pub fn factor(self) -> Option<f64> {
        match self {
            Step::Add(_) => None,
            Step::Mul(k) => finite(k),
            Step::Div(n) => divide(1.0, n),
        }
    }
}

/// What typed text says: a value, or (with a leading `+`, `*` or `/`) a step from the value shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Typed {
    Value(f64),
    Step(Step),
}

/// Read typed `text`. With `steps`, a leading `+`, `*` or `/` is a [`Step`]; without (a field
/// that reads its text on every keystroke, where a step would compound) a leading `+` is a plain
/// sign and a leading `*` or `/` is no value. Spaces, including thousands groups, are ignored
/// (as egui's own parser did) and `−` (U+2212) is a minus. `,` is a decimal point: hot text
/// always read it so, and the dialog fields, whose egui parser rejected it, now do too (so
/// `1,5` is 1.5 and `1,000` is 1, not a thousand). `None` for anything else that isn't a finite
/// number.
pub fn read(text: &str, steps: bool) -> Option<Typed> {
    let s: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '−' => '-',
            ',' => '.',
            c => c,
        })
        .collect();
    if s.is_empty() || s.chars().count() > MAX_LEN {
        return None;
    }
    let first = s.chars().next()?;
    if steps && matches!(first, '+' | '*' | '/') {
        let operand = parse(s.get(first.len_utf8()..)?)?;
        return Some(Typed::Step(match first {
            '+' => Step::Add(operand),
            '*' => Step::Mul(operand),
            _ => Step::Div((operand != 0.0).then_some(operand)?),
        }));
    }
    parse(&s).map(Typed::Value)
}

/// The value typed `text` stands for: [`read`], with a step applied to `current` (the value the
/// field showed; without it a leading `*` or `/` is no value and `+` only a sign).
pub fn calc(text: &str, current: Option<f64>) -> Option<f64> {
    match read(text, current.is_some())? {
        Typed::Value(v) => Some(v),
        Typed::Step(step) => step.apply(current.filter(|c| c.is_finite())?),
    }
}

/// The whole of `s` as an expression.
fn parse(s: &str) -> Option<f64> {
    let mut p = Parser { chars: s.chars().peekable(), depth: 0 };
    let v = p.expr()?;
    if p.chars.next().is_some() {
        return None;
    }
    finite(v)
}

/// `a / b`; `None` for a division by zero.
fn divide(a: f64, b: f64) -> Option<f64> {
    if b == 0.0 {
        return None;
    }
    finite(a / b)
}

/// A finite result, or `None`.
fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

/// A recursive-descent reader of `expr := term (('+'|'-') term)*`,
/// `term := factor (('*'|'/') factor)*`, `factor := ('+'|'-') factor | '(' expr ')' | number`.
struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    /// The brackets and signs open around the current point.
    depth: usize,
}

impl Parser<'_> {
    /// Reads a sum or difference of terms, left to right.
    /// Returns `None` on a syntax error or a non-finite result.
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(&op) = self.chars.peek() {
            match op {
                '+' => {
                    self.chars.next();
                    v = finite(v + self.term()?)?;
                }
                '-' => {
                    self.chars.next();
                    v = finite(v - self.term()?)?;
                }
                _ => break,
            }
        }
        Some(v)
    }

    /// Reads a product or quotient of factors, left to right.
    /// Returns `None` on a syntax error, a division by zero or a non-finite result.
    fn term(&mut self) -> Option<f64> {
        let mut v = self.factor()?;
        while let Some(&op) = self.chars.peek() {
            match op {
                '*' => {
                    self.chars.next();
                    v = finite(v * self.factor()?)?;
                }
                '/' => {
                    self.chars.next();
                    v = divide(v, self.factor()?)?;
                }
                _ => break,
            }
        }
        Some(v)
    }

    /// Reads a signed factor, a bracketed expression or a number.
    /// Nesting deeper than `MAX_DEPTH` gives `None`.
    fn factor(&mut self) -> Option<f64> {
        let c = *self.chars.peek()?;
        match c {
            '+' | '-' | '(' => {
                self.depth = self.depth.checked_add(1)?;
                if self.depth > MAX_DEPTH {
                    return None;
                }
                self.chars.next();
                let v = if c == '(' {
                    let v = self.expr()?;
                    if self.chars.next() != Some(')') {
                        return None;
                    }
                    v
                } else {
                    let v = self.factor()?;
                    if c == '-' { -v } else { v }
                };
                self.depth = self.depth.saturating_sub(1);
                Some(v)
            }
            _ => self.number(),
        }
    }

    /// Digits with at most one decimal point and an optional exponent (`12`, `1.5`, `.5`, `2.`,
    /// `2e2`, `1.5E-3`).
    fn number(&mut self) -> Option<f64> {
        let mut text = String::new();
        let mut point = false;
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_digit() {
                text.push(c);
            } else if c == '.' && !point {
                point = true;
                text.push(c);
            } else {
                break;
            }
            self.chars.next();
        }
        if !text.chars().any(|c| c.is_ascii_digit()) {
            return None;
        }
        if let Some(&e) = self.chars.peek()
            && matches!(e, 'e' | 'E')
        {
            self.chars.next();
            text.push('e');
            if let Some(&sign) = self.chars.peek()
                && matches!(sign, '+' | '-')
            {
                self.chars.next();
                text.push(sign);
            }
            let mut digits = false;
            while let Some(&c) = self.chars.peek() {
                if !c.is_ascii_digit() {
                    break;
                }
                digits = true;
                text.push(c);
                self.chars.next();
            }
            if !digits {
                return None;
            }
        }
        finite(text.parse::<f64>().ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::calc as eval;

    /// Evaluates `text` with no current value, so a leading operator has nothing to apply to.
    fn abs(text: &str) -> Option<f64> {
        eval(text, None)
    }

    #[test]
    fn plain_numbers_read_as_before() {
        assert_eq!(abs("100"), Some(100.0));
        assert_eq!(abs("-12.5"), Some(-12.5));
        assert_eq!(abs(" 12,5 "), Some(12.5), "a decimal comma");
        assert_eq!(abs("1 000"), Some(1000.0), "a thousands space");
        assert_eq!(abs("−3"), Some(-3.0), "the minus sign U+2212");
        assert_eq!(abs("1e3"), Some(1000.0));
        assert_eq!(abs(".5"), Some(0.5));
    }

    #[test]
    fn arithmetic() {
        assert_eq!(abs("960/2"), Some(480.0));
        assert_eq!(abs("100*2"), Some(200.0));
        assert_eq!(abs("(3+4)*2"), Some(14.0));
        assert_eq!(abs("3+4*2"), Some(11.0), "* before +");
        assert_eq!(abs("10-2-3"), Some(5.0), "left to right");
        assert_eq!(abs("8/4/2"), Some(1.0), "left to right");
        assert_eq!(abs("-(2+3)"), Some(-5.0));
        assert_eq!(abs("2*-3"), Some(-6.0));
        assert_eq!(abs("1.5 * 2"), Some(3.0));
        assert_eq!(abs("((((1))))"), Some(1.0));
        assert_eq!(abs("100+50"), Some(150.0));
        assert_eq!(abs("100-50"), Some(50.0));
        assert_eq!(abs("2e2*2"), Some(400.0), "an exponent inside arithmetic");
        assert_eq!(abs("1E-3*1000"), Some(1.0));
        assert_eq!(abs("1e+2/4"), Some(25.0));
    }

    #[test]
    fn steps_are_read_as_steps() {
        use super::{Step, Typed, read};
        assert_eq!(read("*2", true), Some(Typed::Step(Step::Mul(2.0))));
        assert_eq!(read("/4", true), Some(Typed::Step(Step::Div(4.0))));
        assert_eq!(read("+(1+2)", true), Some(Typed::Step(Step::Add(3.0))));
        assert_eq!(read("-5", true), Some(Typed::Value(-5.0)));
        assert_eq!(read("+5", false), Some(Typed::Value(5.0)));
        assert_eq!(read("/0", true), None);
        assert_eq!(Step::Div(4.0).factor(), Some(0.25));
        assert_eq!(Step::Mul(3.0).factor(), Some(3.0));
        assert_eq!(Step::Add(3.0).factor(), None);
        assert_eq!(Step::Div(1e-320).factor(), None, "a factor that overflows");
        assert_eq!(Step::Div(3.0).apply(3.0), Some(1.0), "a division, not a reciprocal factor");
    }

    #[test]
    fn a_leading_operator_applies_to_the_current_value() {
        assert_eq!(eval("*2", Some(100.0)), Some(200.0));
        assert_eq!(eval("/2", Some(100.0)), Some(50.0));
        assert_eq!(eval("+50", Some(100.0)), Some(150.0));
        assert_eq!(eval("*(1+1)", Some(3.0)), Some(6.0));
        assert_eq!(eval("+-50", Some(100.0)), Some(50.0));
        // A leading minus is a negative number, not a subtraction.
        assert_eq!(eval("-50", Some(100.0)), Some(-50.0));
        assert_eq!(eval("-50*2", Some(100.0)), Some(-100.0));
        // Without the current value there is nothing to apply it to; a `+` is only a sign.
        assert_eq!(eval("*2", None), None);
        assert_eq!(eval("/2", None), None);
        assert_eq!(eval("+50", None), Some(50.0));
        assert_eq!(eval("*2", Some(f64::NAN)), None);
        assert_eq!(eval("+1", Some(f64::INFINITY)), None);
        assert_eq!(eval("/0", Some(100.0)), None);
        assert_eq!(eval("/(1-1)", Some(100.0)), None);
        assert_eq!(eval("*", Some(100.0)), None);
        assert_eq!(eval("*1e400", Some(1.0)), None);
        // Overflow past the largest finite value.
        assert_eq!(eval(&format!("*{}", "9".repeat(20)), Some(1e300)), None);
        assert_eq!(eval("/0.0000000001", Some(1e300)), None);
    }

    #[test]
    fn invalid_or_hostile_text_is_no_value() {
        for text in [
            "", " ", "*", "/", "1+", "+", "-", "2**3", "2//3", "1/0", "0/0", "-1/0", "1/(2-2)", "(1", "1)", "()", "(1+2", "abc", "NaN", "nan", "inf", "-inf",
            "infinity", "1e999", "-1e999", "1..2", "1.2.3", ".", "1e", "2x", "x*2", "1+a", "5;rm", "1,2,3", "0x10", "1_000", "%", "50%",
        ] {
            assert_eq!(abs(text), None, "{text:?}");
            assert_eq!(eval(text, Some(10.0)), None, "{text:?} with a current value");
        }
        // Far too long: rejected by length before any reading.
        assert_eq!(abs(&format!("{}1{}", "(".repeat(1000), ")".repeat(1000))), None);
        assert_eq!(abs(&format!("{}5", "-".repeat(10_000))), None);
        assert_eq!(abs(&"(".repeat(10_000)), None);
        assert_eq!(abs(&"1".repeat(10_000)), None);
        assert_eq!(abs(&"1+".repeat(5_000)), None);
        // Short enough, but nested past the depth bound.
        assert_eq!(abs(&format!("{}1{}", "(".repeat(100), ")".repeat(100))), None);
        assert_eq!(abs(&format!("{}5", "-".repeat(200))), None);
        assert_eq!(abs(&format!("2*{}5", "-+".repeat(100))), None);
        // Nesting within the bound reads.
        assert_eq!(abs(&format!("{}1{}", "(".repeat(20), ")".repeat(20))), Some(1.0));
        assert_eq!(abs("--5"), Some(5.0));
        // Overflow on the way is no value.
        assert_eq!(abs("1e308*10"), None);
        assert_eq!(abs("1e308+1e308"), None);
        assert_eq!(abs("1e200*1e200/1e200"), None, "an infinite step is rejected even if the result would be finite");
        // The largest numbers the length allows still multiply to a finite value.
        let big = "9".repeat(120);
        assert!(abs(&format!("{big}*{big}")).is_some_and(f64::is_finite));
    }

    /// The result is always finite (no NaN reaches a property).
    #[test]
    fn results_are_finite() {
        for text in ["0*1e308", "1e-320/1e10", "0-0", "-0"] {
            if let Some(v) = abs(text) {
                assert!(v.is_finite(), "{text:?} → {v}");
            }
        }
    }

    /// A standard numeric editor (dialogs) reads typed arithmetic too.
    /// (egui's own parser read `960/2` as no value, so the field stayed 10.)
    #[test]
    fn a_drag_value_reads_typed_arithmetic() {
        use egui::{Context, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
        let ctx = Context::default();
        let mut value = 10.0_f64;
        let mut time = 0.0;
        let field = Rect::from_min_size(pos2(100.0, 100.0), vec2(80.0, 20.0));
        let mut frame = |events: Vec<Event>, value: &mut f64| {
            time += 0.02;
            let raw = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))),
                time: Some(time),
                events,
                focused: true,
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                ui.put(field, crate::widgets::drag_value(egui::DragValue::new(&mut *value).speed(1.0)));
            });
            out.textures_delta.clear();
        };
        let p = field.center();
        frame(vec![], &mut value);
        frame(
            vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }],
            &mut value,
        );
        frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }], &mut value);
        frame(vec![], &mut value);
        frame(vec![Event::Text("960/2".into())], &mut value);
        frame(vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], &mut value);
        frame(vec![], &mut value);
        assert_eq!(value, 480.0);
    }
}
