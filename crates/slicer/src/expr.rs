//! The expressions a start or end G-code may hold in braces, beside `[name]`: `{first_layer_bed_temp - 5}`, `{if(bed_temp > 90, 1,
//! 0)}`. Numbers, the settings `[name]` knows, `+ - * / %`, comparisons (`< <= > >= == !=`, true is 1 and false 0), `and`, `or`,
//! `not`, parentheses, and the functions `if(test, then, else)`, `min`, `max`, `round`, `floor`, `ceil` and `abs`. A small
//! recursive-descent evaluator of our own, with a length and a depth limit: the G-code comes from a file and is hostile.

/// The longest expression read, and the deepest it may nest.
pub const MAX_LEN: usize = 500;
const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Num(f64),
    Name(String),
    Op(&'static str),
    Open,
    Close,
    Comma,
}

fn tokens(text: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            let n: f64 = text[start..i].parse().map_err(|_| format!("`{}` is not a number", &text[start..i]))?;
            out.push(Token::Num(n));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Token::Name(text[start..i].to_string()));
        } else {
            let two = text.get(i..i + 2).unwrap_or("");
            let op = ["<=", ">=", "==", "!=", "&&", "||"].into_iter().find(|o| *o == two);
            if let Some(op) = op {
                out.push(Token::Op(op));
                i += 2;
                continue;
            }
            out.push(match c {
                b'(' => Token::Open,
                b')' => Token::Close,
                b',' => Token::Comma,
                b'+' => Token::Op("+"),
                b'-' => Token::Op("-"),
                b'*' => Token::Op("*"),
                b'/' => Token::Op("/"),
                b'%' => Token::Op("%"),
                b'<' => Token::Op("<"),
                b'>' => Token::Op(">"),
                b'!' => Token::Op("!"),
                _ => return Err(format!("`{}` has no place in an expression", text[i..].chars().next().unwrap_or('?'))),
            });
            i += 1;
        }
    }
    Ok(out)
}

struct Parser<'a, F: Fn(&str) -> Option<f64>> {
    t: Vec<Token>,
    at: usize,
    depth: usize,
    var: &'a F,
}

fn truth(x: f64) -> bool {
    x != 0.0
}

