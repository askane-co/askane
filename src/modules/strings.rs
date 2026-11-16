use std::cell::RefCell;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "strings",
        &[
            ("split", str_split),
            ("trim", str_trim),
            ("trim_start", str_trim_start),
            ("trim_end", str_trim_end),
            ("replace", str_replace),
            ("contains", str_contains),
            ("starts_with", str_starts_with),
            ("ends_with", str_ends_with),
            ("upper", str_upper),
            ("lower", str_lower),
            ("find", str_find),
            ("repeat", str_repeat),
            ("lines", str_lines),
            ("pad_start", str_pad_start),
            ("pad_end", str_pad_end),
            ("slice", str_slice),
            ("join", str_join),
        ],
        &[],
    )
}

fn expect_str<'a>(v: &'a Value, fn_name: &str) -> Result<&'a str, String> {
    match v {
        Value::Str(s) => Ok(s.as_str()),
        other => Err(format!(
            "strings.{}() expects a string, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn str_split(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.split() expects 2 arguments (text, sep)".to_string());
    }
    let text = expect_str(&args[0], "split")?;
    let sep = expect_str(&args[1], "split")?;
    let parts: Vec<Value> = if sep.is_empty() {
        text.chars().map(|c| Value::Str(c.to_string())).collect()
    } else {
        text.split(sep).map(|p| Value::Str(p.to_string())).collect()
    };
    Ok(Value::List(Rc::new(RefCell::new(parts))))
}

fn str_trim(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.trim() expects 1 argument (text)".to_string());
    }
    Ok(Value::Str(expect_str(&args[0], "trim")?.trim().to_string()))
}

fn str_trim_start(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.trim_start() expects 1 argument (text)".to_string());
    }
    Ok(Value::Str(
        expect_str(&args[0], "trim_start")?.trim_start().to_string(),
    ))
}

fn str_trim_end(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.trim_end() expects 1 argument (text)".to_string());
    }
    Ok(Value::Str(
        expect_str(&args[0], "trim_end")?.trim_end().to_string(),
    ))
}

fn str_replace(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("strings.replace() expects 3 arguments (text, from, to)".to_string());
    }
    let text = expect_str(&args[0], "replace")?;
    let from = expect_str(&args[1], "replace")?;
    let to = expect_str(&args[2], "replace")?;
    Ok(Value::Str(text.replace(from, to)))
}

fn str_contains(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.contains() expects 2 arguments (text, sub)".to_string());
    }
    let text = expect_str(&args[0], "contains")?;
    let sub = expect_str(&args[1], "contains")?;
    Ok(Value::Bool(text.contains(sub)))
}

fn str_starts_with(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.starts_with() expects 2 arguments (text, prefix)".to_string());
    }
    let text = expect_str(&args[0], "starts_with")?;
    let prefix = expect_str(&args[1], "starts_with")?;
    Ok(Value::Bool(text.starts_with(prefix)))
}

fn str_ends_with(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.ends_with() expects 2 arguments (text, suffix)".to_string());
    }
    let text = expect_str(&args[0], "ends_with")?;
    let suffix = expect_str(&args[1], "ends_with")?;
    Ok(Value::Bool(text.ends_with(suffix)))
}

fn str_upper(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.upper() expects 1 argument (text)".to_string());
    }
    Ok(Value::Str(expect_str(&args[0], "upper")?.to_uppercase()))
}

fn str_lower(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.lower() expects 1 argument (text)".to_string());
    }
    Ok(Value::Str(expect_str(&args[0], "lower")?.to_lowercase()))
}

fn str_find(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.find() expects 2 arguments (text, sub)".to_string());
    }
    let text = expect_str(&args[0], "find")?;
    let sub = expect_str(&args[1], "find")?;
    match text.find(sub) {
        Some(byte_idx) => Ok(Value::Int(text[..byte_idx].chars().count() as i64)),
        None => Ok(Value::Int(-1)),
    }
}

