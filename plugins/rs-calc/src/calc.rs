//! Calculator evaluation, base conversion, paste sanitization, and locale formatting.
//! Standard mode uses f64; Programmer mode uses i64 with wrapping arithmetic and C precedence.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base {
    Hex,
    Dec,
    Oct,
    Bin,
}

pub const BASES: [Base; 4] = [Base::Hex, Base::Dec, Base::Oct, Base::Bin];

impl Base {
    pub fn radix(self) -> u32 {
        match self {
            Base::Hex => 16,
            Base::Dec => 10,
            Base::Oct => 8,
            Base::Bin => 2,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Base::Hex => "HEX",
            Base::Dec => "DEC",
            Base::Oct => "OCT",
            Base::Bin => "BIN",
        }
    }
    pub fn is_digit(self, c: char) -> bool {
        c.is_digit(self.radix())
    }
}

fn err_div_zero() -> String {
    t!("Деление на ноль", "Division by zero", ja: "ゼロ除算", pt: "Divisão por zero").into()
}

fn err_neg_sqrt() -> String {
    t!("Корень из отрицательного", "Square root of negative", ja: "負の数の平方根", pt: "Raiz de número negativo").into()
}

fn err_overflow() -> String {
    t!("Переполнение", "Overflow", ja: "オーバーフロー", pt: "Estouro").into()
}

fn err_syntax() -> String {
    t!("Ошибка выражения", "Invalid expression", ja: "無効な式", pt: "Expressão inválida").into()
}

// ---------- Standard evaluator (f64) ----------

#[derive(Clone, PartialEq, Debug)]
enum StdTok {
    Num(f64),
    Plus,
    Minus,
    Mul,
    Div,
    Pow,
    Sqrt,
    Pct,
    LParen,
    RParen,
}

fn tokenize_std(s: &str) -> Result<Vec<StdTok>, String> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || c == '.' || c == ',' {
            let mut buf = String::new();
            let mut dot = false;
            while i < chars.len() {
                let ch = chars[i];
                if ch.is_ascii_digit() {
                    buf.push(ch);
                    i += 1;
                } else if (ch == '.' || ch == ',') && !dot {
                    dot = true;
                    buf.push('.');
                    i += 1;
                } else {
                    break;
                }
            }
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                buf.push('e');
                i += 1;
                if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                    buf.push(chars[i]);
                    i += 1;
                }
                while i < chars.len() && chars[i].is_ascii_digit() {
                    buf.push(chars[i]);
                    i += 1;
                }
            }
            let v: f64 = buf.parse().map_err(|_| err_syntax())?;
            out.push(StdTok::Num(v));
            continue;
        }
        match c {
            '+' => out.push(StdTok::Plus),
            '-' | '−' => out.push(StdTok::Minus),
            '*' | '×' => out.push(StdTok::Mul),
            '/' | '÷' => out.push(StdTok::Div),
            '^' => out.push(StdTok::Pow),
            '√' => out.push(StdTok::Sqrt),
            '%' => out.push(StdTok::Pct),
            '(' => out.push(StdTok::LParen),
            ')' => out.push(StdTok::RParen),
            _ => return Err(err_syntax()),
        }
        i += 1;
    }
    Ok(out)
}

struct StdParser<'a> {
    toks: &'a [StdTok],
    pos: usize,
}

