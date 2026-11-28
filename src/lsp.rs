#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{mpsc, Mutex};
use std::thread;

use crate::ast::{ImportSource, Program, Stmt};
use crate::compiler::Compiler;
use crate::diag::{self, Diagnostic};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::{asset_audit, registry};

pub const MAX_LSP_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum JsonVal {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<JsonVal>),
    Obj(Vec<(String, JsonVal)>),
}

impl JsonVal {
    pub fn obj_get(&self, key: &str) -> Option<&JsonVal> {
        let JsonVal::Obj(pairs) = self else {
            return None;
        };
        pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonVal::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonVal::Num(s) => s.parse::<i64>().ok(),
            _ => None,
        }
    }

    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    fn write_json(&self, out: &mut String) {
        match self {
            JsonVal::Null => out.push_str("null"),
            JsonVal::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            JsonVal::Num(s) => out.push_str(s),
            JsonVal::Str(s) => write_json_str(s, out),
            JsonVal::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
            JsonVal::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_str(k, out);
                    out.push(':');
                    v.write_json(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_json_str(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
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

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn parse_val(&mut self, depth: usize) -> Result<JsonVal, String> {
        if depth > 64 {
            return Err("JSON nesting depth > 64".to_string());
        }
        self.skip_ws();
        let Some(b) = self.peek() else {
            return Err("Unexpected EOF in JSON".to_string());
        };
        match b {
            b'n' => {
                if self.bytes[self.pos..].starts_with(b"null") {
                    self.pos += 4;
                    Ok(JsonVal::Null)
                } else {
                    Err("Invalid literal in JSON".to_string())
                }
            }
            b't' => {
                if self.bytes[self.pos..].starts_with(b"true") {
                    self.pos += 4;
                    Ok(JsonVal::Bool(true))
                } else {
                    Err("Invalid literal in JSON".to_string())
                }
            }
            b'f' => {
                if self.bytes[self.pos..].starts_with(b"false") {
                    self.pos += 5;
                    Ok(JsonVal::Bool(false))
                } else {
                    Err("Invalid literal in JSON".to_string())
                }
            }
            b'"' => Ok(JsonVal::Str(self.parse_string()?)),
            b'[' => {
                self.pos += 1;
                self.skip_ws();
                let mut arr = Vec::new();
                if self.peek() == Some(b']') {
                    self.pos += 1;
                    return Ok(JsonVal::Arr(arr));
                }
                loop {
                    arr.push(self.parse_val(depth + 1)?);
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(JsonVal::Arr(arr));
                        }
                        _ => return Err("Expected ',' or ']' in JSON array".to_string()),
                    }
                }
            }
            b'{' => {
                self.pos += 1;
                self.skip_ws();
                let mut pairs = Vec::new();
                if self.peek() == Some(b'}') {
                    self.pos += 1;
                    return Ok(JsonVal::Obj(pairs));
                }
                loop {
                    self.skip_ws();
                    let k = self.parse_string()?;
                    self.skip_ws();
                    if self.peek() != Some(b':') {
                        return Err("Expected ':' in JSON object".to_string());
                    }
                    self.pos += 1;
                    let v = self.parse_val(depth + 1)?;
                    pairs.push((k, v));
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(JsonVal::Obj(pairs));
                        }
                        _ => return Err("Expected ',' or '}' in JSON object".to_string()),
                    }
                }
            }
            b'-' | b'0'..=b'9' => {
                let start = self.pos;
                if b == b'-' {
                    self.pos += 1;
                }
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-') {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let raw = std::str::from_utf8(&self.bytes[start..self.pos])
                    .map_err(|_| "Invalid UTF-8 number".to_string())?;
                Ok(JsonVal::Num(raw.to_string()))
            }
            other => Err(format!("Unexpected byte 0x{:02X} in JSON", other)),
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        if self.peek() != Some(b'"') {
            return Err("Expected '\"' in JSON".to_string());
        }
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                self.pos += 1;
                return Ok(out);
            }
            if b == b'\\' {
                self.pos += 1;
                let Some(esc) = self.peek() else {
                    return Err("Unterminated JSON string escape".to_string());
                };
                self.pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\x08'),
                    b'f' => out.push('\x0C'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let code = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&code)
                            && self.bytes[self.pos..].starts_with(b"\\u")
                        {
                            let saved = self.pos;
                            self.pos += 2;
                            let low = self.parse_hex4()?;
                            if (0xDC00..=0xDFFF).contains(&low) {
                                let cp = 0x10000
                                    + (((code as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
                                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                            } else {
                                self.pos = saved;
                                out.push('\u{FFFD}');
                            }
                        } else {
                            out.push(char::from_u32(code as u32).unwrap_or('\u{FFFD}'));
                        }
                    }
                    other => return Err(format!("Invalid JSON escape \\{}", other as char)),
                }
            } else {
                let s = std::str::from_utf8(&self.bytes[self.pos..])
                    .map_err(|_| "Invalid UTF-8 in JSON string".to_string())?;
                let ch = s.chars().next().unwrap();
                out.push(ch);
                self.pos += ch.len_utf8();
            }
        }
        Err("Unterminated JSON string".to_string())
    }

    fn parse_hex4(&mut self) -> Result<u16, String> {
        if self.pos + 4 > self.bytes.len() {
            return Err("Short \\uXXXX escape".to_string());
        }
        let s = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4])
            .map_err(|_| "Invalid UTF-8 in \\uXXXX".to_string())?;
        self.pos += 4;
        u16::from_str_radix(s, 16).map_err(|_| format!("Invalid hex '{}'", s))
    }
}

