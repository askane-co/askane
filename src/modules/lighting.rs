use std::cell::RefCell;
use std::rc::Rc;

use crate::media::err_bad_opt;
use crate::modules::build_module;
use crate::modules::gfx;
use crate::modules::scene_util::{
    HandleBank, expect_bool, expect_f32, expect_rgb_f32, expect_str, expect_usize,
    extract_handle_id, is_surface_arg, make_handle_def, new_handle,
};
use crate::modules::tiled::extract_solid;
use crate::scene::geom::Vec3;
use crate::scene::lighting::{Falloff, Light, LightKind, Lightmap, MAX_LIGHTS};
use crate::scene::tiled::SolidGrid;
use crate::value::{StructDef, Value};
use crate::vm::VM;

thread_local! {
    static LIGHTS: RefCell<HandleBank<Light>> = RefCell::new(HandleBank::new());
    static LIGHTMAPS: RefCell<HandleBank<Rc<RefCell<Lightmap>>>> = RefCell::new(HandleBank::new());
    static LIGHT_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
    static LIGHTMAP_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
}

fn light_def() -> Rc<StructDef> {
    LIGHT_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def("Light", &[("close", lighting_close), ("free", lighting_close)]);
        *opt = Some(Rc::clone(&def));
        def
    })
}

fn lightmap_def() -> Rc<StructDef> {
    LIGHTMAP_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "Lightmap",
            &[
                ("clear", lighting_clear),
                ("apply", lighting_apply),
                ("composite", lighting_composite),
                ("close", lighting_close),
                ("free", lighting_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

pub fn reset_state() {
    LIGHTS.with(|l| l.borrow_mut().clear());
    LIGHTMAPS.with(|l| l.borrow_mut().clear());
}

pub(crate) fn extract_light(val: &Value, ctx: &str) -> Result<Light, String> {
    let id = extract_handle_id(val, "Light", ctx)?;
    LIGHTS.with(|b| b.borrow().get(id, "Light").cloned())
}

pub fn module() -> Value {
    build_module(
        "lighting",
        &[
            ("point", lighting_point),
            ("point3d", lighting_point3d),
            ("directional", lighting_directional),
            ("spot", lighting_spot),
            ("lightmap", lighting_lightmap),
            ("clear", lighting_clear),
            ("apply", lighting_apply),
            ("composite", lighting_composite),
            ("close", lighting_close),
            ("free", lighting_close),
        ],
        &[],
    )
}

fn lighting_point(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 7 {
        return Err(err_bad_opt(
            "lighting.point() expects (x, y, radius, [color], [intensity], [falloff], [cast_shadows])",
        ));
    }
    let x = expect_f32(&args[0], "lighting.point() x")?;
    let y = expect_f32(&args[1], "lighting.point() y")?;
    let radius = expect_f32(&args[2], "lighting.point() radius")?;
    let color = match args.get(3) {
        Some(v) if !matches!(v, Value::Unit) => expect_rgb_f32(v, "lighting.point() color")?,
        _ => [1.0, 1.0, 1.0],
    };
    let intensity = match args.get(4) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "lighting.point() intensity")?,
        _ => 1.0,
    };
    let falloff = match args.get(5) {
        Some(v) if !matches!(v, Value::Unit) => Falloff::from_name(expect_str(v, "falloff")?)?,
        _ => Falloff::Smooth,
    };
    let shadows = match args.get(6) {
        Some(v) if !matches!(v, Value::Unit) => expect_bool(v, "lighting.point() cast_shadows")?,
        _ => true,
    };
    let light = Light::new(
        LightKind::Point2D,
        Vec3::new(x, y, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
        color,
        intensity,
        radius,
        falloff,
        30.0,
        45.0,
        shadows,
    )?;
    let id = LIGHTS.with(|b| b.borrow_mut().insert(light, "Light"))?;
    Ok(new_handle(&light_def(), "Light", id))
}

fn lighting_point3d(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 4 || args.len() > 7 {
        return Err(err_bad_opt(
            "lighting.point3d() expects (x, y, z, radius, [color], [intensity], [falloff])",
        ));
    }
    let x = expect_f32(&args[0], "x")?;
    let y = expect_f32(&args[1], "y")?;
    let z = expect_f32(&args[2], "z")?;
    let radius = expect_f32(&args[3], "radius")?;
    let color = match args.get(4) {
        Some(v) if !matches!(v, Value::Unit) => expect_rgb_f32(v, "color")?,
        _ => [1.0, 1.0, 1.0],
    };
    let intensity = match args.get(5) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "intensity")?,
        _ => 1.0,
    };
    let falloff = match args.get(6) {
        Some(v) if !matches!(v, Value::Unit) => Falloff::from_name(expect_str(v, "falloff")?)?,
        _ => Falloff::Quadratic,
    };
    let light = Light::new(
        LightKind::Point3D,
        Vec3::new(x, y, z),
        Vec3::new(0.0, -1.0, 0.0),
        color,
        intensity,
        radius,
        falloff,
        30.0,
        45.0,
        false,
    )?;
    let id = LIGHTS.with(|b| b.borrow_mut().insert(light, "Light"))?;
    Ok(new_handle(&light_def(), "Light", id))
}

