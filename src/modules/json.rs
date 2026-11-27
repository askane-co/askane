use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::{FastMap, FxBuildHasher, new_dict_with_capacity, Value};
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "json",
        &[("parse", json_parse), ("stringify", json_stringify)],
        &[],
    )
}

fn json_parse(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("json.parse() expects 1 argument (json_str)".to_string());
    }
    let raw = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "json.parse() expects a string, got '{}'",
                other.type_name()
            ));
        }
    };
    let mut parser = JsonParser::new(raw);
    let val = parser.parse_value()?;
    parser.skip_ws();
    if !parser.is_eof() {
        return Err("json.parse(): trailing characters after JSON value".to_string());
    }
    Ok(val)
}

fn json_stringify(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("json.stringify() expects 1 argument".to_string());
    }
    let mut out = String::with_capacity(64);
    let mut seen = HashSet::new();
    stringify_value(&args[0], &mut out, &mut seen)?;
    Ok(Value::from_str(out))
}

fn stringify_value(val: &Value, out: &mut String, seen: &mut HashSet<usize>) -> Result<(), String> {
    if seen.len() >= 64 {
        return Err("json.stringify(): maximum nesting depth exceeded".to_string());
    }
    if out.len() > 32 * 1024 * 1024 {
        return Err("json.stringify(): output size limit exceeded".to_string());
    }
    match val {
        Value::Unit => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Float(f) => {
            if f.is_finite() {
                out.push_str(&f.to_string());
            } else {
                out.push_str("null");
            }
        }
        Value::Vec2(x, y) => {
            stringify_f32_slice(&[*x, *y], out);
        }
        Value::Vec3(v) => stringify_f32_slice(v.as_slice(), out),
        Value::Vec4(v) | Value::Quat(v) => stringify_f32_slice(v.as_slice(), out),
        Value::Mat4(m) => stringify_f32_slice(m.as_slice(), out),
        Value::Str(s) => write_json_string(s, out),
        Value::List(list) => {
            let ptr = Rc::as_ptr(list) as usize;
            if !seen.insert(ptr) {
                return Err("json.stringify(): circular reference in list".to_string());
            }
            out.push('[');
            let items = list.borrow();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                stringify_value(item, out, seen)?;
            }
            out.push(']');
            seen.remove(&ptr);
        }
        Value::Dict(dict) => {
            let ptr = Rc::as_ptr(dict) as usize;
            if !seen.insert(ptr) {
                return Err("json.stringify(): circular reference in dict".to_string());
            }
            out.push('{');
            let map = dict.borrow();
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_string(k, out);
                out.push(':');
                stringify_value(v, out, seen)?;
            }
            out.push('}');
            seen.remove(&ptr);
        }
        Value::StructInstance(inst) => {
            let ptr = Rc::as_ptr(inst) as usize;
            if !seen.insert(ptr) {
                return Err("json.stringify(): circular reference in struct".to_string());
            }
            out.push('{');
            let vals = inst.fields.borrow();
            for (i, (k, v)) in inst.def.field_names.iter().zip(vals.iter()).enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_string(k, out);
                out.push(':');
                stringify_value(v, out, seen)?;
            }
            out.push('}');
            seen.remove(&ptr);
        }
        other => {
            return Err(format!(
                "json.stringify() cannot serialize type '{}'",
                other.type_name()
            ));
        }
    }
    Ok(())
}

fn stringify_f32_slice(vals: &[f32], out: &mut String) {
    out.push('[');
    for (i, &f) in vals.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if f.is_finite() {
            out.push_str(&f.to_string());
        } else {
            out.push_str("null");
        }
    }
    out.push(']');
}

fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    let bytes = s.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' || b == b'\\' || b < 0x20 {
            if i > start {
                out.push_str(&s[start..i]);
            }
            match b {
                b'"' => out.push_str("\\\""),
                b'\\' => out.push_str("\\\\"),
                b'\n' => out.push_str("\\n"),
                b'\r' => out.push_str("\\r"),
                b'\t' => out.push_str("\\t"),
                c => {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                }
            }
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < bytes.len() {
        out.push_str(&s[start..]);
    }
    out.push('"');
}

