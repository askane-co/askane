use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "ini",
        &[("parse", ini_parse), ("stringify", ini_stringify)],
        &[],
    )
}

fn ini_parse(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ini.parse() expects 1 string argument".to_string());
    }
    let text = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "ini.parse() expects a string, got '{}'",
                other.type_name()
            ));
        }
    };

    let mut root: HashMap<String, Value> = HashMap::new();
    let mut current_section: Option<String> = None;

    for (idx, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let section = line[1..line.len() - 1].trim().to_string();
            if section.is_empty() {
                return Err(format!(
                    "ini.parse(): empty section name at line {}",
                    idx + 1
                ));
            }
            root.entry(section.clone())
                .or_insert_with(|| Value::Dict(Rc::new(RefCell::new(HashMap::new()))));
            current_section = Some(section);
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!("ini.parse(): expected '=' at line {}", idx + 1));
        };
        let key = k.trim().to_string();
        let val_str = v.trim();
        let val_clean =
            if (val_str.starts_with('"') && val_str.ends_with('"') && val_str.len() >= 2)
                || (val_str.starts_with('\'') && val_str.ends_with('\'') && val_str.len() >= 2)
            {
                val_str[1..val_str.len() - 1].to_string()
            } else {
                val_str.to_string()
            };
        if let Some(sec) = &current_section {
            if let Some(Value::Dict(d)) = root.get(sec) {
                d.borrow_mut().insert(key, Value::Str(val_clean));
            }
        } else {
            root.insert(key, Value::Str(val_clean));
        }
    }

    Ok(Value::Dict(Rc::new(RefCell::new(root))))
}

fn ini_stringify(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ini.stringify() expects 1 dict argument".to_string());
    }
    let dict = match &args[0] {
        Value::Dict(d) => d,
        other => {
            return Err(format!(
                "ini.stringify() expects a dict, got '{}'",
                other.type_name()
            ));
        }
    };
    let map = dict.borrow();
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();

    let mut out = String::new();
    let mut sections: Vec<&String> = Vec::new();
    for k in &keys {
        if matches!(map[*k], Value::Dict(_)) {
            sections.push(*k);
        } else {
            out.push_str(&format!("{} = {}\n", k, map[*k]));
        }
    }
    for sec in sections {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("[{}]\n", sec));
        if let Value::Dict(inner) = &map[sec] {
            let inner_map = inner.borrow();
            let mut inner_keys: Vec<&String> = inner_map.keys().collect();
            inner_keys.sort();
            for ik in inner_keys {
                out.push_str(&format!("{} = {}\n", ik, inner_map[ik]));
            }
        }
    }
    Ok(Value::Str(out))
}