impl<'a> StdParser<'a> {
    fn peek(&self) -> Option<&StdTok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<&StdTok> {
        let t = self.toks.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_add(&mut self) -> Result<f64, String> {
        let (mut lhs, _) = self.parse_mul()?;
        while let Some(op @ (StdTok::Plus | StdTok::Minus)) = self.peek().cloned() {
            self.next();
            let (rhs, is_pct) = self.parse_mul()?;
            let delta = if is_pct { lhs * rhs } else { rhs };
            lhs = if op == StdTok::Plus { lhs + delta } else { lhs - delta };
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<(f64, bool), String> {
        let (mut lhs, mut is_pct) = self.parse_unary()?;
        while let Some(op @ (StdTok::Mul | StdTok::Div)) = self.peek().cloned() {
            self.next();
            let (rhs, _) = self.parse_unary()?;
            is_pct = false;
            lhs = if op == StdTok::Mul {
                lhs * rhs
            } else {
                if rhs == 0.0 {
                    return Err(err_div_zero());
                }
                lhs / rhs
            };
        }
        Ok((lhs, is_pct))
    }

    // Unary minus binds looser than ^ so -2^2 == -4.
    fn parse_unary(&mut self) -> Result<(f64, bool), String> {
        match self.peek() {
            Some(StdTok::Minus) => {
                self.next();
                let (v, pct) = self.parse_unary()?;
                Ok((-v, pct))
            }
            Some(StdTok::Plus) => {
                self.next();
                self.parse_unary()
            }
            Some(StdTok::Sqrt) => {
                self.next();
                let (v, pct) = self.parse_unary()?;
                if v < 0.0 {
                    return Err(err_neg_sqrt());
                }
                Ok((v.sqrt(), pct))
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> Result<(f64, bool), String> {
        let (mut base, mut is_pct) = self.parse_postfix()?;
        if matches!(self.peek(), Some(StdTok::Pow)) {
            self.next();
            let (exp, _) = self.parse_unary()?;
            base = base.powf(exp);
            is_pct = false;
        }
        Ok((base, is_pct))
    }

    fn parse_postfix(&mut self) -> Result<(f64, bool), String> {
        let mut v = self.parse_atom()?;
        let mut is_pct = false;
        while matches!(self.peek(), Some(StdTok::Pct)) {
            self.next();
            v /= 100.0;
            is_pct = true;
        }
        Ok((v, is_pct))
    }

    fn parse_atom(&mut self) -> Result<f64, String> {
        match self.next().cloned() {
            Some(StdTok::Num(n)) => Ok(n),
            Some(StdTok::LParen) => {
                let v = self.parse_add()?;
                // Unclosed parentheses are closed automatically
                if matches!(self.peek(), Some(StdTok::RParen)) {
                    self.next();
                }
                Ok(v)
            }
            _ => Err(err_syntax()),
        }
    }
}

pub fn eval_std(expr: &str) -> Result<f64, String> {
    let toks = tokenize_std(expr)?;
    if toks.is_empty() {
        return Ok(0.0);
    }
    let mut p = StdParser { toks: &toks, pos: 0 };
    let v = p.parse_add()?;
    if p.pos != toks.len() {
        return Err(err_syntax());
    }
    if !v.is_finite() {
        return Err(err_overflow());
    }
    Ok(if v == 0.0 { 0.0 } else { v })
}

// ---------- Programmer evaluator (i64, wrapping, C precedence) ----------

#[derive(Clone, PartialEq, Debug)]
enum ProgTok {
    Num(i64),
    Plus,
    Minus,
    Mul,
    Div,
    Mod,
    And,
    Or,
    Xor,
    Not,
    Shl,
    Shr,
    LParen,
    RParen,
}

fn keyword_len(chars: &[char], i: usize) -> Option<(ProgTok, usize)> {
    let mut word = String::new();
    let mut j = i;
    while j < chars.len() && chars[j].is_ascii_alphabetic() {
        word.push(chars[j].to_ascii_uppercase());
        j += 1;
    }
    // Must not be glued to trailing digits
    if j < chars.len() && chars[j].is_ascii_digit() {
        return None;
    }
    let tok = match word.as_str() {
        "AND" => ProgTok::And,
        "OR" => ProgTok::Or,
        "XOR" => ProgTok::Xor,
        "NOT" => ProgTok::Not,
        "MOD" => ProgTok::Mod,
        _ => return None,
    };
    Some((tok, j - i))
}

fn tokenize_prog(s: &str, base: Base) -> Result<Vec<ProgTok>, String> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() {
            if let Some((tok, len)) = keyword_len(&chars, i) {
                out.push(tok);
                i += len;
                continue;
            }
        }
        if base.is_digit(c) {
            let mut buf = String::new();
            while i < chars.len() && base.is_digit(chars[i]) {
                buf.push(chars[i]);
                i += 1;
            }
            let u = u64::from_str_radix(&buf, base.radix()).map_err(|_| err_overflow())?;
            out.push(ProgTok::Num(u as i64));
            continue;
        }
        match c {
            '+' => out.push(ProgTok::Plus),
            '-' | '−' => out.push(ProgTok::Minus),
            '*' | '×' => out.push(ProgTok::Mul),
            '/' | '÷' => out.push(ProgTok::Div),
            '%' => out.push(ProgTok::Mod),
            '&' => out.push(ProgTok::And),
            '|' => out.push(ProgTok::Or),
            '^' => out.push(ProgTok::Xor),
            '~' => out.push(ProgTok::Not),
            '<' if chars.get(i + 1) == Some(&'<') => {
                out.push(ProgTok::Shl);
                i += 1;
            }
            '>' if chars.get(i + 1) == Some(&'>') => {
                out.push(ProgTok::Shr);
                i += 1;
            }
            '(' => out.push(ProgTok::LParen),
            ')' => out.push(ProgTok::RParen),
            _ => return Err(err_syntax()),
        }
        i += 1;
    }
    Ok(out)
}

struct ProgParser<'a> {
    toks: &'a [ProgTok],
    pos: usize,
}

impl<'a> ProgParser<'a> {
    fn peek(&self) -> Option<&ProgTok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<&ProgTok> {
        let t = self.toks.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    // C precedence: | < ^ < & < << >> < + - < * / % < unary
    fn parse_or(&mut self) -> Result<i64, String> {
        let mut v = self.parse_xor()?;
        while matches!(self.peek(), Some(ProgTok::Or)) {
            self.next();
            v |= self.parse_xor()?;
        }
        Ok(v)
    }

    fn parse_xor(&mut self) -> Result<i64, String> {
        let mut v = self.parse_and()?;
        while matches!(self.peek(), Some(ProgTok::Xor)) {
            self.next();
            v ^= self.parse_and()?;
        }
        Ok(v)
    }

    fn parse_and(&mut self) -> Result<i64, String> {
        let mut v = self.parse_shift()?;
        while matches!(self.peek(), Some(ProgTok::And)) {
            self.next();
            v &= self.parse_shift()?;
        }
        Ok(v)
    }

    fn parse_shift(&mut self) -> Result<i64, String> {
        let mut v = self.parse_add()?;
        while let Some(op @ (ProgTok::Shl | ProgTok::Shr)) = self.peek().cloned() {
            self.next();
            let r = self.parse_add()?;
            v = if op == ProgTok::Shl {
                v.wrapping_shl(r as u32)
            } else {
                v.wrapping_shr(r as u32)
            };
        }
        Ok(v)
    }

    fn parse_add(&mut self) -> Result<i64, String> {
        let mut v = self.parse_mul()?;
        while let Some(op @ (ProgTok::Plus | ProgTok::Minus)) = self.peek().cloned() {
            self.next();
            let r = self.parse_mul()?;
            v = if op == ProgTok::Plus { v.wrapping_add(r) } else { v.wrapping_sub(r) };
        }
        Ok(v)
    }

    fn parse_mul(&mut self) -> Result<i64, String> {
        let mut v = self.parse_unary()?;
        while let Some(op @ (ProgTok::Mul | ProgTok::Div | ProgTok::Mod)) = self.peek().cloned() {
            self.next();
            let r = self.parse_unary()?;
            v = match op {
                ProgTok::Mul => v.wrapping_mul(r),
                ProgTok::Div => {
                    if r == 0 {
                        return Err(err_div_zero());
                    }
                    v.wrapping_div(r)
                }
                ProgTok::Mod => {
                    if r == 0 {
                        return Err(err_div_zero());
                    }
                    v.wrapping_rem(r)
                }
                _ => unreachable!(),
            };
        }
        Ok(v)
    }

    fn parse_unary(&mut self) -> Result<i64, String> {
        match self.peek() {
            Some(ProgTok::Minus) => {
                self.next();
                Ok(self.parse_unary()?.wrapping_neg())
            }
            Some(ProgTok::Plus) => {
                self.next();
                self.parse_unary()
            }
            Some(ProgTok::Not) => {
                self.next();
                Ok(!self.parse_unary()?)
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_atom(&mut self) -> Result<i64, String> {
        match self.next().cloned() {
            Some(ProgTok::Num(n)) => Ok(n),
            Some(ProgTok::LParen) => {
                let v = self.parse_or()?;
                if matches!(self.peek(), Some(ProgTok::RParen)) {
                    self.next();
                }
                Ok(v)
            }
            _ => Err(err_syntax()),
        }
    }
}

pub fn eval_prog(expr: &str, base: Base) -> Result<i64, String> {
    let toks = tokenize_prog(expr, base)?;
    if toks.is_empty() {
        return Ok(0);
    }
    let mut p = ProgParser { toks: &toks, pos: 0 };
    let v = p.parse_or()?;
    if p.pos != toks.len() {
        return Err(err_syntax());
    }
    Ok(v)
}

// ---------- Rebase, paste sanitizer, formatting ----------

/// Rewrites every integer literal in `expr` from `from` base to `to` base, keeping operators intact.
pub fn rebase_expr(expr: &str, from: Base, to: Base) -> String {
    if from == to {
        return expr.to_string();
    }
    let chars: Vec<char> = expr.chars().collect();
    let mut out = String::with_capacity(expr.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_alphabetic() {
            if let Some((_, len)) = keyword_len(&chars, i) {
                for &k in &chars[i..i + len] {
                    out.push(k);
                }
                i += len;
                continue;
            }
        }
        if from.is_digit(c) {
            let mut buf = String::new();
            while i < chars.len() && from.is_digit(chars[i]) {
                buf.push(chars[i]);
                i += 1;
            }
            if let Ok(u) = u64::from_str_radix(&buf, from.radix()) {
                out.push_str(&format_u64(u, to));
            } else {
                out.push_str(&buf);
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

pub fn format_u64(u: u64, base: Base) -> String {
    match base {
        Base::Hex => format!("{u:X}"),
        Base::Dec => format!("{u}"),
        Base::Oct => format!("{u:o}"),
        Base::Bin => format!("{u:b}"),
    }
}

/// Formats `v` for inserting into a programmer expression.
pub fn format_prog_raw(v: i64, base: Base) -> String {
    match base {
        Base::Dec => format!("{v}"),
        _ => format_u64(v as u64, base),
    }
}

/// Formats `v` with digit grouping for the HEX/DEC/OCT/BIN readout.
pub fn format_prog_grouped(v: i64, base: Base) -> String {
    let (neg, raw) = match base {
        Base::Dec if v < 0 => (true, v.unsigned_abs().to_string()),
        Base::Dec => (false, v.to_string()),
        _ => (false, format_u64(v as u64, base)),
    };
    let step = match base {
        Base::Hex | Base::Bin => 4,
        Base::Dec | Base::Oct => 3,
    };
    let mut grouped = String::with_capacity(raw.len() + raw.len() / step + 1);
    if neg {
        grouped.push('-');
    }
    let bytes = raw.as_bytes();
    for (idx, &b) in bytes.iter().enumerate() {
        let rem = bytes.len() - idx;
        if idx > 0 && rem % step == 0 {
            grouped.push(' ');
        }
        grouped.push(b as char);
    }
    grouped
}

/// Formats an f64 result with the current UI decimal separator (comma for ru/pt, dot for en/ja).
pub fn format_std(v: f64) -> String {
    let v = if v == 0.0 { 0.0 } else { v };
    let abs = v.abs();
    let mut s = if abs != 0.0 && (abs >= 1e15 || abs < 1e-9) {
        let raw = format!("{v:.10e}");
        if let Some((m, e)) = raw.split_once('e') {
            let m = m.trim_end_matches('0').trim_end_matches('.');
            format!("{m}e{e}")
        } else {
            raw
        }
    } else {
        let raw = format!("{v:.12}");
        if raw.contains('.') {
            raw.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            raw
        }
    };
    if s == "-0" {
        s = "0".into();
    }
    if !dot!() {
        s = s.replace('.', ",");
    }
    s
}

/// Filters pasted clipboard text to characters valid for the current mode and base.
pub fn sanitize_paste(s: &str, prog: bool, base: Base) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if prog {
            if c.is_ascii_alphabetic() {
                if let Some((_, len)) = keyword_len(&chars, i) {
                    if !out.is_empty() && !out.ends_with(' ') {
                        out.push(' ');
                    }
                    for &k in &chars[i..i + len] {
                        out.push(k.to_ascii_uppercase());
                    }
                    out.push(' ');
                    i += len;
                    continue;
                }
            }
            if base.is_digit(c) {
                out.push(c.to_ascii_uppercase());
            } else {
                match c {
                    '+' | '(' | ')' | '&' | '|' | '^' | '~' | '%' | '<' | '>' => out.push(c),
                    '-' | '−' => out.push('-'),
                    '*' | '×' => out.push('*'),
                    '/' | '÷' => out.push('/'),
                    _ => {}
                }
            }
        } else {
            match c {
                '0'..='9' | '+' | '^' | '%' | '(' | ')' | '√' => out.push(c),
                '.' | ',' => out.push(if dot!() { '.' } else { ',' }),
                '-' | '−' => out.push('-'),
                '*' | '×' => out.push('*'),
                '/' | '÷' => out.push('/'),
                _ => {}
            }
        }
        i += 1;
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_precedence_and_unary_power() {
        assert_eq!(eval_std("2 + 3 * 4").unwrap(), 14.0);
        assert_eq!(eval_std("-2^2").unwrap(), -4.0);
        assert_eq!(eval_std("(-2)^2").unwrap(), 4.0);
        assert_eq!(eval_std("2^-3").unwrap(), 0.125);
        assert_eq!(eval_std("2^3^2").unwrap(), 512.0);
        assert_eq!(eval_std("√9").unwrap(), 3.0);
        assert_eq!(eval_std("-√9").unwrap(), -3.0);
        assert_eq!(eval_std("√(9+16)").unwrap(), 5.0);
    }

    #[test]
    fn standard_percent_and_auto_close_parens() {
        assert_eq!(eval_std("100 + 20%").unwrap(), 120.0);
        assert_eq!(eval_std("100 - 20%").unwrap(), 80.0);
        assert_eq!(eval_std("200 * 10%").unwrap(), 20.0);
        assert_eq!(eval_std("50%").unwrap(), 0.5);
        assert_eq!(eval_std("(2 + 3 * (4 + 1").unwrap(), 17.0);
        assert_eq!(eval_std("1,5 + 2,5").unwrap(), 4.0);
    }

    #[test]
    fn standard_errors_are_russian_in_tests() {
        assert_eq!(eval_std("1 / 0").unwrap_err(), "Деление на ноль");
        assert_eq!(eval_std("√-4").unwrap_err(), "Корень из отрицательного");
        assert_eq!(eval_std("1 + * 2").unwrap_err(), "Ошибка выражения");
    }

    #[test]
    fn programmer_c_precedence_and_wrapping() {
        assert_eq!(eval_prog("1 | 2 & 4", Base::Dec).unwrap(), 1);
        assert_eq!(eval_prog("1 + 2 << 3", Base::Dec).unwrap(), 24);
        assert_eq!(eval_prog("FF AND 0F", Base::Hex).unwrap(), 0x0F);
        assert_eq!(eval_prog("10 XOR 12", Base::Dec).unwrap(), 6);
        assert_eq!(eval_prog("NOT 0", Base::Dec).unwrap(), -1);
        assert_eq!(eval_prog("10 MOD 3", Base::Dec).unwrap(), 1);
        assert_eq!(eval_prog("9223372036854775807 + 1", Base::Dec).unwrap(), i64::MIN);
        assert_eq!(eval_prog("10 / 0", Base::Dec).unwrap_err(), "Деление на ноль");
        assert_eq!(eval_prog("10 MOD 0", Base::Dec).unwrap_err(), "Деление на ноль");
        assert_eq!(eval_prog("(1 + 2 * (3 + 4", Base::Dec).unwrap(), 15);
    }

    #[test]
    fn rebase_and_sanitize_and_format() {
        assert_eq!(rebase_expr("FF + 10", Base::Hex, Base::Dec), "255 + 16");
        assert_eq!(rebase_expr("FF AND 0F", Base::Hex, Base::Dec), "255 AND 15");
        assert_eq!(rebase_expr("10 OR 3", Base::Dec, Base::Bin), "1010 OR 11");
        assert_eq!(sanitize_paste(" $12.5 × 2−1 ", false, Base::Dec), "12,5*2-1");
        assert_eq!(sanitize_paste("0xFFg + 12", true, Base::Hex), "0FF+12");
        assert_eq!(sanitize_paste("109 + 2", true, Base::Oct), "10+2");
        assert_eq!(format_std(0.1 + 0.2), "0,3");
        assert_eq!(format_std(-0.0), "0");
        assert_eq!(format_prog_grouped(0x123456, Base::Hex), "12 3456");
    }
}