pub fn parse_json(src: &str) -> Result<JsonVal, String> {
    let mut p = JsonParser::new(src);
    let val = p.parse_val(0)?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err("Trailing characters after JSON payload".to_string());
    }
    Ok(val)
}

pub fn read_lsp_frame<R: BufRead>(r: &mut R) -> Result<Option<String>, String> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    let mut header_count = 0usize;

    loop {
        line.clear();
        let n = r
            .read_line(&mut line)
            .map_err(|e| format!("LSP header read error: {}", e))?;
        if n == 0 {
            if header_count == 0 {
                return Ok(None);
            }
            return Err("Unexpected EOF while reading LSP headers".to_string());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        header_count += 1;
        if header_count > 64 {
            return Err("Too many LSP headers".to_string());
        }
        if let Some((name, val)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                let len = val
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid Content-Length '{}'", val.trim()))?;
                if len > MAX_LSP_MESSAGE_BYTES {
                    return Err(format!(
                        "LSP payload size {} exceeds 64 MiB safety cap",
                        len
                    ));
                }
                content_length = Some(len);
            }
        }
    }

    let Some(len) = content_length else {
        return Err("Missing Content-Length header in LSP frame".to_string());
    };
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)
        .map_err(|e| format!("Unexpected EOF reading LSP body ({} bytes): {}", len, e))?;
    let body = String::from_utf8(buf).map_err(|_| "LSP message is not valid UTF-8".to_string())?;
    Ok(Some(body))
}

pub fn write_lsp_frame<W: Write>(w: &mut W, json_body: &str) -> Result<(), String> {
    let hdr = format!("Content-Length: {}\r\n\r\n", json_body.len());
    w.write_all(hdr.as_bytes())
        .and_then(|()| w.write_all(json_body.as_bytes()))
        .and_then(|()| w.flush())
        .map_err(|e| format!("LSP write failed: {}", e))
}

/// Convert 0-based Unicode scalar count on a line into 0-based UTF-16 code-unit offset.
pub fn utf16_col_for_char_index(line_text: &str, char_idx: usize) -> u32 {
    let mut u16_off = 0usize;
    for (idx, ch) in line_text.chars().enumerate() {
        if idx >= char_idx {
            break;
        }
        u16_off += ch.len_utf16();
    }
    u16_off as u32
}

/// Convert Askane 1-based Unicode scalar range (`line`, `col`, `end_col`) -> LSP 0-based UTF-16 coordinates.
pub fn diag_to_utf16_range(source: &str, d: &Diagnostic) -> (u32, u32, u32, u32) {
    let line0 = d.line.saturating_sub(1) as u32;
    let line_text = source.lines().nth(line0 as usize).unwrap_or("");
    let start_char_idx = d.col.saturating_sub(1);
    let end_char_idx = d.end_col.max(d.col);
    let start_u16 = utf16_col_for_char_index(line_text, start_char_idx);
    let end_u16 = utf16_col_for_char_index(line_text, end_char_idx).max(start_u16);
    (line0, start_u16, line0, end_u16)
}

