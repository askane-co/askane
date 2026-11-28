use std::cell::RefCell;
use std::rc::Rc;

use crate::media::err_bad_opt;
use crate::modules::build_module;
use crate::modules::gfx;
use crate::modules::scene_util::{
    HandleBank, expect_bool, expect_color_u32, expect_f32, expect_usize, extract_handle_id,
    is_surface_arg, make_handle_def, new_handle,
};
use crate::scene::particles::ParticleSystem;
use crate::value::{StructDef, Value};
use crate::vm::VM;

thread_local! {
    static PSYS: RefCell<HandleBank<Rc<RefCell<ParticleSystem>>>> = RefCell::new(HandleBank::new());
    static PS_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
}

fn ps_def() -> Rc<StructDef> {
    PS_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "ParticleSystem",
            &[
                ("seed", particles_seed),
                ("set_gravity", particles_set_gravity),
                ("spawn", particles_spawn),
                ("emit", particles_emit),
                ("update", particles_update),
                ("draw", particles_draw),
                ("count", particles_count),
                ("clear", particles_clear),
                ("close", particles_close),
                ("free", particles_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

pub fn reset_state() {
    PSYS.with(|p| p.borrow_mut().clear());
}

fn extract_ps(val: &Value, ctx: &str) -> Result<Rc<RefCell<ParticleSystem>>, String> {
    let id = extract_handle_id(val, "ParticleSystem", ctx)?;
    PSYS.with(|b| b.borrow().get(id, "ParticleSystem").cloned())
}

pub fn module() -> Value {
    build_module(
        "particles",
        &[
            ("create", particles_create),
            ("seed", particles_seed),
            ("set_gravity", particles_set_gravity),
            ("spawn", particles_spawn),
            ("emit", particles_emit),
            ("update", particles_update),
            ("draw", particles_draw),
            ("count", particles_count),
            ("clear", particles_clear),
            ("close", particles_close),
            ("free", particles_close),
        ],
        &[],
    )
}

fn particles_create(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 2 {
        return Err(err_bad_opt("particles.create() expects ([max_particles], [seed])"));
    }
    let cap = match args.first() {
        Some(v) if !matches!(v, Value::Unit) => expect_usize(v, "particles.create() max")?,
        _ => 1024,
    };
    let seed = match args.get(1) {
        Some(Value::Int(s)) => Some(*s as u64),
        None | Some(Value::Unit) => None,
        Some(other) => {
            return Err(err_bad_opt(format!(
                "particles.create() seed must be int, got '{}'",
                other.type_name()
            )));
        }
    };
    let sys = ParticleSystem::new(cap, seed, Some(vm.budget_core()))?;
    let id = PSYS.with(|b| b.borrow_mut().insert(Rc::new(RefCell::new(sys)), "ParticleSystem"))?;
    Ok(new_handle(&ps_def(), "ParticleSystem", id))
}

fn particles_seed(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(err_bad_opt("particles.seed() expects (system, seed)"));
    }
    let ps = extract_ps(&args[0], "particles.seed()")?;
    let Value::Int(s) = &args[1] else {
        return Err(err_bad_opt("particles.seed() seed must be int"));
    };
    ps.borrow_mut().set_seed(*s as u64);
    Ok(Value::Unit)
}

fn particles_set_gravity(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(err_bad_opt("particles.set_gravity() expects (system, gx, gy)"));
    }
    let ps = extract_ps(&args[0], "particles.set_gravity()")?;
    let gx = expect_f32(&args[1], "gx")?;
    let gy = expect_f32(&args[2], "gy")?;
    ps.borrow_mut().set_gravity(gx, gy)?;
    Ok(Value::Unit)
}

fn particles_spawn(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 8 || args.len() > 9 {
        return Err(err_bad_opt(
            "particles.spawn() expects (system, x, y, vx, vy, life, size, color, [fade])",
        ));
    }
    let ps = extract_ps(&args[0], "particles.spawn()")?;
    let x = expect_f32(&args[1], "x")?;
    let y = expect_f32(&args[2], "y")?;
    let vx = expect_f32(&args[3], "vx")?;
    let vy = expect_f32(&args[4], "vy")?;
    let life = expect_f32(&args[5], "life")?;
    let size = expect_f32(&args[6], "size")?;
    let color = expect_color_u32(&args[7], "color")?;
    let fade = match args.get(8) {
        Some(v) if !matches!(v, Value::Unit) => expect_bool(v, "fade")?,
        _ => true,
    };
    let ok = ps.borrow_mut().spawn(x, y, vx, vy, life, size, color, fade)?;
    Ok(Value::Bool(ok))
}

fn particles_emit(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(err_bad_opt("particles.emit() expects (system, opts_dict)"));
    }
    let ps = extract_ps(&args[0], "particles.emit()")?;
    let Value::Dict(d) = &args[1] else {
        return Err(err_bad_opt("particles.emit() second arg must be opts dict"));
    };
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    let mut count = 16usize;
    let mut speed_min = 20.0f32;
    let mut speed_max = 80.0f32;
    let mut angle = 0.0f32;
    let mut spread = std::f32::consts::TAU;
    let mut life_min = 0.3f32;
    let mut life_max = 1.0f32;
    let mut size = 3.0f32;
    let mut color = 0xFFFF_FFFFu32;
    let mut fade = true;

    for (k, v) in d.borrow().iter() {
        match k.as_str() {
            "x" => x = expect_f32(v, "x")?,
            "y" => y = expect_f32(v, "y")?,
            "count" => count = expect_usize(v, "count")?,
            "speed_min" => speed_min = expect_f32(v, "speed_min")?,
            "speed_max" => speed_max = expect_f32(v, "speed_max")?,
            "angle" => angle = expect_f32(v, "angle")?,
            "spread" => spread = expect_f32(v, "spread")?,
            "life_min" => life_min = expect_f32(v, "life_min")?,
            "life_max" => life_max = expect_f32(v, "life_max")?,
            "size" => size = expect_f32(v, "size")?,
            "color" => color = expect_color_u32(v, "color")?,
            "fade" => fade = expect_bool(v, "fade")?,
            other => return Err(err_bad_opt(format!("particles.emit(): unknown option '{}'", other))),
        }
    }

    let n = ps.borrow_mut().emit_burst(
        x, y, count, speed_min, speed_max, angle, spread, life_min, life_max, size, color, fade,
    )?;
    Ok(Value::Int(n as i64))
}

