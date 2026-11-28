use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "base64",
        &[
            ("encode", b64_encode),
            ("decode", b64_decode),
            ("decode_bytes", b64_decode_bytes),
            ("encode_url", b64_encode_url),
            ("decode_url", b64_decode_url),
            ("decode_url_bytes", b64_decode_url_bytes),
        ],
        &[],
    )
}

pub(crate) const STD_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub(crate) fn encode_bytes(input: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(alphabet[((triple >> 18) & 0x3F) as usize] as char);
        out.push(alphabet[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(alphabet[((triple >> 6) & 0x3F) as usize] as char);
        } else if pad {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(alphabet[(triple & 0x3F) as usize] as char);
        } else if pad {
            out.push('=');
        }
    }
    out
}

pub(crate) fn decode_str(input: &str, url_safe: bool) -> Result<Vec<u8>, String> {
    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let val_of = |b: u8| -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' if !url_safe => Some(62),
            b'/' if !url_safe => Some(63),
            b'-' if url_safe => Some(62),
            b'_' if url_safe => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    let mut idx = 0;
    while idx < bytes.len() {
        let rem = bytes.len() - idx;
        if rem == 1 {
            return Err("base64: invalid length".to_string());
        }
        let take = rem.min(4);
        let c0 = bytes[idx];
        let c1 = bytes[idx + 1];
        let c2 = if take > 2 { bytes[idx + 2] } else { b'=' };
        let c3 = if take > 3 { bytes[idx + 3] } else { b'=' };
        let v0 = val_of(c0).ok_or_else(|| "base64: invalid character".to_string())? as u32;
        let v1 = val_of(c1).ok_or_else(|| "base64: invalid character".to_string())? as u32;
        let v2 = if c2 == b'=' {
            0
        } else {
            val_of(c2).ok_or_else(|| "base64: invalid character".to_string())? as u32
        };
        let v3 = if c3 == b'=' {
            0
        } else {
            val_of(c3).ok_or_else(|| "base64: invalid character".to_string())? as u32
        };
        if c2 == b'=' && c3 != b'=' {
            return Err("base64: invalid padding".to_string());
        }
        let triple = (v0 << 18) | (v1 << 12) | (v2 << 6) | v3;
        out.push(((triple >> 16) & 0xFF) as u8);
        if c2 != b'=' {
            out.push(((triple >> 8) & 0xFF) as u8);
        }
        if c3 != b'=' {
            out.push((triple & 0xFF) as u8);
        }
        idx += 4;
    }
    Ok(out)
}

fn b64_encode_with(
    vm: &mut VM,
    args: &[Value],
    fn_name: &str,
    alphabet: &[u8; 64],
    pad: bool,
) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!(
            "base64.{}() expects 1 argument (str or bytes)",
            fn_name
        ));
    }
    let raw = match &args[0] {
        Value::Str(s) => s.as_bytes(),
        Value::Bytes(b) => b.as_slice(),
        other => {
            return Err(format!(
                "base64.{}() expects str or bytes, got '{}'",
                fn_name,
                other.type_name()
            ));
        }
    };
    vm.charge_alloc(raw.len().div_ceil(3).saturating_mul(4))?;
    vm.charge_work((raw.len() as u64) / 32 + 1)?;
    Ok(Value::from_str(encode_bytes(raw, alphabet, pad)))
}

fn b64_encode(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    b64_encode_with(vm, args, "encode", STD_ALPHABET, true)
}

fn b64_encode_url(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    b64_encode_with(vm, args, "encode_url", URL_ALPHABET, false)
}

fn b64_decode_impl(
    vm: &mut VM,
    args: &[Value],
    fn_name: &str,
    url_safe: bool,
) -> Result<Vec<u8>, String> {
    if args.len() != 1 {
        return Err(format!("base64.{}() expects 1 string argument", fn_name));
    }
    let s = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "base64.{}() expects a string, got '{}'",
                fn_name,
                other.type_name()
            ));
        }
    };
    vm.charge_alloc((s.len() / 4).saturating_mul(3) + 3)?;
    vm.charge_work((s.len() as u64) / 32 + 1)?;
    decode_str(s, url_safe)
}

fn b64_decode(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let bytes = b64_decode_impl(vm, args, "decode", false)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| "base64.decode(): decoded bytes are not valid UTF-8".to_string())?;
    Ok(Value::from_str(text))
}

fn b64_decode_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let bytes = b64_decode_impl(vm, args, "decode_bytes", false)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}

fn b64_decode_url(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let bytes = b64_decode_impl(vm, args, "decode_url", true)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| "base64.decode_url(): decoded bytes are not valid UTF-8".to_string())?;
    Ok(Value::from_str(text))
}

fn b64_decode_url_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let bytes = b64_decode_impl(vm, args, "decode_url_bytes", true)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}
