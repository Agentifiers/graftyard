//! The Actions expression language: a port of `@actions/expressions` 0.3's lexer and parser (MIT, GitHub), which
//! the runner's `GitHub.Actions.Expressions` matches, and an evaluator over JSON context data. Messages match the
//! oracle's text, because the parser's differential test compares them.

use serde_json::{Map, Value};

const MAX_DEPTH: usize = 50;
const MAX_LENGTH: usize = 21_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tk {
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Bang,
    BangEq,
    EqEq,
    Gt,
    Ge,
    Lt,
    Le,
    And,
    Or,
    Star,
    Number,
    Str,
    Ident,
    True,
    False,
    Null,
    Eof,
}

#[derive(Clone, Debug)]
struct Token {
    kind: Tk,
    lexeme: String,
    num: f64,
    text: String,
}

impl Token {
    /// `tokenString` in the oracle: what an error message quotes.
    fn quoted(&self) -> String {
        match self.kind {
            Tk::Eof => "EOF".into(),
            Tk::Str => self.text.clone(),
            _ => self.lexeme.clone(),
        }
    }
}

/// A parsed expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Ast {
    Lit(Value),
    Ctx(String),
    Index(Box<Ast>, Box<Ast>),
    Star,
    Call(String, Vec<Ast>),
    Not(Box<Ast>),
    Cmp(Cmp, Box<Ast>, Box<Ast>),
    And(Vec<Ast>),
    Or(Vec<Ast>),
    Group(Box<Ast>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

fn boundary(c: char) -> bool {
    matches!(c, '(' | '[' | ')' | ']' | ',' | '.' | '!' | '>' | '<' | '=' | '&' | '|') || c.is_whitespace()
}

fn legal_ident(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => cs.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        _ => false,
    }
}

/// JavaScript's `Number(string)`, which the lexer uses for number literals.
pub fn js_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    for (p, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(d) = t.strip_prefix(p) {
            return if !d.is_empty() && d.chars().all(|c| c.is_digit(radix)) {
                d.chars().fold(0.0, |a, c| a * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0)))
            } else {
                f64::NAN
            };
        }
    }
    if t.chars().any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E') {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

/// JavaScript's `String(number)`.
pub fn js_num_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if x == 0.0 {
        return "0".into();
    }
    let a = x.abs();
    if (1e-6..1e21).contains(&a) {
        return format!("{x}");
    }
    let e = format!("{x:e}");
    match e.split_once('e') {
        Some((m, exp)) if !exp.starts_with('-') => format!("{m}e+{exp}"),
        _ => e,
    }
}

/// C#'s `Double.ToString("G15")`, which names matrix configurations.
pub fn g15(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "∞" } else { "-∞" }.into();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let s = format!("{x:.14e}");
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (neg, mant) = mant.strip_prefix('-').map_or((false, mant), |m| (true, m));
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let sign = if neg { "-" } else { "" };
    if !(-4..15).contains(&exp) {
        let (h, t) = digits.split_at(1);
        let t = if t.is_empty() { String::new() } else { format!(".{t}") };
        let es = if exp < 0 { '-' } else { '+' };
        return format!("{sign}{h}{t}E{es}{:02}", exp.abs());
    }
    if exp < 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-exp - 1) as usize))
    } else {
        let point = exp as usize + 1;
        if digits.len() <= point {
            format!("{sign}{digits}{}", "0".repeat(point - digits.len()))
        } else {
            format!("{sign}{}.{}", &digits[..point], &digits[point..])
        }
    }
}

