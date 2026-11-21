use std::cell::RefCell;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "binary",
        &[
            ("pack", bin_pack),
            ("unpack", bin_unpack),
            ("read_u16_le", read_u16_le),
            ("read_u16_be", read_u16_be),
            ("read_u32_le", read_u32_le),
            ("read_u32_be", read_u32_be),
            ("read_i32_le", read_i32_le),
            ("read_i32_be", read_i32_be),
            ("read_i64_le", read_i64_le),
            ("read_i64_be", read_i64_be),
            ("read_f32_le", read_f32_le),
            ("read_f32_be", read_f32_be),
            ("read_f64_le", read_f64_le),
            ("read_f64_be", read_f64_be),
            ("write_u16_le", write_u16_le),
            ("write_u16_be", write_u16_be),
            ("write_u32_le", write_u32_le),
            ("write_u32_be", write_u32_be),
            ("write_i64_le", write_i64_le),
            ("write_i64_be", write_i64_be),
            ("write_f32_le", write_f32_le),
            ("write_f64_le", write_f64_le),
        ],
        &[],
    )
}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

enum FmtOp {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    FixedBytes(usize),
    Pad(usize),
}

fn parse_format(fmt: &str) -> Result<(Endian, Vec<FmtOp>), String> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0usize;
    let endian = match chars.first().copied() {
        Some('<') => {
            i = 1;
            Endian::Little
        }
        Some('>' | '!') => {
            i = 1;
            Endian::Big
        }
        Some('@' | '=') => {
            i = 1;
            if cfg!(target_endian = "little") {
                Endian::Little
            } else {
                Endian::Big
            }
        }
        _ => Endian::Little,
    };

    let mut ops = Vec::new();
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let mut count = 0usize;
        let mut has_count = false;
        while i < chars.len() && chars[i].is_ascii_digit() {
            has_count = true;
            count = count
                .checked_mul(10)
                .and_then(|c| c.checked_add((chars[i] as u8 - b'0') as usize))
                .ok_or_else(|| "binary format repeat count overflow".to_string())?;
            i += 1;
        }
        if count > 1_048_576 {
            return Err("binary format repeat count exceeds safety cap".to_string());
        }
        let repeat = if has_count { count } else { 1 };
        let Some(code) = chars.get(i).copied() else {
            return Err("Trailing count without format specifier".to_string());
        };
        i += 1;
        match code {
            's' => ops.push(FmtOp::FixedBytes(repeat)),
            'x' => ops.push(FmtOp::Pad(repeat)),
            'b' => ops.extend((0..repeat).map(|_| FmtOp::I8)),
            'B' => ops.extend((0..repeat).map(|_| FmtOp::U8)),
            'h' => ops.extend((0..repeat).map(|_| FmtOp::I16)),
            'H' => ops.extend((0..repeat).map(|_| FmtOp::U16)),
            'i' => ops.extend((0..repeat).map(|_| FmtOp::I32)),
            'I' => ops.extend((0..repeat).map(|_| FmtOp::U32)),
            'q' => ops.extend((0..repeat).map(|_| FmtOp::I64)),
            'Q' => ops.extend((0..repeat).map(|_| FmtOp::U64)),
            'f' => ops.extend((0..repeat).map(|_| FmtOp::F32)),
            'd' => ops.extend((0..repeat).map(|_| FmtOp::F64)),
            other => return Err(format!("Unknown binary format code '{}'", other)),
        }
    }
    Ok((endian, ops))
}

fn expect_int(v: &Value, ctx: &str) -> Result<i64, String> {
    match v {
        Value::Int(i) => Ok(*i),
        other => Err(format!("{} expects int, got '{}'", ctx, other.type_name())),
    }
}

