use crate::modules::build_module;
use crate::modules::diff::lcs_line_diff;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "test",
        &[
            ("assert", test_assert),
            ("assert_eq", test_assert_eq),
            ("assert_ne", test_assert_ne),
            ("assert_err", test_assert_err),
            ("assert_snapshot", test_assert_snapshot),
        ],
        &[],
    )
}

fn test_assert(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("test.assert() expects (cond, [message])".to_string());
    }
    if args[0].is_truthy() {
        Ok(Value::Unit)
    } else if args.len() == 2 {
        Err(format!("Assertion failed: {}", args[1]))
    } else {
        Err("Assertion failed".to_string())
    }
}

fn test_assert_eq(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("test.assert_eq() expects (actual, expected, [message])".to_string());
    }
    if args[0].equals(&args[1]) {
        Ok(Value::Unit)
    } else {
        let base = format!("assert_eq failed: got {}, expected {}", args[0], args[1]);
        if args.len() == 3 {
            Err(format!("{} ({})", base, args[2]))
        } else {
            Err(base)
        }
    }
}

fn test_assert_ne(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("test.assert_ne() expects (a, b, [message])".to_string());
    }
    if !args[0].equals(&args[1]) {
        Ok(Value::Unit)
    } else {
        let base = format!("assert_ne failed: both values equal {}", args[0]);
        if args.len() == 3 {
            Err(format!("{} ({})", base, args[2]))
        } else {
            Err(base)
        }
    }
}

fn test_assert_err(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("test.assert_err() expects (fn, [expected_substr])".to_string());
    }
    let func = args[0].clone();
    match vm.call_value(func, &[]) {
        Ok(v) => Err(format!("assert_err failed: expected error, got {}", v)),
        Err(err) => {
            if VM::is_unrecoverable_error(&err) {
                return Err(err);
            }
            if args.len() == 2 {
                let needle = format!("{}", args[1]);
                if !err.contains(&needle) {
                    return Err(format!(
                        "assert_err failed: error '{}' does not contain '{}'",
                        err, needle
                    ));
                }
            }
            Ok(Value::Unit)
        }
    }
}

fn format_snapshot_canonical(v: &Value, depth: usize, out: &mut String) -> Result<(), String> {
    if depth > 32 {
        out.push_str("<depth-limit>");
        return Ok(());
    }
    match v {
        Value::Unit => out.push_str("unit"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Float(f) => out.push_str(&format!("{}", f)),
        Value::Str(s) => out.push_str(&format!("{:?}", s)),
        Value::Bytes(b) => {
            out.push_str("b\"");
            for &byte in b.iter() {
                out.push_str(&format!("\\x{:02x}", byte));
            }
            out.push('"');
        }
        Value::List(list) => {
            let items = list.borrow();
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_snapshot_canonical(item, depth + 1, out)?;
            }
            out.push(']');
        }
        Value::Dict(dict) => {
            let map = dict.borrow();
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push_str("#{");
            for (i, (k, val)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("{:?}: ", k));
                format_snapshot_canonical(val, depth + 1, out)?;
            }
            out.push('}');
        }
        other => out.push_str(&format!("{}", other)),
    }
    Ok(())
}

fn test_assert_snapshot(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("test.assert_snapshot() expects 2 arguments (name, value)".to_string());
    }
    let Value::Str(name) = &args[0] else {
        return Err(format!(
            "test.assert_snapshot() name must be string, got '{}'",
            args[0].type_name()
        ));
    };
    if name.is_empty()
        || name.len() > 128
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        || name.contains("..")
    {
        return Err(
            "test.assert_snapshot() name must use only [A-Za-z0-9._-] without '..'".to_string(),
        );
    }

    let mut actual = String::new();
    format_snapshot_canonical(&args[1], 0, &mut actual)?;
    actual.push('\n');

    let snap_rel = format!("__snapshots__/{}.snap", name);
    let snap_path = vm.resolve_write_path(&snap_rel)?;
    crate::modules::fs::check_safe_write_target(vm, &snap_path, &snap_rel)?;

    if vm.should_update_snapshots() || !snap_path.exists() {
        if let Some(parent) = snap_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&snap_path, actual.as_bytes()).map_err(|e| e.to_string())?;
        return Ok(Value::Unit);
    }

    let expected = std::fs::read_to_string(&snap_path)
        .map_err(|e| format!("test.assert_snapshot('{}') failed to read: {}", name, e))?
        .replace("\r\n", "\n");
    if expected == actual {
        return Ok(Value::Unit);
    }
    let exp_lines: Vec<&str> = expected.lines().collect();
    let act_lines: Vec<&str> = actual.lines().collect();
    let ops = lcs_line_diff(vm, &exp_lines, &act_lines)?;
    let mut diff_txt = String::new();
    for (tag, line) in ops {
        diff_txt.push(tag);
        diff_txt.push(' ');
        diff_txt.push_str(line);
        diff_txt.push('\n');
    }
    Err(format!(
        "Snapshot '{}' mismatch (run with --update-snapshots to update):\n{}",
        name, diff_txt
    ))
}