fn lex(input: &str) -> Result<Vec<Token>, String> {
    if input.chars().count() > MAX_LENGTH {
        return Err("ErrorExceededMaxLength".into());
    }
    let cs: Vec<char> = input.chars().collect();
    let mut toks: Vec<Token> = Vec::new();
    let mut off = 0;
    let unexpected = |start: usize, lexeme: &str| {
        format!("Unexpected symbol: '{lexeme}'. Located at position {} within expression: {input}", start + 1)
    };
    let tok = |kind: Tk, lexeme: String| Token { kind, lexeme, num: 0.0, text: String::new() };
    while off < cs.len() {
        let start = off;
        let c = cs[off];
        off += 1;
        let prev = toks.last().map(|t| t.kind).unwrap_or(Tk::Eof);
        let peek = |o: usize| cs.get(o).copied().unwrap_or('\0');
        let mut number = false;
        let mut ident = false;
        match c {
            '(' => toks.push(tok(Tk::LParen, c.into())),
            ')' => toks.push(tok(Tk::RParen, c.into())),
            '[' => toks.push(tok(Tk::LBracket, c.into())),
            ']' => toks.push(tok(Tk::RBracket, c.into())),
            ',' => toks.push(tok(Tk::Comma, c.into())),
            '.' => {
                if matches!(prev, Tk::Ident | Tk::RBracket | Tk::RParen | Tk::Star) {
                    toks.push(tok(Tk::Dot, c.into()));
                } else {
                    number = true;
                }
            }
            '-' | '+' => number = true,
            '!' => {
                if peek(off) == '=' {
                    off += 1;
                    toks.push(tok(Tk::BangEq, "!=".into()));
                } else {
                    toks.push(tok(Tk::Bang, c.into()));
                }
            }
            '=' | '&' | '|' => {
                if peek(off) == c {
                    off += 1;
                    let k = match c {
                        '=' => Tk::EqEq,
                        '&' => Tk::And,
                        _ => Tk::Or,
                    };
                    toks.push(tok(k, format!("{c}{c}")));
                } else {
                    ident = true;
                }
            }
            '<' | '>' => {
                let eq = peek(off) == '=';
                if eq {
                    off += 1;
                }
                let k = match (c, eq) {
                    ('<', false) => Tk::Lt,
                    ('<', true) => Tk::Le,
                    ('>', false) => Tk::Gt,
                    _ => Tk::Ge,
                };
                toks.push(tok(k, cs[start..off].iter().collect()));
            }
            '*' => toks.push(tok(Tk::Star, c.into())),
            ' ' | '\r' | '\t' | '\n' => {}
            '\'' => {
                while off < cs.len() && (cs[off] != '\'' || peek(off + 1) == '\'') {
                    if cs[off] == '\'' && peek(off + 1) == '\'' {
                        off += 1;
                    }
                    off += 1;
                }
                if off >= cs.len() {
                    return Err(unexpected(start, &cs[start..].iter().collect::<String>()));
                }
                off += 1;
                let inner: String = cs[start + 1..off - 1].iter().collect();
                // The oracle's `replace("''", "'")` replaces only the first pair.
                let text = inner.replacen("''", "'", 1);
                toks.push(Token { kind: Tk::Str, lexeme: cs[start..off].iter().collect(), num: 0.0, text });
            }
            d if d.is_ascii_digit() => number = true,
            _ => ident = true,
        }
        if number {
            while off < cs.len() && (!boundary(cs[off]) || cs[off] == '.') {
                off += 1;
            }
            let lexeme: String = cs[start..off].iter().collect();
            let v = js_number(&lexeme);
            if v.is_nan() {
                return Err(unexpected(start, &lexeme));
            }
            toks.push(Token { kind: Tk::Number, lexeme, num: v, text: String::new() });
        }
        if ident {
            while off < cs.len() && !boundary(cs[off]) {
                off += 1;
            }
            let lexeme: String = cs[start..off].iter().collect();
            let mut t = tok(Tk::Ident, lexeme.clone());
            if prev != Tk::Dot {
                match lexeme.as_str() {
                    "true" => t.kind = Tk::True,
                    "false" => t.kind = Tk::False,
                    "null" => t.kind = Tk::Null,
                    "NaN" => (t.kind, t.num) = (Tk::Number, f64::NAN),
                    "Infinity" => (t.kind, t.num) = (Tk::Number, f64::INFINITY),
                    _ => {}
                }
            }
            if !legal_ident(&lexeme) {
                return Err(unexpected(start, &lexeme));
            }
            toks.push(t);
        }
    }
    toks.push(tok(Tk::Eof, String::new()));
    Ok(toks)
}

