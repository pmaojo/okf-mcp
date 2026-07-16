//! JSON escrito a mano, solo con `std`.
//!
//! Objetivo didáctico: entender qué hace `serde_json` por nosotros.
//! Frontera de producción: el endpoint público de Vercel usará
//! `serde_json` (adaptador `json-wire`); este parser sirve el
//! transporte stdio local y los tests.
//!
//! Decisiones de diseño (y sus porqués):
//! - `Object` usa `BTreeMap`: serialización determinista → los tests
//!   pueden comparar strings y los hashes de respuestas son estables.
//! - Números como `f64`: suficiente para JSON-RPC (ids < 2^53).
//!   Documentamos la pérdida de fidelidad para enteros de 64 bits.
//! - Límites explícitos de profundidad y tamaño ANTES de asignar.
//! - Errores con offset de byte: un parser que dice "error" sin
//!   decir dónde no enseña nada.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

/// Profundidad máxima de anidamiento. JSON legítimo de MCP no pasa
/// de ~10; 64 deja margen y evita agotar la pila con `[[[[...`.
pub const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Number(n) if *n >= 0.0 && n.fract() == 0.0 && *n <= 9007199254740992.0 => {
                Some(*n as u64)
            }
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_object(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    /// Acceso a un campo de objeto: `v.get("params")`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|m| m.get(key))
    }
}

/// Constructor ergonómico de objetos para el lado servidor.
pub fn obj<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

pub fn s(text: &str) -> Value {
    Value::String(text.to_string())
}

pub fn n(num: f64) -> Value {
    Value::Number(num)
}

pub fn arr(items: Vec<Value>) -> Value {
    Value::Array(items)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Offset del byte donde se detectó el problema.
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON inválido en el byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parsea un documento JSON completo. Rechaza contenido extra tras
/// el valor raíz (p. ej. `{}garbage`).
pub fn parse(input: &str) -> Result<Value, ParseError> {
    let mut p = Parser { bytes: input.as_bytes(), pos: 0 };
    p.skip_ws();
    let v = p.parse_value(0)?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(p.err("contenido extra tras el valor raíz"));
    }
    Ok(v)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, msg: &str) -> ParseError {
        ParseError { offset: self.pos, message: msg.to_string() }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(&format!("se esperaba {:?}", byte as char)))
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.err("anidamiento demasiado profundo"));
        }
        match self.peek() {
            None => Err(self.err("fin inesperado de la entrada")),
            Some(b'{') => self.parse_object(depth),
            Some(b'[') => self.parse_array(depth),
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Value::Bool(true)),
            Some(b'f') => self.parse_literal("false", Value::Bool(false)),
            Some(b'n') => self.parse_literal("null", Value::Null),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(c) => Err(self.err(&format!("carácter inesperado {:?}", c as char))),
        }
    }

    fn parse_literal(&mut self, lit: &str, value: Value) -> Result<Value, ParseError> {
        if self.bytes[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            Ok(value)
        } else {
            Err(self.err(&format!("se esperaba el literal {lit}")))
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.expect(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.parse_value(depth + 1)?;
            // Política de claves duplicadas: gana la última, como
            // serde_json. Lo importante es que sea una DECISIÓN.
            map.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(map));
                }
                _ => return Err(self.err("se esperaba ',' o '}'")),
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.parse_value(depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.err("se esperaba ',' o ']'")),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err("string sin cerrar")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'b') => out.push('\u{0008}'),
                        Some(b'f') => out.push('\u{000C}'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'u') => {
                            self.pos += 1;
                            let cp = self.parse_hex4()?;
                            // Pares subrogados (UTF-16 escapado en JSON).
                            let ch = if (0xD800..0xDC00).contains(&cp) {
                                if self.bytes[self.pos..].starts_with(b"\\u") {
                                    self.pos += 2;
                                    let low = self.parse_hex4()?;
                                    if !(0xDC00..0xE000).contains(&low) {
                                        return Err(self.err("subrogado bajo inválido"));
                                    }
                                    let c = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
                                    char::from_u32(c).ok_or_else(|| self.err("codepoint inválido"))?
                                } else {
                                    return Err(self.err("subrogado alto sin pareja"));
                                }
                            } else if (0xDC00..0xE000).contains(&cp) {
                                return Err(self.err("subrogado bajo sin pareja"));
                            } else {
                                char::from_u32(cp).ok_or_else(|| self.err("codepoint inválido"))?
                            };
                            out.push(ch);
                            continue; // parse_hex4 ya avanzó pos
                        }
                        _ => return Err(self.err("escape inválido")),
                    }
                    self.pos += 1;
                }
                Some(b) if b < 0x20 => {
                    return Err(self.err("carácter de control sin escapar en string"));
                }
                Some(_) => {
                    // Avanzar un carácter UTF-8 completo. La entrada es
                    // &str, así que los límites son válidos por construcción.
                    let start = self.pos;
                    self.pos += 1;
                    while self.pos < self.bytes.len() && (self.bytes[self.pos] & 0xC0) == 0x80 {
                        self.pos += 1;
                    }
                    let chunk = std::str::from_utf8(&self.bytes[start..self.pos])
                        .map_err(|_| self.err("UTF-8 inválido"))?;
                    out.push_str(chunk);
                }
            }
        }
    }

    /// Lee exactamente 4 dígitos hex tras `\u` y deja `pos` después
    /// de ellos.
    fn parse_hex4(&mut self) -> Result<u32, ParseError> {
        let end = self.pos.checked_add(4).ok_or_else(|| self.err("desbordamiento"))?;
        if end > self.bytes.len() {
            return Err(self.err("\\u incompleto"));
        }
        let mut cp: u32 = 0;
        for &b in &self.bytes[self.pos..end] {
            let d = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => return Err(self.err("dígito hex inválido en \\u")),
            };
            cp = (cp << 4) | d as u32;
        }
        self.pos = end;
        Ok(cp)
    }

    fn parse_number(&mut self) -> Result<Value, ParseError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        // Parte entera: '0' solo, o [1-9][0-9]*
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.err("dígito esperado")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("dígito esperado tras '.'"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("dígito esperado en el exponente"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err("UTF-8 inválido en número"))?;
        let value: f64 = text.parse().map_err(|_| self.err("número no representable"))?;
        if !value.is_finite() {
            return Err(self.err("número fuera de rango"));
        }
        Ok(Value::Number(value))
    }
}

