//! Formulas for calculated columns (the Derive step).
//!
//! A small, safe expression language: column names, literals, arithmetic,
//! comparisons, logic, and a fixed set of functions. Formulas are parsed and
//! type-checked against the input schema, compiled to DuckDB SQL, and
//! evaluated independently by the reference engine; the cross-engine tests
//! hold the two to the same results (ADR-0006).
//!
//! Defined where SQL engines differ or surprise:
//! - `/` always gives a decimal; dividing by zero gives a missing value, as
//!   do `%` by zero and any decimal result that is not finite (`sqrt` of a
//!   negative number, overflowing `power`).
//! - Integer `+ - *` overflowing 64 bits is an error, not a wrap.
//! - `&` joins text and is missing if either side is; `concat` skips
//!   missing values.
//! - Comparisons and arithmetic with a missing value are missing; `and`/`or`
//!   follow SQL's three-valued logic; `if` with a missing condition takes
//!   the else branch.
//! - `round(x, n)` rounds half away from zero at `n` decimals.
//! - Text functions count and change case per Unicode character; `trim`
//!   removes spaces only.

use crate::compile::{quote_ident, quote_literal};
use crate::table::{civil_from_days, days_from_civil, parse_date, DataType, Schema, Value, MICROS_PER_DAY};
use crate::Error;
use std::cmp::Ordering;