const MAX_JSON_DEPTH: usize = 128;

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
    intern_cache: FastMap<&'a str, Rc<String>>,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
            depth: 0,
            intern_cache: HashMap::with_hasher(FxBuildHasher),
        }
    }

    #[inline(always)]
    fn is_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    #[inline(always)]
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    #[inline]
    fn skip_ws(&mut self) {
        let b = self.bytes;
        while self.pos < b.len() && matches!(b[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
    }

    #[inline]
    fn intern_str(&mut self, s: Cow<'a, str>) -> Rc<String> {
        match s {
            Cow::Borrowed(slice) => {
                if slice.len() <= 48 {
                    if let Some(cached) = self.intern_cache.get(slice) {
                        return Rc::clone(cached);
                    }
                    let rc = Rc::new(slice.to_string());
                    if self.intern_cache.len() < 512 {
                        self.intern_cache.insert(slice, Rc::clone(&rc));
                    }
                    rc
                } else {
                    Rc::new(slice.to_string())
                }
            }
            Cow::Owned(owned) => Rc::new(owned),
        }
    }

    fn parse_value(&mut self) -> Result<Value, String> {
        self.depth += 1;
        if self.depth > MAX_JSON_DEPTH {
            return Err(format!(
                "json.parse(): maximum nesting depth ({}) exceeded",
                MAX_JSON_DEPTH
            ));
        }
        self.skip_ws();
        let b = self
            .peek()
            .ok_or_else(|| "json.parse(): unexpected end of input".to_string())?;
        let res = match b {
            b'n' => self.parse_null(),
            b't' | b'f' => self.parse_bool(),
            b'"' => {
                let s = self.parse_string_cow()?;
                Ok(Value::Str(self.intern_str(s)))
            }
            b'[' => self.parse_array(),
            b'{' => self.parse_object(),
            b'-' | b'0'..=b'9' => self.parse_number(),
            other => Err(format!(
                "json.parse(): unexpected byte '{}' at pos {}",
                other as char, self.pos
            )),
        };
        self.depth -= 1;
        res
    }

    fn parse_null(&mut self) -> Result<Value, String> {
        if self.bytes[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(Value::Unit)
        } else {
            Err("json.parse(): invalid literal".to_string())
        }
    }

    fn parse_bool(&mut self) -> Result<Value, String> {
        if self.bytes[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(Value::Bool(true))
        } else if self.bytes[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(Value::Bool(false))
        } else {
            Err("json.parse(): invalid boolean literal".to_string())
        }
    }

    fn parse_number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
            }
            Some(b'1'..=b'9') => {
                while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
            }
            _ => return Err("json.parse(): invalid number".to_string()),
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            let digits = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
            if self.pos == digits {
                return Err("json.parse(): missing fraction digits".to_string());
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            let digits = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
            if self.pos == digits {
                return Err("json.parse(): missing exponent digits".to_string());
            }
        }
        let raw_num = &self.bytes[start..self.pos];
        if !is_float && raw_num.len() <= 18 {
            // Fast path for exact signed small/medium integers without std float/decimal parse overhead.
            let (neg, digits) = if raw_num[0] == b'-' {
                (true, &raw_num[1..])
            } else {
                (false, raw_num)
            };
            let mut acc: i64 = 0;
            for &d in digits {
                acc = acc * 10 + ((d - b'0') as i64);
            }
            return Ok(Value::Int(if neg { -acc } else { acc }));
        }
        let slice = std::str::from_utf8(raw_num)
            .map_err(|_| "json.parse(): invalid utf8 in number".to_string())?;
        if is_float {
            let f: f64 = slice
                .parse()
                .map_err(|_| format!("json.parse(): invalid float '{}'", slice))?;
            if !f.is_finite() {
                return Err("json.parse(): number out of range".to_string());
            }
            Ok(Value::Float(f))
        } else {
            let i: i64 = slice
                .parse()
                .map_err(|_| format!("json.parse(): invalid int '{}'", slice))?;
            Ok(Value::Int(i))
        }
    }

    fn parse_string_cow(&mut self) -> Result<Cow<'a, str>, String> {
        self.pos += 1; // skip '"'
        let start = self.pos;
        let bytes = self.bytes;
        let mut i = start;
        let mut has_non_ascii = false;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'"' {
                self.pos = i + 1;
                let s = if has_non_ascii {
                    std::str::from_utf8(&bytes[start..i])
                        .map_err(|_| "json.parse(): invalid UTF-8".to_string())?
                } else {
                    std::str::from_utf8(&bytes[start..i]).unwrap()
                };
                return Ok(Cow::Borrowed(s));
            }
            if b == b'\\' {
                break;
            }
            if b < 0x20 {
                return Err("json.parse(): unescaped control character".to_string());
            }
            if b >= 0x80 {
                has_non_ascii = true;
            }
            i += 1;
        }
        if i >= bytes.len() {
            return Err("json.parse(): unterminated string".to_string());
        }

        // Slow escape path: start with validated prefix up to backslash.
        let prefix = std::str::from_utf8(&bytes[start..i])
            .map_err(|_| "json.parse(): invalid UTF-8".to_string())?;
        let mut out = String::with_capacity(prefix.len() + 16);
        out.push_str(prefix);
        self.pos = i;

        while !self.is_eof() {
            let b = self.bytes[self.pos];
            self.pos += 1;
            if b == b'"' {
                return Ok(Cow::Owned(out));
            }
            if b == b'\\' {
                if self.is_eof() {
                    return Err("json.parse(): unterminated escape".to_string());
                }
                let esc = self.bytes[self.pos];
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
                        if self.pos + 4 > self.bytes.len() {
                            return Err("json.parse(): invalid \\u escape".to_string());
                        }
                        let hex = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4])
                            .map_err(|_| "json.parse(): invalid \\u escape".to_string())?;
                        let code = u32::from_str_radix(hex, 16)
                            .map_err(|_| "json.parse(): invalid \\u hex".to_string())?;
                        self.pos += 4;
                        if (0xD800..=0xDBFF).contains(&code)
                            && self.pos + 6 <= self.bytes.len()
                            && self.bytes[self.pos] == b'\\'
                            && self.bytes[self.pos + 1] == b'u'
                        {
                            let low_hex =
                                std::str::from_utf8(&self.bytes[self.pos + 2..self.pos + 6])
                                    .map_err(|_| "json.parse(): invalid \\u escape".to_string())?;
                            let low = u32::from_str_radix(low_hex, 16)
                                .map_err(|_| "json.parse(): invalid \\u hex".to_string())?;
                            if (0xDC00..=0xDFFF).contains(&low) {
                                let combined = 0x10000 + (((code - 0xD800) << 10) | (low - 0xDC00));
                                out.push(char::from_u32(combined).unwrap_or('\u{FFFD}'));
                                self.pos += 6;
                            } else {
                                out.push('\u{FFFD}');
                            }
                        } else {
                            out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        }
                    }
                    other => {
                        return Err(format!(
                            "json.parse(): unknown escape '\\{}'",
                            other as char
                        ));
                    }
                }
            } else if b < 0x20 {
                return Err("json.parse(): unescaped control character".to_string());
            } else if b < 0x80 {
                out.push(b as char);
            } else {
                let rest = std::str::from_utf8(&self.bytes[self.pos - 1..])
                    .map_err(|_| "json.parse(): invalid UTF-8".to_string())?;
                let ch = rest.chars().next().unwrap();
                out.push(ch);
                self.pos += ch.len_utf8() - 1;
            }
        }
        Err("json.parse(): unterminated string".to_string())
    }

    fn parse_array(&mut self) -> Result<Value, String> {
        self.pos += 1; // skip '['
        let mut items = Vec::with_capacity(8);
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::List(Rc::new(RefCell::new(items))));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err("json.parse(): expected ',' or ']' in array".to_string()),
            }
        }
        Ok(Value::List(Rc::new(RefCell::new(items))))
    }

    fn parse_object(&mut self) -> Result<Value, String> {
        self.pos += 1; // skip '{'
        let mut map = new_dict_with_capacity(4);
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::dict(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err("json.parse(): expected string key in object".to_string());
            }
            let key = self.parse_string_cow()?.into_owned();
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err("json.parse(): expected ':' after object key".to_string());
            }
            self.pos += 1;
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err("json.parse(): expected ',' or '}' in object".to_string()),
            }
        }
        Ok(Value::dict(map))
    }
}
