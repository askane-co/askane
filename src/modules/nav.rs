use std::cell::RefCell;
use std::rc::Rc;

use crate::media::err_bad_opt;
use crate::modules::build_module;
use crate::modules::scene_util::{
    HandleBank, expect_bool, expect_f32, expect_i32, expect_str, expect_usize, extract_handle_id,
    make_handle_def, new_handle,
};
use crate::modules::terrain::extract_heightmap;
use crate::modules::tiled::{extract_map, extract_solid};
use crate::scene::nav::NavGrid;
use crate::value::{StructDef, Value, new_dict_with_capacity};
use crate::vm::VM;

thread_local! {
    static NAVS: RefCell<HandleBank<Rc<RefCell<NavGrid>>>> = RefCell::new(HandleBank::new());
    static NAV_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
}

fn nav_def() -> Rc<StructDef> {
    NAV_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "NavGrid",
            &[
                ("set_cost", nav_set_cost),
                ("is_walkable", nav_is_walkable),
                ("find_path", nav_find_path),
                ("path_smooth", nav_path_smooth),
                ("flow_field", nav_flow_field),
                ("info", nav_info),
                ("close", nav_close),
                ("free", nav_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

pub fn reset_state() {
    NAVS.with(|n| n.borrow_mut().clear());
}

pub(crate) fn extract_nav(val: &Value, ctx: &str) -> Result<Rc<RefCell<NavGrid>>, String> {
    let id = extract_handle_id(val, "NavGrid", ctx)?;
    NAVS.with(|b| b.borrow().get(id, "NavGrid").cloned())
}

pub fn module() -> Value {
    build_module(
        "nav",
        &[
            ("grid", nav_grid),
            ("from_solid", nav_from_solid),
            ("from_tiled", nav_from_tiled),
            ("from_terrain", nav_from_terrain),
            ("set_cost", nav_set_cost),
            ("is_walkable", nav_is_walkable),
            ("find_path", nav_find_path),
            ("path_smooth", nav_path_smooth),
            ("flow_field", nav_flow_field),
            ("info", nav_info),
            ("close", nav_close),
            ("free", nav_close),
        ],
        &[],
    )
}

fn nav_grid(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err(err_bad_opt("nav.grid() expects (width, height, [default_cost])"));
    }
    let w = expect_usize(&args[0], "nav.grid() width")?;
    let h = expect_usize(&args[1], "nav.grid() height")?;
    let cost = match args.get(2) {
        Some(v) if !matches!(v, Value::Unit) => {
            let c = expect_i32(v, "nav.grid() default_cost")?;
            if c < 0 || c > u16::MAX as i32 {
                return Err("E_NAV_INVALID: nav cost out of u16 range".to_string());
            }
            c as u16
        }
        _ => 1,
    };
    let grid = NavGrid::new(w, h, cost, Some(vm.budget_core()))?;
    let id = NAVS.with(|b| b.borrow_mut().insert(Rc::new(RefCell::new(grid)), "NavGrid"))?;
    Ok(new_handle(&nav_def(), "NavGrid", id))
}

fn nav_from_solid(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("nav.from_solid() expects (solid_grid)"));
    }
    let solid = extract_solid(&args[0], "nav.from_solid()")?;
    let grid = NavGrid::from_solid_grid(&solid, Some(vm.budget_core()))?;
    let id = NAVS.with(|b| b.borrow_mut().insert(Rc::new(RefCell::new(grid)), "NavGrid"))?;
    Ok(new_handle(&nav_def(), "NavGrid", id))
}

fn nav_from_tiled(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("nav.from_tiled() expects (tilemap, [layer_name])"));
    }
    let map_rc = extract_map(&args[0], "nav.from_tiled()")?;
    let layer_name = match args.get(1) {
        Some(v) if !matches!(v, Value::Unit) => Some(expect_str(v, "nav.from_tiled()")?),
        _ => None,
    };
    let solid = map_rc
        .borrow()
        .rasterize_solid(layer_name, Some(vm.budget_core()))?;
    let grid = NavGrid::from_solid_grid(&solid, Some(vm.budget_core()))?;
    let id = NAVS.with(|b| b.borrow_mut().insert(Rc::new(RefCell::new(grid)), "NavGrid"))?;
    Ok(new_handle(&nav_def(), "NavGrid", id))
}

fn nav_from_terrain(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(err_bad_opt("nav.from_terrain() expects (heightmap, threshold)"));
    }
    let hm = extract_heightmap(&args[0], "nav.from_terrain()")?;
    let thresh = expect_f32(&args[1], "nav.from_terrain() threshold")?;
    let solid = hm.to_solid(thresh, Some(vm.budget_core()))?;
    let grid = NavGrid::from_solid_grid(&solid, Some(vm.budget_core()))?;
    let id = NAVS.with(|b| b.borrow_mut().insert(Rc::new(RefCell::new(grid)), "NavGrid"))?;
    Ok(new_handle(&nav_def(), "NavGrid", id))
}

