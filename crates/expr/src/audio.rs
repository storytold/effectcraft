//! Exact scalar arithmetic for audio-rate expressions. Everything else uses the JS runtime.
//! No constant folding or reassociation: each sample retains the original f64 operation order.
use effectcraft_render::AudioExpression;

enum Node {
    Number(f64),
    Time,
    Value,
    Neg(Box<Node>),
    Binary(u8, Box<Node>, Box<Node>),
}

impl Node {
    /// Execute the bounded tree using the same number operations as scalar JavaScript.
    fn sample(&self, time: f64, value: f64) -> f64 {
        match self {
            Self::Number(n) => *n,
            Self::Time => time,
            Self::Value => value,
            Self::Neg(n) => -n.sample(time, value),
            Self::Binary(op, a, b) => {
                let (a, b) = (a.sample(time, value), b.sample(time, value));
                match op {
                    b'+' => a + b,
                    b'-' => a - b,
                    b'*' => a * b,
                    _ => a / b,
                }
            }
        }
    }
}
impl AudioExpression for Node {
    /// Evaluate the prepared scalar, treating non-finite numbers as expression errors.
    fn evaluate(&self, time: f64, value: f64) -> Option<f64> {
        let n = self.sample(time, value);
        n.is_finite().then_some(n)
    }
}

/// Decline unsupported syntax, legacy numeric literals, or excessive nesting before evaluation.
pub fn prepare(text: &str) -> Option<Box<dyn AudioExpression>> {
    if text.len() > 1024 || !text.is_ascii() || text.contains("++") || text.contains("--") {
        return None;
    }
    let mut p = Parser { text: text.as_bytes(), at: 0, nodes: 0 };
    let n = p.expression(0, 0)?;
    p.space();
    if p.peek() == Some(b';') {
        p.at += 1;
        p.space();
    }
    (p.at == p.text.len()).then(|| Box::new(n) as Box<dyn AudioExpression>)
}

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
    nodes: usize,
}
impl Parser<'_> {
    /// Read the next byte without advancing or indexing beyond the expression.
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }
    /// Only ASCII JavaScript whitespace is admitted by this fast path.
    fn space(&mut self) {
        while self.peek().is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }
    /// Precedence climbing preserves left associativity and bounds both tree size and depth.
    fn expression(&mut self, min: u8, depth: usize) -> Option<Node> {
        if depth > 32 || self.nodes >= 128 {
            return None;
        }
        self.nodes += 1;
        self.space();
        let mut left = match self.peek()? {
            b'+' | b'-' => {
                let op = self.peek()?;
                self.at += 1;
                let n = self.expression(3, depth + 1)?;
                if op == b'-' { Node::Neg(Box::new(n)) } else { n }
            }
            b'(' => {
                self.at += 1;
                let n = self.expression(0, depth + 1)?;
                self.space();
                if self.peek()? != b')' {
                    return None;
                }
                self.at += 1;
                n
            }
            b'0'..=b'9' | b'.' => {
                let start = self.at;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.at += 1;
                }
                if self.at > start + 1 && self.text.get(start) == Some(&b'0') {
                    return None;
                }
                if self.peek() == Some(b'.') {
                    self.at += 1;
                    while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        self.at += 1;
                    }
                }
                if matches!(self.peek(), Some(b'e' | b'E')) {
                    self.at += 1;
                    if matches!(self.peek(), Some(b'+' | b'-')) {
                        self.at += 1;
                    }
                    while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        self.at += 1;
                    }
                }
                Node::Number(std::str::from_utf8(self.text.get(start..self.at)?).ok()?.parse().ok()?)
            }
            _ => {
                let start = self.at;
                while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
                    self.at += 1;
                }
                match self.text.get(start..self.at)? {
                    b"time" => Node::Time,
                    b"value" => Node::Value,
                    _ => return None,
                }
            }
        };
        loop {
            self.space();
            let Some(op) = self.peek() else { break };
            let precedence = match op {
                b'+' | b'-' => 1,
                b'*' | b'/' => 2,
                _ => break,
            };
            if precedence < min {
                break;
            }
            self.at += 1;
            let right = self.expression(precedence + 1, depth + 1)?;
            left = Node::Binary(op, Box::new(left), Box::new(right));
        }
        Some(left)
    }
}