fn particles_update(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(err_bad_opt("particles.update() expects (system, dt)"));
    }
    let ps = extract_ps(&args[0], "particles.update()")?;
    let dt = expect_f32(&args[1], "dt")?;
    let n = ps.borrow_mut().update(dt)?;
    Ok(Value::Int(n as i64))
}

fn particles_draw(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("particles.draw() expects (system, [surface])"));
    }
    let ps = extract_ps(&args[0], "particles.draw()")?;
    let target = args.get(1).filter(|v| is_surface_arg(v));
    let sys = ps.borrow();
    gfx::with_target_pixels_mut(target, false, |buf, _d, bw, bh, clip, _b| {
        sys.draw(buf, bw, bh, clip);
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn particles_count(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("particles.count() expects (system)"));
    }
    let ps = extract_ps(&args[0], "particles.count()")?;
    Ok(Value::Int(ps.borrow().particles.len() as i64))
}

fn particles_clear(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("particles.clear() expects (system)"));
    }
    let ps = extract_ps(&args[0], "particles.clear()")?;
    ps.borrow_mut().clear();
    Ok(Value::Unit)
}

fn particles_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("particles.close() expects (system)"));
    }
    let id = extract_handle_id(&args[0], "ParticleSystem", "particles.close()")?;
    Ok(Value::Bool(PSYS.with(|b| b.borrow_mut().remove(id).is_some())))
}