// ------------------------------------------------------------------ syntax

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Lit(Value),
    Col(String),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    /// `x is missing` (`negated`: `x is not missing`).
    IsMissing(Box<Expr>, bool),
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl Op {
    fn sql(self) -> &'static str {
        match self {
            Op::Add => "+",
            Op::Sub => "-",
            Op::Mul => "*",
            Op::Div => "/",
            Op::Rem => "%",
            Op::Concat => "||",
            Op::Eq => "=",
            Op::Ne => "<>",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::And => "AND",
            Op::Or => "OR",
        }
    }
    fn is_comparison(self) -> bool {
        matches!(self, Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(String),
    Str(String),
    Ident(String),
    /// `[Column name]`
    Quoted(String),
    Sym(&'static str),
}

fn tokenize(src: &str) -> Result<Vec<(Tok, usize)>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let at = i + 1;
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                if j < chars.len() && chars[j].is_ascii_digit() {
                    i = j;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            out.push((Tok::Num(chars[start..i].iter().collect()), at));
        } else if c == '"' || c == '\'' {
            let mut s = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None => return Err(format!("text starting at character {at} has no closing {c}")),
                    Some(&q) if q == c && chars.get(i + 1) == Some(&c) => {
                        s.push(c);
                        i += 2;
                    }
                    Some(&q) if q == c => {
                        i += 1;
                        break;
                    }
                    Some(&ch) => {
                        s.push(ch);
                        i += 1;
                    }
                }
            }
            out.push((Tok::Str(s), at));
        } else if c == '[' {
            let end = chars[i + 1..]
                .iter()
                .position(|&ch| ch == ']')
                .ok_or_else(|| format!("column name starting at character {at} has no closing ]"))?;
            out.push((Tok::Quoted(chars[i + 1..i + 1 + end].iter().collect()), at));
            i += end + 2;
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push((Tok::Ident(chars[start..i].iter().collect()), at));
        } else {
            let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
            let sym = ["<=", ">=", "<>", "!=", "=="].into_iter().find(|s| *s == two);
            if let Some(s) = sym {
                out.push((Tok::Sym(s), at));
                i += 2;
            } else {
                let one = ["+", "-", "*", "/", "%", "&", "=", "<", ">", "(", ")", ","]
                    .into_iter()
                    .find(|s| s.starts_with(c))
                    .ok_or_else(|| format!("unexpected `{c}` at character {at}"))?;
                out.push((Tok::Sym(one), at));
                i += 1;
            }
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
    len: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|(t, _)| t)
    }
    fn at(&self) -> usize {
        self.toks.get(self.pos).map(|(_, a)| *a).unwrap_or(self.len + 1)
    }
    fn keyword(&self, k: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(w)) if w.eq_ignore_ascii_case(k))
    }
    fn sym(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(x)) if *x == s)
    }
    fn expect_sym(&mut self, s: &str) -> Result<(), String> {
        if self.sym(s) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected `{s}` at character {}", self.at()))
        }
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut e = self.and()?;
        while self.keyword("or") {
            self.pos += 1;
            e = Expr::Bin(Op::Or, Box::new(e), Box::new(self.and()?));
        }
        Ok(e)
    }
    fn and(&mut self) -> Result<Expr, String> {
        let mut e = self.not()?;
        while self.keyword("and") {
            self.pos += 1;
            e = Expr::Bin(Op::And, Box::new(e), Box::new(self.not()?));
        }
        Ok(e)
    }
    fn not(&mut self) -> Result<Expr, String> {
        if self.keyword("not") {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.cmp()
    }
    fn cmp(&mut self) -> Result<Expr, String> {
        let left = self.add()?;
        if self.keyword("is") {
            self.pos += 1;
            let negated = self.keyword("not");
            if negated {
                self.pos += 1;
            }
            if !self.keyword("missing") {
                return Err(format!("expected `missing` after `is` at character {}", self.at()));
            }
            self.pos += 1;
            return Ok(Expr::IsMissing(Box::new(left), negated));
        }
        let op = match self.peek() {
            Some(Tok::Sym("=" | "==")) => Op::Eq,
            Some(Tok::Sym("<>" | "!=")) => Op::Ne,
            Some(Tok::Sym("<")) => Op::Lt,
            Some(Tok::Sym("<=")) => Op::Le,
            Some(Tok::Sym(">")) => Op::Gt,
            Some(Tok::Sym(">=")) => Op::Ge,
            _ => return Ok(left),
        };
        self.pos += 1;
        Ok(Expr::Bin(op, Box::new(left), Box::new(self.add()?)))
    }
    fn add(&mut self) -> Result<Expr, String> {
        let mut e = self.mul()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Sym("+")) => Op::Add,
                Some(Tok::Sym("-")) => Op::Sub,
                Some(Tok::Sym("&")) => Op::Concat,
                _ => return Ok(e),
            };
            self.pos += 1;
            e = Expr::Bin(op, Box::new(e), Box::new(self.mul()?));
        }
    }
    fn mul(&mut self) -> Result<Expr, String> {
        let mut e = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Sym("*")) => Op::Mul,
                Some(Tok::Sym("/")) => Op::Div,
                Some(Tok::Sym("%")) => Op::Rem,
                _ => return Ok(e),
            };
            self.pos += 1;
            e = Expr::Bin(op, Box::new(e), Box::new(self.unary()?));
        }
    }
    fn unary(&mut self) -> Result<Expr, String> {
        if self.sym("-") {
            self.pos += 1;
            return Ok(match self.unary()? {
                Expr::Lit(Value::Int(i)) => Expr::Lit(Value::Int(-i)),
                Expr::Lit(Value::Float(f)) => Expr::Lit(Value::Float(-f)),
                e => Expr::Neg(Box::new(e)),
            });
        }
        self.atom()
    }
    fn atom(&mut self) -> Result<Expr, String> {
        let at = self.at();
        let tok = self.peek().cloned().ok_or_else(|| "the formula ends too early".to_string())?;
        self.pos += 1;
        Ok(match tok {
            Tok::Num(n) => {
                if let Ok(i) = n.parse::<i64>() {
                    Expr::Lit(Value::Int(i))
                } else {
                    let f: f64 = n.parse().map_err(|_| format!("`{n}` at character {at} is not a number"))?;
                    Expr::Lit(Value::Float(f))
                }
            }
            Tok::Str(s) => Expr::Lit(Value::Text(s)),
            Tok::Quoted(c) => Expr::Col(c),
            Tok::Sym("(") => {
                let e = self.or()?;
                self.expect_sym(")")?;
                e
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("true") => Expr::Lit(Value::Bool(true)),
            Tok::Ident(w) if w.eq_ignore_ascii_case("false") => Expr::Lit(Value::Bool(false)),
            Tok::Ident(w) if w.eq_ignore_ascii_case("missing") => Expr::Lit(Value::Null),
            Tok::Ident(w) if self.sym("(") => {
                self.pos += 1;
                let mut args = Vec::new();
                if !self.sym(")") {
                    loop {
                        args.push(self.or()?);
                        if self.sym(",") {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                }
                self.expect_sym(")")?;
                Expr::Call(w.to_ascii_lowercase(), args)
            }
            Tok::Ident(w) => Expr::Col(w),
            Tok::Sym(s) => return Err(format!("unexpected `{s}` at character {at}")),
        })
    }
}

fn describe_tok(t: &Tok) -> String {
    match t {
        Tok::Num(n) => format!("`{n}`"),
        Tok::Str(x) => format!("\"{x}\""),
        Tok::Ident(w) => format!("`{w}`"),
        Tok::Quoted(c) => format!("[{c}]"),
        Tok::Sym(x) => format!("`{x}`"),
    }
}

pub fn parse(src: &str) -> Result<Expr, String> {
    if src.trim().is_empty() {
        return Err("the formula is empty".into());
    }
    let len = src.chars().count();
    let mut p = Parser { toks: tokenize(src)?, pos: 0, len };
    let e = p.or()?;
    if p.pos < p.toks.len() {
        let at = p.at();
        // Two words in a row are usually a column name with a space in it.
        return Err(match (&p.toks[p.pos - 1].0, &p.toks[p.pos].0) {
            (Tok::Ident(a), Tok::Ident(b)) => {
                format!("unexpected `{b}` at character {at}; write names with spaces as [{a} {b}]")
            }
            (_, t) => format!("unexpected {} at character {at}", describe_tok(t)),
        });
    }
    Ok(e)
}

// ------------------------------------------------------------------- types

/// A formula's type: a column type, or not yet known (`missing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Known(DataType),
    Unknown,
}

use DataType::*;
use Ty::Known as K;

fn unify(a: Ty, b: Ty, what: &str) -> Result<Ty, String> {
    Ok(match (a, b) {
        (Ty::Unknown, t) | (t, Ty::Unknown) => t,
        (K(x), K(y)) if x == y => K(x),
        (K(Int | Float), K(Int | Float)) => K(Float),
        (K(Date | Timestamp), K(Date | Timestamp)) => K(Timestamp),
        (K(x), K(y)) => return Err(format!("{what} mixes {} and {}", word(x), word(y))),
    })
}

fn word(t: DataType) -> &'static str {
    match t {
        Bool => "true/false",
        Int => "a whole number",
        Float => "a decimal",
        Text => "text",
        Date => "a date",
        Timestamp => "a date and time",
    }
}

fn numeric(t: Ty) -> bool {
    matches!(t, Ty::Unknown | K(Int | Float))
}

fn temporal(t: Ty) -> bool {
    matches!(t, Ty::Unknown | K(Date | Timestamp))
}

fn is(t: Ty, d: DataType) -> bool {
    matches!(t, Ty::Unknown) || t == K(d)
}

const UNITS: [&str; 7] = ["year", "quarter", "month", "week", "day", "hour", "minute"];

