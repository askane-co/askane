use std::cell::RefCell;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "bytes",
        &[
            ("from_utf8", bytes_from_utf8),
            ("from_utf8_lossy", bytes_from_utf8_lossy),
            ("to_utf8", bytes_to_utf8),
            ("from_str", bytes_to_utf8),
            ("from_list", bytes_from_list),
            ("to_list", bytes_to_list),
            ("zeros", bytes_zeros),
        ],
        &[],
    )
}

fn expect_bytes<'a>(v: &'a Value, fn_name: &str) -> Result<&'a [u8], String> {
    match v {
        Value::Bytes(b) => Ok(b.as_slice()),
        other => Err(format!(
            "bytes.{}() expects bytes, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn bytes_from_utf8(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("bytes.from_utf8() expects 1 argument (bytes)".to_string());
    }
    let raw = expect_bytes(&args[0], "from_utf8")?;
    vm.charge_work((raw.len() as u64) / 64 + 1)?;
    vm.charge_alloc(raw.len())?;
    let text = std::str::from_utf8(raw)
        .map_err(|_| "bytes.from_utf8(): invalid UTF-8 sequence".to_string())?;
    Ok(Value::Str(text.to_string()))
}

fn bytes_from_utf8_lossy(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("bytes.from_utf8_lossy() expects 1 argument (bytes)".to_string());
    }
    let raw = expect_bytes(&args[0], "from_utf8_lossy")?;
    vm.charge_work((raw.len() as u64) / 64 + 1)?;
    vm.charge_alloc(raw.len())?;
    Ok(Value::Str(String::from_utf8_lossy(raw).into_owned()))
}

fn bytes_to_utf8(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("bytes.to_utf8() expects 1 string argument".to_string());
    }
    match &args[0] {
        Value::Str(s) => {
            vm.charge_alloc(s.len())?;
            Ok(Value::Bytes(Rc::new(s.as_bytes().to_vec())))
        }
        other => Err(format!(
            "bytes.to_utf8() expects a string, got '{}'",
            other.type_name()
        )),
    }
}

fn bytes_from_list(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("bytes.from_list() expects 1 list argument".to_string());
    }
    let list = match &args[0] {
        Value::List(l) => l,
        other => {
            return Err(format!(
                "bytes.from_list() expects a list, got '{}'",
                other.type_name()
            ));
        }
    };
    let items = list.borrow();
    vm.charge_alloc(items.len())?;
    vm.charge_work((items.len() as u64) / 64 + 1)?;
    let mut out = Vec::with_capacity(items.len());
    for item in items.iter() {
        match item {
            Value::Int(b) if (0..=255).contains(b) => out.push(*b as u8),
            _ => return Err("bytes.from_list() items must be integers 0..=255".to_string()),
        }
    }
    Ok(Value::Bytes(Rc::new(out)))
}

fn bytes_to_list(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("bytes.to_list() expects 1 argument (bytes or str)".to_string());
    }
    let raw = args[0].as_bytes_cow("bytes.to_list()")?;
    if raw.len() > 10_000_000 {
        return Err("bytes.to_list() exceeds 10,000,000 element limit".to_string());
    }
    vm.charge_alloc(raw.len().saturating_mul(std::mem::size_of::<Value>()))?;
    vm.charge_work((raw.len() as u64) / 64 + 1)?;
    let out: Vec<Value> = raw.iter().map(|&b| Value::Int(b as i64)).collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn bytes_zeros(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("bytes.zeros() expects 1 integer argument (len)".to_string());
    }
    let count = match &args[0] {
        Value::Int(n) if *n >= 0 && *n <= 64 * 1024 * 1024 => *n as usize,
        _ => return Err("bytes.zeros() length must be between 0 and 64 MiB".to_string()),
    };
    vm.charge_alloc(count)?;
    Ok(Value::Bytes(Rc::new(vec![0u8; count])))
}