/// Functions every expression may call, with their argument bounds.
const WELL_KNOWN: &[(&str, usize, usize)] = &[
    ("case", 3, usize::MAX),
    ("contains", 2, 2),
    ("endswith", 2, 2),
    ("format", 1, 255),
    ("fromjson", 1, 1),
    ("join", 1, 2),
    ("startswith", 2, 2),
    ("tojson", 1, 1),
];

/// The allowed contexts and functions of a schema definition, split as the oracle's `splitAllowedContext` does:
/// `name(min,max)` is a function, anything else a named context.
pub struct Allowed {
    named: Vec<String>,
    funcs: Vec<(String, usize, usize)>,
}

impl Allowed {
    pub fn new<S: AsRef<str>>(items: &[S]) -> Allowed {
        let mut named = Vec::new();
        let mut funcs = Vec::new();
        for it in items {
            let it = it.as_ref();
            let parsed = it.strip_suffix(')').and_then(|s| s.split_once('(')).and_then(|(n, args)| {
                let (lo, hi) = args.split_once(',')?;
                let ok = !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                let lo = lo.parse::<usize>().ok().filter(|_| ok)?;
                let hi = if hi == "MAX" { usize::MAX } else { hi.parse().ok()? };
                Some((n.to_lowercase(), lo, hi))
            });
            match parsed {
                Some(f) => funcs.push(f),
                None => named.push(it.to_lowercase()),
            }
        }
        Allowed { named, funcs }
    }
}

struct Parser<'a> {
    toks: Vec<Token>,
    off: usize,
    depth: usize,
    allowed: &'a Allowed,
}

type PResult = Result<Ast, String>;