fn str_repeat(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.repeat() expects 2 arguments (text, count)".to_string());
    }
    let text = expect_str(&args[0], "repeat")?;
    let count = match &args[1] {
        Value::Int(n) if *n >= 0 => *n as usize,
        _ => return Err("strings.repeat() expects a non-negative integer count".to_string()),
    };
    if text.len().saturating_mul(count) > 16 * 1024 * 1024 {
        return Err("strings.repeat() output exceeds 16 MiB limit".to_string());
    }
    Ok(Value::Str(text.repeat(count)))
}

fn str_lines(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.lines() expects 1 argument (text)".to_string());
    }
    let text = expect_str(&args[0], "lines")?;
    let lines: Vec<Value> = text.lines().map(|l| Value::Str(l.to_string())).collect();
    Ok(Value::List(Rc::new(RefCell::new(lines))))
}

fn pad_impl(text: &str, width: usize, fill: &str, start: bool) -> Result<String, String> {
    if width > 1_000_000 {
        return Err("strings.pad() width exceeds limit".to_string());
    }
    let current = text.chars().count();
    if current >= width {
        return Ok(text.to_string());
    }
    let pad_ch = fill.chars().next().unwrap_or(' ');
    let padding: String = std::iter::repeat_n(pad_ch, width - current).collect();
    if start {
        Ok(format!("{}{}", padding, text))
    } else {
        Ok(format!("{}{}", text, padding))
    }
}

fn str_pad_start(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("strings.pad_start() expects (text, width, [fill])".to_string());
    }
    let text = expect_str(&args[0], "pad_start")?;
    let width = match &args[1] {
        Value::Int(n) if *n >= 0 => *n as usize,
        _ => return Err("strings.pad_start() width must be a non-negative integer".to_string()),
    };
    let fill = if args.len() == 3 {
        expect_str(&args[2], "pad_start")?
    } else {
        " "
    };
    Ok(Value::Str(pad_impl(text, width, fill, true)?))
}

fn str_pad_end(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("strings.pad_end() expects (text, width, [fill])".to_string());
    }
    let text = expect_str(&args[0], "pad_end")?;
    let width = match &args[1] {
        Value::Int(n) if *n >= 0 => *n as usize,
        _ => return Err("strings.pad_end() width must be a non-negative integer".to_string()),
    };
    let fill = if args.len() == 3 {
        expect_str(&args[2], "pad_end")?
    } else {
        " "
    };
    Ok(Value::Str(pad_impl(text, width, fill, false)?))
}

fn str_slice(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("strings.slice() expects (text, start, [end])".to_string());
    }
    let text = expect_str(&args[0], "slice")?;
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len() as i64;
    let start_raw = match &args[1] {
        Value::Int(n) => *n,
        _ => return Err("strings.slice() start must be an integer".to_string()),
    };
    let end_raw = if args.len() == 3 {
        match &args[2] {
            Value::Int(n) => *n,
            _ => return Err("strings.slice() end must be an integer".to_string()),
        }
    } else {
        len
    };
    let norm = |idx: i64| -> usize {
        let resolved = if idx < 0 { len + idx } else { idx };
        resolved.clamp(0, len) as usize
    };
    let s = norm(start_raw);
    let e = norm(end_raw);
    if s >= e {
        Ok(Value::Str(String::new()))
    } else {
        Ok(Value::Str(chars[s..e].iter().collect()))
    }
}

fn str_join(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.join() expects 2 arguments (list, sep)".to_string());
    }
    let list = match &args[0] {
        Value::List(l) => l,
        other => {
            return Err(format!(
                "strings.join() expects a list, got '{}'",
                other.type_name()
            ));
        }
    };
    let sep = expect_str(&args[1], "join")?;
    let parts: Vec<String> = list.borrow().iter().map(|v| format!("{}", v)).collect();
    Ok(Value::Str(parts.join(sep)))
}