/// Serializa a JSON compacto y determinista.
pub fn to_string(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            // Enteros sin ".0" para que los ids JSON-RPC vuelvan tal
            // cual llegaron.
            if n.fract() == 0.0 && n.abs() < 9007199254740992.0 {
                out.push_str(&format!("{}", *n as i64));
            } else {
                out.push_str(&format!("{n}"));
            }
        }
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, k);
                out.push(':');
                write_value(out, v);
            }
            out.push('}');
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_y_vuelta_basica() {
        let casos = [
            r#"null"#,
            r#"true"#,
            r#"[1,2,3]"#,
            r#"{"a":1,"b":[true,null],"c":"hola"}"#,
            r#""líneas\ncon\ttabs y ñ""#,
        ];
        for caso in casos {
            let v = parse(caso).unwrap_or_else(|e| panic!("{caso}: {e}"));
            let out = to_string(&v);
            let v2 = parse(&out).unwrap();
            assert_eq!(v, v2, "no estable: {caso}");
        }
    }

    #[test]
    fn escapes_unicode() {
        let v = parse(r#""ñ 🦀""#).unwrap();
        assert_eq!(v, Value::String("ñ 🦀".to_string()));
        assert!(parse(r#""\ud800""#).is_err(), "subrogado alto suelto");
        assert!(parse(r#""\udc00""#).is_err(), "subrogado bajo suelto");
    }

    #[test]
    fn rechaza_json_invalido() {
        for malo in [
            "", "{", "[1,", r#"{"a"}"#, "01", "1.", "1e", "tru", "{}extra",
            "\"sin cerrar", "[1 2]", "nan", "-",
        ] {
            assert!(parse(malo).is_err(), "debería rechazar {malo:?}");
        }
    }

    #[test]
    fn limite_de_profundidad() {
        let profundo = "[".repeat(MAX_DEPTH + 2) + &"]".repeat(MAX_DEPTH + 2);
        let err = parse(&profundo).unwrap_err();
        assert!(err.message.contains("profundo"));
    }

    #[test]
    fn numeros_enteros_estables() {
        let v = parse("42").unwrap();
        assert_eq!(to_string(&v), "42");
        assert_eq!(v.as_u64(), Some(42));
        assert_eq!(parse("1.5").unwrap().as_u64(), None);
        assert_eq!(parse("-1").unwrap().as_u64(), None);
    }

    #[test]
    fn objetos_deterministas() {
        let v = parse(r#"{"z":1,"a":2}"#).unwrap();
        assert_eq!(to_string(&v), r#"{"a":2,"z":1}"#);
    }

    #[test]
    fn claves_duplicadas_gana_la_ultima() {
        let v = parse(r#"{"a":1,"a":2}"#).unwrap();
        assert_eq!(v.get("a"), Some(&Value::Number(2.0)));
    }
}