fn expect_float(v: &Value, ctx: &str) -> Result<f64, String> {
    match v {
        Value::Float(f) => Ok(*f),
        Value::Int(i) => Ok(*i as f64),
        other => Err(format!(
            "{} expects number, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn push_ordered(out: &mut Vec<u8>, bytes: &[u8], endian: Endian) {
    match endian {
        Endian::Little => out.extend_from_slice(bytes),
        Endian::Big => out.extend(bytes.iter().rev()),
    }
}

fn bin_pack(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() {
        return Err("binary.pack() expects format string and values".to_string());
    }
    let Value::Str(fmt) = &args[0] else {
        return Err("binary.pack() first argument must be format string".to_string());
    };
    let (endian, ops) = parse_format(fmt)?;
    let vals = if args.len() == 2 && matches!(&args[1], Value::List(_)) {
        if let Value::List(l) = &args[1] {
            l.borrow().clone()
        } else {
            unreachable!()
        }
    } else {
        args[1..].to_vec()
    };

    let expected = ops.iter().filter(|op| !matches!(op, FmtOp::Pad(_))).count();
    if vals.len() != expected {
        return Err(format!(
            "binary.pack() expects {} value(s) for format '{}', got {}",
            expected,
            fmt,
            vals.len()
        ));
    }

    let mut out = Vec::new();
    let mut vi = 0usize;
    for op in ops {
        match op {
            FmtOp::Pad(n) => out.resize(out.len() + n, 0),
            FmtOp::I8 => {
                let v = expect_int(&vals[vi], "binary.pack(b)")?;
                vi += 1;
                out.push((v as i8) as u8);
            }
            FmtOp::U8 => {
                let v = expect_int(&vals[vi], "binary.pack(B)")?;
                vi += 1;
                out.push(v as u8);
            }
            FmtOp::I16 => {
                let v = expect_int(&vals[vi], "binary.pack(h)")?;
                vi += 1;
                push_ordered(&mut out, &(v as i16).to_le_bytes(), endian);
            }
            FmtOp::U16 => {
                let v = expect_int(&vals[vi], "binary.pack(H)")?;
                vi += 1;
                push_ordered(&mut out, &(v as u16).to_le_bytes(), endian);
            }
            FmtOp::I32 => {
                let v = expect_int(&vals[vi], "binary.pack(i)")?;
                vi += 1;
                push_ordered(&mut out, &(v as i32).to_le_bytes(), endian);
            }
            FmtOp::U32 => {
                let v = expect_int(&vals[vi], "binary.pack(I)")?;
                vi += 1;
                push_ordered(&mut out, &(v as u32).to_le_bytes(), endian);
            }
            FmtOp::I64 | FmtOp::U64 => {
                let v = expect_int(&vals[vi], "binary.pack(q)")?;
                vi += 1;
                push_ordered(&mut out, &v.to_le_bytes(), endian);
            }
            FmtOp::F32 => {
                let v = expect_float(&vals[vi], "binary.pack(f)")? as f32;
                vi += 1;
                push_ordered(&mut out, &v.to_le_bytes(), endian);
            }
            FmtOp::F64 => {
                let v = expect_float(&vals[vi], "binary.pack(d)")?;
                vi += 1;
                push_ordered(&mut out, &v.to_le_bytes(), endian);
            }
            FmtOp::FixedBytes(n) => {
                let raw = vals[vi].as_bytes_cow("binary.pack(s)")?;
                vi += 1;
                let copy_len = raw.len().min(n);
                out.extend_from_slice(&raw[..copy_len]);
                out.resize(out.len() + (n - copy_len), 0);
            }
        }
    }
    vm.charge_alloc(out.len())?;
    Ok(Value::Bytes(Rc::new(out)))
}

fn slice_bytes<'a>(args: &'a [Value], ctx: &str) -> Result<(&'a [u8], usize), String> {
    if args.is_empty() || args.len() > 2 {
        return Err(format!("{} expects (bytes, offset=0)", ctx));
    }
    let offset = if args.len() == 2 {
        let i = expect_int(&args[1], ctx)?;
        if i < 0 {
            return Err(format!("{} offset must be >= 0", ctx));
        }
        i as usize
    } else {
        0
    };
    let Value::Bytes(b) = &args[0] else {
        return Err(format!(
            "{} expects bytes, got '{}'",
            ctx,
            args[0].type_name()
        ));
    };
    Ok((b.as_slice(), offset))
}

fn bin_unpack(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("binary.unpack() expects 2 arguments (fmt, bytes)".to_string());
    }
    let Value::Str(fmt) = &args[0] else {
        return Err("binary.unpack() format must be a string".to_string());
    };
    let (endian, ops) = parse_format(fmt)?;
    let raw = args[1].as_bytes_cow("binary.unpack")?;
    let mut off = 0usize;
    let mut out = Vec::new();
    for op in ops {
        let need = match op {
            FmtOp::Pad(n) | FmtOp::FixedBytes(n) => n,
            FmtOp::I8 | FmtOp::U8 => 1,
            FmtOp::I16 | FmtOp::U16 => 2,
            FmtOp::I32 | FmtOp::U32 | FmtOp::F32 => 4,
            FmtOp::I64 | FmtOp::U64 | FmtOp::F64 => 8,
        };
        if off + need > raw.len() {
            return Err(format!(
                "binary.unpack(): buffer too short (need {} bytes at offset {}, len {})",
                need,
                off,
                raw.len()
            ));
        }
        let s = &raw[off..off + need];
        off += need;
        let val = match op {
            FmtOp::Pad(_) => continue,
            FmtOp::I8 => Value::Int((s[0] as i8) as i64),
            FmtOp::U8 => Value::Int(s[0] as i64),
            FmtOp::I16 => Value::Int(match endian {
                Endian::Little => i16::from_le_bytes([s[0], s[1]]) as i64,
                Endian::Big => i16::from_be_bytes([s[0], s[1]]) as i64,
            }),
            FmtOp::U16 => Value::Int(match endian {
                Endian::Little => u16::from_le_bytes([s[0], s[1]]) as i64,
                Endian::Big => u16::from_be_bytes([s[0], s[1]]) as i64,
            }),
            FmtOp::I32 => Value::Int(match endian {
                Endian::Little => i32::from_le_bytes(s.try_into().unwrap()) as i64,
                Endian::Big => i32::from_be_bytes(s.try_into().unwrap()) as i64,
            }),
            FmtOp::U32 => Value::Int(match endian {
                Endian::Little => u32::from_le_bytes(s.try_into().unwrap()) as i64,
                Endian::Big => u32::from_be_bytes(s.try_into().unwrap()) as i64,
            }),
            FmtOp::I64 | FmtOp::U64 => Value::Int(match endian {
                Endian::Little => i64::from_le_bytes(s.try_into().unwrap()),
                Endian::Big => i64::from_be_bytes(s.try_into().unwrap()),
            }),
            FmtOp::F32 => Value::Float(match endian {
                Endian::Little => f32::from_le_bytes(s.try_into().unwrap()) as f64,
                Endian::Big => f32::from_be_bytes(s.try_into().unwrap()) as f64,
            }),
            FmtOp::F64 => Value::Float(match endian {
                Endian::Little => f64::from_le_bytes(s.try_into().unwrap()),
                Endian::Big => f64::from_be_bytes(s.try_into().unwrap()),
            }),
            FmtOp::FixedBytes(_) => Value::Bytes(Rc::new(s.to_vec())),
        };
        out.push(val);
    }
    vm.charge_alloc(out.len().saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn read_n<const N: usize>(args: &[Value], ctx: &str) -> Result<[u8; N], String> {
    let (b, off) = slice_bytes(args, ctx)?;
    if off + N > b.len() {
        return Err(format!(
            "{} offset {} out of range (len {})",
            ctx,
            off,
            b.len()
        ));
    }
    Ok(b[off..off + N].try_into().unwrap())
}

fn read_u16_le(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Int(
        u16::from_le_bytes(read_n::<2>(a, "binary.read_u16_le")?) as i64,
    ))
}
fn read_u16_be(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Int(
        u16::from_be_bytes(read_n::<2>(a, "binary.read_u16_be")?) as i64,
    ))
}
fn read_u32_le(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Int(
        u32::from_le_bytes(read_n::<4>(a, "binary.read_u32_le")?) as i64,
    ))
}
fn read_u32_be(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Int(
        u32::from_be_bytes(read_n::<4>(a, "binary.read_u32_be")?) as i64,
    ))
}
fn read_i32_le(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Int(
        i32::from_le_bytes(read_n::<4>(a, "binary.read_i32_le")?) as i64,
    ))
}
fn read_i32_be(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Int(
        i32::from_be_bytes(read_n::<4>(a, "binary.read_i32_be")?) as i64,
    ))
}
fn read_i64_le(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Int(i64::from_le_bytes(read_n::<8>(
        a,
        "binary.read_i64_le",
    )?)))
}
fn read_i64_be(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Int(i64::from_be_bytes(read_n::<8>(
        a,
        "binary.read_i64_be",
    )?)))
}
fn read_f32_le(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Float(
        f32::from_le_bytes(read_n::<4>(a, "binary.read_f32_le")?) as f64,
    ))
}
fn read_f32_be(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Float(
        f32::from_be_bytes(read_n::<4>(a, "binary.read_f32_be")?) as f64,
    ))
}
fn read_f64_le(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Float(f64::from_le_bytes(read_n::<8>(
        a,
        "binary.read_f64_le",
    )?)))
}
fn read_f64_be(_vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    Ok(Value::Float(f64::from_be_bytes(read_n::<8>(
        a,
        "binary.read_f64_be",
    )?)))
}

