use std::cell::Cell;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "log",
        &[
            ("set_level", log_set_level),
            ("json", log_set_json),
            ("format", log_format),
            ("debug", log_debug),
            ("info", log_info),
            ("warn", log_warn),
            ("error", log_error),
        ],
        &[],
    )
}

thread_local! {
    // 0=debug, 1=info, 2=warn, 3=error
    static LOG_LEVEL: Cell<u8> = const { Cell::new(1) };
    static LOG_JSON: Cell<bool> = const { Cell::new(false) };
}

fn log_set_json(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("log.json() expects 1 boolean argument".to_string());
    }
    let Value::Bool(b) = args[0] else {
        return Err("log.json() expects a bool".to_string());
    };
    LOG_JSON.with(|c| c.set(b));
    Ok(Value::Unit)
}

fn log_set_level(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(
            "log.set_level() expects 1 string argument (debug|info|warn|error)".to_string(),
        );
    }
    let level = match &args[0] {
        Value::Str(s) => match s.to_ascii_lowercase().as_str() {
            "debug" => 0,
            "info" => 1,
            "warn" => 2,
            "error" => 3,
            other => return Err(format!("Unknown log level '{}'", other)),
        },
        other => {
            return Err(format!(
                "log.set_level() expects a string, got '{}'",
                other.type_name()
            ));
        }
    };
    LOG_LEVEL.with(|c| c.set(level));
    Ok(Value::Unit)
}

const SECRET_KEYS: &[&str] = &[
    "private_key",
    "authorization",
    "api_key",
    "apikey",
    "password",
    "passwd",
    "secret",
    "token",
    "auth",
];

fn match_secret_kv_prefix(rest_lower: &str) -> Option<(usize, usize)> {
    let bytes = rest_lower.as_bytes();
    for key in SECRET_KEYS {
        if rest_lower.starts_with(key) {
            let mut j = key.len();
            if bytes.get(j..j + 2) == Some(b"\\\"") {
                j += 2;
            } else if matches!(bytes.get(j), Some(b'"' | b'\'')) {
                j += 1;
            }
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                j += 1;
            }
            if matches!(bytes.get(j), Some(b'=' | b':')) {
                j += 1;
                while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                    j += 1;
                }
                return Some((key.len(), j));
            }
        }
    }
    None
}

fn consume_unquoted_secret(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len()
        && !bytes[i].is_ascii_whitespace()
        && !matches!(bytes[i], b'&' | b'"' | b'\'' | b',' | b'}' | b';')
    {
        i += 1;
    }
    i
}

pub(crate) fn redact_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    while i < bytes.len() {
        let rest_lower = &lower[i..];
        if rest_lower.starts_with("bearer ") {
            out.push_str(&text[i..i + 7]);
            out.push_str("[REDACTED]");
            i += 7;
            while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
                i += 1;
            }
            i = consume_unquoted_secret(bytes, i);
            continue;
        }
        if let Some((_klen, after_sep)) = match_secret_kv_prefix(rest_lower) {
            out.push_str(&text[i..i + after_sep]);
            i += after_sep;
            if bytes.get(i..i + 2) == Some(b"\\\"") {
                out.push_str("\\\"[REDACTED]");
                i += 2;
                let mut escaped = false;
                while i < bytes.len() {
                    if !escaped && bytes.get(i..i + 2) == Some(b"\\\"") {
                        out.push_str("\\\"");
                        i += 2;
                        break;
                    }
                    if !escaped && bytes[i] == b'\\' {
                        escaped = true;
                    } else {
                        escaped = false;
                    }
                    if matches!(bytes[i], b'\r' | b'\n') {
                        break;
                    }
                    i += 1;
                }
            } else if let Some(&q @ (b'"' | b'\'')) = bytes.get(i) {
                out.push(q as char);
                out.push_str("[REDACTED]");
                i += 1;
                let mut escaped = false;
                while i < bytes.len() {
                    if !escaped && bytes[i] == q {
                        out.push(q as char);
                        i += 1;
                        break;
                    }
                    if !escaped && bytes[i] == b'\\' {
                        escaped = true;
                    } else {
                        escaped = false;
                    }
                    if matches!(bytes[i], b'\r' | b'\n') {
                        break;
                    }
                    i += 1;
                }
            } else {
                out.push_str("[REDACTED]");
                if lower[i..].starts_with("bearer ") {
                    i += 7;
                    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
                        i += 1;
                    }
                }
                i = consume_unquoted_secret(bytes, i);
            }
        } else if let Some(ch) = text[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn is_secret_field_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("passwd")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("private_key")
        || lower.contains("auth")
}