impl Parser<'_> {
    fn peek(&self) -> &Token {
        &self.toks[self.off]
    }
    fn at_end(&self) -> bool {
        self.peek().kind == Tk::Eof
    }
    fn check(&self, k: Tk) -> bool {
        !self.at_end() && self.peek().kind == k
    }
    fn eat(&mut self, ks: &[Tk]) -> Option<Token> {
        let k = self.peek().kind;
        if !self.at_end() && ks.contains(&k) {
            self.off += 1;
            return Some(self.toks[self.off - 1].clone());
        }
        None
    }
    fn previous(&self) -> &Token {
        &self.toks[self.off.saturating_sub(1)]
    }
    fn err(desc: &str, t: &Token) -> String {
        format!("{desc}: '{}'", t.quoted())
    }
    fn incr(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Self::err(&format!("Exceeded max expression depth {MAX_DEPTH}"), self.peek()));
        }
        Ok(())
    }
    fn parse(mut self) -> Result<Option<Ast>, String> {
        if self.at_end() {
            return Ok(None);
        }
        let e = self.expression()?;
        if !self.at_end() {
            return Err(Self::err("Unexpected symbol", self.peek()));
        }
        Ok(Some(e))
    }
    fn expression(&mut self) -> PResult {
        self.incr()?;
        let r = self.logical(true);
        self.depth -= 1;
        r
    }
    fn logical(&mut self, or: bool) -> PResult {
        let (k, next): (Tk, fn(&mut Self) -> PResult) =
            if or { (Tk::Or, |p| p.logical(false)) } else { (Tk::And, |p| p.equality()) };
        let first = next(self)?;
        if !self.check(k) {
            return Ok(first);
        }
        self.incr()?;
        let mut args = vec![first];
        while self.eat(&[k]).is_some() {
            args.push(next(self)?);
        }
        self.depth -= 1;
        Ok(if or { Ast::Or(args) } else { Ast::And(args) })
    }
    fn equality(&mut self) -> PResult {
        let mut e = self.comparison()?;
        while let Some(op) = self.eat(&[Tk::BangEq, Tk::EqEq]) {
            let r = self.comparison()?;
            e = Ast::Cmp(if op.kind == Tk::EqEq { Cmp::Eq } else { Cmp::Ne }, Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn comparison(&mut self) -> PResult {
        let mut e = self.unary()?;
        while let Some(op) = self.eat(&[Tk::Gt, Tk::Ge, Tk::Lt, Tk::Le]) {
            let r = self.unary()?;
            let c = match op.kind {
                Tk::Gt => Cmp::Gt,
                Tk::Ge => Cmp::Ge,
                Tk::Lt => Cmp::Lt,
                _ => Cmp::Le,
            };
            e = Ast::Cmp(c, Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn unary(&mut self) -> PResult {
        if self.eat(&[Tk::Bang]).is_some() {
            self.incr()?;
            let u = self.unary();
            self.depth -= 1;
            return Ok(Ast::Not(Box::new(u?)));
        }
        self.index()
    }
    fn index(&mut self) -> PResult {
        let mut e = self.call()?;
        let mut grew = 0;
        if matches!(e, Ast::Group(_) | Ast::Call(..) | Ast::Ctx(_)) {
            loop {
                if self.eat(&[Tk::LBracket]).is_some() {
                    let idx = if self.eat(&[Tk::Star]).is_some() { Ast::Star } else { self.expression()? };
                    if self.eat(&[Tk::RBracket]).is_none() {
                        return Err(Self::err("Unexpected symbol", self.peek()));
                    }
                    self.incr()?;
                    grew += 1;
                    e = Ast::Index(Box::new(e), Box::new(idx));
                } else if self.eat(&[Tk::Dot]).is_some() {
                    self.incr()?;
                    grew += 1;
                    if let Some(p) = self.eat(&[Tk::Ident]) {
                        e = Ast::Index(Box::new(e), Box::new(Ast::Lit(Value::String(p.lexeme))));
                    } else if self.eat(&[Tk::Star]).is_some() {
                        e = Ast::Index(Box::new(e), Box::new(Ast::Star));
                    } else {
                        return Err(Self::err("Unexpected symbol", self.peek()));
                    }
                } else {
                    break;
                }
            }
        }
        self.depth -= grew;
        Ok(e)
    }
    fn call(&mut self) -> PResult {
        if !self.check(Tk::Ident) {
            return self.primary();
        }
        let id = self.eat(&[Tk::Ident]).expect("checked");
        let name = id.lexeme.to_lowercase();
        if self.eat(&[Tk::LParen]).is_none() {
            if !self.allowed.named.contains(&name) {
                return Err(Self::err("Unrecognized named-value", &id));
            }
            return Ok(Ast::Ctx(name));
        }
        let mut args = Vec::new();
        while self.eat(&[Tk::RParen]).is_none() {
            args.push(self.expression()?);
            if !self.check(Tk::RParen) && self.eat(&[Tk::Comma]).is_none() {
                return Err(Self::err("Unexpected symbol", self.peek()));
            }
            if self.at_end() {
                return Err(Self::err("Unexpected end of expression", self.previous()));
            }
        }
        let bounds = WELL_KNOWN
            .iter()
            .find(|f| f.0 == name)
            .map(|f| (f.1, f.2))
            .or_else(|| self.allowed.funcs.iter().find(|f| f.0 == name).map(|f| (f.1, f.2)));
        let Some((lo, hi)) = bounds else {
            return Err(Self::err("Unrecognized function", &id));
        };
        if args.len() < lo {
            return Err(Self::err("Too few parameters supplied", &id));
        }
        if args.len() > hi {
            return Err(Self::err("Too many parameters supplied", &id));
        }
        if name == "case" && args.len() % 2 == 0 {
            return Err(Self::err("Even number of parameters supplied, requires an odd number of parameters", &id));
        }
        Ok(Ast::Call(name, args))
    }
    fn primary(&mut self) -> PResult {
        if let Some(t) = self.eat(&[Tk::False, Tk::True, Tk::Null, Tk::Number, Tk::Str]) {
            return Ok(Ast::Lit(match t.kind {
                Tk::False => Value::Bool(false),
                Tk::True => Value::Bool(true),
                Tk::Null => Value::Null,
                Tk::Number => num(t.num),
                _ => Value::String(t.text),
            }));
        }
        if self.eat(&[Tk::LParen]).is_some() {
            let e = self.expression()?;
            if self.at_end() {
                return Err(Self::err("Unexpected end of expression", self.previous()));
            }
            if self.eat(&[Tk::RParen]).is_none() {
                return Err(Self::err("Unexpected symbol", self.peek()));
            }
            return Ok(Ast::Group(Box::new(e)));
        }
        if self.at_end() {
            return Err(Self::err("Unexpected end of expression", self.previous()));
        }
        Err(Self::err("Unexpected symbol", self.peek()))
    }
}

/// Parses `expr` against the allowed contexts and functions; the error is the oracle's message.
pub fn parse(expr: &str, allowed: &Allowed) -> Result<Option<Ast>, String> {
    let toks = lex(expr)?;
    Parser { toks, off: 0, depth: 0, allowed }.parse()
}

/// Whether the expression calls `success`, `failure`, `cancelled`, or `always` (the oracle's
/// `walkTreeToFindStatusFunctionCalls`).
pub fn has_status_call(a: &Ast) -> bool {
    match a {
        Ast::Call(n, args) => {
            matches!(n.as_str(), "success" | "failure" | "cancelled" | "always") || args.iter().any(has_status_call)
        }
        Ast::Cmp(_, l, r) => has_status_call(l) || has_status_call(r),
        Ast::Not(e) | Ast::Group(e) => has_status_call(e),
        Ast::And(v) | Ast::Or(v) => v.iter().any(has_status_call),
        Ast::Index(e, i) => has_status_call(e) || has_status_call(i),
        Ast::Lit(_) | Ast::Ctx(_) | Ast::Star => false,
    }
}

/// A JSON number from an `f64`, as an integer when it is one, so it compares equal to the oracle's JSON.
pub fn num(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() < 9.0e15 {
        Value::from(x as i64)
    } else {
        serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
    }
}

// ---- Evaluation

/// The expression value a status function (`success()` and the like) has, which only the caller knows.
pub type StatusFn<'a> = &'a dyn Fn(&str) -> Option<bool>;

pub fn falsy(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64().is_none_or(|x| x == 0.0 || x.is_nan()),
        Value::String(s) => s.is_empty(),
        _ => false,
    }
}

/// The oracle's `coerceString`: numbers keep at most 15 decimals.
pub fn coerce_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            let x = n.as_f64().unwrap_or(0.0);
            if x == 0.0 {
                return "0".into();
            }
            js_num_string(format!("{x:.15}").parse::<f64>().unwrap_or(x))
        }
        Value::String(s) => s.clone(),
        Value::Array(_) => "Array".into(),
        Value::Object(_) => "Object".into(),
    }
}

/// A value as comparison sees it after the oracle's `coerceTypes`, which may produce NaN, which JSON cannot hold.
#[derive(Debug)]
enum Prim<'a> {
    Null,
    Bool(bool),
    Num(f64),
    Str(&'a str),
    Other(&'a Value),
}

fn prim(v: &Value) -> Prim<'_> {
    match v {
        Value::Null => Prim::Null,
        Value::Bool(b) => Prim::Bool(*b),
        Value::Number(n) => Prim::Num(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => Prim::Str(s),
        other => Prim::Other(other),
    }
}

fn coerce<'a>(l: Prim<'a>, r: Prim<'a>) -> (Prim<'a>, Prim<'a>) {
    let n = |p: &Prim<'_>| match p {
        Prim::Null => 0.0,
        Prim::Bool(b) => f64::from(u8::from(*b)),
        Prim::Num(x) => *x,
        Prim::Str(s) => js_number(s),
        Prim::Other(_) => f64::NAN,
    };
    match (&l, &r) {
        (Prim::Null, Prim::Null) | (Prim::Bool(_), Prim::Bool(_)) | (Prim::Num(_), Prim::Num(_)) => (l, r),
        (Prim::Str(_), Prim::Str(_)) | (Prim::Other(_), Prim::Other(_)) => (l, r),
        (Prim::Num(_), Prim::Str(_)) => (l, Prim::Num(n(&r))),
        (Prim::Str(_), Prim::Num(_)) => (Prim::Num(n(&l)), r),
        (Prim::Null | Prim::Bool(_), _) => coerce(Prim::Num(n(&l)), r),
        (_, Prim::Null | Prim::Bool(_)) => coerce(l, Prim::Num(n(&r))),
        _ => (l, r),
    }
}