fn write_u16_le(vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    if a.len() != 1 {
        return Err("binary.write_u16_le() expects 1 integer argument".to_string());
    }
    let bytes = (expect_int(&a[0], "binary.write_u16_le")? as u16)
        .to_le_bytes()
        .to_vec();
    vm.charge_alloc(2)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}
fn write_u16_be(vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    if a.len() != 1 {
        return Err("binary.write_u16_be() expects 1 integer argument".to_string());
    }
    let bytes = (expect_int(&a[0], "binary.write_u16_be")? as u16)
        .to_be_bytes()
        .to_vec();
    vm.charge_alloc(2)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}
fn write_u32_le(vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    if a.len() != 1 {
        return Err("binary.write_u32_le() expects 1 integer argument".to_string());
    }
    let bytes = (expect_int(&a[0], "binary.write_u32_le")? as u32)
        .to_le_bytes()
        .to_vec();
    vm.charge_alloc(4)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}
fn write_u32_be(vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    if a.len() != 1 {
        return Err("binary.write_u32_be() expects 1 integer argument".to_string());
    }
    let bytes = (expect_int(&a[0], "binary.write_u32_be")? as u32)
        .to_be_bytes()
        .to_vec();
    vm.charge_alloc(4)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}
fn write_i64_le(vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    if a.len() != 1 {
        return Err("binary.write_i64_le() expects 1 integer argument".to_string());
    }
    let bytes = expect_int(&a[0], "binary.write_i64_le")?
        .to_le_bytes()
        .to_vec();
    vm.charge_alloc(8)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}
fn write_i64_be(vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    if a.len() != 1 {
        return Err("binary.write_i64_be() expects 1 integer argument".to_string());
    }
    let bytes = expect_int(&a[0], "binary.write_i64_be")?
        .to_be_bytes()
        .to_vec();
    vm.charge_alloc(8)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}
fn write_f32_le(vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    if a.len() != 1 {
        return Err("binary.write_f32_le() expects 1 number argument".to_string());
    }
    let bytes = (expect_float(&a[0], "binary.write_f32_le")? as f32)
        .to_le_bytes()
        .to_vec();
    vm.charge_alloc(4)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}
fn write_f64_le(vm: &mut VM, a: &[Value]) -> Result<Value, String> {
    if a.len() != 1 {
        return Err("binary.write_f64_le() expects 1 number argument".to_string());
    }
    let bytes = expect_float(&a[0], "binary.write_f64_le")?
        .to_le_bytes()
        .to_vec();
    vm.charge_alloc(8)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}
