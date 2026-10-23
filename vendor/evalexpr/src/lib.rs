//! Arithmetic evaluation for `ifeval::[]` operands: numbers, `+ - * / %`, unary minus and
//! parentheses. Only the two functions acdc-parser calls are provided.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvalexprError(String);

impl fmt::Display for EvalexprError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for EvalexprError {}

/// Evaluates `expression` as a floating-point number.
pub fn eval_float(expression: &str) -> Result<f64, EvalexprError> {
    let mut parser = Parser::new(expression);
    let value = parser.expression()?;
    parser.finish()?;
    Ok(value)
}

/// Evaluates `expression` and returns it as an integer when it is a whole number.
pub fn eval_int(expression: &str) -> Result<i64, EvalexprError> {
    let value = eval_float(expression)?;
    if value.fract() == 0.0 && value.is_finite() && value.abs() < 9.0e15 {
        #[allow(clippy::cast_possible_truncation)]
        Ok(value as i64)
    } else {
        Err(EvalexprError(format!("{value} is not an integer")))
    }
}

struct Parser<'a> {
    input: &'a [u8],
    position: usize,
    depth: usize,
}

/// Deeper nesting than this is rejected rather than risking the stack.
const MAX_DEPTH: usize = 64;

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
            depth: 0,
        }
    }

    fn error(&self, message: &str) -> EvalexprError {
        EvalexprError(format!("{message} at {}", self.position))
    }

    fn skip_whitespace(&mut self) {
        while self
            .input
            .get(self.position)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.position += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_whitespace();
        self.input.get(self.position).copied()
    }

    fn finish(&mut self) -> Result<(), EvalexprError> {
        match self.peek() {
            None => Ok(()),
            Some(_) => Err(self.error("unexpected input")),
        }
    }

    fn expression(&mut self) -> Result<f64, EvalexprError> {
        let mut value = self.term()?;
        while let Some(operator @ (b'+' | b'-')) = self.peek() {
            self.position += 1;
            let right = self.term()?;
            value = if operator == b'+' { value + right } else { value - right };
        }
        Ok(value)
    }

    fn term(&mut self) -> Result<f64, EvalexprError> {
        let mut value = self.unary()?;
        while let Some(operator @ (b'*' | b'/' | b'%')) = self.peek() {
            self.position += 1;
            let right = self.unary()?;
            value = match operator {
                b'*' => value * right,
                _ if right == 0.0 => return Err(self.error("division by zero")),
                b'/' => value / right,
                _ => value % right,
            };
        }
        Ok(value)
    }

    fn unary(&mut self) -> Result<f64, EvalexprError> {
        match self.peek() {
            Some(b'-') => {
                self.position += 1;
                self.nested(|parser| parser.unary()).map(|value| -value)
            }
            Some(b'+') => {
                self.position += 1;
                self.nested(Self::unary)
            }
            _ => self.primary(),
        }
    }

    fn nested(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<f64, EvalexprError>,
    ) -> Result<f64, EvalexprError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("expression nested too deeply"));
        }
        let value = parse(self);
        self.depth -= 1;
        value
    }

    fn primary(&mut self) -> Result<f64, EvalexprError> {
        match self.peek() {
            Some(b'(') => {
                self.position += 1;
                let value = self.nested(Self::expression)?;
                if self.peek() != Some(b')') {
                    return Err(self.error("expected `)`"));
                }
                self.position += 1;
                Ok(value)
            }
            Some(byte) if byte.is_ascii_digit() || byte == b'.' => {
                let start = self.position;
                while self
                    .input
                    .get(self.position)
                    .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.')
                {
                    self.position += 1;
                }
                std::str::from_utf8(&self.input[start..self.position])
                    .ok()
                    .and_then(|number| number.parse().ok())
                    .ok_or_else(|| self.error("invalid number"))
            }
            _ => Err(self.error("expected a number")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_arithmetic() {
        assert_eq!(eval_float("1 + 2 * 3"), Ok(7.0));
        assert_eq!(eval_float("(1 + 2) * 3"), Ok(9.0));
        assert_eq!(eval_float("-4 / 2 + 10 % 4"), Ok(0.0));
        assert_eq!(eval_float(" 2.5 * 2 "), Ok(5.0));
        assert_eq!(eval_int("7 - 2"), Ok(5));
    }

    #[test]
    fn rejects_everything_else() {
        for expression in ["", "a", "1 +", "(1", "1 / 0", "2 3", "\"x\"", "true", "1..2"] {
            assert!(eval_float(expression).is_err(), "{expression}");
        }
        assert!(eval_int("1 / 2").is_err());
        let deep = format!("{}1{}", "(".repeat(100), ")".repeat(100));
        assert!(eval_float(&deep).is_err());
    }
}
