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

fn hex_encode(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("hex.encode() expects 1 argument (bytes, str, or byte list)".to_string());
    }
    let bytes = args[0].as_bytes_cow("hex.encode()")?;
    let out_len = bytes.len().saturating_mul(2);
    vm.charge_alloc(out_len)?;
    vm.charge_work((bytes.len() as u64) / 32 + 1)?;
    let mut out = String::with_capacity(out_len);
    for &b in bytes.iter() {
        out.push_str(&format!("{:02x}", b));
    }
    Ok(Value::Str(out))
}

fn hex_decode(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
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
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("hex.decode(): invalid hex digit".to_string());
    }
    vm.charge_alloc(s.len() / 2)?;
    vm.charge_work((s.len() as u64) / 32 + 1)?;
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let hex =
            std::str::from_utf8(pair).map_err(|_| "hex.decode(): invalid UTF-8".to_string())?;
        let val = u8::from_str_radix(hex, 16)
            .map_err(|_| "hex.decode(): invalid hex digit".to_string())?;
        out.push(val);
    }
    Ok(Value::Bytes(Rc::new(out)))
}
