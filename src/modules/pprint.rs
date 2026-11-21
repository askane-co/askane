use std::collections::HashSet;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "pprint",
        &[("format", pprint_format), ("print", pprint_print)],
        &[],
    )
}

fn fmt_pretty(
    val: &Value,
    indent_step: usize,
    depth: usize,
    seen: &mut HashSet<usize>,
    out: &mut String,
) {
    if out.len() > 256 * 1024 {
        out.push_str("...");
        return;
    }
    if depth > 32 {
        out.push_str("...");
        return;
    }
    let pad = " ".repeat(depth * indent_step);
    let inner_pad = " ".repeat((depth + 1) * indent_step);

    match val {
        Value::Str(s) => out.push_str(&format!("{:?}", s)),
        Value::List(l) => {
            let ptr = std::rc::Rc::as_ptr(l) as usize;
            if !seen.insert(ptr) {
                out.push_str("[...]");
                return;
            }
            let items = l.borrow();
            if items.is_empty() {
                out.push_str("[]");
            } else if items.len() <= 4
                && items.iter().all(|v| {
                    matches!(
                        v,
                        Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Unit
                    )
                })
            {
                out.push('[');
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    fmt_pretty(it, indent_step, depth + 1, seen, out);
                }
                out.push(']');
            } else {
                out.push_str("[\n");
                for (i, it) in items.iter().enumerate() {
                    out.push_str(&inner_pad);
                    fmt_pretty(it, indent_step, depth + 1, seen, out);
                    if i + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad);
                out.push(']');
            }
            seen.remove(&ptr);
        }
        Value::Dict(d) => {
            let ptr = std::rc::Rc::as_ptr(d) as usize;
            if !seen.insert(ptr) {
                out.push_str("#{...}");
                return;
            }
            let map = d.borrow();
            if map.is_empty() {
                out.push_str("#{}");
            } else {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push_str("#{\n");
                for (i, k) in keys.iter().enumerate() {
                    out.push_str(&inner_pad);
                    out.push_str(k);
                    out.push_str(": ");
                    fmt_pretty(&map[*k], indent_step, depth + 1, seen, out);
                    if i + 1 < keys.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad);
                out.push('}');
            }
            seen.remove(&ptr);
        }
        Value::StructInstance(inst) => {
            let ptr = std::rc::Rc::as_ptr(inst) as usize;
            if !seen.insert(ptr) {
                out.push_str(&format!("{} {{ ... }}", inst.def.name));
                return;
            }
            let fields = inst.fields.borrow();
            if fields.is_empty() {
                out.push_str(&format!("{} {{}}", inst.def.name));
            } else {
                out.push_str(&format!("{} {{\n", inst.def.name));
                for (i, fname) in inst.def.field_names.iter().enumerate() {
                    out.push_str(&inner_pad);
                    out.push_str(fname);
                    out.push_str(": ");
                    fmt_pretty(&fields[i], indent_step, depth + 1, seen, out);
                    if i + 1 < inst.def.field_names.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad);
                out.push('}');
            }
            seen.remove(&ptr);
        }
        other => out.push_str(&other.to_string()),
    }
}

fn pprint_format(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("pprint.format() expects (value, indent=2)".to_string());
    }
    let indent = if args.len() == 2 {
        let Value::Int(n) = args[1] else {
            return Err("pprint.format() indent must be int".to_string());
        };
        n.clamp(0, 8) as usize
    } else {
        2
    };
    let mut out = String::new();
    let mut seen = HashSet::new();
    fmt_pretty(&args[0], indent, 0, &mut seen, &mut out);
    vm.charge_alloc(out.len())?;
    Ok(Value::Str(out))
}

fn pprint_print(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let formatted = pprint_format(vm, args)?;
    if let Value::Str(s) = formatted {
        println!("{}", s);
    }
    Ok(Value::Unit)
}
