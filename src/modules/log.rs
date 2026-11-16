use std::cell::Cell;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "log",
        &[
            ("set_level", log_set_level),
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

fn redact_secrets(text: &str) -> String {
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

fn emit_log(level: u8, tag: &str, args: &[Value]) {
    if LOG_LEVEL.with(|c| c.get()) > level {
        return;
    }
    let parts: Vec<String> = args.iter().map(|v| format!("{}", v)).collect();
    let msg = redact_secrets(&parts.join(" "));
    if level >= 2 {
        eprintln!("[{}] {}", tag, msg);
    } else {
        println!("[{}] {}", tag, msg);
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
