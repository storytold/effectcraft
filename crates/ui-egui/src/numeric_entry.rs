//! Bounded arithmetic for typed numeric fields, independent of the expression runtime.

#[derive(Debug, PartialEq)]
pub(crate) enum NumericEntryError {
    Syntax,
    Limit,
    NonFinite,
}

pub(crate) fn parse(text: &str) -> Result<f64, NumericEntryError> {
    if text.len() > 512 || !text.is_ascii() {
        return Err(NumericEntryError::Limit);
    }
    let mut parser = Parser { text, pos: 0, work: 0 };
    let value = parser.sum(0)?;
    parser.space();
    if parser.pos != text.len() {
        return Err(NumericEntryError::Syntax);
    }
    finite(value)
}

fn finite(value: f64) -> Result<f64, NumericEntryError> {
    if value.is_finite() { Ok(value) } else { Err(NumericEntryError::NonFinite) }
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
    work: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }
    fn space(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }
    fn budget(&mut self, depth: usize) -> Result<(), NumericEntryError> {
        self.work += 1;
        if depth > 32 || self.work > 2048 { Err(NumericEntryError::Limit) } else { Ok(()) }
    }
    fn sum(&mut self, depth: usize) -> Result<f64, NumericEntryError> {
        self.budget(depth)?;
        let mut value = self.product(depth)?;
        loop {
            self.space();
            let operator = self.peek();
            if !matches!(operator, Some(b'+' | b'-')) {
                break;
            }
            self.pos += 1;
            let rhs = self.product(depth)?;
            value = finite(if operator == Some(b'+') { value + rhs } else { value - rhs })?;
        }
        Ok(value)
    }
    fn product(&mut self, depth: usize) -> Result<f64, NumericEntryError> {
        self.budget(depth)?;
        let mut value = self.atom(depth)?;
        loop {
            self.space();
            let operator = self.peek();
            if !matches!(operator, Some(b'*' | b'/')) {
                break;
            }
            self.pos += 1;
            let rhs = self.atom(depth)?;
            if operator == Some(b'/') && rhs == 0.0 {
                return Err(NumericEntryError::NonFinite);
            }
            value = finite(if operator == Some(b'*') { value * rhs } else { value / rhs })?;
        }
        Ok(value)
    }
    fn atom(&mut self, depth: usize) -> Result<f64, NumericEntryError> {
        self.budget(depth)?;
        self.space();
        match self.peek() {
            Some(b'+' | b'-') => {
                let negative = self.peek() == Some(b'-');
                self.pos += 1;
                let value = self.atom(depth + 1)?;
                Ok(if negative { -value } else { value })
            }
            Some(b'(') => {
                self.pos += 1;
                let value = self.sum(depth + 1)?;
                self.space();
                if self.peek() != Some(b')') {
                    return Err(NumericEntryError::Syntax);
                }
                self.pos += 1;
                Ok(value)
            }
            _ => self.number(),
        }
    }
    fn digits(&mut self) -> usize {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        self.pos - start
    }
    fn number(&mut self) -> Result<f64, NumericEntryError> {
        let start = self.pos;
        let mut digits = self.digits();
        if self.peek() == Some(b'.') {
            self.pos += 1;
            digits += self.digits();
        }
        if digits == 0 {
            return Err(NumericEntryError::Syntax);
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return Err(NumericEntryError::Syntax);
            }
        }
        let literal = self.text.get(start..self.pos).ok_or(NumericEntryError::Syntax)?;
        finite(literal.parse().map_err(|_| NumericEntryError::Syntax)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_examples_and_precedence() {
        for (text, value) in [
            ("2*3", 6.0),
            ("4/2", 2.0),
            ("2e2", 200.0),
            (" 2 + 3*4 ", 14.0),
            ("(2+3)*4", 20.0),
            ("-(-.5 + +2E-1)", 0.3),
            ("8/2/2", 2.0),
            ("8-3-2", 3.0),
            ("2/-2", -1.0),
            ("2*-3", -6.0),
        ] {
            assert!((parse(text).unwrap() - value).abs() < 1e-12, "{text}");
        }
    }

    #[test]
    fn invalid_nonfinite_and_hostile_inputs_are_rejected() {
        for text in [
            "",
            "2*",
            "(2+3",
            "2 3",
            "2e",
            "2e+",
            "NaN",
            "inf",
            "1/0",
            "1/-0",
            "1e309",
            "1e308*2",
            "sqrt(4)",
            "2^3",
            "2**3",
            "2;3",
            "１",
            "()",
            "2+",
            "2e-",
            "1.2.3",
            "2+3)",
            "1e308*2/2",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        assert_eq!(parse(&"1".repeat(513)), Err(NumericEntryError::Limit));
        assert_eq!(parse(&format!("{}2*3", " ".repeat(509))), Ok(6.0));
        assert_eq!(parse(&format!("{}1{}", "(".repeat(32), ")".repeat(32))), Ok(1.0));
        assert_eq!(parse(&format!("{}1", "-".repeat(32))), Ok(1.0));
        assert_eq!(parse(&format!("{}{}1{}", "(".repeat(16), "-".repeat(16), ")".repeat(16))), Ok(1.0));
        assert_eq!(parse(&format!("{}{}1{}", "(".repeat(17), "-".repeat(16), ")".repeat(17))), Err(NumericEntryError::Limit));
        assert_eq!(parse(&format!("{}1{}", "(".repeat(33), ")".repeat(33))), Err(NumericEntryError::Limit));
        assert_eq!(parse(&format!("{}1", "-".repeat(33))), Err(NumericEntryError::Limit));
        assert!(parse(&format!("{}1", "1+".repeat(200))).is_ok());
    }
}