/// Convert 0-based UTF-16 offset on a line into 0-based Unicode scalar index.
pub fn char_index_for_utf16_col(line_text: &str, utf16_col: u32) -> usize {
    let target = utf16_col as usize;
    let mut acc = 0usize;
    for (idx, ch) in line_text.chars().enumerate() {
        if acc >= target {
            return idx;
        }
        acc += ch.len_utf16();
    }
    line_text.chars().count()
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file:///").or_else(|| uri.strip_prefix("file://"))?;
    let mut decoded = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    decoded.push(v as char);
                    i += 3;
                    continue;
                }
            }
        }
        decoded.push(bytes[i] as char);
        i += 1;
    }
    #[cfg(not(windows))]
    if uri.starts_with("file:///") && !decoded.starts_with('/') {
        decoded.insert(0, '/');
    }
    Some(PathBuf::from(decoded))
}

fn analyze_source(uri: &str, source: &str) -> Vec<Diagnostic> {
    let path_opt = uri_to_path(uri);
    let label = path_opt
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| uri.to_string());

    let tokens = match Lexer::new(source).tokenize() {
        Ok(t) => t,
        Err(e) => return vec![diag::classify_error(&e, Some(&label))],
    };

    let (program, parse_errs) = Parser::new(tokens).parse_program_recover();
    let mut diags: Vec<Diagnostic> = parse_errs
        .iter()
        .map(|e| diag::classify_error(e, Some(&label)))
        .collect();

    diags.extend(diag::analyze_warnings(&program, Some(&label)));
    let asset_rep = asset_audit::audit_program(&program, path_opt.as_deref());
    diags.extend(asset_rep.errors);
    diags.extend(asset_rep.warnings);

    if parse_errs.is_empty() {
        if let Err(e) = Compiler::compile_with_specs(&program) {
            diags.push(diag::classify_error(&e, Some(&label)));
        }
    }
    diags
}

fn diag_to_lsp_json(source: &str, d: &Diagnostic) -> JsonVal {
    let (sl, sc, el, ec) = diag_to_utf16_range(source, d);
    let sev = if d.severity == "warning" { "2" } else { "1" };
    let msg = match &d.hint {
        Some(h) if !h.is_empty() => format!("{}\nhint: {}", d.message, h),
        _ => d.message.clone(),
    };
    JsonVal::Obj(vec![
        (
            "range".to_string(),
            lsp_range(sl, sc, el, ec),
        ),
        ("severity".to_string(), JsonVal::Num(sev.to_string())),
        ("code".to_string(), JsonVal::Str(d.code.to_string())),
        ("source".to_string(), JsonVal::Str("askane".to_string())),
        ("message".to_string(), JsonVal::Str(msg)),
    ])
}

fn lsp_range(sl: u32, sc: u32, el: u32, ec: u32) -> JsonVal {
    JsonVal::Obj(vec![
        (
            "start".to_string(),
            JsonVal::Obj(vec![
                ("line".to_string(), JsonVal::Num(sl.to_string())),
                ("character".to_string(), JsonVal::Num(sc.to_string())),
            ]),
        ),
        (
            "end".to_string(),
            JsonVal::Obj(vec![
                ("line".to_string(), JsonVal::Num(el.to_string())),
                ("character".to_string(), JsonVal::Num(ec.to_string())),
            ]),
        ),
    ])
}

struct DocSummary {
    imports: HashMap<String, String>,
    symbols: Vec<JsonVal>,
    fn_sigs: HashMap<String, String>,
    local_idents: Vec<String>,
}

fn summarize_document(source: &str) -> DocSummary {
    let mut imports = HashMap::new();
    let mut symbols = Vec::new();
    let mut fn_sigs = HashMap::new();
    let mut local_idents = Vec::new();
    let mut seen_idents = HashSet::new();

    if let Ok(tokens) = Lexer::new(source).tokenize() {
        let (program, _) = Parser::new(tokens).parse_program_recover();
        collect_program_summary(
            source,
            &program,
            &mut imports,
            &mut symbols,
            &mut fn_sigs,
            &mut local_idents,
            &mut seen_idents,
        );
    }

    DocSummary {
        imports,
        symbols,
        fn_sigs,
        local_idents,
    }
}

fn span_point_range(source: &str, line1: usize, col1: usize, name_len: usize) -> JsonVal {
    let l0 = line1.saturating_sub(1);
    let line_text = source.lines().nth(l0).unwrap_or("");
    let c0 = col1.saturating_sub(1);
    let s16 = utf16_col_for_char_index(line_text, c0);
    let e16 = utf16_col_for_char_index(line_text, c0 + name_len.max(1));
    lsp_range(l0 as u32, s16, l0 as u32, e16)
}