fn upper(s: &str) -> String {
    // The oracle's `toUpperSpecial` keeps the dotless ı as it is.
    s.chars().map(|c| if c == 'ı' { c.to_string() } else { c.to_uppercase().collect() }).collect()
}

pub fn equals(l: &Value, r: &Value) -> bool {
    match coerce(prim(l), prim(r)) {
        (Prim::Null, Prim::Null) => true,
        (Prim::Num(a), Prim::Num(b)) => a == b,
        (Prim::Str(a), Prim::Str(b)) => upper(a) == upper(b),
        (Prim::Bool(a), Prim::Bool(b)) => a == b,
        (Prim::Other(a), Prim::Other(b)) => std::ptr::eq(a, b),
        _ => false,
    }
}

fn order(l: &Value, r: &Value) -> Option<std::cmp::Ordering> {
    match coerce(prim(l), prim(r)) {
        (Prim::Num(a), Prim::Num(b)) => a.partial_cmp(&b),
        (Prim::Str(a), Prim::Str(b)) => Some(upper(a).cmp(&upper(b))),
        (Prim::Bool(a), Prim::Bool(b)) => Some(a.cmp(&b)),
        _ => None,
    }
}

/// Looks up `key` case-insensitively, as context data does.
pub fn get_ci<'a>(m: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    m.get(key).or_else(|| m.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v))
}

