use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "toml",
        &[("parse", toml_parse), ("stringify", toml_stringify)],
        &[],
    )
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_dq = false;
    let mut in_sq = false;
    let mut esc = false;
    for (idx, ch) in line.char_indices() {
        if in_dq {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_dq = false;
            }
        } else if in_sq {
            if ch == '\'' {
                in_sq = false;
            }
        } else if ch == '"' {
            in_dq = true;
        } else if ch == '\'' {
            in_sq = true;
        } else if ch == '#' {
            return &line[..idx];
        }
    }
    line
}

fn split_eq_top_level(text: &str) -> Option<(&str, &str)> {
    let mut in_dq = false;
    let mut in_sq = false;
    let mut esc = false;
    for (idx, ch) in text.char_indices() {
        if in_dq {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_dq = false;
            }
        } else if in_sq {
            if ch == '\'' {
                in_sq = false;
            }
        } else {
            match ch {
                '"' => in_dq = true,
                '\'' => in_sq = true,
                '=' => return Some((&text[..idx], &text[idx + 1..])),
                _ => {}
            }
        }
    }
    None
}

fn split_top_level(text: &str, sep: char) -> Result<Vec<String>, String> {
    let mut items = Vec::new();
    let mut start = 0usize;
    let mut depth_brack = 0usize;
    let mut depth_brace = 0usize;
    let mut in_dq = false;
    let mut in_sq = false;
    let mut esc = false;
    for (idx, ch) in text.char_indices() {
        if in_dq {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_dq = false;
            }
        } else if in_sq {
            if ch == '\'' {
                in_sq = false;
            }
        } else {
            match ch {
                '"' => in_dq = true,
                '\'' => in_sq = true,
                '[' => depth_brack += 1,
                ']' => {
                    if depth_brack == 0 {
                        return Err("Unexpected ']' in TOML".to_string());
                    }
                    depth_brack -= 1;
                }
                '{' => depth_brace += 1,
                '}' => {
                    if depth_brace == 0 {
                        return Err("Unexpected '}' in TOML".to_string());
                    }
                    depth_brace -= 1;
                }
                c if c == sep && depth_brack == 0 && depth_brace == 0 => {
                    items.push(text[start..idx].to_string());
                    start = idx + c.len_utf8();
                }
                _ => {}
            }
        }
    }
    if in_dq || in_sq || depth_brack != 0 || depth_brace != 0 {
        return Err("Unterminated string or collection in TOML".to_string());
    }
    items.push(text[start..].to_string());
    Ok(items)
}

