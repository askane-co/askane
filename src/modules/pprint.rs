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

const MAX_PPRINT_BYTES: usize = 256 * 1024;

fn push_bounded(out: &mut String, text: &str) {
    let rem = MAX_PPRINT_BYTES.saturating_sub(out.len());
    if rem == 0 {
        return;
    }
    if text.len() <= rem {
        out.push_str(text);
    } else {
        let mut cut = rem;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        out.push_str(&text[..cut]);
        out.push_str("...");
    }
}

fn fmt_pretty(
    vm: &mut VM,
    val: &Value,
    indent_step: usize,
    depth: usize,
    seen: &mut HashSet<usize>,
    out: &mut String,
    steps: &mut usize,
) -> Result<(), String> {
    if out.len() >= MAX_PPRINT_BYTES || *steps >= 200_000 {
        out.push_str("...");
        return Ok(());
    }
    *steps += 1;
    if (*steps & 1023) == 0 {
        vm.check_timeout()?;
    }
    if depth > 32 {
        out.push_str("...");
        return Ok(());
    }
    let pad = " ".repeat(depth * indent_step);
    let inner_pad = " ".repeat((depth + 1) * indent_step);

    match val {
        Value::Str(s) => push_bounded(out, &format!("{:?}", s)),
        Value::List(l) => {
            let ptr = std::rc::Rc::as_ptr(l) as usize;
            if !seen.insert(ptr) {
                out.push_str("[...]");
                return Ok(());
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
                    if out.len() >= MAX_PPRINT_BYTES {
                        out.push_str("...");
                        break;
                    }
                    if i > 0 {
                        out.push_str(", ");
                    }
                    fmt_pretty(vm, it, indent_step, depth + 1, seen, out, steps)?;
                }
                out.push(']');
            } else {
                out.push_str("[\n");
                for (i, it) in items.iter().enumerate() {
                    if out.len() >= MAX_PPRINT_BYTES || *steps >= 200_000 {
                        out.push_str(&inner_pad);
                        out.push_str("...\n");
                        break;
                    }
                    out.push_str(&inner_pad);
                    fmt_pretty(vm, it, indent_step, depth + 1, seen, out, steps)?;
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
                return Ok(());
            }
            let map = d.borrow();
            if map.is_empty() {
                out.push_str("#{}");
            } else {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push_str("#{\n");
                for (i, k) in keys.iter().enumerate() {
                    if out.len() >= MAX_PPRINT_BYTES || *steps >= 200_000 {
                        out.push_str(&inner_pad);
                        out.push_str("...\n");
                        break;
                    }
                    out.push_str(&inner_pad);
                    push_bounded(out, k);
                    out.push_str(": ");
                    fmt_pretty(vm, &map[*k], indent_step, depth + 1, seen, out, steps)?;
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
                return Ok(());
            }
            let fields = inst.fields.borrow();
            if fields.is_empty() {
                out.push_str(&format!("{} {{}}", inst.def.name));
            } else {
                out.push_str(&format!("{} {{\n", inst.def.name));
                for (i, fname) in inst.def.field_names.iter().enumerate() {
                    if out.len() >= MAX_PPRINT_BYTES || *steps >= 200_000 {
                        out.push_str(&inner_pad);
                        out.push_str("...\n");
                        break;
                    }
                    out.push_str(&inner_pad);
                    push_bounded(out, fname);
                    out.push_str(": ");
                    fmt_pretty(vm, &fields[i], indent_step, depth + 1, seen, out, steps)?;
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
        other => push_bounded(out, &other.to_string()),
    }
    Ok(())
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
    let mut steps = 0usize;
    fmt_pretty(vm, &args[0], indent, 0, &mut seen, &mut out, &mut steps)?;
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