fn bool_num(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

impl<F: Fn(&str) -> Option<f64>> Parser<'_, F> {
    fn peek(&self) -> Option<&Token> {
        self.t.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.t.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn eat_op(&mut self, ops: &[&str]) -> Option<&'static str> {
        match self.peek() {
            Some(Token::Op(o)) if ops.contains(o) => {
                let o = *o;
                self.at += 1;
                Some(o)
            }
            Some(Token::Name(n)) if ops.contains(&n.as_str()) => {
                let o = ["and", "or", "not"].into_iter().find(|w| w == n)?;
                self.at += 1;
                Some(o)
            }
            _ => None,
        }
    }

    fn deeper(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH { Err(format!("the expression nests more than {MAX_DEPTH} deep")) } else { Ok(()) }
    }

    fn expr(&mut self) -> Result<f64, String> {
        self.deeper()?;
        let mut v = self.and()?;
        while self.eat_op(&["or", "||"]).is_some() {
            let r = self.and()?;
            v = bool_num(truth(v) || truth(r));
        }
        self.depth -= 1;
        Ok(v)
    }

    fn and(&mut self) -> Result<f64, String> {
        let mut v = self.cmp()?;
        while self.eat_op(&["and", "&&"]).is_some() {
            let r = self.cmp()?;
            v = bool_num(truth(v) && truth(r));
        }
        Ok(v)
    }

    fn cmp(&mut self) -> Result<f64, String> {
        let v = self.sum()?;
        let Some(op) = self.eat_op(&["<", "<=", ">", ">=", "==", "!="]) else { return Ok(v) };
        let r = self.sum()?;
        Ok(bool_num(match op {
            "<" => v < r,
            "<=" => v <= r,
            ">" => v > r,
            ">=" => v >= r,
            "==" => (v - r).abs() < 1e-9,
            _ => (v - r).abs() >= 1e-9,
        }))
    }

    fn sum(&mut self) -> Result<f64, String> {
        let mut v = self.term()?;
        while let Some(op) = self.eat_op(&["+", "-"]) {
            let r = self.term()?;
            v = if op == "+" { v + r } else { v - r };
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<f64, String> {
        let mut v = self.unary()?;
        while let Some(op) = self.eat_op(&["*", "/", "%"]) {
            let r = self.unary()?;
            v = match op {
                "*" => v * r,
                _ if r == 0.0 => return Err("a division by zero".into()),
                "/" => v / r,
                _ => v % r,
            };
        }
        Ok(v)
    }

    fn unary(&mut self) -> Result<f64, String> {
        self.deeper()?;
        let v = match self.eat_op(&["-", "!", "not"]) {
            Some("-") => -self.unary()?,
            Some(_) => bool_num(!truth(self.unary()?)),
            None => self.atom()?,
        };
        self.depth -= 1;
        Ok(v)
    }

    fn atom(&mut self) -> Result<f64, String> {
        match self.next() {
            Some(Token::Num(n)) => Ok(n),
            Some(Token::Open) => {
                let v = self.expr()?;
                match self.next() {
                    Some(Token::Close) => Ok(v),
                    _ => Err("a `(` is never closed".into()),
                }
            }
            Some(Token::Name(name)) if self.peek() == Some(&Token::Open) => {
                self.at += 1;
                let mut args = Vec::new();
                if self.peek() != Some(&Token::Close) {
                    loop {
                        args.push(self.expr()?);
                        match self.next() {
                            Some(Token::Comma) => continue,
                            Some(Token::Close) => break,
                            _ => return Err(format!("`{name}(` is never closed")),
                        }
                    }
                } else {
                    self.at += 1;
                }
                call(&name, &args)
            }
            Some(Token::Name(name)) => (self.var)(&name)
                .ok_or_else(|| format!("unknown name `{name}` in an expression; known: {}", crate::profile::PLACEHOLDERS.join(", "))),
            Some(t) => Err(format!("an expression cannot go on with {t:?}")),
            None => Err("the expression ends too soon".into()),
        }
    }
}

fn call(name: &str, args: &[f64]) -> Result<f64, String> {
    let one = || if args.len() == 1 { Ok(args[0]) } else { Err(format!("`{name}` takes one number")) };
    match name {
        "if" if args.len() == 3 => Ok(if truth(args[0]) { args[1] } else { args[2] }),
        "if" => Err("`if` takes three: if(test, then, else)".into()),
        "min" | "max" if !args.is_empty() => Ok(args
            .iter()
            .copied()
            .fold(if name == "min" { f64::INFINITY } else { f64::NEG_INFINITY }, |a, b| if name == "min" { a.min(b) } else { a.max(b) })),
        "round" => one().map(f64::round),
        "floor" => one().map(f64::floor),
        "ceil" => one().map(f64::ceil),
        "abs" => one().map(f64::abs),
        _ => Err(format!("unknown function `{name}`: if, min, max, round, floor, ceil or abs")),
    }
}

/// The value of `text`, its names read by `var`. An error says what is wrong.
pub fn eval(text: &str, var: &impl Fn(&str) -> Option<f64>) -> Result<f64, String> {
    if text.len() > MAX_LEN {
        return Err(format!("an expression is at most {MAX_LEN} characters"));
    }
    let mut p = Parser { t: tokens(text)?, at: 0, depth: 0, var };
    let v = p.expr()?;
    if p.at < p.t.len() {
        return Err(format!("the expression `{text}` goes on after its end"));
    }
    if !v.is_finite() {
        return Err(format!("the expression `{text}` is not a number"));
    }
    Ok(v)
}

/// A value as the G-code writes it: a whole number without a point, else up to four places.
pub fn show(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(text: &str) -> Result<f64, String> {
        eval(text, &|n| match n {
            "bed_temp" => Some(100.0),
            "nozzle_temp" => Some(250.0),
            "layer_height" => Some(0.2),
            _ => None,
        })
    }

    #[test]
    fn arithmetic_comparisons_and_if_give_what_they_say() {
        assert_eq!(with("1 + 2 * 3").unwrap(), 7.0);
        assert_eq!(with("(1 + 2) * 3").unwrap(), 9.0);
        assert_eq!(with("-bed_temp + 5").unwrap(), -95.0);
        assert_eq!(with("10 % 4").unwrap(), 2.0);
        assert_eq!(with("bed_temp > 90").unwrap(), 1.0);
        assert_eq!(with("if(bed_temp > 90 and nozzle_temp >= 250, 1, 0)").unwrap(), 1.0);
        assert_eq!(with("if(not (bed_temp == 100) || 0, 7, 8)").unwrap(), 8.0);
        assert_eq!(with("max(bed_temp, 110, 60) - min(3, 2)").unwrap(), 108.0);
        assert_eq!(with("round(layer_height * 10)").unwrap(), 2.0);
        assert_eq!((with("floor(2.7)").unwrap(), with("ceil(2.1)").unwrap(), with("abs(-3)").unwrap()), (2.0, 3.0, 3.0));
        assert_eq!((show(215.0), show(0.25), show(1.0 / 3.0)), ("215".to_string(), "0.25".to_string(), "0.3333".to_string()));
    }

    #[test]
    fn what_is_wrong_is_said_never_a_panic() {
        for (bad, says) in [
            ("1 / 0", "division"),
            ("speed + 1", "unknown name"),
            ("(1 + 2", "never closed"),
            ("1 2", "goes on"),
            ("if(1, 2)", "three"),
            ("sqrt(4)", "unknown function"),
            ("1 + ", "too soon"),
            ("1 $ 2", "no place"),
            ("1..2", "not a number"),
        ] {
            assert!(with(bad).is_err_and(|e| e.contains(says)), "{bad}: {:?}", with(bad));
        }
        assert!(with(&"(".repeat(200)).is_err());
        assert!(with(&"-".repeat(100)).is_err());
        assert!(with(&"1+".repeat(400)).is_err(), "too long");
        let mut r = lamina_core::fuzz::Rng::new(0x5e);
        for _ in 0..2000 {
            let _ = with(&String::from_utf8_lossy(&r.input(&[b"if(bed_temp > 90, (1 + 2) * 3, max(1, 2))"])));
        }
    }
}