fn collect_program_summary(
    source: &str,
    prog: &Program,
    imports: &mut HashMap<String, String>,
    symbols: &mut Vec<JsonVal>,
    fn_sigs: &mut HashMap<String, String>,
    local_idents: &mut Vec<String>,
    seen_idents: &mut HashSet<String>,
) {
    for stmt in &prog.stmts {
        match stmt {
            Stmt::Import {
                source: ImportSource::Name(mname),
                alias,
                ..
            } => {
                imports.insert(alias.clone(), mname.clone());
                if seen_idents.insert(alias.clone()) {
                    local_idents.push(alias.clone());
                }
            }
            Stmt::FnDecl {
                name,
                params,
                span,
                ..
            } => {
                let plist: Vec<String> = params
                    .iter()
                    .map(|p| {
                        if p.is_variadic {
                            format!("...{}", p.name)
                        } else {
                            p.name.clone()
                        }
                    })
                    .collect();
                let sig = format!("fn {}({})", name, plist.join(", "));
                fn_sigs.insert(name.clone(), sig);
                if seen_idents.insert(name.clone()) {
                    local_idents.push(name.clone());
                }
                let r = span_point_range(source, span.line, span.col, name.chars().count());
                symbols.push(JsonVal::Obj(vec![
                    ("name".to_string(), JsonVal::Str(name.clone())),
                    ("kind".to_string(), JsonVal::Num("12".to_string())), // Function
                    ("range".to_string(), r.clone()),
                    ("selectionRange".to_string(), r),
                ]));
            }
            Stmt::StructDecl {
                name,
                fields,
                methods,
                span,
                ..
            } => {
                let flist: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
                fn_sigs.insert(
                    name.clone(),
                    format!("struct {} {{ {} }}", name, flist.join(", ")),
                );
                if seen_idents.insert(name.clone()) {
                    local_idents.push(name.clone());
                }
                let mut children = Vec::new();
                for m in methods {
                    let mr = span_point_range(
                        source,
                        m.span.line,
                        m.span.col,
                        m.name.chars().count(),
                    );
                    children.push(JsonVal::Obj(vec![
                        ("name".to_string(), JsonVal::Str(m.name.clone())),
                        ("kind".to_string(), JsonVal::Num("6".to_string())), // Method
                        ("range".to_string(), mr.clone()),
                        ("selectionRange".to_string(), mr),
                    ]));
                }
                let sr = span_point_range(source, span.line, span.col, name.chars().count());
                symbols.push(JsonVal::Obj(vec![
                    ("name".to_string(), JsonVal::Str(name.clone())),
                    ("kind".to_string(), JsonVal::Num("23".to_string())), // Struct
                    ("range".to_string(), sr.clone()),
                    ("selectionRange".to_string(), sr),
                    ("children".to_string(), JsonVal::Arr(children)),
                ]));
            }
            Stmt::Let {
                name,
                name_span,
                ..
            } => {
                if seen_idents.insert(name.clone()) {
                    local_idents.push(name.clone());
                }
                let lr = span_point_range(
                    source,
                    name_span.line,
                    name_span.col,
                    name.chars().count(),
                );
                symbols.push(JsonVal::Obj(vec![
                    ("name".to_string(), JsonVal::Str(name.clone())),
                    ("kind".to_string(), JsonVal::Num("13".to_string())), // Variable
                    ("range".to_string(), lr.clone()),
                    ("selectionRange".to_string(), lr),
                ]));
            }
            Stmt::Spec { name, span, .. } => {
                let sr = span_point_range(source, span.line, span.col, name.chars().count());
                symbols.push(JsonVal::Obj(vec![
                    (
                        "name".to_string(),
                        JsonVal::Str(format!("spec \"{}\"", name)),
                    ),
                    ("kind".to_string(), JsonVal::Num("12".to_string())),
                    ("range".to_string(), sr.clone()),
                    ("selectionRange".to_string(), sr),
                ]));
            }
            _ => {}
        }
    }
}

fn word_at_cursor(source: &str, line0: u32, u16_char: u32) -> (Option<String>, String) {
    let Some(line_str) = source.lines().nth(line0 as usize) else {
        return (None, String::new());
    };
    let chars: Vec<char> = line_str.chars().collect();
    let idx = char_index_for_utf16_col(line_str, u16_char).min(chars.len());
    let mut start = idx;
    while start > 0 && (chars[start - 1].is_ascii_alphanumeric() || chars[start - 1] == '_') {
        start -= 1;
    }
    let mut end = idx;
    while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
        end += 1;
    }
    let word: String = chars[start..end].iter().collect();
    let qualifier = if start >= 2 && chars[start - 1] == '.' {
        let mut qstart = start - 1;
        while qstart > 0
            && (chars[qstart - 1].is_ascii_alphanumeric() || chars[qstart - 1] == '_')
        {
            qstart -= 1;
        }
        let q: String = chars[qstart..start - 1].iter().collect();
        if q.is_empty() { None } else { Some(q) }
    } else {
        None
    };
    (qualifier, word)
}

