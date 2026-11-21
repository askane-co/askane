use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "cli",
        &[("parse", cli_parse), ("has_flag", cli_has_flag)],
        &[],
    )
}

fn cli_parse(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 1 {
        return Err("cli.parse() expects 0 or 1 list argument (argv)".to_string());
    }
    let raw_args: Vec<String> = if args.is_empty() {
        vm.check_sys_args()?;
        std::env::args().skip(1).collect()
    } else {
        let Value::List(list_rc) = &args[0] else {
            return Err("cli.parse() expects a list of strings".to_string());
        };
        list_rc.borrow().iter().map(|v| v.to_string()).collect()
    };

    let mut flags: HashMap<String, Value> = HashMap::new();
    let mut options: HashMap<String, Value> = HashMap::new();
    let mut positional: Vec<Value> = Vec::new();
    let mut rest_positional = false;
    let mut i = 0usize;

    while i < raw_args.len() {
        let arg = &raw_args[i];
        if rest_positional {
            positional.push(Value::Str(arg.clone()));
            i += 1;
            continue;
        }
        if arg == "--" {
            rest_positional = true;
            i += 1;
            continue;
        }
        if let Some(long) = arg.strip_prefix("--") {
            if let Some((k, v)) = long.split_once('=') {
                options.insert(k.to_string(), Value::Str(v.to_string()));
            } else if i + 1 < raw_args.len() && !raw_args[i + 1].starts_with('-') {
                options.insert(long.to_string(), Value::Str(raw_args[i + 1].clone()));
                flags.insert(long.to_string(), Value::Bool(true));
                i += 2;
                continue;
            } else {
                flags.insert(long.to_string(), Value::Bool(true));
            }
            i += 1;
            continue;
        }
        if let Some(short) = arg.strip_prefix('-') {
            if !short.is_empty() && !short.chars().all(|c| c.is_ascii_digit()) {
                for ch in short.chars() {
                    flags.insert(ch.to_string(), Value::Bool(true));
                }
                i += 1;
                continue;
            }
        }
        positional.push(Value::Str(arg.clone()));
        i += 1;
    }

    let command = positional.first().cloned().unwrap_or(Value::Unit);
    let mut out = HashMap::new();
    out.insert("command".to_string(), command);
    out.insert(
        "flags".to_string(),
        Value::Dict(Rc::new(RefCell::new(flags))),
    );
    out.insert(
        "options".to_string(),
        Value::Dict(Rc::new(RefCell::new(options))),
    );
    out.insert(
        "positional".to_string(),
        Value::List(Rc::new(RefCell::new(positional))),
    );
    vm.charge_alloc(out.len().saturating_mul(64))?;
    Ok(Value::Dict(Rc::new(RefCell::new(out))))
}

fn cli_has_flag(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("cli.has_flag() expects (parsed_dict, name)".to_string());
    }
    let Value::Dict(d) = &args[0] else {
        return Err("cli.has_flag() first argument must be parsed dict".to_string());
    };
    let Value::Str(name) = &args[1] else {
        return Err("cli.has_flag() flag name must be string".to_string());
    };
    let found = match d.borrow().get("flags") {
        Some(Value::Dict(flags)) => flags.borrow().get(name).is_some_and(|v| v.is_truthy()),
        _ => false,
    };
    Ok(Value::Bool(found))
}
