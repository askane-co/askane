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
            ("byte_len", str_byte_len),
            ("eq_ignore_case", str_eq_ignore_case),
            ("contains_ignore_case", str_contains_ignore_case),
            ("strip_prefix", str_strip_prefix),
            ("strip_suffix", str_strip_suffix),
            ("split_once", str_split_once),
            ("rsplit_once", str_rsplit_once),
            ("truncate_utf8", str_truncate_utf8),
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

pub(crate) fn str_split(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.split() expects 2 arguments (text, sep)".to_string());
    }
    let text = expect_str(&args[0], "split")?;
    let sep = expect_str(&args[1], "split")?;
    vm.charge_work((text.len() as u64) / 32 + 1)?;
    let parts: Vec<Value> = if sep.is_empty() {
        let count = text.chars().count();
        vm.charge_alloc(
            count
                .saturating_mul(std::mem::size_of::<Value>() + 4)
                .saturating_add(text.len()),
        )?;
        text.chars()
            .map(|c| Value::from_str(c.to_string()))
            .collect()
    } else {
        let count = text.split(sep).count();
        vm.charge_alloc(
            count
                .saturating_mul(std::mem::size_of::<Value>())
                .saturating_add(text.len()),
        )?;
        text.split(sep).map(Value::from_str).collect()
    };
    Ok(Value::List(Rc::new(RefCell::new(parts))))
}

pub(crate) fn str_trim(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.trim() expects 1 argument (text)".to_string());
    }
    let text = expect_str(&args[0], "trim")?;
    let trimmed = text.trim();
    vm.charge_alloc(trimmed.len())?;
    vm.charge_work((text.len() as u64) / 64 + 1)?;
    Ok(Value::from_str(trimmed))
}

pub(crate) fn str_trim_start(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.trim_start() expects 1 argument (text)".to_string());
    }
    let text = expect_str(&args[0], "trim_start")?;
    let trimmed = text.trim_start();
    vm.charge_alloc(trimmed.len())?;
    vm.charge_work((text.len() as u64) / 64 + 1)?;
    Ok(Value::from_str(trimmed))
}

pub(crate) fn str_trim_end(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.trim_end() expects 1 argument (text)".to_string());
    }
    let text = expect_str(&args[0], "trim_end")?;
    let trimmed = text.trim_end();
    vm.charge_alloc(trimmed.len())?;
    vm.charge_work((text.len() as u64) / 64 + 1)?;
    Ok(Value::from_str(trimmed))
}

pub(crate) fn str_replace(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("strings.replace() expects 3 arguments (text, from, to)".to_string());
    }
    let text = expect_str(&args[0], "replace")?;
    let from = expect_str(&args[1], "replace")?;
    let to = expect_str(&args[2], "replace")?;
    vm.charge_work((text.len() as u64) / 32 + 1)?;
    const MAX_OUT: usize = 64 * 1024 * 1024;
    if from.is_empty() {
        let count = text.chars().count() + 1;
        let out_len = text.len().saturating_add(count.saturating_mul(to.len()));
        if out_len > MAX_OUT {
            return Err("strings.replace() output exceeds 64 MiB limit".to_string());
        }
        vm.charge_alloc(out_len)?;
        return Ok(Value::from_str(text.replace(from, to)));
    }
    let mut out = String::new();
    let mut last = 0;
    for (idx, _) in text.match_indices(from) {
        let seg = &text[last..idx];
        if out.len().saturating_add(seg.len()).saturating_add(to.len()) > MAX_OUT {
            return Err("strings.replace() output exceeds 64 MiB limit".to_string());
        }
        out.push_str(seg);
        out.push_str(to);
        last = idx + from.len();
    }
    let tail = &text[last..];
    if out.len().saturating_add(tail.len()) > MAX_OUT {
        return Err("strings.replace() output exceeds 64 MiB limit".to_string());
    }
    out.push_str(tail);
    vm.charge_alloc(out.len())?;
    Ok(Value::from_str(out))
}

pub(crate) fn str_contains(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.contains() expects 2 arguments (text, sub)".to_string());
    }
    let text = expect_str(&args[0], "contains")?;
    let sub = expect_str(&args[1], "contains")?;
    Ok(Value::Bool(text.contains(sub)))
}

pub(crate) fn str_starts_with(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.starts_with() expects 2 arguments (text, prefix)".to_string());
    }
    let text = expect_str(&args[0], "starts_with")?;
    let prefix = expect_str(&args[1], "starts_with")?;
    Ok(Value::Bool(text.starts_with(prefix)))
}

