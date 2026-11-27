//! Lightweight zero-dependency JSON parser for usage log adapters.

use super::super::domain::parse_iso8601;
use std::collections::HashMap;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum JsonVal {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<JsonVal>),
    Obj(HashMap<String, JsonVal>),
}

impl JsonVal {
    pub fn get(&self, key: &str) -> Option<&JsonVal> {
        match self {
            JsonVal::Obj(map) => map.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonVal::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsonVal]> {
        match self {
            JsonVal::Arr(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(JsonVal::as_str)
    }

    pub fn get_u64(&self, key: &str) -> u64 {
        match self.get(key) {
            Some(JsonVal::Num(n)) if *n >= 0.0 => *n as u64,
            _ => 0,
        }
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        match self.get(key) {
            Some(JsonVal::Num(n)) => Some(*n),
            _ => None,
        }
    }
}

pub fn parse_timestamp(val: &JsonVal) -> Option<u64> {
    match val {
        JsonVal::Num(n) => {
            if *n <= 0.0 {
                return None;
            }
            let v = *n as u64;
            if v > 100_000_000_000 {
                Some(v / 1000)
            } else {
                Some(v)
            }
        }
        JsonVal::Str(s) => parse_iso8601(s),
        _ => None,
    }
}

pub fn parse_json(input: &str) -> Option<JsonVal> {
    let mut p = JsonParser {
        input,
        bytes: input.as_bytes(),
        pos: 0,
    };
    p.parse_value()
}

struct JsonParser<'a> {
    input: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn parse_value(&mut self) -> Option<JsonVal> {
        self.skip_ws();
        match self.peek()? {
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            b'"' => self.parse_string().map(JsonVal::Str),
            b't' => self.parse_literal(b"true", JsonVal::Bool(true)),
            b'f' => self.parse_literal(b"false", JsonVal::Bool(false)),
            b'n' => self.parse_literal(b"null", JsonVal::Null),
            b'-' | b'0'..=b'9' => self.parse_number().map(JsonVal::Num),
            _ => None,
        }
    }

    fn parse_literal(&mut self, lit: &[u8], val: JsonVal) -> Option<JsonVal> {
        if self.bytes[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            Some(val)
        } else {
            None
        }
    }

    fn parse_object(&mut self) -> Option<JsonVal> {
        self.pos += 1;
        let mut map = HashMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Some(JsonVal::Obj(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return None;
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return None;
            }
            self.pos += 1;
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Some(JsonVal::Obj(map));
                }
                _ => return None,
            }
        }
    }

    fn parse_array(&mut self) -> Option<JsonVal> {
        self.pos += 1;
        let mut arr = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Some(JsonVal::Arr(arr));
        }
        loop {
            let val = self.parse_value()?;
            arr.push(val);
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b']' => {
                    self.pos += 1;
                    return Some(JsonVal::Arr(arr));
                }
                _ => return None,
            }
        }
    }

    fn parse_string(&mut self) -> Option<String> {
        self.pos += 1;
        let start = self.pos;
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                let s = self.input[start..self.pos].to_string();
                self.pos += 1;
                return Some(s);
            }
            if b == b'\\' {
                break;
            }
            self.pos += 1;
        }
        let mut out = String::with_capacity(self.pos - start + 32);
        out.push_str(&self.input[start..self.pos]);
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = *self.bytes.get(self.pos)?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\x08'),
                        b'f' => out.push('\x0c'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self.input.get(self.pos..self.pos + 4)?;
                            let code = u32::from_str_radix(hex, 16).ok()?;
                            self.pos += 4;
                            if let Some(ch) = char::from_u32(code) {
                                out.push(ch);
                            }
                        }
                        other if other < 0x80 => out.push(other as char),
                        _ => {
                            let ch = self.input.get(self.pos - 1..)?.chars().next()?;
                            out.push(ch);
                            self.pos += ch.len_utf8() - 1;
                        }
                    }
                }
                _ => {
                    let chunk_start = self.pos;
                    while self.pos < self.bytes.len() {
                        let c = self.bytes[self.pos];
                        if c == b'"' || c == b'\\' {
                            break;
                        }
                        self.pos += 1;
                    }
                    out.push_str(&self.input[chunk_start..self.pos]);
                }
            }
        }
        None
    }

    fn parse_number(&mut self) -> Option<f64> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-' => self.pos += 1,
                _ => break,
            }
        }
        self.input[start..self.pos].parse::<f64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_large_escaped_string_is_linear() {
        let chunk = "hello \\\"world\\\" — café 🚀 \\n ";
        let repeated = chunk.repeat(10_000);
        let doc = format!("{{\"payload\":\"{repeated}\",\"tokens\":42}}");
        let t0 = std::time::Instant::now();
        let val = parse_json(&doc).expect("valid json");
        let elapsed = t0.elapsed();
        assert_eq!(val.get_u64("tokens"), 42);
        let payload = val.get_str("payload").expect("payload string");
        assert!(payload.starts_with("hello \"world\" — café 🚀 \n "));
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "parse_json took {:?} on 300KB escaped string (quadratic regression?)",
            elapsed
        );
    }
}
