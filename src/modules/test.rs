use crate::modules::build_module;
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