fn nav_info(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("nav.info() expects (nav_grid)"));
    }
    let nav_rc = extract_nav(&args[0], "nav.info()")?;
    let g = nav_rc.borrow();
    vm.charge_alloc(64)?;
    let mut d = new_dict_with_capacity(2);
    d.insert("width".to_string(), Value::Int(g.width as i64));
    d.insert("height".to_string(), Value::Int(g.height as i64));
    Ok(Value::dict(d))
}

fn nav_set_cost(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 4 {
        return Err(err_bad_opt("nav.set_cost() expects (nav, x, y, cost)"));
    }
    let nav_rc = extract_nav(&args[0], "nav.set_cost()")?;
    let x = expect_i32(&args[1], "nav.set_cost() x")?;
    let y = expect_i32(&args[2], "nav.set_cost() y")?;
    let c = expect_i32(&args[3], "nav.set_cost() cost")?;
    if c < 0 || c > u16::MAX as i32 {
        return Err("E_NAV_INVALID: nav cost out of u16 range".to_string());
    }
    nav_rc.borrow_mut().set_cost(x, y, c as u16)?;
    Ok(Value::Unit)
}

fn nav_is_walkable(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(err_bad_opt("nav.is_walkable() expects (nav, x, y)"));
    }
    let nav_rc = extract_nav(&args[0], "nav.is_walkable()")?;
    let x = expect_i32(&args[1], "nav.is_walkable() x")?;
    let y = expect_i32(&args[2], "nav.is_walkable() y")?;
    let res = nav_rc.borrow().is_walkable(x, y);
    Ok(Value::Bool(res))
}

fn nav_find_path(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 5 || args.len() > 6 {
        return Err(err_bad_opt("nav.find_path() expects (nav, sx, sy, gx, gy, [diagonal])"));
    }
    let nav_rc = extract_nav(&args[0], "nav.find_path()")?;
    let sx = expect_i32(&args[1], "nav.find_path() sx")?;
    let sy = expect_i32(&args[2], "nav.find_path() sy")?;
    let gx = expect_i32(&args[3], "nav.find_path() gx")?;
    let gy = expect_i32(&args[4], "nav.find_path() gy")?;
    let diagonal = match args.get(5) {
        Some(v) if !matches!(v, Value::Unit) => expect_bool(v, "nav.find_path() diagonal")?,
        _ => true,
    };
    let path = nav_rc
        .borrow()
        .find_path(sx, sy, gx, gy, diagonal, Some(vm.budget_core()))?;
    vm.charge_alloc(path.len().saturating_mul(48))?;
    let out: Vec<Value> = path
        .into_iter()
        .map(|[x, y]| {
            Value::List(Rc::new(RefCell::new(vec![
                Value::Int(x as i64),
                Value::Int(y as i64),
            ])))
        })
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn nav_path_smooth(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(err_bad_opt("nav.path_smooth() expects (nav, path)"));
    }
    let nav_rc = extract_nav(&args[0], "nav.path_smooth()")?;
    let Value::List(list_rc) = &args[1] else {
        return Err(err_bad_opt("nav.path_smooth() path must be list of [x, y]"));
    };
    let mut pts = Vec::with_capacity(list_rc.borrow().len());
    for item in list_rc.borrow().iter() {
        let Value::List(pair) = item else {
            return Err(err_bad_opt("nav.path_smooth() path point must be [x, y]"));
        };
        let pb = pair.borrow();
        if pb.len() != 2 {
            return Err(err_bad_opt("nav.path_smooth() path point must have 2 elements"));
        }
        pts.push([expect_i32(&pb[0], "x")?, expect_i32(&pb[1], "y")?]);
    }
    let smoothed = nav_rc.borrow().smooth_path(&pts);
    vm.charge_alloc(smoothed.len().saturating_mul(48))?;
    let out: Vec<Value> = smoothed
        .into_iter()
        .map(|[x, y]| {
            Value::List(Rc::new(RefCell::new(vec![
                Value::Int(x as i64),
                Value::Int(y as i64),
            ])))
        })
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn nav_flow_field(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(err_bad_opt("nav.flow_field() expects (nav, gx, gy)"));
    }
    let nav_rc = extract_nav(&args[0], "nav.flow_field()")?;
    let gx = expect_i32(&args[1], "nav.flow_field() gx")?;
    let gy = expect_i32(&args[2], "nav.flow_field() gy")?;
    let g = nav_rc.borrow();
    let dirs = g.flow_field(gx, gy, Some(vm.budget_core()))?;
    vm.charge_alloc(dirs.len().saturating_mul(16).saturating_add(64))?;
    let dir_vals: Vec<Value> = dirs.into_iter().map(|d| Value::Int(d as i64)).collect();
    let mut dict = new_dict_with_capacity(3);
    dict.insert("width".to_string(), Value::Int(g.width as i64));
    dict.insert("height".to_string(), Value::Int(g.height as i64));
    dict.insert("dirs".to_string(), Value::List(Rc::new(RefCell::new(dir_vals))));
    Ok(Value::dict(dict))
}

fn nav_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("nav.close() expects (nav)"));
    }
    let id = extract_handle_id(&args[0], "NavGrid", "nav.close()")?;
    Ok(Value::Bool(NAVS.with(|b| b.borrow_mut().remove(id).is_some())))
}
