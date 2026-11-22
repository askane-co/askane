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

#[derive(Clone, Copy)]
enum FmtOp {
    I8(usize),
    U8(usize),
    I16(usize),
    U16(usize),
    I32(usize),
    U32(usize),
    I64(usize),
    U64(usize),
    F32(usize),
    F64(usize),
    FixedBytes(usize),
    Pad(usize),
}

const MAX_BINARY_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_BINARY_TOTAL_VALS: usize = 1_048_576;

struct ParsedFmt {
    endian: Endian,
    ops: Vec<FmtOp>,
    total_bytes: usize,
    total_vals: usize,
}

fn parse_format(fmt: &str) -> Result<ParsedFmt, String> {
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
    let mut total_bytes = 0usize;
    let mut total_vals = 0usize;
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
        let (op, elem_size, consumes_vals) = match code {
            's' => (FmtOp::FixedBytes(repeat), repeat, 1usize),
            'x' => (FmtOp::Pad(repeat), repeat, 0usize),
            'b' => (FmtOp::I8(repeat), 1, repeat),
            'B' => (FmtOp::U8(repeat), 1, repeat),
            'h' => (FmtOp::I16(repeat), 2, repeat),
            'H' => (FmtOp::U16(repeat), 2, repeat),
            'i' => (FmtOp::I32(repeat), 4, repeat),
            'I' => (FmtOp::U32(repeat), 4, repeat),
            'q' => (FmtOp::I64(repeat), 8, repeat),
            'Q' => (FmtOp::U64(repeat), 8, repeat),
            'f' => (FmtOp::F32(repeat), 4, repeat),
            'd' => (FmtOp::F64(repeat), 8, repeat),
            other => return Err(format!("Unknown binary format code '{}'", other)),
        };
        let add_bytes = if matches!(code, 's' | 'x') {
            elem_size
        } else {
            elem_size
                .checked_mul(repeat)
                .ok_or_else(|| "binary format total size overflow".to_string())?
        };
        total_bytes = total_bytes
            .checked_add(add_bytes)
            .filter(|&b| b <= MAX_BINARY_TOTAL_BYTES)
            .ok_or_else(|| "binary format total output size exceeds 32 MiB cap".to_string())?;
        total_vals = total_vals
            .checked_add(consumes_vals)
            .filter(|&v| v <= MAX_BINARY_TOTAL_VALS)
            .ok_or_else(|| "binary format value count exceeds safety cap".to_string())?;
        ops.push(op);
    }
    Ok(ParsedFmt {
        endian,
        ops,
        total_bytes,
        total_vals,
    })
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
    let parsed = parse_format(fmt)?;
    let vals = if args.len() == 2 && matches!(&args[1], Value::List(_)) {
        if let Value::List(l) = &args[1] {
            l.borrow().clone()
        } else {
            unreachable!()
        }
    } else {
        args[1..].to_vec()
    };

    if vals.len() != parsed.total_vals {
        return Err(format!(
            "binary.pack() expects {} value(s) for format '{}', got {}",
            parsed.total_vals,
            fmt,
            vals.len()
        ));
    }

    vm.charge_alloc(parsed.total_bytes)?;
    vm.charge_work((parsed.total_bytes as u64 / 32).max(1))?;
    let mut out = Vec::with_capacity(parsed.total_bytes);
    let mut vi = 0usize;
    let endian = parsed.endian;
    for op in parsed.ops {
        match op {
            FmtOp::Pad(n) => out.resize(out.len() + n, 0),
            FmtOp::I8(rep) => {
                for _ in 0..rep {
                    let v = expect_int(&vals[vi], "binary.pack(b)")?;
                    vi += 1;
                    out.push((v as i8) as u8);
                }
            }
            FmtOp::U8(rep) => {
                for _ in 0..rep {
                    let v = expect_int(&vals[vi], "binary.pack(B)")?;
                    vi += 1;
                    out.push(v as u8);
                }
            }
            FmtOp::I16(rep) => {
                for _ in 0..rep {
                    let v = expect_int(&vals[vi], "binary.pack(h)")?;
                    vi += 1;
                    push_ordered(&mut out, &(v as i16).to_le_bytes(), endian);
                }
            }
            FmtOp::U16(rep) => {
                for _ in 0..rep {
                    let v = expect_int(&vals[vi], "binary.pack(H)")?;
                    vi += 1;
                    push_ordered(&mut out, &(v as u16).to_le_bytes(), endian);
                }
            }
            FmtOp::I32(rep) => {
                for _ in 0..rep {
                    let v = expect_int(&vals[vi], "binary.pack(i)")?;
                    vi += 1;
                    push_ordered(&mut out, &(v as i32).to_le_bytes(), endian);
                }
            }
            FmtOp::U32(rep) => {
                for _ in 0..rep {
                    let v = expect_int(&vals[vi], "binary.pack(I)")?;
                    vi += 1;
                    push_ordered(&mut out, &(v as u32).to_le_bytes(), endian);
                }
            }
            FmtOp::I64(rep) | FmtOp::U64(rep) => {
                for _ in 0..rep {
                    let v = expect_int(&vals[vi], "binary.pack(q)")?;
                    vi += 1;
                    push_ordered(&mut out, &v.to_le_bytes(), endian);
                }
            }
            FmtOp::F32(rep) => {
                for _ in 0..rep {
                    let v = expect_float(&vals[vi], "binary.pack(f)")? as f32;
                    vi += 1;
                    push_ordered(&mut out, &v.to_le_bytes(), endian);
                }
            }
            FmtOp::F64(rep) => {
                for _ in 0..rep {
                    let v = expect_float(&vals[vi], "binary.pack(d)")?;
                    vi += 1;
                    push_ordered(&mut out, &v.to_le_bytes(), endian);
                }
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
    Ok(Value::Bytes(Rc::new(out)))
}

fn slice_bytes<'a>(args: &'a [Value], ctx: &str) -> Result<(&'a [u8], usize), String> {
    if args.is_empty() || args.len() > 2 {
        return Err(format!("{} expects (bytes, offset=0)", ctx));
    }
    let offset = if args.len() == 2 {
        let i = expect_int(&args[1], ctx)?;
        usize::try_from(i).map_err(|_| format!("{} offset out of range", ctx))?
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
    let parsed = parse_format(fmt)?;
    let raw = args[1].as_bytes_cow("binary.unpack")?;
    if raw.len() < parsed.total_bytes {
        return Err(format!(
            "binary.unpack(): buffer too short (need {} bytes, got {})",
            parsed.total_bytes,
            raw.len()
        ));
    }
    vm.charge_alloc(
        parsed
            .total_vals
            .saturating_mul(std::mem::size_of::<Value>()),
    )?;
    vm.charge_work((parsed.total_bytes as u64 / 32).max(1))?;
    let mut off = 0usize;
    let mut out = Vec::with_capacity(parsed.total_vals);
    let endian = parsed.endian;
    for op in parsed.ops {
        let (unit, rep, is_bytes, is_pad) = match op {
            FmtOp::Pad(n) => (n, 1, false, true),
            FmtOp::FixedBytes(n) => (n, 1, true, false),
            FmtOp::I8(rep) | FmtOp::U8(rep) => (1, rep, false, false),
            FmtOp::I16(rep) | FmtOp::U16(rep) => (2, rep, false, false),
            FmtOp::I32(rep) | FmtOp::U32(rep) | FmtOp::F32(rep) => (4, rep, false, false),
            FmtOp::I64(rep) | FmtOp::U64(rep) | FmtOp::F64(rep) => (8, rep, false, false),
        };
        if is_pad {
            off += unit;
            continue;
        }
        if is_bytes {
            let s = &raw[off..off + unit];
            off += unit;
            vm.charge_alloc(s.len())?;
            out.push(Value::Bytes(Rc::new(s.to_vec())));
            continue;
        }
        for idx in 0..rep {
            if (idx & 1023) == 0 {
                vm.check_timeout()?;
            }
            let s = &raw[off..off + unit];
            off += unit;
            let val = match op {
                FmtOp::I8(_) => Value::Int((s[0] as i8) as i64),
                FmtOp::U8(_) => Value::Int(s[0] as i64),
                FmtOp::I16(_) => Value::Int(match endian {
                    Endian::Little => i16::from_le_bytes([s[0], s[1]]) as i64,
                    Endian::Big => i16::from_be_bytes([s[0], s[1]]) as i64,
                }),
                FmtOp::U16(_) => Value::Int(match endian {
                    Endian::Little => u16::from_le_bytes([s[0], s[1]]) as i64,
                    Endian::Big => u16::from_be_bytes([s[0], s[1]]) as i64,
                }),
                FmtOp::I32(_) => Value::Int(match endian {
                    Endian::Little => i32::from_le_bytes(s.try_into().unwrap()) as i64,
                    Endian::Big => i32::from_be_bytes(s.try_into().unwrap()) as i64,
                }),
                FmtOp::U32(_) => Value::Int(match endian {
                    Endian::Little => u32::from_le_bytes(s.try_into().unwrap()) as i64,
                    Endian::Big => u32::from_be_bytes(s.try_into().unwrap()) as i64,
                }),
                FmtOp::I64(_) | FmtOp::U64(_) => Value::Int(match endian {
                    Endian::Little => i64::from_le_bytes(s.try_into().unwrap()),
                    Endian::Big => i64::from_be_bytes(s.try_into().unwrap()),
                }),
                FmtOp::F32(_) => Value::Float(match endian {
                    Endian::Little => f32::from_le_bytes(s.try_into().unwrap()) as f64,
                    Endian::Big => f32::from_be_bytes(s.try_into().unwrap()) as f64,
                }),
                FmtOp::F64(_) => Value::Float(match endian {
                    Endian::Little => f64::from_le_bytes(s.try_into().unwrap()),
                    Endian::Big => f64::from_be_bytes(s.try_into().unwrap()),
                }),
                FmtOp::Pad(_) | FmtOp::FixedBytes(_) => unreachable!(),
            };
            out.push(val);
        }
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn read_n<const N: usize>(args: &[Value], ctx: &str) -> Result<[u8; N], String> {
    let (b, off) = slice_bytes(args, ctx)?;
    let end = off
        .checked_add(N)
        .filter(|&e| e <= b.len())
        .ok_or_else(|| format!("{} offset {} out of range (len {})", ctx, off, b.len()))?;
    Ok(b[off..end].try_into().unwrap())
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