#[derive(Debug)]
enum Ev {
    One(Value),
    /// The result of a `*` index: a filtered array.
    Many(Vec<Value>),
}

impl Ev {
    fn value(self) -> Value {
        match self {
            Ev::One(v) => v,
            Ev::Many(v) => Value::Array(v),
        }
    }
}

/// Evaluates an expression over `ctx`, whose keys are the named contexts. `status` answers the status functions and
/// any other function the definition allowed.
pub fn evaluate(a: &Ast, ctx: &Map<String, Value>, status: StatusFn<'_>) -> Result<Value, String> {
    Ok(eval(a, ctx, status)?.value())
}

fn eval(a: &Ast, ctx: &Map<String, Value>, status: StatusFn<'_>) -> Result<Ev, String> {
    let one = |v| Ok(Ev::One(v));
    match a {
        Ast::Lit(v) => one(v.clone()),
        Ast::Ctx(n) => one(get_ci(ctx, n).cloned().unwrap_or(Value::Null)),
        Ast::Group(e) => eval(e, ctx, status),
        Ast::Not(e) => one(Value::Bool(falsy(&eval(e, ctx, status)?.value()))),
        Ast::Cmp(op, l, r) => {
            let (l, r) = (eval(l, ctx, status)?.value(), eval(r, ctx, status)?.value());
            let eq = equals(&l, &r);
            let o = order(&l, &r);
            use std::cmp::Ordering::{Greater, Less};
            one(Value::Bool(match op {
                Cmp::Eq => eq,
                Cmp::Ne => !eq,
                Cmp::Gt => o == Some(Greater),
                Cmp::Ge => eq || o == Some(Greater),
                Cmp::Lt => o == Some(Less),
                Cmp::Le => eq || o == Some(Less),
            }))
        }
        Ast::And(args) | Ast::Or(args) => {
            let and = matches!(a, Ast::And(_));
            let mut last = Value::Null;
            for x in args {
                last = eval(x, ctx, status)?.value();
                if and == falsy(&last) {
                    break;
                }
            }
            one(last)
        }
        Ast::Star => one(Value::Null),
        Ast::Index(e, idx) => {
            let star = matches!(**idx, Ast::Star);
            let key = if star { Value::Null } else { eval(idx, ctx, status)?.value() };
            let base = eval(e, ctx, status)?;
            let pick = |v: &Value, out: &mut Vec<Value>| match v {
                Value::Object(m) if star => out.extend(m.values().cloned()),
                Value::Array(a) if star => out.extend(a.iter().cloned()),
                Value::Object(m) => {
                    if let Value::String(k) = &key
                        && let Some(x) = get_ci(m, k)
                    {
                        out.push(x.clone());
                    }
                }
                Value::Array(a) => {
                    if let Some(i) = index_of(&key)
                        && let Some(x) = a.get(i)
                    {
                        out.push(x.clone());
                    }
                }
                _ => {}
            };
            match base {
                Ev::Many(items) => {
                    let mut out = Vec::new();
                    for it in &items {
                        pick(it, &mut out);
                    }
                    Ok(Ev::Many(out))
                }
                Ev::One(v) => {
                    let mut out = Vec::new();
                    pick(&v, &mut out);
                    if star { Ok(Ev::Many(out)) } else { one(out.pop().unwrap_or(Value::Null)) }
                }
            }
        }
        Ast::Call(n, args) => {
            let vs: Vec<Value> = args.iter().map(|x| eval(x, ctx, status).map(Ev::value)).collect::<Result<_, _>>()?;
            one(call(n, &vs, status)?)
        }
    }
}

