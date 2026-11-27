use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::buffer::{BufData, Elem};
use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "buffer",
        &[
            ("new", buffer_new),
            ("len", buffer_len),
            ("byte_len", buffer_byte_len),
            ("elem_type", buffer_elem_type),
            ("get", buffer_get),
            ("set", buffer_set),
            ("fill", buffer_fill),
            ("fill_rgba", buffer_fill_rgba),
            ("blit", buffer_blit),
            ("copy_from", buffer_copy_from),
            ("slice", buffer_slice),
            ("clone", buffer_clone),
            ("as_bytes", buffer_as_bytes),
            ("from_bytes", buffer_from_bytes),
        ],
        &[],
    )
}

fn expect_buf<'a>(v: &'a Value, fn_name: &str) -> Result<&'a Rc<RefCell<BufData>>, String> {
    match v {
        Value::Buffer(b) => Ok(b),
        other => Err(format!(
            "buffer.{}() expects a buffer, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn expect_int(v: &Value, ctx: &str) -> Result<i64, String> {
    match v {
        Value::Int(i) => Ok(*i),
        other => Err(format!("{} expects int, got '{}'", ctx, other.type_name())),
    }
}

fn expect_nonneg_usize(v: &Value, ctx: &str) -> Result<usize, String> {
    match v {
        Value::Int(i) if *i >= 0 => usize::try_from(*i).map_err(|_| format!("{}: integer out of range", ctx)),
        Value::Int(i) => Err(format!("{} must be non-negative, got {}", ctx, i)),
        other => Err(format!("{} expects non-negative int, got '{}'", ctx, other.type_name())),
    }
}

fn buffer_new(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let (elem, len) = match args {
        [len_v] => (Elem::U8, expect_nonneg_usize(len_v, "buffer.new() length")?),
        [elem_v, len_v] => {
            let Some(elem_s) = elem_v.as_str() else {
                return Err(format!(
                    "buffer.new() expects element type string ('u8'|'i32'|'u32'|'f32'), got '{}'",
                    elem_v.type_name()
                ));
            };
            let elem = Elem::parse(elem_s)?;
            let len = expect_nonneg_usize(len_v, "buffer.new() length")?;
            (elem, len)
        }
        _ => {
            return Err(
                "buffer.new() expects 1 or 2 arguments: (len) or (elem_type, len)".to_string(),
            );
        }
    };
    let core = Arc::clone(vm.budget_core());
    let data = BufData::new_zeroed(elem, len, Some(core))?;
    Ok(Value::Buffer(Rc::new(RefCell::new(data))))
}

fn buffer_len(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("buffer.len() expects 1 argument (buf)".to_string());
    }
    let buf = expect_buf(&args[0], "len")?;
    Ok(Value::Int(buf.borrow().len as i64))
}

fn buffer_byte_len(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("buffer.byte_len() expects 1 argument (buf)".to_string());
    }
    let buf = expect_buf(&args[0], "byte_len")?;
    Ok(Value::Int(buf.borrow().byte_len() as i64))
}

fn buffer_elem_type(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("buffer.elem_type() expects 1 argument (buf)".to_string());
    }
    let buf = expect_buf(&args[0], "elem_type")?;
    Ok(Value::from_str(buf.borrow().elem.name()))
}

fn buffer_get(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("buffer.get() expects 2 arguments (buf, index)".to_string());
    }
    let buf = expect_buf(&args[0], "get")?.borrow();
    let idx = expect_int(&args[1], "buffer.get() index")?;
    let actual = buf.resolve_index(idx, "buffer.get()")?;
    buf.get_value(actual)
}

fn buffer_set(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("buffer.set() expects 3 arguments (buf, index, val)".to_string());
    }
    let mut buf = expect_buf(&args[0], "set")?.borrow_mut();
    let idx = expect_int(&args[1], "buffer.set() index")?;
    let actual = buf.resolve_index(idx, "buffer.set()")?;
    buf.set_value(actual, &args[2])?;
    Ok(args[2].clone())
}

fn buffer_fill(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("buffer.fill() expects 2 arguments (buf, val)".to_string());
    }
    let rc = expect_buf(&args[0], "fill")?;
    let bytes = rc.borrow().byte_len();
    vm.charge_work((bytes as u64) / 64 + 1)?;
    rc.borrow_mut().fill_value(&args[1])?;
    Ok(args[0].clone())
}

fn expect_channel_u8(v: &Value, ctx: &str) -> Result<u8, String> {
    match v {
        Value::Int(i) if (0..=255).contains(i) => Ok(*i as u8),
        Value::Int(i) => Err(format!("{} channel {} out of range 0..=255", ctx, i)),
        other => Err(format!("{} expects int 0..=255, got '{}'", ctx, other.type_name())),
    }
}

