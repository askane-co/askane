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
                    let hex: String = chars.by_ref().take(4).collect();
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
    if s.starts_with("\"\"\"") && s.ends_with("\"\"\"") && s.len() >= 6 {
        let mut inner = &s[3..s.len() - 3];
        if let Some(rest) = inner.strip_prefix("\r\n") {
            inner = rest;
        } else if let Some(rest) = inner.strip_prefix('\n') {
            inner = rest;
        }
        vm.charge_alloc(inner.len())?;
        return Ok(Value::Str(inner.to_string()));
    }
    if s.starts_with("'''") && s.ends_with("'''") && s.len() >= 6 {
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
                let Some((k, v)) = p.split_once('=') else {
                    return Err(format!("Expected '=' in inline TOML table near '{}'", p));
                };
                let key_path = parse_key_path(k.trim())?;
                let val = parse_toml_value(vm, v, depth + 1)?;
                insert_path(&mut map, &key_path, val)?;
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

fn ensure_table(
    root: &mut HashMap<String, Value>,
    path: &[String],
    is_array_elem: bool,
) -> Result<Rc<RefCell<HashMap<String, Value>>>, String> {
    let mut cur_map_rc: Option<Rc<RefCell<HashMap<String, Value>>>> = None;
    for (idx, seg) in path.iter().enumerate() {
        let is_last = idx + 1 == path.len();
        let next_rc = if let Some(ref rc) = cur_map_rc {
            let mut m = rc.borrow_mut();
            if is_last && is_array_elem {
                let entry = m
                    .entry(seg.clone())
                    .or_insert_with(|| Value::List(Rc::new(RefCell::new(Vec::new()))));
                let Value::List(list_rc) = entry else {
                    return Err(format!("TOML key '{}' is not an array of tables", seg));
                };
                let new_tab = Rc::new(RefCell::new(HashMap::new()));
                list_rc.borrow_mut().push(Value::Dict(new_tab.clone()));
                new_tab
            } else {
                let entry = m
                    .entry(seg.clone())
                    .or_insert_with(|| Value::Dict(Rc::new(RefCell::new(HashMap::new()))));
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
            let entry = root
                .entry(seg.clone())
                .or_insert_with(|| Value::List(Rc::new(RefCell::new(Vec::new()))));
            let Value::List(list_rc) = entry else {
                return Err(format!("TOML key '{}' is not an array of tables", seg));
            };
            let new_tab = Rc::new(RefCell::new(HashMap::new()));
            list_rc.borrow_mut().push(Value::Dict(new_tab.clone()));
            new_tab
        } else {
            let entry = root
                .entry(seg.clone())
                .or_insert_with(|| Value::Dict(Rc::new(RefCell::new(HashMap::new()))));
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
    cur_map_rc.ok_or_else(|| "Empty TOML table path".to_string())
}

fn insert_path(
    target: &mut HashMap<String, Value>,
    path: &[String],
    val: Value,
) -> Result<(), String> {
    if path.len() == 1 {
        target.insert(path[0].clone(), val);
        return Ok(());
    }
    let tab = ensure_table(target, &path[..path.len() - 1], false)?;
    tab.borrow_mut().insert(path[path.len() - 1].clone(), val);
    Ok(())
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
    let mut current_table: Option<Rc<RefCell<HashMap<String, Value>>>> = None;
    let lines: Vec<&str> = text.lines().collect();
    let mut idx = 0usize;

    while idx < lines.len() {
        let stripped = strip_toml_comment(lines[idx]).trim();
        idx += 1;
        if stripped.is_empty() {
            continue;
        }
        if stripped.starts_with("[[") && stripped.ends_with("]]") {
            let path = parse_key_path(&stripped[2..stripped.len() - 2])?;
            current_table = Some(ensure_table(&mut root, &path, true)?);
            continue;
        }
        if stripped.starts_with('[') && stripped.ends_with(']') {
            let path = parse_key_path(&stripped[1..stripped.len() - 1])?;
            current_table = Some(ensure_table(&mut root, &path, false)?);
            continue;
        }
        let Some((k, v_head)) = stripped.split_once('=') else {
            return Err(format!("toml.parse(): expected '=' at line {}", idx));
        };
        let key_path = parse_key_path(k.trim())?;
        let mut val_buf = v_head.trim().to_string();
        while (val_buf.starts_with('[') && !val_buf.ends_with(']')
            || val_buf.starts_with("\"\"\"") && !val_buf[3..].contains("\"\"\"")
            || val_buf.starts_with("'''") && !val_buf[3..].contains("'''"))
            && idx < lines.len()
        {
            let next_line = if val_buf.starts_with("\"\"\"") || val_buf.starts_with("'''") {
                lines[idx]
            } else {
                strip_toml_comment(lines[idx]).trim()
            };
            val_buf.push('\n');
            val_buf.push_str(next_line);
            idx += 1;
        }
        let parsed = parse_toml_value(vm, &val_buf, 0)?;
        if let Some(ref cur) = current_table {
            insert_path(&mut cur.borrow_mut(), &key_path, parsed)?;
        } else {
            insert_path(&mut root, &key_path, parsed)?;
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

fn format_toml_scalar(val: &Value, depth: usize) -> Result<String, String> {
    if depth > 64 {
        return Err("toml.stringify(): nesting depth limit exceeded".to_string());
    }
    match val {
        Value::Int(i) => Ok(i.to_string()),
        Value::Float(f) => {
            if !f.is_finite() {
                return Err("toml.stringify(): non-finite float not supported".to_string());
            }
            let s = f.to_string();
            if s.contains('.') || s.contains('e') || s.contains('E') {
                Ok(s)
            } else {
                Ok(format!("{}.0", s))
            }
        }
        Value::Bool(b) => Ok(b.to_string()),
        Value::Str(s) => Ok(format!("{:?}", s)),
        Value::List(l) => {
            let items = l.borrow();
            let mut parts = Vec::with_capacity(items.len());
            for item in items.iter() {
                parts.push(format_toml_scalar(item, depth + 1)?);
            }
            Ok(format!("[{}]", parts.join(", ")))
        }
        Value::Dict(d) => {
            let map = d.borrow();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut parts = Vec::with_capacity(keys.len());
            for k in keys {
                parts.push(format!(
                    "{} = {}",
                    format_toml_key(k),
                    format_toml_scalar(&map[k], depth + 1)?
                ));
            }
            Ok(format!("{{ {} }}", parts.join(", ")))
        }
        other => Err(format!(
            "toml.stringify(): unsupported value type '{}'",
            other.type_name()
        )),
    }
}

fn write_toml_section(
    prefix: &[String],
    map: &HashMap<String, Value>,
    out: &mut String,
    depth: usize,
) -> Result<(), String> {
    if depth > 64 {
        return Err("toml.stringify(): nesting depth limit exceeded".to_string());
    }
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut subtables = Vec::new();

    for k in keys {
        match &map[k] {
            Value::Dict(sub) => subtables.push((k.clone(), sub.clone())),
            v => {
                out.push_str(&format!(
                    "{} = {}\n",
                    format_toml_key(k),
                    format_toml_scalar(v, depth + 1)?
                ));
            }
        }
    }

    for (name, sub) in subtables {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        let mut next_prefix = prefix.to_vec();
        next_prefix.push(format_toml_key(&name));
        out.push_str(&format!("[{}]\n", next_prefix.join(".")));
        write_toml_section(&next_prefix, &sub.borrow(), out, depth + 1)?;
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
    write_toml_section(&[], &d.borrow(), &mut out, 0)?;
    vm.charge_alloc(out.len())?;
    Ok(Value::Str(out))
}