fn index_of(k: &Value) -> Option<usize> {
    let x = match k {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => js_number(s),
        _ => return None,
    };
    (x >= 0.0 && x.fract() == 0.0).then_some(x as usize)
}

fn primitive(v: &Value) -> bool {
    !matches!(v, Value::Array(_) | Value::Object(_))
}

fn call(n: &str, a: &[Value], status: StatusFn<'_>) -> Result<Value, String> {
    let s = |i: usize| coerce_string(&a[i]);
    Ok(match n {
        "contains" => Value::Bool(match &a[0] {
            Value::Array(items) => items.iter().any(|x| equals(&a[1], x)),
            l if primitive(l) && primitive(&a[1]) => s(0).to_lowercase().contains(&s(1).to_lowercase()),
            _ => false,
        }),
        "startswith" | "endswith" => {
            if !primitive(&a[0]) || !primitive(&a[1]) {
                return Ok(Value::Bool(false));
            }
            let (l, r) = (upper(&s(0)), upper(&s(1)));
            Value::Bool(if n == "startswith" { l.starts_with(&r) } else { l.ends_with(&r) })
        }
        "join" => match &a[0] {
            Value::Array(items) => {
                let sep = if a.len() > 1 && primitive(&a[1]) { s(1) } else { ",".into() };
                Value::String(items.iter().map(coerce_string).collect::<Vec<_>>().join(&sep))
            }
            v if primitive(v) => Value::String(s(0)),
            _ => Value::String(String::new()),
        },
        "tojson" => Value::String(serde_json::to_string_pretty(&a[0]).unwrap_or_default()),
        "fromjson" => {
            let t = s(0);
            if t.trim().is_empty() {
                return Err("empty input".into());
            }
            serde_json::from_str(&t).map_err(|_| "Error parsing JSON when evaluating fromJson".to_string())?
        }
        "format" => Value::String(format_fn(&s(0), &a[1..])?),
        "case" => {
            let mut i = 0;
            while i + 1 < a.len() {
                match a[i] {
                    Value::Bool(true) => return Ok(a[i + 1].clone()),
                    Value::Bool(false) => {}
                    _ => return Err("case predicate must evaluate to a boolean value".into()),
                }
                i += 2;
            }
            a[a.len() - 1].clone()
        }
        other => match status(other) {
            Some(b) => Value::Bool(b),
            None => return Err(format!("Unrecognized function: '{other}'")),
        },
    })
}

