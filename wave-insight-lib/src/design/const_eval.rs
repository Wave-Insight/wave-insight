//! A deliberately small constant expression evaluator.
//!
//! Elaboration needs integer values for packed ranges and parameter overrides
//! (`[WIDTH-1:0]`, `#(.DEPTH(1 << 4))`).  The full SystemVerilog constant
//! expression language is out of scope; this evaluator understands decimal and
//! based integer literals, identifiers already bound in the parameter
//! environment, the usual arithmetic/bitwise/logical operators, `$clog2`,
//! parentheses and unary operators.  Anything else yields `None`, which turns
//! into an "unknown width" instead of an error or a panic.

use std::collections::HashMap;

pub(crate) type ParamEnv = HashMap<String, i64>;

/// Evaluate `text` as an integer constant expression using `env`.
pub(crate) fn eval_const(text: &str, env: &ParamEnv) -> Option<i64> {
    let mut parser = Parser {
        src: text.as_bytes(),
        pos: 0,
        env,
    };
    let value = parser.parse_expr(0)?;
    parser.skip_ws();
    if parser.pos == parser.src.len() {
        Some(value)
    } else {
        None
    }
}

/// Evaluate a range dimension `[msb:lsb]` to its bit width.
pub(crate) fn dim_width(msb: &str, lsb: &str, env: &ParamEnv) -> Option<u32> {
    let msb = eval_const(msb, env)?;
    let lsb = eval_const(lsb, env)?;
    let width = (msb - lsb).unsigned_abs().checked_add(1)?;
    if width > u32::MAX as u64 {
        None
    } else {
        Some(width as u32)
    }
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    env: &'a ParamEnv,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.skip_ws();
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_expr(&mut self, min_bp: u8) -> Option<i64> {
        let mut lhs = self.parse_unary()?;
        loop {
            self.skip_ws();
            let Some((op, left_bp, right_bp)) = self.peek_binary_op() else {
                break;
            };
            if left_bp < min_bp {
                break;
            }
            self.consume_binary_op(op);
            let rhs = self.parse_expr(right_bp)?;
            lhs = apply_binary(op, lhs, rhs)?;
        }
        Some(lhs)
    }

    fn peek_binary_op(&self) -> Option<(BinOp, u8, u8)> {
        let rest = &self.src[self.pos..];
        // Longest match first.
        let candidates: &[(&[u8], BinOp)] = &[
            (b"<<<", BinOp::Shl),
            (b">>>", BinOp::Shr),
            (b"**", BinOp::Pow),
            (b"<<", BinOp::Shl),
            (b">>", BinOp::Shr),
            (b"<=", BinOp::Le),
            (b">=", BinOp::Ge),
            (b"==", BinOp::Eq),
            (b"!=", BinOp::Ne),
            (b"&&", BinOp::AndAnd),
            (b"||", BinOp::OrOr),
            (b"<", BinOp::Lt),
            (b">", BinOp::Gt),
            (b"+", BinOp::Add),
            (b"-", BinOp::Sub),
            (b"*", BinOp::Mul),
            (b"/", BinOp::Div),
            (b"%", BinOp::Mod),
            (b"&", BinOp::And),
            (b"|", BinOp::Or),
            (b"^", BinOp::Xor),
        ];
        for (token, op) in candidates {
            if rest.starts_with(token) {
                let (l, r) = binding_power(*op);
                return Some((*op, l, r));
            }
        }
        None
    }

    fn consume_binary_op(&mut self, op: BinOp) {
        let len = match op {
            BinOp::Shl | BinOp::Shr => {
                // `<<<` / `>>>` are consumed as the 3 byte form; the token
                // matcher above only reports the enum, so decide by source.
                let rest = &self.src[self.pos..];
                if rest.starts_with(b"<<<") || rest.starts_with(b">>>") {
                    3
                } else {
                    2
                }
            }
            BinOp::AndAnd | BinOp::OrOr | BinOp::Le | BinOp::Ge | BinOp::Eq | BinOp::Ne => 2,
            BinOp::Pow => 2,
            _ => 1,
        };
        self.pos += len;
    }

    fn parse_unary(&mut self) -> Option<i64> {
        self.skip_ws();
        match self.peek() {
            Some(b'+') => {
                self.pos += 1;
                self.parse_unary()
            }
            Some(b'-') => {
                self.pos += 1;
                Some(self.parse_unary()?.wrapping_neg())
            }
            Some(b'~') => {
                self.pos += 1;
                Some(!self.parse_unary()?)
            }
            Some(b'!') => {
                self.pos += 1;
                Some((self.parse_unary()? == 0) as i64)
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_atom(&mut self) -> Option<i64> {
        self.skip_ws();
        if self.eat(b'(') {
            let value = self.parse_expr(0)?;
            if !self.eat(b')') {
                return None;
            }
            return Some(value);
        }
        let byte = self.peek()?;
        if byte == b'$' {
            return self.parse_system_function();
        }
        if byte.is_ascii_digit() || byte == b'\'' {
            return self.parse_number();
        }
        if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = self.pos;
            while let Some(b) = self.peek() {
                if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            let name = std::str::from_utf8(&self.src[start..self.pos]).ok()?;
            return self.env.get(name).copied();
        }
        None
    }

    fn parse_system_function(&mut self) -> Option<i64> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let name = std::str::from_utf8(&self.src[start..self.pos]).ok()?;
        if !self.eat(b'(') {
            return None;
        }
        let argument = self.parse_expr(0)?;
        if !self.eat(b')') {
            return None;
        }
        match name {
            "$clog2" => Some(clog2(argument)),
            _ => None,
        }
    }

    fn parse_number(&mut self) -> Option<i64> {
        let start = self.pos;
        // Optional size before the apostrophe.
        let mut seen_apostrophe = false;
        while let Some(b) = self.peek() {
            if b == b'\'' {
                seen_apostrophe = true;
                self.pos += 1;
                break;
            } else if b.is_ascii_digit() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if seen_apostrophe {
            // Optional signedness and base.
            if matches!(self.peek(), Some(b's' | b'S')) {
                self.pos += 1;
            }
            let base = match self.peek() {
                Some(b'b' | b'B') => 2,
                Some(b'o' | b'O') => 8,
                Some(b'd' | b'D') => 10,
                Some(b'h' | b'H') => 16,
                _ => return None,
            };
            self.pos += 1;
            let digits_start = self.pos;
            while let Some(b) = self.peek() {
                if b.is_ascii_alphanumeric() || b == b'_' {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            let digits = std::str::from_utf8(&self.src[digits_start..self.pos]).ok()?;
            let cleaned: String = digits.chars().filter(|c| *c != '_').collect();
            if cleaned.is_empty()
                || cleaned
                    .chars()
                    .any(|c| matches!(c, 'x' | 'X' | 'z' | 'Z' | '?'))
            {
                return None;
            }
            return i64::from_str_radix(&cleaned, base).ok();
        }
        // Plain decimal (possibly with underscores).
        let digits = std::str::from_utf8(&self.src[start..self.pos]).ok()?;
        let cleaned: String = digits.chars().filter(|c| *c != '_').collect();
        if cleaned.is_empty() {
            return None;
        }
        cleaned.parse::<i64>().ok()
    }
}

fn clog2(value: i64) -> i64 {
    if value <= 1 {
        0
    } else {
        (value - 1).ilog2() as i64 + 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Shl,
    Shr,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    AndAnd,
    OrOr,
    And,
    Or,
    Xor,
}

/// Binding powers, loosest first.  The exact SystemVerilog precedence is not
/// important here; what matters is that `a + b * c` and `WIDTH - 1` evaluate
/// the way a designer expects.
fn binding_power(op: BinOp) -> (u8, u8) {
    match op {
        BinOp::OrOr => (1, 2),
        BinOp::AndAnd => (3, 4),
        BinOp::Or => (5, 6),
        BinOp::Xor => (7, 8),
        BinOp::And => (9, 10),
        BinOp::Eq | BinOp::Ne => (11, 12),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => (13, 14),
        BinOp::Shl | BinOp::Shr => (15, 16),
        BinOp::Add | BinOp::Sub => (17, 18),
        BinOp::Mul | BinOp::Div | BinOp::Mod => (19, 20),
        BinOp::Pow => (22, 21),
    }
}

fn apply_binary(op: BinOp, lhs: i64, rhs: i64) -> Option<i64> {
    Some(match op {
        BinOp::Add => lhs.wrapping_add(rhs),
        BinOp::Sub => lhs.wrapping_sub(rhs),
        BinOp::Mul => lhs.wrapping_mul(rhs),
        BinOp::Div => {
            if rhs == 0 {
                return None;
            }
            lhs.wrapping_div(rhs)
        }
        BinOp::Mod => {
            if rhs == 0 {
                return None;
            }
            lhs.wrapping_rem(rhs)
        }
        BinOp::Shl => lhs.checked_shl(rhs.try_into().ok()?)?,
        BinOp::Shr => lhs.checked_shr(rhs.try_into().ok()?)?,
        BinOp::Pow => {
            if rhs < 0 || rhs > 62 {
                return None;
            }
            lhs.checked_pow(rhs as u32)?
        }
        BinOp::Lt => (lhs < rhs) as i64,
        BinOp::Le => (lhs <= rhs) as i64,
        BinOp::Gt => (lhs > rhs) as i64,
        BinOp::Ge => (lhs >= rhs) as i64,
        BinOp::Eq => (lhs == rhs) as i64,
        BinOp::Ne => (lhs != rhs) as i64,
        BinOp::AndAnd => ((lhs != 0) && (rhs != 0)) as i64,
        BinOp::OrOr => ((lhs != 0) || (rhs != 0)) as i64,
        BinOp::And => lhs & rhs,
        BinOp::Or => lhs | rhs,
        BinOp::Xor => lhs ^ rhs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> ParamEnv {
        let mut env = ParamEnv::new();
        env.insert("WIDTH".to_string(), 8);
        env.insert("DEPTH".to_string(), 16);
        env
    }

    #[test]
    fn literals_and_operators() {
        let env = env();
        assert_eq!(eval_const("8", &env), Some(8));
        assert_eq!(eval_const("8'hFF", &env), Some(255));
        assert_eq!(eval_const("4'b1010", &env), Some(10));
        assert_eq!(eval_const("WIDTH-1", &env), Some(7));
        assert_eq!(eval_const("(WIDTH-1) / 2", &env), Some(3));
        assert_eq!(eval_const("1 << 4", &env), Some(16));
        assert_eq!(eval_const("$clog2(DEPTH)", &env), Some(4));
        assert_eq!(eval_const("WIDTH * 2 + 1", &env), Some(17));
        assert_eq!(eval_const("32'd10_000", &env), Some(10_000));
    }

    #[test]
    fn rejects_unsupported_expressions() {
        let env = env();
        assert_eq!(eval_const("WIDTH + UNKNOWN", &env), None);
        assert_eq!(eval_const("foo(1)", &env), None);
        assert_eq!(eval_const("8'hx", &env), None);
        assert_eq!(eval_const("", &env), None);
    }

    #[test]
    fn range_widths() {
        let env = env();
        assert_eq!(dim_width("7", "0", &env), Some(8));
        assert_eq!(dim_width("WIDTH-1", "0", &env), Some(8));
        assert_eq!(dim_width("0", "7", &env), Some(8));
        assert_eq!(dim_width("3", "0", &env), Some(4));
        assert_eq!(dim_width("UNKNOWN", "0", &env), None);
    }
}