/// Function name → (min args, max args).
const FUNCTIONS: &[(&str, usize, usize)] = &[
    ("if", 3, 3),
    ("coalesce", 2, 16),
    ("round", 1, 2),
    ("floor", 1, 1),
    ("ceil", 1, 1),
    ("abs", 1, 1),
    ("bucket", 2, 2),
    ("sqrt", 1, 1),
    ("power", 2, 2),
    ("upper", 1, 1),
    ("lower", 1, 1),
    ("trim", 1, 1),
    ("length", 1, 1),
    ("contains", 2, 2),
    ("starts_with", 2, 2),
    ("ends_with", 2, 2),
    ("replace", 3, 3),
    ("substr", 3, 3),
    ("concat", 1, 16),
    ("text", 1, 1),
    ("format", 2, 2),
    ("number", 1, 1),
    ("integer", 1, 1),
    ("date", 1, 1),
    ("year", 1, 1),
    ("quarter", 1, 1),
    ("month", 1, 1),
    ("day", 1, 1),
    ("weekday", 1, 1),
    ("hour", 1, 1),
    ("minute", 1, 1),
    ("date_trunc", 2, 2),
    ("days_between", 2, 2),
    ("add_days", 2, 2),
];

/// The functions formulas can use, for the editor's reference list.
pub fn function_names() -> Vec<&'static str> {
    FUNCTIONS.iter().map(|f| f.0).collect()
}

fn lit_int(e: &Expr) -> Option<i64> {
    match e {
        Expr::Lit(Value::Int(i)) => Some(*i),
        _ => None,
    }
}

fn lit_text(e: &Expr) -> Option<&str> {
    match e {
        Expr::Lit(Value::Text(s)) => Some(s),
        _ => None,
    }
}

/// Type-check a formula against the columns it can see.
pub fn type_of(e: &Expr, schema: &Schema) -> Result<Ty, String> {
    let t = |e: &Expr| type_of(e, schema);
    Ok(match e {
        Expr::Lit(Value::Null) => Ty::Unknown,
        Expr::Lit(v) => K(v.dtype().unwrap()),
        Expr::Col(c) => K(schema
            .field(c)
            .ok_or_else(|| {
                let names: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
                let hint = if c.contains(' ') { "" } else { " (write names with spaces as [Column name])" };
                format!("no column `{c}`{hint}; columns: {}", names.join(", "))
            })?
            .dtype),
        Expr::Neg(x) => {
            let tx = t(x)?;
            if !numeric(tx) {
                return Err(format!("`-` needs a number, not {}", word_ty(tx)));
            }
            tx
        }
        Expr::Not(x) => {
            let tx = t(x)?;
            if !is(tx, Bool) {
                return Err(format!("`not` needs true/false, not {}", word_ty(tx)));
            }
            K(Bool)
        }
        Expr::IsMissing(x, _) => {
            t(x)?;
            K(Bool)
        }
        Expr::Bin(op, a, b) => {
            let (ta, tb) = (t(a)?, t(b)?);
            match op {
                Op::Add | Op::Sub | Op::Mul | Op::Rem => {
                    if !numeric(ta) || !numeric(tb) {
                        let hint = if matches!(op, Op::Add) && (is(ta, Text) || is(tb, Text)) {
                            " (join text with &)"
                        } else if temporal(ta) && temporal(tb) {
                            " (use days_between or add_days for dates)"
                        } else {
                            ""
                        };
                        return Err(format!("`{}` needs numbers{hint}", op.sql()));
                    }
                    match (ta, tb) {
                        (K(Int), K(Int)) | (K(Int), Ty::Unknown) | (Ty::Unknown, K(Int)) => K(Int),
                        (Ty::Unknown, Ty::Unknown) => K(Int),
                        _ => K(Float),
                    }
                }
                Op::Div => {
                    if !numeric(ta) || !numeric(tb) {
                        return Err("`/` needs numbers".into());
                    }
                    K(Float)
                }
                Op::Concat => {
                    if !is(ta, Text) || !is(tb, Text) {
                        return Err("`&` joins text; turn other values into text with text(...) or format(...)".into());
                    }
                    K(Text)
                }
                Op::And | Op::Or => {
                    if !is(ta, Bool) || !is(tb, Bool) {
                        return Err(format!("`{}` needs true/false on both sides", op.sql().to_lowercase()));
                    }
                    K(Bool)
                }
                _ => {
                    if matches!(**a, Expr::Lit(Value::Null)) || matches!(**b, Expr::Lit(Value::Null)) {
                        return Err("a comparison with `missing` is never true; use `is missing`".into());
                    }
                    unify(ta, tb, "the comparison")?;
                    K(Bool)
                }
            }
        }
        Expr::Call(name, args) => {
            let (_, min, max) = *FUNCTIONS.iter().find(|f| f.0 == name).ok_or_else(|| {
                format!("unknown function `{name}`; available: {}", function_names().join(", "))
            })?;
            if args.len() < min || args.len() > max {
                let n = if min == max { format!("{min}") } else { format!("{min} to {max}") };
                return Err(format!("{name}(...) takes {n} values, not {}", args.len()));
            }
            let ts: Vec<Ty> = args.iter().map(t).collect::<Result<_, _>>()?;
            let need = |ok: bool, what: &str| if ok { Ok(()) } else { Err(format!("{name}(...) needs {what}")) };
            match name.as_str() {
                "if" => {
                    need(is(ts[0], Bool), "a true/false condition first")?;
                    unify(ts[1], ts[2], "if(...)")?
                }
                "coalesce" => ts.iter().try_fold(Ty::Unknown, |acc, x| unify(acc, *x, "coalesce(...)"))?,
                "round" => {
                    need(numeric(ts[0]), "a number")?;
                    if args.len() == 2 {
                        need(lit_int(&args[1]).is_some_and(|n| (0..=10).contains(&n)), "a number of decimals from 0 to 10")?;
                    }
                    ts[0]
                }
                "floor" | "ceil" | "abs" => {
                    need(numeric(ts[0]), "a number")?;
                    ts[0]
                }
                "bucket" => {
                    need(numeric(ts[0]) && numeric(ts[1]), "a number and a bucket width")?;
                    if ts[0] == K(Float) || ts[1] == K(Float) {
                        K(Float)
                    } else {
                        K(Int)
                    }
                }
                "sqrt" => {
                    need(numeric(ts[0]), "a number")?;
                    K(Float)
                }
                "power" => {
                    need(numeric(ts[0]) && numeric(ts[1]), "numbers")?;
                    K(Float)
                }
                "upper" | "lower" | "trim" => {
                    need(is(ts[0], Text), "text")?;
                    K(Text)
                }
                "length" => {
                    need(is(ts[0], Text), "text")?;
                    K(Int)
                }
                "contains" | "starts_with" | "ends_with" => {
                    need(is(ts[0], Text) && is(ts[1], Text), "text")?;
                    K(Bool)
                }
                "replace" => {
                    need(ts.iter().all(|x| is(*x, Text)), "text")?;
                    K(Text)
                }
                "substr" => {
                    need(is(ts[0], Text) && is(ts[1], Int) && is(ts[2], Int), "text, a start position and a length")?;
                    K(Text)
                }
                "concat" => {
                    need(ts.iter().all(|x| is(*x, Text)), "text (use text(...) for other values)")?;
                    K(Text)
                }
                "text" => {
                    need(!matches!(ts[0], K(Float)), "a value that is not a decimal (use format(x, decimals))")?;
                    K(Text)
                }
                "format" => {
                    need(numeric(ts[0]), "a number")?;
                    need(lit_int(&args[1]).is_some_and(|n| (0..=10).contains(&n)), "a number of decimals from 0 to 10")?;
                    K(Text)
                }
                "number" => {
                    need(matches!(ts[0], Ty::Unknown | K(Int | Float | Text)), "a number or text")?;
                    K(Float)
                }
                "integer" => {
                    need(matches!(ts[0], Ty::Unknown | K(Int | Float | Text)), "a number or text")?;
                    K(Int)
                }
                "date" => {
                    need(matches!(ts[0], Ty::Unknown | K(Text | Date | Timestamp)), "text or a date")?;
                    K(Date)
                }
                "year" | "quarter" | "month" | "day" | "weekday" | "hour" | "minute" => {
                    need(temporal(ts[0]), "a date")?;
                    K(Int)
                }
                "date_trunc" => {
                    let unit = lit_text(&args[0]).map(str::to_ascii_lowercase);
                    need(
                        unit.as_deref().is_some_and(|u| UNITS.contains(&u)),
                        "a unit in quotes first: \"year\", \"quarter\", \"month\", \"week\", \"day\", \"hour\" or \"minute\"",
                    )?;
                    need(temporal(ts[1]), "a date second")?;
                    if matches!(ts[1], K(Date)) && matches!(unit.as_deref(), Some("hour" | "minute")) {
                        return Err(format!("date_trunc(...): a date has no {}", unit.unwrap()));
                    }
                    if ts[1] == Ty::Unknown {
                        K(Date)
                    } else {
                        ts[1]
                    }
                }
                "days_between" => {
                    need(temporal(ts[0]) && temporal(ts[1]), "two dates")?;
                    K(Int)
                }
                "add_days" => {
                    need(temporal(ts[0]) && is(ts[1], Int), "a date and a whole number of days")?;
                    if ts[0] == Ty::Unknown {
                        K(Date)
                    } else {
                        ts[0]
                    }
                }
                _ => unreachable!("every listed function is typed"),
            }
        }
    })
}