fn buffer_fill_rgba(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let (rc, packed) = match args {
        [buf_v, packed_v] => {
            let rc = expect_buf(buf_v, "fill_rgba")?;
            let p = match packed_v {
                Value::Int(i) => {
                    u32::try_from(*i)
                        .map_err(|_| format!("buffer.fill_rgba() color {} out of u32 range", i))?
                }
                other => {
                    return Err(format!(
                        "buffer.fill_rgba() expects packed integer, got '{}'",
                        other.type_name()
                    ));
                }
            };
            (rc, p)
        }
        [buf_v, r_v, g_v, b_v] => {
            let rc = expect_buf(buf_v, "fill_rgba")?;
            let r = expect_channel_u8(r_v, "buffer.fill_rgba()")? as u32;
            let g = expect_channel_u8(g_v, "buffer.fill_rgba()")? as u32;
            let b = expect_channel_u8(b_v, "buffer.fill_rgba()")? as u32;
            (rc, (0xFFu32 << 24) | (r << 16) | (g << 8) | b)
        }
        [buf_v, r_v, g_v, b_v, a_v] => {
            let rc = expect_buf(buf_v, "fill_rgba")?;
            let r = expect_channel_u8(r_v, "buffer.fill_rgba()")? as u32;
            let g = expect_channel_u8(g_v, "buffer.fill_rgba()")? as u32;
            let b = expect_channel_u8(b_v, "buffer.fill_rgba()")? as u32;
            let a = expect_channel_u8(a_v, "buffer.fill_rgba()")? as u32;
            (rc, (a << 24) | (r << 16) | (g << 8) | b)
        }
        _ => {
            return Err(
                "buffer.fill_rgba() expects (buf, argb_u32) or (buf, r, g, b, [a])".to_string(),
            );
        }
    };
    let bytes = rc.borrow().byte_len();
    vm.charge_work((bytes as u64) / 64 + 1)?;
    rc.borrow_mut().fill_rgba(packed)?;
    Ok(args[0].clone())
}

fn buffer_copy_from(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 5 {
        return Err(
            "buffer.copy_from() expects 3 to 5 arguments (dst, dst_off, src, [src_off, count])"
                .to_string(),
        );
    }
    let dst_rc = expect_buf(&args[0], "copy_from")?;
    let dst_off = expect_nonneg_usize(&args[1], "buffer.copy_from() dst_off")?;
    let src_rc = expect_buf(&args[2], "copy_from")?;
    let src_off = if args.len() >= 4 {
        expect_nonneg_usize(&args[3], "buffer.copy_from() src_off")?
    } else {
        0
    };
    let self_alias = Rc::ptr_eq(dst_rc, src_rc);
    if self_alias {
        let (src_elem, count, src_raw_copy) = {
            let s = src_rc.borrow();
            let count = if args.len() == 5 {
                expect_nonneg_usize(&args[4], "buffer.copy_from() count")?
            } else {
                s.len.saturating_sub(src_off)
            };
            let w = s.elem.width();
            vm.charge_work(((count * w) as u64) / 64 + 1)?;
            let needed_bytes = (count * w) as u64;
            let copied = vm.budget_core().with_temp_charge(needed_bytes, || {
                if src_off.saturating_add(count) > s.len {
                    return Err("buffer.copy_from(): source range out of bounds".to_string());
                }
                let start_b = src_off * w;
                let end_b = start_b + count * w;
                Ok(s.raw[start_b..end_b].to_vec())
            })?;
            (s.elem, count, copied)
        };
        let mut dst = dst_rc.borrow_mut();
        dst.copy_range_from(dst_off, src_elem, &src_raw_copy, 0, count)?;
    } else {
        let src = src_rc.borrow();
        let count = if args.len() == 5 {
            expect_nonneg_usize(&args[4], "buffer.copy_from() count")?
        } else {
            src.len.saturating_sub(src_off)
        };
        let w = src.elem.width();
        vm.charge_work(((count * w) as u64) / 64 + 1)?;
        let mut dst = dst_rc.borrow_mut();
        dst.copy_range_from(dst_off, src.elem, &src.raw, src_off, count)?;
    }
    Ok(args[0].clone())
}

