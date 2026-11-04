//! Minimal JSON value, parser and pretty printer.
//!
//! TEMPORARY: replace with serde_json once crates.io is reachable. The on-disk
//! analysis format defined in `persist.rs` is the durable contract, not this code.

use crate::Error;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn obj(pairs: Vec<(&str, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.get(key),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn req_str(&self, key: &str) -> Result<&str, Error> {
        self.get(key)
            .and_then(Json::as_str)
            .ok_or_else(|| Error::Format(format!("missing string field `{key}`")))
    }
    pub fn req_arr(&self, key: &str) -> Result<&Vec<Json>, Error> {
        self.get(key)
            .and_then(Json::as_arr)
            .ok_or_else(|| Error::Format(format!("missing array field `{key}`")))
    }

    pub fn to_string_pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    pub fn to_string_compact(&self) -> String {
        let mut out = String::new();
        self.write_compact(&mut out);
        out
    }

    fn write_compact(&self, out: &mut String) {
        match self {
            Json::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write_compact(out);
                }
                out.push(']');
            }
            Json::Obj(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(out, k);
                    out.push(':');
                    v.write_compact(out);
                }
                out.push('}');
            }
            scalar => scalar.write(out, 0),
        }
    }

    fn write(&self, out: &mut String, indent: usize) {
        let pad = |n: usize| "  ".repeat(n);
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 9.0e15 {
                    out.push_str(&format!("{}", *n as i64))
                } else {
                    out.push_str(&format!("{n}"))
                }
            }
            Json::Str(s) => write_str(out, s),
            Json::Arr(a) if a.is_empty() => out.push_str("[]"),
            Json::Arr(a) => {
                out.push_str("[\n");
                for (i, v) in a.iter().enumerate() {
                    out.push_str(&pad(indent + 1));
                    v.write(out, indent + 1);
                    if i + 1 < a.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad(indent));
                out.push(']');
            }
            Json::Obj(m) if m.is_empty() => out.push_str("{}"),
            Json::Obj(m) => {
                out.push_str("{\n");
                for (i, (k, v)) in m.iter().enumerate() {
                    out.push_str(&pad(indent + 1));
                    write_str(out, k);
                    out.push_str(": ");
                    v.write(out, indent + 1);
                    if i + 1 < m.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad(indent));
                out.push('}');
            }
        }
    }

    pub fn parse(text: &str) -> Result<Json, Error> {
        let mut p = Parser { s: text.as_bytes(), i: 0 };
        let v = p.value()?;
        p.ws();
        if p.i != p.s.len() {
            return Err(p.err("trailing characters"));
        }
        Ok(v)
    }
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> Error {
        Error::Format(format!("JSON: {msg} at byte {}", self.i))
    }
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }
    fn value(&mut self) -> Result<Json, Error> {
        self.ws();
        match self.s.get(self.i) {
            None => Err(self.err("unexpected end")),
            Some(b'n') if self.eat("null") => Ok(Json::Null),
            Some(b't') if self.eat("true") => Ok(Json::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Json::Bool(false)),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.eat("]") {
                    return Ok(Json::Arr(items));
                }
                loop {
                    items.push(self.value()?);
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("]") {
                        return Ok(Json::Arr(items));
                    }
                    return Err(self.err("expected , or ]"));
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut m = BTreeMap::new();
                self.ws();
                if self.eat("}") {
                    return Ok(Json::Obj(m));
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return Err(self.err("expected key"));
                    }
                    let k = self.string()?;
                    self.ws();
                    if !self.eat(":") {
                        return Err(self.err("expected :"));
                    }
                    m.insert(k, self.value()?);
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("}") {
                        return Ok(Json::Obj(m));
                    }
                    return Err(self.err("expected , or }"));
                }
            }
            Some(_) => self.number(),
        }
    }
    fn number(&mut self) -> Result<Json, Error> {
        let start = self.i;
        while self.i < self.s.len() && b"+-0123456789.eE".contains(&self.s[self.i]) {
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i])
            .ok()
            .and_then(|t| t.parse::<f64>().ok())
            .map(Json::Num)
            .ok_or_else(|| self.err("invalid number"))
    }
    fn string(&mut self) -> Result<String, Error> {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let b = *self.s.get(self.i).ok_or_else(|| self.err("unterminated string"))?;
            self.i += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or_else(|| self.err("bad escape"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = std::str::from_utf8(self.s.get(self.i..self.i + 4).unwrap_or(&[]))
                                .ok()
                                .and_then(|h| u32::from_str_radix(h, 16).ok())
                                .ok_or_else(|| self.err("bad \\u escape"))?;
                            self.i += 4;
                            let c = char::from_u32(hex).unwrap_or('\u{fffd}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                }
                _ => out.push(b),
            }
        }
        String::from_utf8(out).map_err(|_| self.err("invalid utf-8"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = Json::obj(vec![
            ("a", Json::Num(1.0)),
            ("b", Json::Arr(vec![Json::Bool(true), Json::Null, Json::Num(2.5)])),
            ("c", Json::str("line\n\"q\" é")),
            ("d", Json::Obj(BTreeMap::new())),
        ]);
        assert_eq!(Json::parse(&v.to_string_pretty()).unwrap(), v);
        assert_eq!(Json::parse(&v.to_string_compact()).unwrap(), v);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Json::parse("{\"a\":}").is_err());
        assert!(Json::parse("[1,2").is_err());
        assert!(Json::parse("1 2").is_err());
    }
}