fn word_ty(t: Ty) -> &'static str {
    match t {
        K(d) => word(d),
        Ty::Unknown => "missing",
    }
}

/// The column type a formula produces (`missing` alone becomes text).
pub fn result_type(e: &Expr, schema: &Schema) -> Result<DataType, String> {
    Ok(match type_of(e, schema)? {
        K(d) => d,
        Ty::Unknown => Text,
    })
}

// --------------------------------------------------------------------- SQL

fn sql_type(d: DataType) -> &'static str {
    d.name()
}

/// Cast `s` (of type `from`) to `to` when they differ.
fn cast(s: String, from: Ty, to: Ty) -> String {
    match (from, to) {
        (a, b) if a == b => s,
        (_, K(d)) => format!("CAST({s} AS {})", sql_type(d)),
        (_, Ty::Unknown) => s,
    }
}

/// Missing unless finite: decimal results never hold inf or NaN.
fn finite(s: &str) -> String {
    format!("(CASE WHEN isfinite({s}) THEN {s} END)")
}

/// Compile a type-checked formula to DuckDB SQL whose result has the type
/// [`result_type`] reports.
pub fn to_sql(e: &Expr, schema: &Schema) -> Result<String, String> {
    let ty = type_of(e, schema)?;
    let target = K(result_type(e, schema)?);
    Ok(cast(sql(e, schema)?, ty, target))
}

