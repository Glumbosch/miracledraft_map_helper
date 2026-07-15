use crate::{ByteSource, Error, Result, Value};

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    P(char),
    String,
    Number,
    Ident,
    Eof,
}
#[derive(Clone, Debug)]
struct Token {
    kind: Kind,
    text: String,
    pos: usize,
}
struct Lexer<'a> {
    s: &'a str,
    pos: usize,
}
impl<'a> Lexer<'a> {
    fn next(&mut self) -> Result<Token> {
        let b = self.s.as_bytes();
        while self.pos < b.len() && (b[self.pos] as char).is_whitespace() {
            self.pos += 1;
        }
        let start = self.pos;
        if start == b.len() {
            return Ok(Token {
                kind: Kind::Eof,
                text: String::new(),
                pos: start,
            });
        }
        let c = b[start] as char;
        if "{}[]():,".contains(c) {
            self.pos += 1;
            return Ok(Token {
                kind: Kind::P(c),
                text: c.to_string(),
                pos: start,
            });
        }
        if c == '"' {
            self.pos += 1;
            let mut esc = false;
            while self.pos < b.len() {
                let ch = b[self.pos] as char;
                self.pos += 1;
                if esc {
                    esc = false;
                } else if ch == '\\' {
                    esc = true;
                } else if ch == '"' {
                    return Ok(Token {
                        kind: Kind::String,
                        text: self.s[start..self.pos].to_owned(),
                        pos: start,
                    });
                }
            }
            return Err(Error::format(format!(
                "unterminated string at character {start}"
            )));
        }
        if c.is_ascii_digit() || matches!(c, '+' | '-' | '.') {
            let mut i = start;
            if matches!(b[i] as char, '+' | '-') {
                i += 1;
            }
            let mut digits = false;
            while i < b.len() && (b[i] as char).is_ascii_digit() {
                i += 1;
                digits = true;
            }
            if i < b.len() && b[i] == b'.' {
                i += 1;
                while i < b.len() && (b[i] as char).is_ascii_digit() {
                    i += 1;
                    digits = true;
                }
            }
            if digits {
                if i < b.len() && matches!(b[i] as char, 'e' | 'E') {
                    let save = i;
                    i += 1;
                    if i < b.len() && matches!(b[i] as char, '+' | '-') {
                        i += 1;
                    }
                    let es = i;
                    while i < b.len() && (b[i] as char).is_ascii_digit() {
                        i += 1;
                    }
                    if es == i {
                        i = save;
                    }
                }
                self.pos = i;
                return Ok(Token {
                    kind: Kind::Number,
                    text: self.s[start..i].to_owned(),
                    pos: start,
                });
            }
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let mut i = start + 1;
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            self.pos = i;
            return Ok(Token {
                kind: Kind::Ident,
                text: self.s[start..i].to_owned(),
                pos: start,
            });
        }
        Err(Error::format(format!(
            "unexpected character {c:?} at character {start}"
        )))
    }
}

struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
}
impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Result<Self> {
        let mut lexer = Lexer { s, pos: 0 };
        let current = lexer.next()?;
        Ok(Self { lexer, current })
    }
    fn bump(&mut self) -> Result<Token> {
        let old = self.current.clone();
        self.current = self.lexer.next()?;
        Ok(old)
    }
    fn punct(&mut self, c: char) -> Result<()> {
        if self.current.kind != Kind::P(c) {
            return Err(Error::format(format!(
                "expected {c}, found {:?} at character {}",
                self.current.kind, self.current.pos
            )));
        }
        self.bump()?;
        Ok(())
    }
    fn value(&mut self) -> Result<Value> {
        match self.current.kind.clone() {
            Kind::String => {
                let t = self.bump()?;
                Ok(Value::String(serde_json::from_str(&t.text)?))
            }
            Kind::Number => {
                let t = self.bump()?;
                if t.text.contains(['.', 'e', 'E']) {
                    Ok(Value::Real(
                        t.text
                            .parse()
                            .map_err(|_| Error::format("invalid number"))?,
                    ))
                } else {
                    Ok(Value::Int(
                        t.text
                            .parse()
                            .map_err(|_| Error::format("invalid integer"))?,
                    ))
                }
            }
            Kind::P('{') => self.dict(),
            Kind::P('[') => self.array(),
            Kind::Ident => {
                let t = self.bump()?;
                match t.text.as_str() {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    "null" => Ok(Value::Nil),
                    "nan" => Ok(Value::Real(f64::NAN)),
                    "inf" => Ok(Value::Real(f64::INFINITY)),
                    _ => {
                        self.punct('(')?;
                        let a = self.args()?;
                        constructor(&t.text, a, t.pos)
                    }
                }
            }
            _ => Err(Error::format(format!(
                "unexpected token at character {}",
                self.current.pos
            ))),
        }
    }
    fn dict(&mut self) -> Result<Value> {
        self.punct('{')?;
        let mut d = Vec::new();
        if self.current.kind == Kind::P('}') {
            self.bump()?;
            return Ok(Value::Dictionary(d));
        }
        loop {
            let k = self.value()?;
            self.punct(':')?;
            let v = self.value()?;
            d.push((k, v));
            if self.current.kind == Kind::P(',') {
                self.bump()?;
                if self.current.kind == Kind::P('}') {
                    break;
                }
            } else {
                break;
            }
        }
        self.punct('}')?;
        Ok(Value::Dictionary(d))
    }
    fn array(&mut self) -> Result<Value> {
        self.punct('[')?;
        let mut a = Vec::new();
        if self.current.kind == Kind::P(']') {
            self.bump()?;
            return Ok(Value::Array(a));
        }
        loop {
            a.push(self.value()?);
            if self.current.kind == Kind::P(',') {
                self.bump()?;
                if self.current.kind == Kind::P(']') {
                    break;
                }
            } else {
                break;
            }
        }
        self.punct(']')?;
        Ok(Value::Array(a))
    }
    fn args(&mut self) -> Result<Vec<Value>> {
        let mut a = Vec::new();
        if self.current.kind == Kind::P(')') {
            self.bump()?;
            return Ok(a);
        }
        loop {
            a.push(self.value()?);
            if self.current.kind == Kind::P(',') {
                self.bump()?;
            } else {
                break;
            }
        }
        self.punct(')')?;
        Ok(a)
    }
}

fn number(v: &Value) -> Result<f32> {
    v.as_f64()
        .map(|n| n as f32)
        .ok_or_else(|| Error::format("constructor requires numeric arguments"))
}
fn constructor(name: &str, args: Vec<Value>, pos: usize) -> Result<Value> {
    let fixed = match name {
        "Vector2" => Some(2),
        "Rect2" => Some(4),
        "Vector3" => Some(3),
        "Transform2D" => Some(6),
        "Plane" | "Quat" | "Color" => Some(4),
        "AABB" => Some(6),
        "Basis" => Some(9),
        "Transform" => Some(12),
        _ => None,
    };
    if let Some(n) = fixed {
        if args.len() != n {
            return Err(Error::format(format!(
                "{name} requires {n} arguments at character {pos}"
            )));
        }
        return Ok(Value::Vector {
            kind: name.into(),
            values: args.iter().map(number).collect::<Result<_>>()?,
        });
    }
    Ok(match name {
        "PoolByteArray" => Value::PoolByteArray(ByteSource::Memory(
            args.iter()
                .map(|v| number(v).map(|n| n as u8))
                .collect::<Result<_>>()?,
        )),
        "PoolIntArray" => Value::PoolIntArray(
            args.iter()
                .map(|v| number(v).map(|n| n as i32))
                .collect::<Result<_>>()?,
        ),
        "PoolRealArray" => Value::PoolRealArray(args.iter().map(number).collect::<Result<_>>()?),
        "PoolStringArray" => Value::PoolStringArray(
            args.into_iter()
                .map(|v| {
                    if let Value::String(s) = v {
                        Ok(s)
                    } else {
                        Err(Error::format("PoolStringArray requires strings"))
                    }
                })
                .collect::<Result<_>>()?,
        ),
        "PoolVector2Array" | "PoolVector3Array" | "PoolColorArray" => {
            let n = if name == "PoolVector2Array" {
                2
            } else if name == "PoolVector3Array" {
                3
            } else {
                4
            };
            if !args.len().is_multiple_of(n) {
                return Err(Error::format(format!(
                    "{name} argument count must be divisible by {n}"
                )));
            }
            let flat = args.iter().map(number).collect::<Result<Vec<_>>>()?;
            Value::PoolVectors {
                kind: name.into(),
                components: n,
                values: flat.chunks(n).map(|x| x.to_vec()).collect(),
            }
        }
        _ => {
            return Err(Error::format(format!(
                "unknown constructor {name:?} at character {pos}"
            )));
        }
    })
}