fn dot_prefix_before_cursor(source: &str, line0: u32, u16_char: u32) -> Option<String> {
    let line_str = source.lines().nth(line0 as usize)?;
    let chars: Vec<char> = line_str.chars().collect();
    let mut idx = char_index_for_utf16_col(line_str, u16_char).min(chars.len());
    while idx > 0 && (chars[idx - 1].is_ascii_alphanumeric() || chars[idx - 1] == '_') {
        idx -= 1;
    }
    if idx >= 2 && chars[idx - 1] == '.' {
        let mut qstart = idx - 1;
        while qstart > 0
            && (chars[qstart - 1].is_ascii_alphanumeric() || chars[qstart - 1] == '_')
        {
            qstart -= 1;
        }
        let q: String = chars[qstart..idx - 1].iter().collect();
        if !q.is_empty() {
            return Some(q);
        }
    }
    None
}

fn handle_completion(source: &str, line0: u32, u16_char: u32) -> JsonVal {
    let summary = summarize_document(source);
    let mut items = Vec::new();

    if let Some(qual) = dot_prefix_before_cursor(source, line0, u16_char) {
        let real_mod = summary
            .imports
            .get(&qual)
            .map(String::as_str)
            .unwrap_or(qual.as_str());
        for doc in registry::module_exports_docs(real_mod) {
            let cap_note = doc
                .capability
                .map(|c| format!("\n\nCapability: `{}`", c))
                .unwrap_or_default();
            let detail = format!("{} — {}{}", doc.signature, doc.summary, cap_note);
            let kind = if doc.signature.contains(": ") && !doc.signature.contains('(') {
                "21" // Constant
            } else {
                "3" // Function
            };
            items.push(JsonVal::Obj(vec![
                ("label".to_string(), JsonVal::Str(doc.name.to_string())),
                ("kind".to_string(), JsonVal::Num(kind.to_string())),
                ("detail".to_string(), JsonVal::Str(doc.signature.to_string())),
                ("documentation".to_string(), JsonVal::Str(detail)),
            ]));
        }
        return JsonVal::Arr(items);
    }

    for kw in registry::KEYWORDS {
        items.push(JsonVal::Obj(vec![
            ("label".to_string(), JsonVal::Str(kw.word.to_string())),
            ("kind".to_string(), JsonVal::Num("14".to_string())), // Keyword
            ("detail".to_string(), JsonVal::Str(kw.kind.to_string())),
            (
                "documentation".to_string(),
                JsonVal::Str(kw.summary.to_string()),
            ),
        ]));
    }
    for &(name, sig, doc) in registry::GLOBAL_BUILTINS {
        items.push(JsonVal::Obj(vec![
            ("label".to_string(), JsonVal::Str(name.to_string())),
            ("kind".to_string(), JsonVal::Num("3".to_string())),
            ("detail".to_string(), JsonVal::Str(sig.to_string())),
            ("documentation".to_string(), JsonVal::Str(doc.to_string())),
        ]));
    }
    for m in registry::REGISTRY {
        items.push(JsonVal::Obj(vec![
            ("label".to_string(), JsonVal::Str(m.name.to_string())),
            ("kind".to_string(), JsonVal::Num("9".to_string())), // Module
            (
                "detail".to_string(),
                JsonVal::Str(format!("module {} [{}]", m.name, m.platform)),
            ),
            (
                "documentation".to_string(),
                JsonVal::Str(m.summary.to_string()),
            ),
        ]));
    }
    for local in summary.local_idents {
        items.push(JsonVal::Obj(vec![
            ("label".to_string(), JsonVal::Str(local)),
            ("kind".to_string(), JsonVal::Num("6".to_string())),
        ]));
    }
    JsonVal::Arr(items)
}