fn format_fn(fs: &str, args: &[Value]) -> Result<String, String> {
    let cs: Vec<char> = fs.chars().collect();
    let bad = || format!("The following format string is invalid: {fs}");
    let mut out = String::new();
    let mut i = 0;
    while i < cs.len() {
        let l = cs[i..].iter().position(|&c| c == '{').map(|p| p + i);
        let r = cs[i..].iter().position(|&c| c == '}').map(|p| p + i);
        match (l, r) {
            (Some(l), r) if r.is_none_or(|r| r > l) => {
                if cs.get(l + 1) == Some(&'{') {
                    out.extend(&cs[i..=l]);
                    i = l + 2;
                    continue;
                }
                let r = r.ok_or_else(bad)?;
                let digits: String = cs[l + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
                if r > l + 1 && !digits.is_empty() {
                    let k: usize = digits.parse().map_err(|_| bad())?;
                    let v = args.get(k).ok_or_else(|| {
                        format!("The following format string references more arguments than were supplied: {fs}")
                    })?;
                    out.extend(&cs[i..l]);
                    out.push_str(&coerce_string(v));
                    i = r + 1;
                    continue;
                }
                return Err(bad());
            }
            (_, Some(r)) => {
                if cs.get(r + 1) != Some(&'}') {
                    return Err(bad());
                }
                out.extend(&cs[i..=r]);
                i = r + 2;
            }
            _ => {
                out.extend(&cs[i..]);
                break;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(e: &str, ctx: &[&str]) -> Ast {
        parse(e, &Allowed::new(ctx)).unwrap().unwrap()
    }

    #[test]
    fn messages_match_the_oracle() {
        let a = Allowed::new(&["github"]);
        assert_eq!(parse("env.Y", &a).unwrap_err(), "Unrecognized named-value: 'env'");
        assert_eq!(parse("github.x ==", &a).unwrap_err(), "Unexpected end of expression: '=='");
        assert_eq!(parse("nope(1)", &a).unwrap_err(), "Unrecognized function: 'nope'");
        assert_eq!(parse("contains(1)", &a).unwrap_err(), "Too few parameters supplied: 'contains'");
        assert_eq!(
            parse("1 = 2", &a).unwrap_err(),
            "Unexpected symbol: '='. Located at position 3 within expression: 1 = 2"
        );
        assert!(parse("'open", &a).unwrap_err().starts_with("Unexpected symbol: ''open'"));
    }

    #[test]
    fn status_calls_and_values() {
        assert!(has_status_call(&ok("!cancelled()", &["cancelled(0,0)"])));
        assert!(!has_status_call(&ok("github.ref == 'success()'", &["github"])));
        let mut ctx = Map::new();
        ctx.insert("matrix".into(), serde_json::json!({"OS": "Linux", "v": [1, 2]}));
        let none: StatusFn<'_> = &|_| None;
        let v = |e: &str| evaluate(&ok(e, &["matrix"]), &ctx, none).unwrap();
        assert_eq!(v("matrix.os == 'LINUX'"), Value::Bool(true));
        assert_eq!(v("format('{0}-{{x}}-{1}', matrix.os, matrix.v[1])"), Value::from("Linux-{x}-2"));
        assert_eq!(v("matrix.v.*"), serde_json::json!([1, 2]));
        assert_eq!(v("matrix.*[1]"), serde_json::json!([2]));
        assert_eq!(v("matrix.v[*]"), serde_json::json!([1, 2]));
        assert_eq!(v("'1' == 1 && true == 1"), Value::Bool(true));
        assert_eq!(v("matrix.missing || 'dflt'"), Value::from("dflt"));
        assert_eq!(v("fromJSON('[1,2]')[0] < 2"), Value::Bool(true));
    }

    #[test]
    fn numbers_print_as_js_and_dotnet_do() {
        assert_eq!(js_num_string(1e21), "1e+21");
        assert_eq!(js_num_string(3.1), "3.1");
        assert_eq!(js_num_string(1.5e-7), "1.5e-7");
        assert_eq!(g15(2.0), "2");
        assert_eq!(g15(1.5), "1.5");
        assert_eq!(g15(1e20), "1E+20");
        assert_eq!(g15(0.00001), "1E-05");
        assert_eq!(g15(0.0001), "0.0001");
        assert_eq!(g15(0.1 + 0.2), "0.3");
        assert_eq!(js_number("0x1F"), 31.0);
        assert!(js_number("1_0").is_nan());
    }
}