fn sql(e: &Expr, schema: &Schema) -> Result<String, String> {
    let s = |e: &Expr| sql(e, schema);
    let t = |e: &Expr| type_of(e, schema);
    let as_ty = |e: &Expr, to: Ty| -> Result<String, String> { Ok(cast(s(e)?, t(e)?, to)) };
    let dbl = |e: &Expr| as_ty(e, K(Float));
    Ok(match e {
        Expr::Lit(Value::Int(i)) => format!("CAST({i} AS BIGINT)"),
        Expr::Lit(v) => quote_literal(v),
        Expr::Col(c) => quote_ident(c),
        Expr::Neg(x) => match t(x)? {
            K(Float) => format!("(- {})", s(x)?),
            _ => format!("(CAST(0 AS BIGINT) - {})", as_ty(x, K(Int))?),
        },
        Expr::Not(x) => format!("(NOT {})", s(x)?),
        Expr::IsMissing(x, false) => format!("({} IS NULL)", s(x)?),
        Expr::IsMissing(x, true) => format!("({} IS NOT NULL)", s(x)?),
        Expr::Bin(op, a, b) => {
            let out = type_of(e, schema)?;
            match op {
                Op::Add | Op::Sub | Op::Mul if out == K(Int) => {
                    format!("({} {} {})", as_ty(a, K(Int))?, op.sql(), as_ty(b, K(Int))?)
                }
                Op::Add | Op::Sub | Op::Mul => finite(&format!("({} {} {})", dbl(a)?, op.sql(), dbl(b)?)),
                Op::Div => finite(&format!("({} / NULLIF({}, 0))", dbl(a)?, dbl(b)?)),
                Op::Rem if out == K(Int) => format!("({} % NULLIF({}, 0))", as_ty(a, K(Int))?, as_ty(b, K(Int))?),
                // DuckDB's fmod() follows the divisor's sign; `%` on doubles,
                // like integer `%` (and Rust), follows the dividend's.
                Op::Rem => finite(&format!("({} % NULLIF({}, 0))", dbl(a)?, dbl(b)?)),
                Op::Concat => format!("({} || {})", as_ty(a, K(Text))?, as_ty(b, K(Text))?),
                Op::And | Op::Or => format!("({} {} {})", as_ty(a, K(Bool))?, op.sql(), as_ty(b, K(Bool))?),
                _ => {
                    let common = unify(t(a)?, t(b)?, "")?;
                    format!("({} {} {})", as_ty(a, common)?, op.sql(), as_ty(b, common)?)
                }
            }
        }
        Expr::Call(name, args) => {
            let out = type_of(e, schema)?;
            let a = |i: usize| s(&args[i]);
            match name.as_str() {
                "if" => format!(
                    "(CASE WHEN {} THEN {} ELSE {} END)",
                    as_ty(&args[0], K(Bool))?,
                    as_ty(&args[1], out)?,
                    as_ty(&args[2], out)?
                ),
                "coalesce" => format!(
                    "COALESCE({})",
                    args.iter().map(|x| as_ty(x, out)).collect::<Result<Vec<_>, _>>()?.join(", ")
                ),
                "round" | "floor" | "ceil" if out == K(Int) => a(0)?,
                "round" if args.len() == 2 => {
                    let n = lit_int(&args[1]).unwrap();
                    finite(&format!("(round({} * 1e{n}) / 1e{n})", dbl(&args[0])?))
                }
                "round" => format!("round({})", dbl(&args[0])?),
                "floor" => format!("floor({})", dbl(&args[0])?),
                "ceil" => format!("ceil({})", dbl(&args[0])?),
                "abs" => format!("abs({})", as_ty(&args[0], out)?),
                // The start of the width-wide bucket holding x; missing unless
                // the width is positive.
                "bucket" if out == K(Int) => {
                    let (x, w) = (as_ty(&args[0], K(Int))?, as_ty(&args[1], K(Int))?);
                    format!("(CASE WHEN {w} > 0 THEN {x} - ((({x} % {w}) + {w}) % {w}) END)")
                }
                "bucket" => {
                    let (x, w) = (dbl(&args[0])?, dbl(&args[1])?);
                    finite(&format!("(CASE WHEN {w} > 0 THEN floor({x} / {w}) * {w} END)"))
                }
                "sqrt" => finite(&format!("sqrt({})", dbl(&args[0])?)),
                "power" => finite(&format!("power({}, {})", dbl(&args[0])?, dbl(&args[1])?)),
                "upper" | "lower" | "length" => format!("{name}({})", as_ty(&args[0], K(Text))?),
                "trim" => format!("trim({}, ' ')", as_ty(&args[0], K(Text))?),
                "contains" => format!("contains({}, {})", as_ty(&args[0], K(Text))?, as_ty(&args[1], K(Text))?),
                "starts_with" => format!("starts_with({}, {})", as_ty(&args[0], K(Text))?, as_ty(&args[1], K(Text))?),
                "ends_with" => format!("suffix({}, {})", as_ty(&args[0], K(Text))?, as_ty(&args[1], K(Text))?),
                "replace" => {
                    let (x, f, r) = (as_ty(&args[0], K(Text))?, as_ty(&args[1], K(Text))?, as_ty(&args[2], K(Text))?);
                    format!("(CASE WHEN {f} = '' THEN {x} ELSE replace({x}, {f}, {r}) END)")
                }
                "substr" => format!(
                    "(CASE WHEN {len} < 0 THEN '' ELSE substring({}, greatest({}, 1), {len}) END)",
                    as_ty(&args[0], K(Text))?,
                    as_ty(&args[1], K(Int))?,
                    len = as_ty(&args[2], K(Int))?
                ),
                "concat" => format!(
                    "concat({})",
                    args.iter().map(|x| as_ty(x, K(Text))).collect::<Result<Vec<_>, _>>()?.join(", ")
                ),
                "text" => format!("CAST({} AS VARCHAR)", a(0)?),
                "format" => format!("printf('%.{}f', {})", lit_int(&args[1]).unwrap(), dbl(&args[0])?),
                "number" => match t(&args[0])? {
                    K(Text) => finite(&format!("TRY_CAST({} AS DOUBLE)", a(0)?)),
                    _ => dbl(&args[0])?,
                },
                // Casting a double rounds half to even; round() first so
                // integer() agrees with round() (half away from zero).
                "integer" => match t(&args[0])? {
                    K(Float) => format!("TRY_CAST(round({}) AS BIGINT)", a(0)?),
                    _ => format!("TRY_CAST({} AS BIGINT)", a(0)?),
                },
                "date" => match t(&args[0])? {
                    K(Text) => format!("TRY_CAST({} AS DATE)", a(0)?),
                    _ => format!("CAST({} AS DATE)", a(0)?),
                },
                "year" | "quarter" | "month" | "day" | "hour" | "minute" => {
                    format!("CAST({name}({}) AS BIGINT)", as_ty(&args[0], date_or_ts(t(&args[0])?))?)
                }
                "weekday" => format!("CAST(isodow({}) AS BIGINT)", as_ty(&args[0], date_or_ts(t(&args[0])?))?),
                "date_trunc" => {
                    let unit = lit_text(&args[0]).unwrap().to_ascii_lowercase();
                    format!(
                        "CAST(date_trunc('{unit}', {}) AS {})",
                        as_ty(&args[1], date_or_ts(t(&args[1])?))?,
                        sql_type(match out {
                            K(d) => d,
                            Ty::Unknown => Date,
                        })
                    )
                }
                "days_between" => format!(
                    "date_diff('day', CAST({} AS DATE), CAST({} AS DATE))",
                    as_ty(&args[0], date_or_ts(t(&args[0])?))?,
                    as_ty(&args[1], date_or_ts(t(&args[1])?))?
                ),
                "add_days" => match out {
                    K(Timestamp) => format!("({} + to_days(CAST({} AS INTEGER)))", a(0)?, as_ty(&args[1], K(Int))?),
                    _ => format!("({} + CAST({} AS INTEGER))", as_ty(&args[0], K(Date))?, as_ty(&args[1], K(Int))?),
                },
                _ => unreachable!("every listed function compiles"),
            }
        }
    })
}