fn handle_hover(source: &str, line0: u32, u16_char: u32) -> JsonVal {
    let summary = summarize_document(source);
    let (qual_opt, word) = word_at_cursor(source, line0, u16_char);
    if word.is_empty() {
        return JsonVal::Null;
    }
    let md_text = if let Some(qual) = qual_opt {
        let real_mod = summary
            .imports
            .get(&qual)
            .map(String::as_str)
            .unwrap_or(qual.as_str());
        if let Some(fd) = registry::lookup_fn_doc(real_mod, &word) {
            let cap = fd
                .capability
                .map(|c| format!("\n\nCapability required: `{}`", c))
                .unwrap_or_default();
            Some(format!(
                "```askane\n{}\n```\n\n{}{}",
                fd.signature, fd.summary, cap
            ))
        } else {
            None
        }
    } else if let Some(sig) = summary.fn_sigs.get(&word) {
        Some(format!("```askane\n{}\n```", sig))
    } else if let Some(mod_doc) = registry::lookup_module(&word) {
        Some(format!(
            "**module `{}`** (`{}` / {})\n\n{}",
            mod_doc.name, mod_doc.category, mod_doc.platform, mod_doc.summary
        ))
    } else if let Some((sig, doc)) = registry::lookup_global_doc(&word) {
        Some(format!("```askane\n{}\n```\n\n{}", sig, doc))
    } else if let Some(kw) = registry::lookup_keyword_doc(&word) {
        Some(format!("**{} `{}`**\n\n{}", kw.kind, kw.word, kw.summary))
    } else if let Some(real_mod) = summary.imports.get(&word) {
        registry::lookup_module(real_mod).map(|mod_doc| {
            format!(
                "**import `{}` -> module `{}`** (`{}`)\n\n{}",
                word, mod_doc.name, mod_doc.platform, mod_doc.summary
            )
        })
    } else {
        None
    };

    let Some(text) = md_text else {
        return JsonVal::Null;
    };
    JsonVal::Obj(vec![(
        "contents".to_string(),
        JsonVal::Obj(vec![
            ("kind".to_string(), JsonVal::Str("markdown".to_string())),
            ("value".to_string(), JsonVal::Str(text)),
        ]),
    )])
}

fn handle_formatting(source: &str) -> JsonVal {
    let Ok(tokens) = Lexer::new(source).tokenize() else {
        return JsonVal::Arr(Vec::new());
    };
    let mut parser = Parser::new(tokens);
    if parser.parse_program().is_err() {
        return JsonVal::Arr(Vec::new());
    }
    let formatted = crate::format_askane_source_with_indent(source, 4);
    if formatted == source {
        return JsonVal::Arr(Vec::new());
    }
    let line_count = source.split('\n').count();
    let last_line_0 = line_count.saturating_sub(1) as u32;
    let last_line_text = source.split('\n').last().unwrap_or("");
    let last_u16 = utf16_col_for_char_index(last_line_text, last_line_text.chars().count());
    JsonVal::Arr(vec![JsonVal::Obj(vec![
        (
            "range".to_string(),
            lsp_range(0, 0, last_line_0, last_u16),
        ),
        ("newText".to_string(), JsonVal::Str(formatted)),
    ])])
}

fn rpc_response(id: JsonVal, result: JsonVal) -> String {
    JsonVal::Obj(vec![
        ("jsonrpc".to_string(), JsonVal::Str("2.0".to_string())),
        ("id".to_string(), id),
        ("result".to_string(), result),
    ])
    .to_json()
}

fn rpc_notification(method: &str, params: JsonVal) -> String {
    JsonVal::Obj(vec![
        ("jsonrpc".to_string(), JsonVal::Str("2.0".to_string())),
        ("method".to_string(), JsonVal::Str(method.to_string())),
        ("params".to_string(), params),
    ])
    .to_json()
}

enum WorkerMsg {
    Analyze { uri: String, text: String },
    Clear { uri: String },
    Stop,
}

fn write_locked<W: Write>(out: &Mutex<W>, json_body: &str) -> Result<(), String> {
    let mut guard = out.lock().map_err(|_| "LSP output lock poisoned".to_string())?;
    write_lsp_frame(&mut *guard, json_body)
}