fn redact_value_json(
    v: &Value,
    depth: usize,
    seen: &mut std::collections::HashSet<usize>,
) -> String {
    if depth >= 32 {
        return "\"[...]\"".to_string();
    }
    match v {
        Value::Int(i) => i.to_string(),
        Value::Float(f) if f.is_finite() => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Unit => "null".to_string(),
        Value::Dict(d) => {
            let ptr = std::rc::Rc::as_ptr(d) as usize;
            if !seen.insert(ptr) {
                return "\"[cycle]\"".to_string();
            }
            let out = if let Ok(map) = d.try_borrow() {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut pairs = Vec::with_capacity(keys.len());
                for k in keys {
                    let safe_v = if is_secret_field_key(k) {
                        "\"[REDACTED]\"".to_string()
                    } else {
                        redact_value_json(&map[k], depth + 1, seen)
                    };
                    pairs.push(format!("{:?}:{}", k, safe_v));
                }
                format!("{{{}}}", pairs.join(","))
            } else {
                "\"[borrowed]\"".to_string()
            };
            seen.remove(&ptr);
            out
        }
        Value::List(l) => {
            let ptr = std::rc::Rc::as_ptr(l) as usize;
            if !seen.insert(ptr) {
                return "\"[cycle]\"".to_string();
            }
            let out = if let Ok(items) = l.try_borrow() {
                let parts: Vec<String> = items
                    .iter()
                    .map(|item| redact_value_json(item, depth + 1, seen))
                    .collect();
                format!("[{}]", parts.join(","))
            } else {
                "\"[borrowed]\"".to_string()
            };
            seen.remove(&ptr);
            out
        }
        Value::StructInstance(inst) => {
            let ptr = std::rc::Rc::as_ptr(inst) as usize;
            if !seen.insert(ptr) {
                return "\"[cycle]\"".to_string();
            }
            let out = if let Ok(fields) = inst.fields.try_borrow() {
                let mut pairs = Vec::with_capacity(fields.len());
                for (idx, val) in fields.iter().enumerate() {
                    let name = inst
                        .def
                        .field_names
                        .get(idx)
                        .map(|s| s.as_str())
                        .unwrap_or("?");
                    let safe_v = if is_secret_field_key(name) {
                        "\"[REDACTED]\"".to_string()
                    } else {
                        redact_value_json(val, depth + 1, seen)
                    };
                    pairs.push(format!("{:?}:{}", name, safe_v));
                }
                format!("{{{}}}", pairs.join(","))
            } else {
                "\"[borrowed]\"".to_string()
            };
            seen.remove(&ptr);
            out
        }
        other => format!("{:?}", redact_secrets(&other.to_string())),
    }
}