fn parse_key_path(raw: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let key = if chars[i] == '"' {
            let start = i;
            i += 1;
            let mut esc = false;
            while i < chars.len() {
                if esc {
                    esc = false;
                    i += 1;
                } else if chars[i] == '\\' {
                    esc = true;
                    i += 1;
                } else if chars[i] == '"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            unquote_dq(&chars[start..i].iter().collect::<String>())?
        } else if chars[i] == '\'' {
            let start = i + 1;
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                i += 1;
            }
            if i >= chars.len() {
                return Err("Unterminated single-quoted key in TOML".to_string());
            }
            let s: String = chars[start..i].iter().collect();
            i += 1;
            s
        } else {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '-')
            {
                i += 1;
            }
            if i == start {
                return Err(format!("Invalid TOML key near '{}'", chars[i]));
            }
            chars[start..i].iter().collect()
        };
        parts.push(key);
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i < chars.len() {
            if chars[i] == '.' {
                i += 1;
            } else {
                return Err(format!(
                    "Expected '.' in TOML key path, found '{}'",
                    chars[i]
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err("Empty TOML key".to_string());
    }
    Ok(parts)
}

fn unquote_dq(s: &str) -> Result<String, String> {
    if !s.starts_with('"') || !s.ends_with('"') || s.len() < 2 {
        return Err("Invalid double-quoted string in TOML".to_string());
    }
    let inner = &s[1..s.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('b') => out.push('\x08'),
                Some('f') => out.push('\x0c'),
                Some('u') => {
                    let mut hex = String::with_capacity(4);
                    for _ in 0..4 {
                        match chars.next() {
                            Some(ch) if ch.is_ascii_hexdigit() => hex.push(ch),
                            _ => return Err("Invalid \\u escape in TOML".to_string()),
                        }
                    }
                    let cp = u32::from_str_radix(&hex, 16)
                        .map_err(|_| "Invalid \\u escape in TOML".to_string())?;
                    let ch = char::from_u32(cp)
                        .ok_or_else(|| "Invalid unicode code point in TOML".to_string())?;
                    out.push(ch);
                }
                Some(other) => return Err(format!("Unknown escape '\\{}' in TOML", other)),
                None => return Err("Unterminated escape in TOML string".to_string()),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn parse_toml_value(vm: &mut VM, raw: &str, depth: usize) -> Result<Value, String> {
    if depth > 64 {
        return Err("toml.parse(): nesting depth limit exceeded".to_string());
    }
    let s = raw.trim();
    if s.is_empty() {
        return Err("toml.parse(): empty value".to_string());
    }
    if s == "true" {
        return Ok(Value::Bool(true));
    }
    if s == "false" {
        return Ok(Value::Bool(false));
    }
    if s.starts_with("\"\"\"") {
        if !(s.ends_with("\"\"\"") && s.len() >= 6) {
            return Err("Unterminated multiline basic TOML string".to_string());
        }
        let mut inner = &s[3..s.len() - 3];
        if let Some(rest) = inner.strip_prefix("\r\n") {
            inner = rest;
        } else if let Some(rest) = inner.strip_prefix('\n') {
            inner = rest;
        }
        vm.charge_alloc(inner.len())?;
        return Ok(Value::Str(inner.to_string()));
    }
    if s.starts_with("'''") {
        if !(s.ends_with("'''") && s.len() >= 6) {
            return Err("Unterminated multiline literal TOML string".to_string());
        }
        let mut inner = &s[3..s.len() - 3];
        if let Some(rest) = inner.strip_prefix("\r\n") {
            inner = rest;
        } else if let Some(rest) = inner.strip_prefix('\n') {
            inner = rest;
        }
        vm.charge_alloc(inner.len())?;
        return Ok(Value::Str(inner.to_string()));
    }
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let parsed = unquote_dq(s)?;
        vm.charge_alloc(parsed.len())?;
        return Ok(Value::Str(parsed));
    }
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        vm.charge_alloc(inner.len())?;
        return Ok(Value::Str(inner.to_string()));
    }
    if s.starts_with('[') && s.ends_with(']') {
        let inner = s[1..s.len() - 1].trim();
        let mut out = Vec::new();
        if !inner.is_empty() {
            for part in split_top_level(inner, ',')? {
                let p = part.trim();
                if p.is_empty() {
                    continue;
                }
                out.push(parse_toml_value(vm, p, depth + 1)?);
            }
        }
        vm.charge_alloc(out.len().saturating_mul(std::mem::size_of::<Value>()))?;
        return Ok(Value::List(Rc::new(RefCell::new(out))));
    }
    if s.starts_with('{') && s.ends_with('}') {
        let inner = s[1..s.len() - 1].trim();
        let mut map = HashMap::new();
        if !inner.is_empty() {
            for part in split_top_level(inner, ',')? {
                let p = part.trim();
                if p.is_empty() {
                    continue;
                }
                let Some((k, v)) = split_eq_top_level(p) else {
                    return Err(format!("Expected '=' in inline TOML table near '{}'", p));
                };
                let key_path = parse_key_path(k.trim())?;
                let val = parse_toml_value(vm, v, depth + 1)?;
                insert_path(vm, &mut map, &key_path, val, depth + 1)?;
            }
        }
        vm.charge_alloc(map.len().saturating_mul(64))?;
        return Ok(Value::Dict(Rc::new(RefCell::new(map))));
    }
    let clean_num = s.replace('_', "");
    if let Ok(i) = clean_num.parse::<i64>() {
        return Ok(Value::Int(i));
    }
    if let Some(hex) = clean_num.strip_prefix("0x") {
        if let Ok(i) = i64::from_str_radix(hex, 16) {
            return Ok(Value::Int(i));
        }
    }
    if let Ok(f) = clean_num.parse::<f64>() {
        if f.is_finite() {
            return Ok(Value::Float(f));
        }
    }
    Err(format!("Invalid TOML value '{}'", s))
}

const MAX_TOML_DEPTH: usize = 64;
const MAX_TOML_OUT: usize = 16 * 1024 * 1024;

fn ensure_table(
    vm: &mut VM,
    root: &mut HashMap<String, Value>,
    path: &[String],
    is_array_elem: bool,
    base_depth: usize,
) -> Result<(Rc<RefCell<HashMap<String, Value>>>, usize), String> {
    let total_depth = base_depth.saturating_add(path.len());
    if total_depth > MAX_TOML_DEPTH {
        return Err("toml.parse(): table nesting depth limit exceeded".to_string());
    }
    let mut cur_map_rc: Option<Rc<RefCell<HashMap<String, Value>>>> = None;
    for (idx, seg) in path.iter().enumerate() {
        let is_last = idx + 1 == path.len();
        let next_rc = if let Some(ref rc) = cur_map_rc {
            let mut m = rc.borrow_mut();
            if is_last && is_array_elem {
                let is_new_list = !m.contains_key(seg);
                let entry = m
                    .entry(seg.clone())
                    .or_insert_with(|| Value::List(Rc::new(RefCell::new(Vec::new()))));
                let Value::List(list_rc) = entry else {
                    return Err(format!("TOML key '{}' is not an array of tables", seg));
                };
                vm.charge_alloc(if is_new_list { 128 } else { 64 })?;
                let new_tab = Rc::new(RefCell::new(HashMap::new()));
                list_rc.borrow_mut().push(Value::Dict(new_tab.clone()));
                new_tab
            } else {
                let existed = m.contains_key(seg);
                let entry = m
                    .entry(seg.clone())
                    .or_insert_with(|| Value::Dict(Rc::new(RefCell::new(HashMap::new()))));
                if !existed {
                    vm.charge_alloc(64)?;
                }
                match entry {
                    Value::Dict(d) => d.clone(),
                    Value::List(l) if !is_last => match l.borrow().last() {
                        Some(Value::Dict(d)) => d.clone(),
                        _ => return Err(format!("TOML array '{}' has no table", seg)),
                    },
                    _ => return Err(format!("TOML key '{}' conflicts with non-table", seg)),
                }
            }
        } else if is_last && is_array_elem {
            let is_new_list = !root.contains_key(seg);
            let entry = root
                .entry(seg.clone())
                .or_insert_with(|| Value::List(Rc::new(RefCell::new(Vec::new()))));
            let Value::List(list_rc) = entry else {
                return Err(format!("TOML key '{}' is not an array of tables", seg));
            };
            vm.charge_alloc(if is_new_list { 128 } else { 64 })?;
            let new_tab = Rc::new(RefCell::new(HashMap::new()));
            list_rc.borrow_mut().push(Value::Dict(new_tab.clone()));
            new_tab
        } else {
            let existed = root.contains_key(seg);
            let entry = root
                .entry(seg.clone())
                .or_insert_with(|| Value::Dict(Rc::new(RefCell::new(HashMap::new()))));
            if !existed {
                vm.charge_alloc(64)?;
            }
            match entry {
                Value::Dict(d) => d.clone(),
                Value::List(l) if !is_last => match l.borrow().last() {
                    Some(Value::Dict(d)) => d.clone(),
                    _ => return Err(format!("TOML array '{}' has no table", seg)),
                },
                _ => return Err(format!("TOML key '{}' conflicts with non-table", seg)),
            }
        };
        cur_map_rc = Some(next_rc);
    }
    cur_map_rc
        .map(|rc| (rc, total_depth))
        .ok_or_else(|| "Empty TOML table path".to_string())
}

fn insert_path(
    vm: &mut VM,
    target: &mut HashMap<String, Value>,
    path: &[String],
    val: Value,
    base_depth: usize,
) -> Result<(), String> {
    if path.len() == 1 {
        if !target.contains_key(&path[0]) {
            vm.charge_alloc(path[0].len().saturating_add(48))?;
        }
        target.insert(path[0].clone(), val);
        return Ok(());
    }
    let (tab, _) = ensure_table(vm, target, &path[..path.len() - 1], false, base_depth)?;
    let leaf = &path[path.len() - 1];
    if !tab.borrow().contains_key(leaf) {
        vm.charge_alloc(leaf.len().saturating_add(48))?;
    }
    tab.borrow_mut().insert(leaf.clone(), val);
    Ok(())
}

fn has_closing_delim_overlap(buf: &str, added_len: usize, delim: &str) -> bool {
    let start = buf.len().saturating_sub(added_len + delim.len());
    let scan_from = start.max(delim.len());
    if scan_from >= buf.len() {
        return false;
    }
    buf[scan_from..].contains(delim)
}

fn toml_parse(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("toml.parse() expects 1 string argument".to_string());
    }
    let Value::Str(text) = &args[0] else {
        return Err(format!(
            "toml.parse() expects a string, got '{}'",
            args[0].type_name()
        ));
    };
    if text.len() > 16 * 1024 * 1024 {
        return Err("toml.parse(): input exceeds 16 MiB limit".to_string());
    }
    let text = text.clone();
    let mut root: HashMap<String, Value> = HashMap::new();
    let mut current_table: Option<(Rc<RefCell<HashMap<String, Value>>>, usize)> = None;
    let lines: Vec<&str> = text.lines().collect();
    vm.charge_alloc(lines.len().saturating_mul(std::mem::size_of::<&str>()))?;
    let mut idx = 0usize;

    while idx < lines.len() {
        if (idx & 1023) == 0 {
            vm.check_timeout()?;
        }
        let stripped = strip_toml_comment(lines[idx]).trim();
        idx += 1;
        if stripped.is_empty() {
            continue;
        }
        if stripped.starts_with("[[") && stripped.ends_with("]]") {
            let path = parse_key_path(&stripped[2..stripped.len() - 2])?;
            current_table = Some(ensure_table(vm, &mut root, &path, true, 0)?);
            continue;
        }
        if stripped.starts_with('[') && stripped.ends_with(']') {
            let path = parse_key_path(&stripped[1..stripped.len() - 1])?;
            current_table = Some(ensure_table(vm, &mut root, &path, false, 0)?);
            continue;
        }
        let Some((k, v_head)) = split_eq_top_level(stripped) else {
            return Err(format!("toml.parse(): expected '=' at line {}", idx));
        };
        let key_path = parse_key_path(k.trim())?;
        let mut val_buf = v_head.trim().to_string();
        if let Some(delim) = if val_buf.starts_with("\"\"\"") {
            Some("\"\"\"")
        } else if val_buf.starts_with("'''") {
            Some("'''")
        } else {
            None
        } {
            let mut closed = val_buf[3..].contains(delim);
            while !closed && idx < lines.len() {
                if (idx & 1023) == 0 {
                    vm.check_timeout()?;
                }
                let next_line = lines[idx];
                idx += 1;
                val_buf.push('\n');
                val_buf.push_str(next_line);
                if val_buf.len() > 16 * 1024 * 1024 {
                    return Err("toml.parse(): multiline string exceeds 16 MiB limit".to_string());
                }
                closed = has_closing_delim_overlap(&val_buf, next_line.len() + 1, delim);
            }
            if !closed {
                return Err("toml.parse(): unterminated multiline string".to_string());
            }
        } else if val_buf.starts_with('[') && !val_buf.ends_with(']') {
            while !val_buf.ends_with(']') && idx < lines.len() {
                if (idx & 1023) == 0 {
                    vm.check_timeout()?;
                }
                let next_line = strip_toml_comment(lines[idx]).trim();
                idx += 1;
                val_buf.push('\n');
                val_buf.push_str(next_line);
                if val_buf.len() > 16 * 1024 * 1024 {
                    return Err("toml.parse(): multiline array exceeds 16 MiB limit".to_string());
                }
            }
        }
        let cur_depth = current_table.as_ref().map(|t| t.1).unwrap_or(0);
        let parsed = parse_toml_value(vm, &val_buf, 0)?;
        if let Some((ref cur, _)) = current_table {
            insert_path(vm, &mut cur.borrow_mut(), &key_path, parsed, cur_depth)?;
        } else {
            insert_path(vm, &mut root, &key_path, parsed, 0)?;
        }
    }

    Ok(Value::Dict(Rc::new(RefCell::new(root))))
}

fn format_toml_key(k: &str) -> String {
    if !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        k.to_string()
    } else {
        format!("{:?}", k)
    }
}