pub(crate) fn str_ends_with(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.ends_with() expects 2 arguments (text, suffix)".to_string());
    }
    let text = expect_str(&args[0], "ends_with")?;
    let suffix = expect_str(&args[1], "ends_with")?;
    Ok(Value::Bool(text.ends_with(suffix)))
}

pub(crate) fn str_upper(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.upper() expects 1 argument (text)".to_string());
    }
    let s = expect_str(&args[0], "upper")?;
    vm.charge_work((s.len() as u64) / 32 + 1)?;
    vm.charge_alloc(s.len())?;
    let out = s.to_uppercase();
    if out.len() > s.len() {
        vm.charge_alloc(out.len() - s.len())?;
    }
    Ok(Value::from_str(out))
}

pub(crate) fn str_lower(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.lower() expects 1 argument (text)".to_string());
    }
    let s = expect_str(&args[0], "lower")?;
    vm.charge_work((s.len() as u64) / 32 + 1)?;
    vm.charge_alloc(s.len())?;
    let out = s.to_lowercase();
    if out.len() > s.len() {
        vm.charge_alloc(out.len() - s.len())?;
    }
    Ok(Value::from_str(out))
}

pub(crate) fn str_find(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
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

pub(crate) fn str_repeat(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.repeat() expects 2 arguments (text, count)".to_string());
    }
    let text = expect_str(&args[0], "repeat")?;
    let count = match &args[1] {
        Value::Int(n) if *n >= 0 => *n as usize,
        _ => return Err("strings.repeat() expects a non-negative integer count".to_string()),
    };
    let out_len = text.len().saturating_mul(count);
    if out_len > 16 * 1024 * 1024 {
        return Err("strings.repeat() output exceeds 16 MiB limit".to_string());
    }
    vm.charge_alloc(out_len)?;
    vm.charge_work((out_len as u64) / 64 + 1)?;
    Ok(Value::from_str(text.repeat(count)))
}

pub(crate) fn str_lines(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.lines() expects 1 argument (text)".to_string());
    }
    let text = expect_str(&args[0], "lines")?;
    vm.charge_work((text.len() as u64) / 32 + 1)?;
    let count = text.lines().count();
    vm.charge_alloc(
        count
            .saturating_mul(std::mem::size_of::<Value>())
            .saturating_add(text.len()),
    )?;
    let lines: Vec<Value> = text.lines().map(Value::from_str).collect();
    Ok(Value::List(Rc::new(RefCell::new(lines))))
}

fn pad_impl(
    vm: &mut VM,
    text: &str,
    width: usize,
    fill: &str,
    start: bool,
) -> Result<String, String> {
    if width > 1_000_000 {
        return Err("strings.pad() width exceeds limit".to_string());
    }
    let current = text.chars().count();
    if current >= width {
        vm.charge_alloc(text.len())?;
        return Ok(text.to_string());
    }
    let pad_ch = fill.chars().next().unwrap_or(' ');
    let needed = width - current;
    let out_bytes = text
        .len()
        .saturating_add(needed.saturating_mul(pad_ch.len_utf8()));
    vm.charge_alloc(out_bytes)?;
    vm.charge_work((out_bytes as u64) / 32 + 1)?;
    let padding: String = std::iter::repeat_n(pad_ch, needed).collect();
    if start {
        Ok(format!("{}{}", padding, text))
    } else {
        Ok(format!("{}{}", text, padding))
    }
}

pub(crate) fn str_pad_start(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
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
    Ok(Value::from_str(pad_impl(vm, text, width, fill, true)?))
}

pub(crate) fn str_pad_end(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
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
    Ok(Value::from_str(pad_impl(vm, text, width, fill, false)?))
}

pub(crate) fn str_slice(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("strings.slice() expects (text, start, [end])".to_string());
    }
    let text = expect_str(&args[0], "slice")?;
    vm.charge_work((text.len() as u64) / 64 + 1)?;
    let char_len = text.chars().count();
    let len = char_len as i64;
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
        return Ok(Value::from_str(""));
    }
    let mut start_byte = text.len();
    let mut end_byte = text.len();
    for (ci, (bi, _)) in text.char_indices().enumerate() {
        if ci == s {
            start_byte = bi;
        }
        if ci == e {
            end_byte = bi;
            break;
        }
    }
    let sliced = &text[start_byte..end_byte];
    vm.charge_alloc(sliced.len())?;
    Ok(Value::from_str(sliced))
}