fn lighting_directional(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 5 {
        return Err(err_bad_opt(
            "lighting.directional() expects (dx, dy, dz, [color], [intensity])",
        ));
    }
    let dx = expect_f32(&args[0], "dx")?;
    let dy = expect_f32(&args[1], "dy")?;
    let dz = expect_f32(&args[2], "dz")?;
    let color = match args.get(3) {
        Some(v) if !matches!(v, Value::Unit) => expect_rgb_f32(v, "color")?,
        _ => [1.0, 1.0, 1.0],
    };
    let intensity = match args.get(4) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "intensity")?,
        _ => 1.0,
    };
    let light = Light::new(
        LightKind::Directional,
        Vec3::zero(),
        Vec3::new(dx, dy, dz),
        color,
        intensity,
        1.0,
        Falloff::Linear,
        30.0,
        45.0,
        false,
    )?;
    let id = LIGHTS.with(|b| b.borrow_mut().insert(light, "Light"))?;
    Ok(new_handle(&light_def(), "Light", id))
}

fn lighting_spot(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 7 || args.len() > 10 {
        return Err(err_bad_opt(
            "lighting.spot() expects (x, y, dx, dy, radius, inner_deg, outer_deg, [color], [intensity], [shadows])",
        ));
    }
    let x = expect_f32(&args[0], "x")?;
    let y = expect_f32(&args[1], "y")?;
    let dx = expect_f32(&args[2], "dx")?;
    let dy = expect_f32(&args[3], "dy")?;
    let radius = expect_f32(&args[4], "radius")?;
    let inner = expect_f32(&args[5], "inner_deg")?;
    let outer = expect_f32(&args[6], "outer_deg")?;
    let color = match args.get(7) {
        Some(v) if !matches!(v, Value::Unit) => expect_rgb_f32(v, "color")?,
        _ => [1.0, 1.0, 1.0],
    };
    let intensity = match args.get(8) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "intensity")?,
        _ => 1.0,
    };
    let shadows = match args.get(9) {
        Some(v) if !matches!(v, Value::Unit) => expect_bool(v, "shadows")?,
        _ => true,
    };
    let light = Light::new(
        LightKind::Spot,
        Vec3::new(x, y, 0.0),
        Vec3::new(dx, dy, 0.0),
        color,
        intensity,
        radius,
        Falloff::Smooth,
        inner,
        outer,
        shadows,
    )?;
    let id = LIGHTS.with(|b| b.borrow_mut().insert(light, "Light"))?;
    Ok(new_handle(&light_def(), "Light", id))
}

fn lighting_lightmap(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 4 {
        return Err(err_bad_opt(
            "lighting.lightmap() expects (canvas_w, canvas_h, [scale], [ambient])",
        ));
    }
    let w = expect_usize(&args[0], "lighting.lightmap() canvas_w")?;
    let h = expect_usize(&args[1], "lighting.lightmap() canvas_h")?;
    let scale = match args.get(2) {
        Some(v) if !matches!(v, Value::Unit) => expect_usize(v, "lighting.lightmap() scale")?,
        _ => 2,
    };
    let ambient = match args.get(3) {
        Some(v) if !matches!(v, Value::Unit) => expect_rgb_f32(v, "lighting.lightmap() ambient")?,
        _ => [0.1, 0.1, 0.1],
    };
    let lm = Lightmap::new(w, h, scale, ambient, Some(vm.budget_core()))?;
    let id = LIGHTMAPS.with(|b| b.borrow_mut().insert(Rc::new(RefCell::new(lm)), "Lightmap"))?;
    Ok(new_handle(&lightmap_def(), "Lightmap", id))
}