fn date_or_ts(t: Ty) -> Ty {
    match t {
        Ty::Unknown => K(Date),
        t => t,
    }
}

// ------------------------------------------------------------- evaluation

fn overflow(what: &str) -> Error {
    Error::Data(format!("a formula overflowed 64-bit integers in {what}"))
}

fn fin(x: f64) -> Value {
    if x.is_finite() {
        Value::Float(x)
    } else {
        Value::Null
    }
}

fn to_f64(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn as_text(v: &Value) -> Option<&str> {
    match v {
        Value::Text(s) => Some(s),
        _ => None,
    }
}

/// Cast a value to the type a branch of `if`/`coalesce` produces.
fn widen(v: Value, to: Ty) -> Value {
    match (to, v) {
        (K(Float), Value::Int(i)) => Value::Float(i as f64),
        (K(Timestamp), Value::Date(d)) => Value::Timestamp(d as i64 * MICROS_PER_DAY),
        (_, v) => v,
    }
}

fn days_of(v: &Value) -> Option<i64> {
    match v {
        Value::Date(d) => Some(*d as i64),
        Value::Timestamp(t) => Some(t.div_euclid(MICROS_PER_DAY)),
        _ => None,
    }
}

/// Parse text as a decimal the way `TRY_CAST(... AS DOUBLE)` does for
/// ordinary numbers: surrounding spaces allowed, decimal notation.
fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim_matches(' ');
    if s.is_empty() || s.contains('_') {
        return None;
    }
    s.parse::<f64>().ok()
}

/// Round half away from zero to an integer, if it fits.
fn round_to_i64(x: f64) -> Option<i64> {
    let r = x.round();
    (-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&r).then_some(r as i64)
}

fn trunc_days(days: i64, unit: &str) -> i64 {
    let (y, m, _) = civil_from_days(days);
    match unit {
        "year" => days_from_civil(y, 1, 1),
        "quarter" => days_from_civil(y, (m - 1) / 3 * 3 + 1, 1),
        "month" => days_from_civil(y, m, 1),
        // ISO weeks start on Monday; 1970-01-01 was a Thursday.
        "week" => days - (days + 3).rem_euclid(7),
        _ => days,
    }
}

/// Evaluate a type-checked formula on one row.
pub fn eval(e: &Expr, schema: &Schema, row: &[Value]) -> Result<Value, Error> {
    let target = K(result_type(e, schema).map_err(Error::Validation)?);
    Ok(widen(ev(e, schema, row)?, target))
}

