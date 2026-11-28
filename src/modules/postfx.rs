use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::media::err_bad_opt;
use crate::modules::build_module;
use crate::modules::gfx;
use crate::modules::scene_util::{
    HandleBank, expect_f32, expect_i32, expect_str, extract_handle_id, is_surface_arg,
    make_handle_def, new_handle,
};
use crate::scene::postfx::{
    Lut3D, apply_bloom, apply_scanlines, apply_vignette, encode_bmp24,
};
use crate::value::{StructDef, Value};
use crate::vm::VM;

thread_local! {
    static LUTS: RefCell<HandleBank<Rc<Lut3D>>> = RefCell::new(HandleBank::new());
    static LUT_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
}

fn lut_def() -> Rc<StructDef> {
    LUT_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "Lut3D",
            &[
                ("apply", postfx_lut_apply),
                ("size", postfx_lut_size),
                ("close", postfx_close),
                ("free", postfx_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

pub fn reset_state() {
    LUTS.with(|l| l.borrow_mut().clear());
}

pub fn module() -> Value {
    build_module(
        "postfx",
        &[
            ("bloom", postfx_bloom),
            ("lut_load", postfx_lut_load),
            ("lut_apply", postfx_lut_apply),
            ("lut_size", postfx_lut_size),
            ("vignette", postfx_vignette),
            ("scanlines", postfx_scanlines),
            ("snapshot", postfx_snapshot),
            ("close", postfx_close),
            ("free", postfx_close),
        ],
        &[],
    )
}

fn split_surface_arg<'a>(args: &'a [Value], start: usize) -> (Option<&'a Value>, usize) {
    if let Some(v) = args.get(start) {
        if is_surface_arg(v) {
            return (Some(v), start + 1);
        } else if matches!(v, Value::Unit) && args.len() > start + 1 {
            return (None, start + 1);
        }
    }
    (None, start)
}

fn postfx_bloom(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 4 {
        return Err(err_bad_opt(
            "postfx.bloom() expects ([surface], [threshold], [intensity], [radius])",
        ));
    }
    let (target, idx) = split_surface_arg(args, 0);
    let threshold = match args.get(idx) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "postfx.bloom() threshold")?,
        _ => 0.75,
    };
    let intensity = match args.get(idx + 1) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "postfx.bloom() intensity")?,
        _ => 0.8,
    };
    let radius = match args.get(idx + 2) {
        Some(v) if !matches!(v, Value::Unit) => expect_i32(v, "postfx.bloom() radius")?,
        _ => 3,
    };
    let budget = Arc::clone(vm.budget_core());
    gfx::with_target_pixels_mut(target, false, |buf, _d, bw, bh, clip, _b| {
        apply_bloom(buf, bw, bh, clip, threshold, intensity, radius, Some(&budget))
    })?;
    Ok(Value::Unit)
}

fn postfx_lut_load(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("postfx.lut_load() expects (path)"));
    }
    let path = expect_str(&args[0], "postfx.lut_load()")?;
    let lut = Lut3D::load_file(vm, path)?;
    let id = LUTS.with(|b| b.borrow_mut().insert(Rc::new(lut), "Lut3D"))?;
    Ok(new_handle(&lut_def(), "Lut3D", id))
}

fn postfx_lut_size(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("postfx.lut_size() expects (lut)"));
    }
    let id = extract_handle_id(&args[0], "Lut3D", "postfx.lut_size()")?;
    let lut = LUTS.with(|b| b.borrow().get(id, "Lut3D").cloned())?;
    Ok(Value::Int(lut.size as i64))
}

fn postfx_lut_apply(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err(err_bad_opt("postfx.lut_apply() expects (lut, [surface], [mix])"));
    }
    let id = extract_handle_id(&args[0], "Lut3D", "postfx.lut_apply()")?;
    let lut = LUTS.with(|b| b.borrow().get(id, "Lut3D").cloned())?;
    let (target, idx) = split_surface_arg(args, 1);
    let mix = match args.get(idx) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "postfx.lut_apply() mix")?,
        _ => 1.0,
    };
    gfx::with_target_pixels_mut(target, false, |buf, _d, bw, bh, clip, _b| {
        lut.apply_surface(buf, bw, bh, clip, mix);
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn postfx_vignette(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 3 {
        return Err(err_bad_opt("postfx.vignette() expects ([surface], [strength], [radius])"));
    }
    let (target, idx) = split_surface_arg(args, 0);
    let strength = match args.get(idx) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "postfx.vignette() strength")?,
        _ => 0.45,
    };
    let radius = match args.get(idx + 1) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "postfx.vignette() radius")?,
        _ => 0.8,
    };
    gfx::with_target_pixels_mut(target, false, |buf, _d, bw, bh, clip, _b| {
        apply_vignette(buf, bw, bh, clip, strength, radius);
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn postfx_scanlines(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 3 {
        return Err(err_bad_opt("postfx.scanlines() expects ([surface], [opacity], [step])"));
    }
    let (target, idx) = split_surface_arg(args, 0);
    let opacity = match args.get(idx) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "postfx.scanlines() opacity")?,
        _ => 0.35,
    };
    let step = match args.get(idx + 1) {
        Some(v) if !matches!(v, Value::Unit) => expect_i32(v, "postfx.scanlines() step")?,
        _ => 2,
    };
    gfx::with_target_pixels_mut(target, false, |buf, _d, bw, bh, clip, _b| {
        apply_scanlines(buf, bw, bh, clip, opacity, step);
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn postfx_snapshot(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 2 {
        return Err(err_bad_opt("postfx.snapshot() expects ([surface], [format])"));
    }
    let (target, idx) = split_surface_arg(args, 0);
    if let Some(fv) = args.get(idx) {
        if !matches!(fv, Value::Unit) {
            let fmt = expect_str(fv, "postfx.snapshot() format")?.to_ascii_lowercase();
            if fmt != "bmp" && fmt != "bmp24" {
                return Err(format!("E_MEDIA_UNSUPPORTED: snapshot format '{}' unsupported (expected 'bmp')", fmt));
            }
        }
    }
    let bmp_bytes = gfx::with_target_pixels_read(target, |buf, bw, bh| encode_bmp24(buf, bw, bh))?;
    vm.charge_alloc(bmp_bytes.len())?;
    Ok(Value::Bytes(Rc::new(bmp_bytes)))
}

fn postfx_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("postfx.close() expects (lut)"));
    }
    let id = extract_handle_id(&args[0], "Lut3D", "postfx.close()")?;
    Ok(Value::Bool(LUTS.with(|b| b.borrow_mut().remove(id).is_some())))
}