fn buffer_blit(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 8 && args.len() != 12 {
        return Err(
            "buffer.blit() expects (dst, dst_w, dst_h, dst_x, dst_y, src, src_w, src_h, [src_x, src_y, copy_w, copy_h])"
                .to_string(),
        );
    }
    let dst_rc = expect_buf(&args[0], "blit")?;
    let dst_w = expect_nonneg_usize(&args[1], "buffer.blit() dst_w")?;
    let dst_h = expect_nonneg_usize(&args[2], "buffer.blit() dst_h")?;
    let dst_x = expect_int(&args[3], "buffer.blit() dst_x")?;
    let dst_y = expect_int(&args[4], "buffer.blit() dst_y")?;
    let src_rc = expect_buf(&args[5], "blit")?;
    let src_w = expect_nonneg_usize(&args[6], "buffer.blit() src_w")?;
    let src_h = expect_nonneg_usize(&args[7], "buffer.blit() src_h")?;
    let (src_x, src_y, copy_w, copy_h) = if args.len() == 12 {
        (
            expect_int(&args[8], "buffer.blit() src_x")?,
            expect_int(&args[9], "buffer.blit() src_y")?,
            expect_int(&args[10], "buffer.blit() copy_w")?,
            expect_int(&args[11], "buffer.blit() copy_h")?,
        )
    } else {
        (0, 0, src_w as i64, src_h as i64)
    };
    let work_pixels = (copy_w.max(0) as u64).saturating_mul(copy_h.max(0) as u64);
    vm.charge_work(work_pixels / 64 + 1)?;

    if Rc::ptr_eq(dst_rc, src_rc) {
        let (src_elem, src_raw_copy) = {
            let s = src_rc.borrow();
            let raw_bytes = s.byte_len() as u64;
            let copied = vm.budget_core().with_temp_charge(raw_bytes, || Ok(s.raw.clone()))?;
            (s.elem, copied)
        };
        let mut dst = dst_rc.borrow_mut();
        dst.blit_2d(
            dst_w,
            dst_h,
            dst_x,
            dst_y,
            src_elem,
            &src_raw_copy,
            src_w,
            src_h,
            src_x,
            src_y,
            copy_w,
            copy_h,
        )?;
    } else {
        let src = src_rc.borrow();
        let mut dst = dst_rc.borrow_mut();
        dst.blit_2d(
            dst_w,
            dst_h,
            dst_x,
            dst_y,
            src.elem,
            &src.raw,
            src_w,
            src_h,
            src_x,
            src_y,
            copy_w,
            copy_h,
        )?;
    }
    Ok(args[0].clone())
}

fn buffer_slice(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 && args.len() != 3 {
        return Err("buffer.slice() expects 2 or 3 arguments (buf, start, [end])".to_string());
    }
    let src = expect_buf(&args[0], "slice")?.borrow();
    let len_i = src.len as i64;
    let start_i = expect_int(&args[1], "buffer.slice() start")?;
    let end_i = if args.len() == 3 {
        expect_int(&args[2], "buffer.slice() end")?
    } else {
        len_i
    };
    let s = (if start_i < 0 { len_i + start_i } else { start_i }).clamp(0, len_i) as usize;
    let e = (if end_i < 0 { len_i + end_i } else { end_i }).clamp(0, len_i) as usize;
    let count = e.saturating_sub(s);
    let w = src.elem.width();
    let byte_len = BufData::byte_len_for(src.elem, count)?;
    let core = Arc::clone(vm.budget_core());
    core.charge_alloc(byte_len as u64)?;
    let raw = if count > 0 {
        src.raw[s * w..e * w].to_vec()
    } else {
        Vec::new()
    };
    let sliced = BufData::from_raw(src.elem, raw, byte_len, Some(Arc::downgrade(&core)))?;
    Ok(Value::Buffer(Rc::new(RefCell::new(sliced))))
}

fn buffer_clone(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("buffer.clone() expects 1 argument (buf)".to_string());
    }
    let src = expect_buf(&args[0], "clone")?.borrow();
    let byte_len = src.byte_len();
    let core = Arc::clone(vm.budget_core());
    core.charge_alloc(byte_len as u64)?;
    vm.charge_work((byte_len as u64) / 64 + 1)?;
    let cloned = BufData::from_raw(src.elem, src.raw.clone(), byte_len, Some(Arc::downgrade(&core)))?;
    Ok(Value::Buffer(Rc::new(RefCell::new(cloned))))
}

fn buffer_as_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("buffer.as_bytes() expects 1 argument (buf)".to_string());
    }
    let src = expect_buf(&args[0], "as_bytes")?.borrow();
    vm.charge_alloc(src.raw.len())?;
    vm.charge_work((src.raw.len() as u64) / 64 + 1)?;
    Ok(Value::Bytes(Rc::new(src.raw.clone())))
}

fn buffer_from_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let (raw, elem) = match args {
        [b_v] => (b_v.as_bytes_cow("buffer.from_bytes()")?, Elem::U8),
        [b_v, elem_v] => {
            let Some(elem_s) = elem_v.as_str() else {
                return Err(format!(
                    "buffer.from_bytes() expects element type string, got '{}'",
                    elem_v.type_name()
                ));
            };
            (b_v.as_bytes_cow("buffer.from_bytes()")?, Elem::parse(elem_s)?)
        }
        _ => {
            return Err("buffer.from_bytes() expects 1 or 2 arguments (bytes, [elem_type])".to_string());
        }
    };
    if raw.len() > BufData::MAX_BYTE_LEN {
        return Err("buffer.from_bytes() exceeds 64 MiB limit".to_string());
    }
    if raw.len() % elem.width() != 0 {
        return Err(format!(
            "buffer.from_bytes(): byte length {} is not divisible by '{}' width {}",
            raw.len(),
            elem.name(),
            elem.width()
        ));
    }
    let byte_len = raw.len();
    let core = Arc::clone(vm.budget_core());
    core.charge_alloc(byte_len as u64)?;
    vm.charge_work((byte_len as u64) / 64 + 1)?;
    let buf = BufData::from_raw(
        elem,
        raw.into_owned(),
        byte_len,
        Some(Arc::downgrade(&core)),
    )?;
    Ok(Value::Buffer(Rc::new(RefCell::new(buf))))
}