fn charge_toml_delta(
    vm: &mut VM,
    out: &str,
    last_charged: &mut usize,
    steps: &mut usize,
) -> Result<(), String> {
    if out.len() > MAX_TOML_OUT {
        return Err("toml.stringify(): output exceeds 16 MiB limit".to_string());
    }
    *steps += 1;
    if (*steps & 1023) == 0 {
        vm.charge_work(128)?;
    }
    let delta = out.len().saturating_sub(*last_charged);
    if delta >= 4096 {
        vm.charge_alloc(delta)?;
        *last_charged = out.len();
    }
    Ok(())
}

fn write_toml_scalar(
    vm: &mut VM,
    val: &Value,
    out: &mut String,
    depth: usize,
    last_charged: &mut usize,
    steps: &mut usize,
) -> Result<(), String> {
    if depth > MAX_TOML_DEPTH {
        return Err("toml.stringify(): nesting depth limit exceeded".to_string());
    }
    charge_toml_delta(vm, out, last_charged, steps)?;
    match val {
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Float(f) => {
            if !f.is_finite() {
                return Err("toml.stringify(): non-finite float not supported".to_string());
            }
            let s = f.to_string();
            out.push_str(&s);
            if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                out.push_str(".0");
            }
        }
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Str(s) => out.push_str(&format!("{:?}", s)),
        Value::List(l) => {
            let items = l.borrow();
            out.push('[');
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push_str(", ");
                }
                write_toml_scalar(vm, item, out, depth + 1, last_charged, steps)?;
            }
            out.push(']');
        }
        Value::Dict(d) => {
            let map = d.borrow();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push_str("{ ");
            for (idx, k) in keys.iter().enumerate() {
                if idx > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_toml_key(k));
                out.push_str(" = ");
                write_toml_scalar(vm, &map[*k], out, depth + 1, last_charged, steps)?;
            }
            out.push_str(" }");
        }
        other => {
            return Err(format!(
                "toml.stringify(): unsupported value type '{}'",
                other.type_name()
            ));
        }
    }
    charge_toml_delta(vm, out, last_charged, steps)
}