pub(crate) fn str_join(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
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
    const MAX_OUT: usize = 64 * 1024 * 1024;
    let mut out = String::new();
    for (i, v) in list.borrow().iter().enumerate() {
        if i > 0 {
            if out.len().saturating_add(sep.len()) > MAX_OUT {
                return Err("strings.join() output exceeds 64 MiB limit".to_string());
            }
            out.push_str(sep);
        }
        let s = format!("{}", v);
        if out.len().saturating_add(s.len()) > MAX_OUT {
            return Err("strings.join() output exceeds 64 MiB limit".to_string());
        }
        out.push_str(&s);
    }
    vm.charge_alloc(out.len())?;
    vm.charge_work((out.len() as u64) / 64 + 1)?;
    Ok(Value::from_str(out))
}

fn str_byte_len(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("strings.byte_len() expects 1 argument (text)".to_string());
    }
    let text = expect_str(&args[0], "byte_len")?;
    Ok(Value::Int(text.len() as i64))
}

fn case_fold_basic(s: &str) -> String {
    s.to_lowercase().replace(['ß', 'ẞ'], "ss")
}

fn str_eq_ignore_case(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.eq_ignore_case() expects 2 arguments (a, b)".to_string());
    }
    let a = expect_str(&args[0], "eq_ignore_case")?;
    let b = expect_str(&args[1], "eq_ignore_case")?;
    vm.charge_work(((a.len().max(b.len()) as u64) / 32) + 1)?;
    if a.is_ascii() && b.is_ascii() {
        return Ok(Value::Bool(a.eq_ignore_ascii_case(b)));
    }
    vm.charge_alloc(a.len().saturating_add(b.len()))?;
    Ok(Value::Bool(case_fold_basic(a) == case_fold_basic(b)))
}

fn str_contains_ignore_case(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.contains_ignore_case() expects 2 arguments (text, pat)".to_string());
    }
    let text = expect_str(&args[0], "contains_ignore_case")?;
    let pat = expect_str(&args[1], "contains_ignore_case")?;
    vm.charge_work(((text.len().saturating_add(pat.len()) as u64) / 32) + 1)?;
    vm.charge_alloc(text.len().saturating_add(pat.len()))?;
    Ok(Value::Bool(
        case_fold_basic(text).contains(&case_fold_basic(pat)),
    ))
}

fn str_strip_prefix(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.strip_prefix() expects 2 arguments (text, prefix)".to_string());
    }
    let text = expect_str(&args[0], "strip_prefix")?;
    let pfx = expect_str(&args[1], "strip_prefix")?;
    let out = text.strip_prefix(pfx).unwrap_or(text);
    vm.charge_alloc(out.len())?;
    Ok(Value::from_str(out))
}

fn str_strip_suffix(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.strip_suffix() expects 2 arguments (text, suffix)".to_string());
    }
    let text = expect_str(&args[0], "strip_suffix")?;
    let sfx = expect_str(&args[1], "strip_suffix")?;
    let out = text.strip_suffix(sfx).unwrap_or(text);
    vm.charge_alloc(out.len())?;
    Ok(Value::from_str(out))
}

fn split_once_impl(vm: &mut VM, args: &[Value], rev: bool, name: &str) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!(
            "strings.{}() expects 2 arguments (text, sep)",
            name
        ));
    }
    let text = expect_str(&args[0], name)?;
    let sep = expect_str(&args[1], name)?;
    if sep.is_empty() {
        return Err(format!("strings.{}() separator cannot be empty", name));
    }
    vm.charge_work((text.len() as u64) / 32 + 1)?;
    let res = if rev {
        text.rsplit_once(sep)
    } else {
        text.split_once(sep)
    };
    match res {
        Some((l, r)) => {
            vm.charge_alloc(text.len())?;
            Ok(Value::List(Rc::new(RefCell::new(vec![
                Value::from_str(l),
                Value::from_str(r),
            ]))))
        }
        None => Ok(Value::Unit),
    }
}

fn str_split_once(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    split_once_impl(vm, args, false, "split_once")
}

fn str_rsplit_once(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    split_once_impl(vm, args, true, "rsplit_once")
}

fn str_truncate_utf8(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("strings.truncate_utf8() expects 2 arguments (text, max_bytes)".to_string());
    }
    let text = expect_str(&args[0], "truncate_utf8")?;
    let max_bytes = match &args[1] {
        Value::Int(n) if *n >= 0 => *n as usize,
        _ => {
            return Err(
                "strings.truncate_utf8() max_bytes must be a non-negative integer".to_string(),
            );
        }
    };
    if max_bytes >= text.len() {
        return Ok(args[0].clone());
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    vm.charge_alloc(end)?;
    Ok(Value::from_str(&text[..end]))
}