fn ev(e: &Expr, schema: &Schema, row: &[Value]) -> Result<Value, Error> {
    let v = |e: &Expr| ev(e, schema, row);
    let ty = |e: &Expr| type_of(e, schema).map_err(Error::Validation);
    Ok(match e {
        Expr::Lit(x) => x.clone(),
        Expr::Col(c) => row[schema.index_of(c).unwrap()].clone(),
        Expr::Neg(x) => match v(x)? {
            Value::Int(i) => Value::Int(i.checked_neg().ok_or_else(|| overflow("-"))?),
            Value::Float(f) => Value::Float(-f),
            _ => Value::Null,
        },
        Expr::Not(x) => match v(x)? {
            Value::Bool(b) => Value::Bool(!b),
            _ => Value::Null,
        },
        Expr::IsMissing(x, negated) => Value::Bool(v(x)?.is_null() != *negated),
        Expr::Bin(op, a, b) => {
            let out = ty(e)?;
            let (x, y) = (v(a)?, v(b)?);
            match op {
                Op::And => match (x, y) {
                    (Value::Bool(false), _) | (_, Value::Bool(false)) => Value::Bool(false),
                    (Value::Bool(true), Value::Bool(true)) => Value::Bool(true),
                    _ => Value::Null,
                },
                Op::Or => match (x, y) {
                    (Value::Bool(true), _) | (_, Value::Bool(true)) => Value::Bool(true),
                    (Value::Bool(false), Value::Bool(false)) => Value::Bool(false),
                    _ => Value::Null,
                },
                _ if x.is_null() || y.is_null() => Value::Null,
                Op::Add | Op::Sub | Op::Mul if out == K(Int) => {
                    let (Value::Int(p), Value::Int(q)) = (x, y) else { unreachable!() };
                    let r = match op {
                        Op::Add => p.checked_add(q),
                        Op::Sub => p.checked_sub(q),
                        _ => p.checked_mul(q),
                    };
                    Value::Int(r.ok_or_else(|| overflow(op.sql()))?)
                }
                Op::Add | Op::Sub | Op::Mul => {
                    let (p, q) = (to_f64(&x).unwrap(), to_f64(&y).unwrap());
                    fin(match op {
                        Op::Add => p + q,
                        Op::Sub => p - q,
                        _ => p * q,
                    })
                }
                Op::Div => {
                    let (p, q) = (to_f64(&x).unwrap(), to_f64(&y).unwrap());
                    if q == 0.0 {
                        Value::Null
                    } else {
                        fin(p / q)
                    }
                }
                Op::Rem if out == K(Int) => {
                    let (Value::Int(p), Value::Int(q)) = (x, y) else { unreachable!() };
                    if q == 0 {
                        Value::Null
                    } else {
                        Value::Int(p.wrapping_rem(q))
                    }
                }
                Op::Rem => {
                    let (p, q) = (to_f64(&x).unwrap(), to_f64(&y).unwrap());
                    if q == 0.0 {
                        Value::Null
                    } else {
                        fin(p % q)
                    }
                }
                Op::Concat => Value::Text(format!("{}{}", as_text(&x).unwrap(), as_text(&y).unwrap())),
                _ => {
                    debug_assert!(op.is_comparison());
                    match x.sql_cmp(&y) {
                        None => Value::Null,
                        Some(o) => Value::Bool(match op {
                            Op::Eq => o == Ordering::Equal,
                            Op::Ne => o != Ordering::Equal,
                            Op::Lt => o == Ordering::Less,
                            Op::Le => o != Ordering::Greater,
                            Op::Gt => o == Ordering::Greater,
                            _ => o != Ordering::Less,
                        }),
                    }
                }
            }
        }
        Expr::Call(name, args) => {
            let out = ty(e)?;
            let a = |i: usize| v(&args[i]);
            match name.as_str() {
                "if" => match a(0)? {
                    Value::Bool(true) => widen(a(1)?, out),
                    _ => widen(a(2)?, out),
                },
                "coalesce" => {
                    for x in args {
                        let val = v(x)?;
                        if !val.is_null() {
                            return Ok(widen(val, out));
                        }
                    }
                    Value::Null
                }
                _ => {
                    let vals: Vec<Value> = args.iter().map(v).collect::<Result<_, _>>()?;
                    // concat skips missing values; every other function is missing when an input is.
                    if name == "concat" {
                        return Ok(Value::Text(vals.iter().filter_map(as_text).collect()));
                    }
                    if vals.iter().any(Value::is_null) {
                        return Ok(Value::Null);
                    }
                    call(name, &vals, out, args)?
                }
            }
        }
    })
}