fn write_toml_section(
    vm: &mut VM,
    prefix: &[String],
    map: &HashMap<String, Value>,
    out: &mut String,
    depth: usize,
    last_charged: &mut usize,
    steps: &mut usize,
) -> Result<(), String> {
    if depth > MAX_TOML_DEPTH {
        return Err("toml.stringify(): nesting depth limit exceeded".to_string());
    }
    charge_toml_delta(vm, out, last_charged, steps)?;
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut subtables = Vec::new();

    for k in keys {
        match &map[k] {
            Value::Dict(sub) => subtables.push((k.clone(), sub.clone())),
            v => {
                out.push_str(&format_toml_key(k));
                out.push_str(" = ");
                write_toml_scalar(vm, v, out, depth + 1, last_charged, steps)?;
                out.push('\n');
            }
        }
    }

    for (name, sub) in subtables {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        let mut next_prefix = prefix.to_vec();
        next_prefix.push(format_toml_key(&name));
        out.push('[');
        out.push_str(&next_prefix.join("."));
        out.push_str("]\n");
        write_toml_section(
            vm,
            &next_prefix,
            &sub.borrow(),
            out,
            depth + 1,
            last_charged,
            steps,
        )?;
    }
    Ok(())
}

fn toml_stringify(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("toml.stringify() expects 1 dict argument".to_string());
    }
    let Value::Dict(d) = &args[0] else {
        return Err(format!(
            "toml.stringify() expects a dict, got '{}'",
            args[0].type_name()
        ));
    };
    let mut out = String::new();
    let mut last_charged = 0usize;
    let mut steps = 0usize;
    write_toml_section(
        vm,
        &[],
        &d.borrow(),
        &mut out,
        0,
        &mut last_charged,
        &mut steps,
    )?;
    let rem = out.len().saturating_sub(last_charged);
    if rem > 0 {
        vm.charge_alloc(rem)?;
    }
    Ok(Value::Str(out))
}