pub fn parse(text: &str) -> Result<Value> {
    let mut p = Parser::new(text)?;
    let v = p.value()?;
    if p.current.kind != Kind::Eof {
        return Err(Error::format(format!(
            "unexpected content at character {}",
            p.current.pos
        )));
    }
    Ok(v)
}

fn fmt_num(v: f64, force: bool) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v.is_sign_positive() {
            "inf".into()
        } else {
            "-inf".into()
        };
    }
    let mut s = format!("{v:.6}");
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s == "-0" {
        s = "0".into();
    }
    if force && !s.contains(['.', 'e', 'E']) {
        s.push_str(".0");
    }
    s
}
pub fn format(value: &Value) -> String {
    let mut out = String::new();
    write_text(value, &mut out);
    out
}
fn write_text(v: &Value, out: &mut String) {
    match v {
        Value::Nil => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Real(n) => out.push_str(&fmt_num(*n, true)),
        Value::String(s) => out.push_str(&serde_json::to_string(s).unwrap()),
        Value::Dictionary(d) => {
            if d.is_empty() {
                out.push_str("{\n}");
                return;
            }
            out.push_str("{\n");
            let mut items: Vec<_> = d.iter().collect();
            if items.iter().all(|(k, _)| matches!(k, Value::String(_))) {
                items.sort_by_key(|(k, _)| k.as_str().unwrap());
            }
            for (i, (k, v)) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                write_text(k, out);
                out.push_str(": ");
                write_text(v, out);
            }
            out.push_str("\n}");
        }
        Value::Array(a) => {
            out.push_str("[ ");
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_text(v, out);
            }
            out.push_str(" ]");
        }
        Value::Vector { kind, values } => {
            out.push_str(kind);
            out.push_str("( ");
            for (i, n) in values.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&fmt_num(*n as f64, false));
            }
            out.push_str(" )");
        }
        Value::PoolByteArray(b) => {
            out.push_str("PoolByteArray( ");
            if let ByteSource::Memory(bytes) = b {
                for (i, n) in bytes.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&n.to_string());
                }
            } else {
                out.push_str("/* disk-backed data omitted */");
            }
            out.push_str(" )");
        }
        Value::PoolIntArray(a) => pool_fmt(out, "PoolIntArray", a.iter().map(ToString::to_string)),
        Value::PoolRealArray(a) => pool_fmt(
            out,
            "PoolRealArray",
            a.iter().map(|n| fmt_num(*n as f64, false)),
        ),
        Value::PoolStringArray(a) => pool_fmt(
            out,
            "PoolStringArray",
            a.iter().map(|s| serde_json::to_string(s).unwrap()),
        ),
        Value::PoolVectors { kind, values, .. } => pool_fmt(
            out,
            kind,
            values.iter().flatten().map(|n| fmt_num(*n as f64, false)),
        ),
        Value::Object { class, properties } => {
            let d = Value::Dictionary(vec![
                (
                    Value::String("__class__".into()),
                    Value::String(class.clone()),
                ),
                (
                    Value::String("properties".into()),
                    Value::Dictionary(
                        properties
                            .iter()
                            .map(|(k, v)| (Value::String(k.clone()), v.clone()))
                            .collect(),
                    ),
                ),
            ]);
            write_text(&d, out);
        }
        Value::NodePath {
            names,
            subnames,
            absolute,
        } => {
            let text = format!(
                "{}{}{}",
                if *absolute { "/" } else { "" },
                names.join("/"),
                if subnames.is_empty() {
                    String::new()
                } else {
                    format!(":{}", subnames.join(":"))
                }
            );
            out.push_str(&serde_json::to_string(&text).unwrap());
        }
        Value::Rid => out.push_str("null"),
        Value::ObjectId(id) => out.push_str(&id.to_string()),
    }
}
fn pool_fmt(out: &mut String, name: &str, items: impl Iterator<Item = String>) {
    out.push_str(name);
    out.push_str("( ");
    for (i, s) in items.enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&s);
    }
    out.push_str(" )");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_and_formats() {
        let s = "{\n\"x\": Vector2( 1, 2 ),\n\"a\": [ true, null, 4.5 ]\n}";
        let v = parse(s).unwrap();
        assert!(matches!(parse(&format(&v)).unwrap(), Value::Dictionary(_)));
    }
}