fn redact_value_text(
    v: &Value,
    depth: usize,
    seen: &mut std::collections::HashSet<usize>,
) -> String {
    if depth >= 32 {
        return "[...]".to_string();
    }
    match v {
        Value::Dict(d) => {
            let ptr = std::rc::Rc::as_ptr(d) as usize;
            if !seen.insert(ptr) {
                return "[cycle]".to_string();
            }
            let out = if let Ok(map) = d.try_borrow() {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut pairs = Vec::with_capacity(keys.len());
                for k in keys {
                    let safe_v = if is_secret_field_key(k) {
                        "[REDACTED]".to_string()
                    } else {
                        redact_value_text(&map[k], depth + 1, seen)
                    };
                    pairs.push(format!("{}: {}", k, safe_v));
                }
                format!("#{{{}}}", pairs.join(", "))
            } else {
                "[borrowed]".to_string()
            };
            seen.remove(&ptr);
            out
        }
        Value::List(l) => {
            let ptr = std::rc::Rc::as_ptr(l) as usize;
            if !seen.insert(ptr) {
                return "[cycle]".to_string();
            }
            let out = if let Ok(items) = l.try_borrow() {
                let parts: Vec<String> = items
                    .iter()
                    .map(|item| redact_value_text(item, depth + 1, seen))
                    .collect();
                format!("[{}]", parts.join(", "))
            } else {
                "[borrowed]".to_string()
            };
            seen.remove(&ptr);
            out
        }
        Value::StructInstance(inst) => {
            let ptr = std::rc::Rc::as_ptr(inst) as usize;
            if !seen.insert(ptr) {
                return "[cycle]".to_string();
            }
            let out = if let Ok(fields) = inst.fields.try_borrow() {
                let mut pairs = Vec::with_capacity(fields.len());
                for (idx, val) in fields.iter().enumerate() {
                    let name = inst
                        .def
                        .field_names
                        .get(idx)
                        .map(|s| s.as_str())
                        .unwrap_or("?");
                    let safe_v = if is_secret_field_key(name) {
                        "[REDACTED]".to_string()
                    } else {
                        redact_value_text(val, depth + 1, seen)
                    };
                    pairs.push(format!("{}: {}", name, safe_v));
                }
                format!("{} {{{}}}", inst.def.name, pairs.join(", "))
            } else {
                "[borrowed]".to_string()
            };
            seen.remove(&ptr);
            out
        }
        other => redact_secrets(&other.to_string()),
    }
}

fn format_structured(tag: &str, args: &[Value], as_json: bool) -> String {
    let mut seen = std::collections::HashSet::new();
    if args.len() == 2 {
        if let Value::Dict(fields_rc) = &args[1] {
            let msg = redact_value_text(&args[0], 0, &mut seen);
            if let Ok(map) = fields_rc.try_borrow() {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                if as_json {
                    let mut pairs = Vec::with_capacity(keys.len());
                    for k in keys {
                        let safe_v = if is_secret_field_key(k) {
                            "\"[REDACTED]\"".to_string()
                        } else {
                            redact_value_json(&map[k], 0, &mut seen)
                        };
                        pairs.push(format!("{:?}:{}", k, safe_v));
                    }
                    return format!(
                        "{{\"level\":{:?},\"msg\":{:?},\"fields\":{{{}}}}}",
                        tag,
                        msg,
                        pairs.join(",")
                    );
                } else {
                    let mut kvs = Vec::with_capacity(keys.len());
                    for k in keys {
                        let safe_v = if is_secret_field_key(k) {
                            "[REDACTED]".to_string()
                        } else {
                            redact_value_text(&map[k], 0, &mut seen)
                        };
                        kvs.push(format!("{}={}", k, safe_v));
                    }
                    return format!("[{}] {} {}", tag, msg, kvs.join(" "));
                }
            }
        }
    }
    let parts: Vec<String> = args
        .iter()
        .map(|v| redact_value_text(v, 0, &mut seen))
        .collect();
    let msg = redact_secrets(&parts.join(" "));
    if as_json {
        format!("{{\"level\":{:?},\"msg\":{:?}}}", tag, msg)
    } else {
        format!("[{}] {}", tag, msg)
    }
}

fn log_format(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("log.format() expects (level, msg, fields?)".to_string());
    }
    let tag = args[0].to_string().to_ascii_uppercase();
    let as_json = LOG_JSON.with(|c| c.get());
    Ok(Value::Str(format_structured(&tag, &args[1..], as_json)))
}

fn emit_log(level: u8, tag: &str, args: &[Value]) {
    if LOG_LEVEL.with(|c| c.get()) > level {
        return;
    }
    let as_json = LOG_JSON.with(|c| c.get());
    let line = format_structured(tag, args, as_json);
    if level >= 2 {
        eprintln!("{}", line);
    } else {
        println!("{}", line);
    }
}

fn log_debug(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    emit_log(0, "DEBUG", args);
    Ok(Value::Unit)
}

fn log_info(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    emit_log(1, "INFO", args);
    Ok(Value::Unit)
}

fn log_warn(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    emit_log(2, "WARN", args);
    Ok(Value::Unit)
}

fn log_error(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    emit_log(3, "ERROR", args);
    Ok(Value::Unit)
}