fn call(name: &str, vals: &[Value], out: Ty, args: &[Expr]) -> Result<Value, Error> {
    let f = |i: usize| to_f64(&vals[i]).unwrap();
    let s = |i: usize| as_text(&vals[i]).unwrap();
    let int = |i: usize| match vals[i] {
        Value::Int(n) => n,
        _ => unreachable!(),
    };
    Ok(match name {
        "round" | "floor" | "ceil" if out == K(Int) => vals[0].clone(),
        "round" if args.len() == 2 => {
            let m = 10f64.powi(lit_int(&args[1]).unwrap() as i32);
            fin((f(0) * m).round() / m)
        }
        "round" => Value::Float(f(0).round()),
        "floor" => Value::Float(f(0).floor()),
        "ceil" => Value::Float(f(0).ceil()),
        "abs" => match vals[0] {
            Value::Int(i) => Value::Int(i.checked_abs().ok_or_else(|| overflow("abs"))?),
            _ => Value::Float(f(0).abs()),
        },
        "bucket" => match (&vals[0], &vals[1]) {
            (Value::Int(x), Value::Int(w)) if out == K(Int) => {
                if *w <= 0 {
                    Value::Null
                } else {
                    Value::Int(x.checked_sub(x.rem_euclid(*w)).ok_or_else(|| overflow("bucket"))?)
                }
            }
            _ if f(1) > 0.0 => fin((f(0) / f(1)).floor() * f(1)),
            _ => Value::Null,
        },
        "sqrt" => fin(f(0).sqrt()),
        "power" => fin(f(0).powf(f(1))),
        "upper" => Value::Text(s(0).to_uppercase()),
        "lower" => Value::Text(s(0).to_lowercase()),
        "trim" => Value::Text(s(0).trim_matches(' ').to_string()),
        "length" => Value::Int(s(0).chars().count() as i64),
        "contains" => Value::Bool(s(0).contains(s(1))),
        "starts_with" => Value::Bool(s(0).starts_with(s(1))),
        "ends_with" => Value::Bool(s(0).ends_with(s(1))),
        "replace" if s(1).is_empty() => vals[0].clone(),
        "replace" => Value::Text(s(0).replace(s(1), s(2))),
        "substr" => {
            let (start, len) = (int(1).max(1), int(2));
            if len < 0 {
                Value::Text(String::new())
            } else {
                Value::Text(s(0).chars().skip((start - 1) as usize).take(len as usize).collect())
            }
        }
        "text" => Value::Text(vals[0].to_string()),
        "format" => Value::Text(format!("{:.*}", lit_int(&args[1]).unwrap() as usize, f(0))),
        "number" => match &vals[0] {
            Value::Text(t) => parse_number(t).map(fin).unwrap_or(Value::Null),
            v => Value::Float(to_f64(v).unwrap()),
        },
        "integer" => match &vals[0] {
            Value::Int(i) => Value::Int(*i),
            Value::Float(x) => round_to_i64(*x).map(Value::Int).unwrap_or(Value::Null),
            Value::Text(t) => {
                let t = t.trim_matches(' ');
                match t.parse::<i64>() {
                    Ok(i) => Value::Int(i),
                    Err(_) => parse_number(t).and_then(round_to_i64).map(Value::Int).unwrap_or(Value::Null),
                }
            }
            _ => Value::Null,
        },
        "date" => match &vals[0] {
            Value::Text(t) => parse_date(t).map(Value::Date).unwrap_or(Value::Null),
            v => Value::Date(days_of(v).unwrap() as i32),
        },
        "year" | "quarter" | "month" | "day" => {
            let (y, m, d) = civil_from_days(days_of(&vals[0]).unwrap());
            Value::Int(match name {
                "year" => y,
                "quarter" => ((m - 1) / 3 + 1) as i64,
                "month" => m as i64,
                _ => d as i64,
            })
        }
        "weekday" => Value::Int((days_of(&vals[0]).unwrap() + 3).rem_euclid(7) + 1),
        "hour" | "minute" => {
            let micros = match vals[0] {
                Value::Timestamp(t) => t.rem_euclid(MICROS_PER_DAY),
                _ => 0,
            };
            let mins = micros / 60_000_000;
            Value::Int(if name == "hour" { mins / 60 } else { mins % 60 })
        }
        "date_trunc" => {
            let unit = lit_text(&args[0]).unwrap().to_ascii_lowercase();
            match vals[1] {
                Value::Date(d) => Value::Date(trunc_days(d as i64, &unit) as i32),
                Value::Timestamp(t) => {
                    let day = t.div_euclid(MICROS_PER_DAY);
                    let base = trunc_days(day, &unit) * MICROS_PER_DAY;
                    Value::Timestamp(match unit.as_str() {
                        "hour" => t.div_euclid(3_600_000_000) * 3_600_000_000,
                        "minute" => t.div_euclid(60_000_000) * 60_000_000,
                        _ => base,
                    })
                }
                _ => unreachable!(),
            }
        }
        "days_between" => Value::Int(days_of(&vals[1]).unwrap() - days_of(&vals[0]).unwrap()),
        "add_days" => {
            let n = int(1);
            if n.unsigned_abs() > i32::MAX as u64 {
                return Err(overflow("add_days"));
            }
            match vals[0] {
                Value::Date(d) => Value::Date(
                    i32::try_from(d as i64 + n).map_err(|_| overflow("add_days"))?,
                ),
                Value::Timestamp(t) => Value::Timestamp(
                    n.checked_mul(MICROS_PER_DAY).and_then(|x| t.checked_add(x)).ok_or_else(|| overflow("add_days"))?,
                ),
                _ => unreachable!(),
            }
        }
        _ => unreachable!("every listed function evaluates"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Schema {
        Schema::new(vec![("units", Int), ("price", Float), ("region", Text), ("day", Date), ("Unit price", Float)])
    }

    #[test]
    fn parses_precedence_and_names() {
        assert_eq!(
            parse("1 + 2 * 3").unwrap(),
            Expr::Bin(
                Op::Add,
                Box::new(Expr::Lit(Value::Int(1))),
                Box::new(Expr::Bin(Op::Mul, Box::new(Expr::Lit(Value::Int(2))), Box::new(Expr::Lit(Value::Int(3)))))
            )
        );
        assert_eq!(parse("[Unit price]").unwrap(), Expr::Col("Unit price".into()));
        assert!(matches!(parse("region is not missing").unwrap(), Expr::IsMissing(_, true)));
        assert_eq!(parse("'it''s'").unwrap(), Expr::Lit(Value::Text("it's".into())));
        assert_eq!(parse("IF(TRUE, 1, 2)").unwrap(), Expr::Call("if".into(), vec![
            Expr::Lit(Value::Bool(true)), Expr::Lit(Value::Int(1)), Expr::Lit(Value::Int(2))
        ]));
    }

    #[test]
    fn errors_say_where_and_what() {
        let check = |f: &str| type_of(&parse(f).unwrap(), &schema()).unwrap_err();
        assert!(parse("1 +").unwrap_err().contains("ends too early"));
        assert!(parse("(1").unwrap_err().contains("expected `)`"));
        assert!(parse("1 $ 2").unwrap_err().contains("unexpected `$` at character 3"));
        assert!(check("nope + 1").contains("no column `nope`"));
        assert!(parse("Unit price * 2").unwrap_err().contains("write names with spaces as [Unit price]"));
        assert!(check("region + 1").contains("join text with &"));
        assert!(check("region = missing").contains("is missing"));
        assert!(check("frobnicate(1)").contains("unknown function"));
        assert!(check("round(price, units)").contains("decimals from 0 to 10"));
        assert!(check("date_trunc(\"fortnight\", day)").contains("unit in quotes"));
        assert!(check("text(price)").contains("format(x, decimals)"));
        assert!(check("if(units, 1, 2)").contains("condition"));
    }

    #[test]
    fn types_follow_the_rules() {
        let ty = |f: &str| result_type(&parse(f).unwrap(), &schema()).unwrap();
        assert_eq!(ty("units * 2"), Int);
        assert_eq!(ty("units / 2"), Float);
        assert_eq!(ty("units + price"), Float);
        assert_eq!(ty("if(units > 1, units, price)"), Float);
        assert_eq!(ty("coalesce(region, \"Unknown\")"), Text);
        assert_eq!(ty("missing"), Text);
        assert_eq!(ty("date_trunc(\"month\", day)"), Date);
        assert_eq!(ty("year(day)"), Int);
    }
}
