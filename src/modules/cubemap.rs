use std::cell::RefCell;
use std::rc::Rc;

use crate::media::err_bad_opt;
use crate::modules::build_module;
use crate::modules::scene_util::{
    HandleBank, expect_color_u32, expect_f32, expect_str, expect_usize, expect_vec3,
    extract_handle_id, make_handle_def, new_handle,
};
use crate::scene::cubemap::Cubemap;
use crate::scene::geom::Vec3;
use crate::value::{StructDef, Value};
use crate::vm::VM;

thread_local! {
    static CUBEMAPS: RefCell<HandleBank<Rc<Cubemap>>> = RefCell::new(HandleBank::new());
    static CM_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
}

fn cm_def() -> Rc<StructDef> {
    CM_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "Cubemap",
            &[
                ("sample", cubemap_sample),
                ("size", cubemap_size),
                ("close", cubemap_close),
                ("free", cubemap_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

pub fn reset_state() {
    CUBEMAPS.with(|c| c.borrow_mut().clear());
}

pub(crate) fn extract_cubemap(val: &Value, ctx: &str) -> Result<Rc<Cubemap>, String> {
    let id = extract_handle_id(val, "Cubemap", ctx)?;
    CUBEMAPS.with(|b| b.borrow().get(id, "Cubemap").cloned())
}

pub fn module() -> Value {
    build_module(
        "cubemap",
        &[
            ("load", cubemap_load),
            ("from_colors", cubemap_from_colors),
            ("sample", cubemap_sample),
            ("size", cubemap_size),
            ("close", cubemap_close),
            ("free", cubemap_close),
        ],
        &[],
    )
}

fn cubemap_load(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let paths: Vec<String> = if args.len() == 1 {
        let Value::List(l) = &args[0] else {
            return Err("E_CUBEMAP_INVALID: cubemap.load() expects 6 face paths or list of 6 paths".to_string());
        };
        let b = l.borrow();
        if b.len() != 6 {
            return Err("E_CUBEMAP_INVALID: cubemap.load() expects exactly 6 face paths".to_string());
        }
        let mut v = Vec::with_capacity(6);
        for item in b.iter() {
            v.push(expect_str(item, "cubemap.load()")?.to_string());
        }
        v
    } else if args.len() == 6 {
        let mut v = Vec::with_capacity(6);
        for item in args {
            v.push(expect_str(item, "cubemap.load()")?.to_string());
        }
        v
    } else {
        return Err("E_CUBEMAP_INVALID: cubemap.load() expects 6 face paths (+X,-X,+Y,-Y,+Z,-Z)".to_string());
    };
    let refs: [&str; 6] = [
        &paths[0], &paths[1], &paths[2], &paths[3], &paths[4], &paths[5],
    ];
    let cm = Cubemap::load_paths(vm, &refs)?;
    let id = CUBEMAPS.with(|b| b.borrow_mut().insert(Rc::new(cm), "Cubemap"))?;
    Ok(new_handle(&cm_def(), "Cubemap", id))
}

fn cubemap_from_colors(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 6 || args.len() > 7 {
        return Err(err_bad_opt(
            "cubemap.from_colors() expects (px, nx, py, ny, pz, nz, [size])",
        ));
    }
    let size = match args.get(6) {
        Some(v) if !matches!(v, Value::Unit) => expect_usize(v, "cubemap.from_colors() size")? as u32,
        _ => 4,
    };
    let mut faces: Vec<Vec<u32>> = Vec::with_capacity(6);
    for a in &args[0..6] {
        let c = expect_color_u32(a, "cubemap.from_colors() color")?;
        faces.push(vec![c; (size as usize).saturating_mul(size as usize)]);
    }
    let arr: [Vec<u32>; 6] = faces
        .try_into()
        .map_err(|_| "E_CUBEMAP_INVALID: failed to build 6 faces".to_string())?;
    let cm = Cubemap::from_faces(size, arr, Some(vm.budget_core()))?;
    let id = CUBEMAPS.with(|b| b.borrow_mut().insert(Rc::new(cm), "Cubemap"))?;
    Ok(new_handle(&cm_def(), "Cubemap", id))
}

fn cubemap_sample(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 && args.len() != 4 {
        return Err(err_bad_opt("cubemap.sample() expects (cubemap, dir) or (cubemap, x, y, z)"));
    }
    let cm = extract_cubemap(&args[0], "cubemap.sample()")?;
    let dir = if args.len() == 2 {
        expect_vec3(&args[1], "cubemap.sample() dir")?
    } else {
        Vec3::new(
            expect_f32(&args[1], "x")?,
            expect_f32(&args[2], "y")?,
            expect_f32(&args[3], "z")?,
        )
    };
    let rgba = cm.sample_dir(dir);
    Ok(Value::List(Rc::new(RefCell::new(vec![
        Value::Float(rgba[0] as f64),
        Value::Float(rgba[1] as f64),
        Value::Float(rgba[2] as f64),
        Value::Float(rgba[3] as f64),
    ]))))
}

fn cubemap_size(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("cubemap.size() expects (cubemap)"));
    }
    let cm = extract_cubemap(&args[0], "cubemap.size()")?;
    Ok(Value::Int(cm.size as i64))
}

fn cubemap_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("cubemap.close() expects (cubemap)"));
    }
    let id = extract_handle_id(&args[0], "Cubemap", "cubemap.close()")?;
    Ok(Value::Bool(CUBEMAPS.with(|b| b.borrow_mut().remove(id).is_some())))
}
