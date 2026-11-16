use std::cell::RefCell;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "hex",
        &[("encode", hex_encode), ("decode", hex_decode)],
        &[],
    )
}

fn hex_encode(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("hex.encode() expects 1 argument (str or byte list)".to_string());
    }
    let bytes: Vec<u8> = match &args[0] {
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::List(l) => {
            let items = l.borrow();
            let mut out = Vec::with_capacity(items.len());
            for v in items.iter() {
                match v {
                    Value::Int(b) if (0..=255).contains(b) => out.push(*b as u8),
                    _ => return Err("hex.encode() list items must be bytes 0..=255".to_string()),
                }
            }
            out
        }
        other => {
            return Err(format!(
                "hex.encode() expects string or byte list, got '{}'",
                other.type_name()
            ));
        }
    };
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    Ok(Value::Str(out))
}

fn hex_decode(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("hex.decode() expects 1 hex string argument".to_string());
    }
    let s = match &args[0] {
        Value::Str(s) => s.trim(),
        other => {
            return Err(format!(
                "hex.decode() expects a string, got '{}'",
                other.type_name()
            ));
        }
    };
    if s.len() % 2 != 0 {
        return Err("hex.decode(): hex string length must be even".to_string());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let hex =
            std::str::from_utf8(pair).map_err(|_| "hex.decode(): invalid UTF-8".to_string())?;
        let val = u8::from_str_radix(hex, 16)
            .map_err(|_| "hex.decode(): invalid hex digit".to_string())?;
        out.push(Value::Int(val as i64));
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}
