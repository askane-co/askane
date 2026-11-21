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

pub(crate) fn redact_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    while i < bytes.len() {
        let rest_lower = &lower[i..];
        let matched_key = [
            "bearer ",
            "token=",
            "secret=",
            "password=",
            "api_key=",
            "apikey=",
        ]
        .iter()
        .find(|kw| rest_lower.starts_with(*kw));

        if let Some(kw) = matched_key {
            out.push_str(&text[i..i + kw.len()]);
            out.push_str("[REDACTED]");
            i += kw.len();
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && bytes[i] != b'&'
                && bytes[i] != b'"'
            {
                i += 1;
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

fn format_structured(tag: &str, args: &[Value], as_json: bool) -> String {
    if args.len() == 2 {
        if let Value::Dict(fields_rc) = &args[1] {
            let msg = redact_secrets(&args[0].to_string());
            let map = fields_rc.borrow();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            if as_json {
                let mut pairs = Vec::with_capacity(keys.len());
                for k in keys {
                    let safe_v = if is_secret_field_key(k) {
                        "\"[REDACTED]\"".to_string()
                    } else {
                        match &map[k] {
                            Value::Int(i) => i.to_string(),
                            Value::Float(f) if f.is_finite() => f.to_string(),
                            Value::Bool(b) => b.to_string(),
                            Value::Unit => "null".to_string(),
                            other => format!("{:?}", redact_secrets(&other.to_string())),
                        }
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
                        redact_secrets(&map[k].to_string())
                    };
                    kvs.push(format!("{}={}", k, safe_v));
                }
                return format!("[{}] {} {}", tag, msg, kvs.join(" "));
            }
        }
    }
    let parts: Vec<String> = args.iter().map(|v| format!("{}", v)).collect();
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
