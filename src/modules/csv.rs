use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "csv",
        &[
            ("parse", csv_parse),
            ("parse_dicts", csv_parse_dicts),
            ("stringify", csv_stringify),
        ],
        &[],
    )
}

fn get_delimiter(arg: Option<&Value>, fn_name: &str) -> Result<char, String> {
    match arg {
        None => Ok(','),
        Some(Value::Str(s)) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c != '\n' && c != '\r' && c != '"' => Ok(c),
                _ => Err(format!(
                    "csv.{}() delimiter must be a single character",
                    fn_name
                )),
            }
        }
        Some(other) => Err(format!(
            "csv.{}() delimiter must be a string, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn parse_csv_rows(text: &str, delim: char) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut at_field_start = true;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else if at_field_start && c == '"' {
            in_quotes = true;
            at_field_start = false;
        } else if c == delim {
            row.push(std::mem::take(&mut field));
            at_field_start = true;
        } else if c == '\r' || c == '\n' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
            at_field_start = true;
        } else {
            field.push(c);
            at_field_start = false;
        }
    }
    if in_quotes {
        return Err("csv: unterminated quoted field".to_string());
    }
    if !at_field_start || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

fn csv_parse(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("csv.parse() expects (text, [delimiter])".to_string());
    }
    let text = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "csv.parse() expects a string, got '{}'",
                other.type_name()
            ));
        }
    };
    let delim = get_delimiter(args.get(1), "parse")?;
    let rows = parse_csv_rows(text, delim)?;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let cells = r.into_iter().map(Value::Str).collect();
            Value::List(Rc::new(RefCell::new(cells)))
        })
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn csv_parse_dicts(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("csv.parse_dicts() expects (text, [delimiter])".to_string());
    }
    let text = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "csv.parse_dicts() expects a string, got '{}'",
                other.type_name()
            ));
        }
    };
    let delim = get_delimiter(args.get(1), "parse_dicts")?;
    let mut rows = parse_csv_rows(text, delim)?;
    if rows.is_empty() {
        return Ok(Value::List(Rc::new(RefCell::new(Vec::new()))));
    }
    let headers = rows.remove(0);
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut map = HashMap::new();
        for (idx, h) in headers.iter().enumerate() {
            let val = row.get(idx).cloned().unwrap_or_default();
            map.insert(h.clone(), Value::Str(val));
        }
        out.push(Value::Dict(Rc::new(RefCell::new(map))));
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn csv_stringify(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("csv.stringify() expects (rows, [delimiter])".to_string());
    }
    let rows = match &args[0] {
        Value::List(l) => l,
        other => {
            return Err(format!(
                "csv.stringify() expects a list of lists, got '{}'",
                other.type_name()
            ));
        }
    };
    let delim = get_delimiter(args.get(1), "stringify")?;
    let mut lines = Vec::new();
    for row_val in rows.borrow().iter() {
        let row = match row_val {
            Value::List(r) => r,
            other => {
                return Err(format!(
                    "csv.stringify() each row must be a list, got '{}'",
                    other.type_name()
                ));
            }
        };
        let mut cells = Vec::new();
        for cell_val in row.borrow().iter() {
            let s = format!("{}", cell_val);
            if s.contains(delim) || s.contains('"') || s.contains('\n') || s.contains('\r') {
                cells.push(format!("\"{}\"", s.replace('"', "\"\"")));
            } else {
                cells.push(s);
            }
        }
        lines.push(cells.join(&delim.to_string()));
    }
    Ok(Value::Str(lines.join("\n")))
}