pub fn serve_lsp_io<R: BufRead, W: Write + Send>(mut input: R, output: W) -> Result<(), String> {
    let writer = Mutex::new(output);
    let (tx, rx) = mpsc::channel::<WorkerMsg>();

    thread::scope(|scope| {
        let writer_ref = &writer;
        let worker_handle = scope.spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut latest: BTreeMap<String, Option<String>> = BTreeMap::new();
                let mut stop = false;
                let mut process_msg = |msg: WorkerMsg| match msg {
                    WorkerMsg::Analyze { uri, text } => {
                        latest.insert(uri, Some(text));
                    }
                    WorkerMsg::Clear { uri } => {
                        latest.insert(uri, None);
                    }
                    WorkerMsg::Stop => {
                        stop = true;
                    }
                };
                process_msg(first);
                while let Ok(more) = rx.try_recv() {
                    process_msg(more);
                }

                for (uri, text_opt) in latest {
                    let diag_vals = if let Some(ref text) = text_opt {
                        analyze_source(&uri, text)
                            .iter()
                            .map(|d| diag_to_lsp_json(text, d))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let notif = rpc_notification(
                        "textDocument/publishDiagnostics",
                        JsonVal::Obj(vec![
                            ("uri".to_string(), JsonVal::Str(uri)),
                            ("diagnostics".to_string(), JsonVal::Arr(diag_vals)),
                        ]),
                    );
                    let _ = write_locked(writer_ref, &notif);
                }

                if stop {
                    break;
                }
            }
        });

        let mut docs: HashMap<String, String> = HashMap::new();
        let mut shutdown_requested = false;

        loop {
            let msg_str = match read_lsp_frame(&mut input) {
                Ok(Some(s)) => s,
                Ok(None) => break,
                Err(_) => break,
            };
            let Ok(msg) = parse_json(&msg_str) else {
                continue;
            };
            let method = msg
                .obj_get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let id_opt = msg.obj_get("id").cloned();
            let params = msg.obj_get("params");

            match method {
                "initialize" => {
                    let caps = JsonVal::Obj(vec![
                        (
                            "capabilities".to_string(),
                            JsonVal::Obj(vec![
                                ("textDocumentSync".to_string(), JsonVal::Num("1".to_string())),
                                ("hoverProvider".to_string(), JsonVal::Bool(true)),
                                ("documentSymbolProvider".to_string(), JsonVal::Bool(true)),
                                ("documentFormattingProvider".to_string(), JsonVal::Bool(true)),
                                (
                                    "completionProvider".to_string(),
                                    JsonVal::Obj(vec![(
                                        "triggerCharacters".to_string(),
                                        JsonVal::Arr(vec![JsonVal::Str(".".to_string())]),
                                    )]),
                                ),
                            ]),
                        ),
                        (
                            "serverInfo".to_string(),
                            JsonVal::Obj(vec![
                                ("name".to_string(), JsonVal::Str("askane-lsp".to_string())),
                                (
                                    "version".to_string(),
                                    JsonVal::Str(crate::VERSION.to_string()),
                                ),
                            ]),
                        ),
                    ]);
                    if let Some(id) = id_opt {
                        let _ = write_locked(writer_ref, &rpc_response(id, caps));
                    }
                }
                "initialized" => {}
                "shutdown" => {
                    shutdown_requested = true;
                    if let Some(id) = id_opt {
                        let _ = write_locked(writer_ref, &rpc_response(id, JsonVal::Null));
                    }
                }
                "exit" => {
                    break;
                }
                "textDocument/didOpen" => {
                    if let Some(td) = params.and_then(|p| p.obj_get("textDocument")) {
                        if let (Some(uri), Some(text)) = (
                            td.obj_get("uri").and_then(|v| v.as_str()),
                            td.obj_get("text").and_then(|v| v.as_str()),
                        ) {
                            docs.insert(uri.to_string(), text.to_string());
                            let _ = tx.send(WorkerMsg::Analyze {
                                uri: uri.to_string(),
                                text: text.to_string(),
                            });
                        }
                    }
                }
                "textDocument/didChange" => {
                    if let Some(p) = params {
                        let uri = p
                            .obj_get("textDocument")
                            .and_then(|td| td.obj_get("uri"))
                            .and_then(|v| v.as_str());
                        let text = p
                            .obj_get("contentChanges")
                            .and_then(|cc| match cc {
                                JsonVal::Arr(a) => a.last(),
                                _ => None,
                            })
                            .and_then(|c| c.obj_get("text"))
                            .and_then(|v| v.as_str());
                        if let (Some(u), Some(t)) = (uri, text) {
                            docs.insert(u.to_string(), t.to_string());
                            let _ = tx.send(WorkerMsg::Analyze {
                                uri: u.to_string(),
                                text: t.to_string(),
                            });
                        }
                    }
                }
                "textDocument/didSave" => {
                    if let Some(p) = params {
                        let uri = p
                            .obj_get("textDocument")
                            .and_then(|td| td.obj_get("uri"))
                            .and_then(|v| v.as_str());
                        if let Some(u) = uri {
                            if let Some(text) = p
                                .obj_get("text")
                                .and_then(|v| v.as_str())
                                .map(str::to_string)
                                .or_else(|| docs.get(u).cloned())
                            {
                                docs.insert(u.to_string(), text.clone());
                                let _ = tx.send(WorkerMsg::Analyze {
                                    uri: u.to_string(),
                                    text,
                                });
                            }
                        }
                    }
                }
                "textDocument/didClose" => {
                    if let Some(uri) = params
                        .and_then(|p| p.obj_get("textDocument"))
                        .and_then(|td| td.obj_get("uri"))
                        .and_then(|v| v.as_str())
                    {
                        docs.remove(uri);
                        let _ = tx.send(WorkerMsg::Clear {
                            uri: uri.to_string(),
                        });
                    }
                }
                "textDocument/documentSymbol" => {
                    let res = if let Some(uri) = params
                        .and_then(|p| p.obj_get("textDocument"))
                        .and_then(|td| td.obj_get("uri"))
                        .and_then(|v| v.as_str())
                    {
                        if let Some(src) = docs.get(uri) {
                            JsonVal::Arr(summarize_document(src).symbols)
                        } else {
                            JsonVal::Arr(Vec::new())
                        }
                    } else {
                        JsonVal::Arr(Vec::new())
                    };
                    if let Some(id) = id_opt {
                        let _ = write_locked(writer_ref, &rpc_response(id, res));
                    }
                }
                "textDocument/completion" => {
                    let res = if let Some(p) = params {
                        let uri = p
                            .obj_get("textDocument")
                            .and_then(|td| td.obj_get("uri"))
                            .and_then(|v| v.as_str());
                        let pos = p.obj_get("position");
                        let line0 = pos
                            .and_then(|x| x.obj_get("line"))
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0)
                            .max(0) as u32;
                        let ch0 = pos
                            .and_then(|x| x.obj_get("character"))
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0)
                            .max(0) as u32;
                        if let Some(src) = uri.and_then(|u| docs.get(u)) {
                            handle_completion(src, line0, ch0)
                        } else {
                            JsonVal::Arr(Vec::new())
                        }
                    } else {
                        JsonVal::Arr(Vec::new())
                    };
                    if let Some(id) = id_opt {
                        let _ = write_locked(writer_ref, &rpc_response(id, res));
                    }
                }
                "textDocument/hover" => {
                    let res = if let Some(p) = params {
                        let uri = p
                            .obj_get("textDocument")
                            .and_then(|td| td.obj_get("uri"))
                            .and_then(|v| v.as_str());
                        let pos = p.obj_get("position");
                        let line0 = pos
                            .and_then(|x| x.obj_get("line"))
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0)
                            .max(0) as u32;
                        let ch0 = pos
                            .and_then(|x| x.obj_get("character"))
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0)
                            .max(0) as u32;
                        if let Some(src) = uri.and_then(|u| docs.get(u)) {
                            handle_hover(src, line0, ch0)
                        } else {
                            JsonVal::Null
                        }
                    } else {
                        JsonVal::Null
                    };
                    if let Some(id) = id_opt {
                        let _ = write_locked(writer_ref, &rpc_response(id, res));
                    }
                }
                "textDocument/formatting" => {
                    let res = if let Some(uri) = params
                        .and_then(|p| p.obj_get("textDocument"))
                        .and_then(|td| td.obj_get("uri"))
                        .and_then(|v| v.as_str())
                    {
                        if let Some(src) = docs.get(uri) {
                            handle_formatting(src)
                        } else {
                            JsonVal::Arr(Vec::new())
                        }
                    } else {
                        JsonVal::Arr(Vec::new())
                    };
                    if let Some(id) = id_opt {
                        let _ = write_locked(writer_ref, &rpc_response(id, res));
                    }
                }
                _ => {
                    if let Some(id) = id_opt {
                        let err_obj = JsonVal::Obj(vec![
                            ("jsonrpc".to_string(), JsonVal::Str("2.0".to_string())),
                            ("id".to_string(), id),
                            (
                                "error".to_string(),
                                JsonVal::Obj(vec![
                                    ("code".to_string(), JsonVal::Num("-32601".to_string())),
                                    (
                                        "message".to_string(),
                                        JsonVal::Str(format!("Method not found: {}", method)),
                                    ),
                                ]),
                            ),
                        ])
                        .to_json();
                        let _ = write_locked(writer_ref, &err_obj);
                    }
                }
            }
        }

        let _ = tx.send(WorkerMsg::Stop);
        drop(tx);
        let _ = worker_handle.join();
        let _ = shutdown_requested;
    });

    Ok(())
}

pub fn run_lsp_stdio() -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve_lsp_io(stdin.lock(), stdout)
}