fn lighting_clear(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("lighting.clear() expects (lightmap, [ambient])"));
    }
    let id = extract_handle_id(&args[0], "Lightmap", "lighting.clear()")?;
    let lm_rc = LIGHTMAPS.with(|b| b.borrow().get(id, "Lightmap").cloned())?;
    let ambient = match args.get(1) {
        Some(v) if !matches!(v, Value::Unit) => Some(expect_rgb_f32(v, "ambient")?),
        _ => None,
    };
    lm_rc.borrow_mut().clear(ambient)?;
    Ok(Value::Unit)
}

fn lighting_apply(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err(err_bad_opt("lighting.apply() expects (lightmap, lights_list, [opts])"));
    }
    let id = extract_handle_id(&args[0], "Lightmap", "lighting.apply()")?;
    let lm_rc = LIGHTMAPS.with(|b| b.borrow().get(id, "Lightmap").cloned())?;

    let Value::List(list_rc) = &args[1] else {
        return Err(err_bad_opt("lighting.apply() lights_list must be a list of Light"));
    };
    let list_b = list_rc.borrow();
    if list_b.len() > MAX_LIGHTS {
        return Err(format!("E_MEDIA_LIMIT: lighting.apply() lights exceed {}", MAX_LIGHTS));
    }
    let mut lights = Vec::with_capacity(list_b.len());
    for item in list_b.iter() {
        lights.push(extract_light(item, "lighting.apply()")?);
    }

    let mut solid_rc: Option<Rc<SolidGrid>> = None;
    let mut origin_x = 0.0f32;
    let mut origin_y = 0.0f32;
    let mut cam_x = 0.0f32;
    let mut cam_y = 0.0f32;
    if let Some(arg2) = args.get(2) {
        if !matches!(arg2, Value::Unit) {
            if let Ok(sg) = extract_solid(arg2, "lighting.apply()") {
                solid_rc = Some(sg);
            } else if let Value::Dict(d) = arg2 {
                for (k, v) in d.borrow().iter() {
                    match k.as_str() {
                        "solid" => solid_rc = Some(extract_solid(v, "lighting.apply() solid")?),
                        "origin_x" => origin_x = expect_f32(v, "origin_x")?,
                        "origin_y" => origin_y = expect_f32(v, "origin_y")?,
                        "cam_x" => cam_x = expect_f32(v, "cam_x")?,
                        "cam_y" => cam_y = expect_f32(v, "cam_y")?,
                        other => return Err(err_bad_opt(format!("lighting.apply(): unknown opt '{}'", other))),
                    }
                }
            } else {
                return Err(err_bad_opt("lighting.apply() third arg must be SolidGrid or opts dict"));
            }
        }
    }

    let shadows = solid_rc
        .as_deref()
        .map(|s| (s, origin_x, origin_y));
    lm_rc.borrow_mut().apply_lights(&lights, shadows, cam_x, cam_y)?;
    Ok(Value::Unit)
}

fn lighting_composite(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err(err_bad_opt("lighting.composite() expects (lightmap, [surface], [exposure])"));
    }
    let id = extract_handle_id(&args[0], "Lightmap", "lighting.composite()")?;
    let lm_rc = LIGHTMAPS.with(|b| b.borrow().get(id, "Lightmap").cloned())?;
    let mut target: Option<&Value> = None;
    let mut exp_idx = 1usize;
    if let Some(v) = args.get(1) {
        if is_surface_arg(v) {
            target = Some(v);
            exp_idx = 2;
        } else if matches!(v, Value::Unit) && args.len() > 2 {
            exp_idx = 2;
        }
    }
    let exposure = match args.get(exp_idx) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "lighting.composite() exposure")?,
        _ => 1.0,
    };
    let lm = lm_rc.borrow();
    gfx::with_target_pixels_mut(target, false, |buf, _d, bw, bh, clip, _b| {
        lm.composite_surface(buf, bw, bh, clip, exposure);
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn lighting_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("lighting.close() expects (handle)"));
    }
    if let Ok(id) = extract_handle_id(&args[0], "Light", "lighting.close()") {
        return Ok(Value::Bool(LIGHTS.with(|b| b.borrow_mut().remove(id).is_some())));
    }
    if let Ok(id) = extract_handle_id(&args[0], "Lightmap", "lighting.close()") {
        return Ok(Value::Bool(LIGHTMAPS.with(|b| b.borrow_mut().remove(id).is_some())));
    }
    Err(err_bad_opt("lighting.close() expects Light or Lightmap handle"))
}
