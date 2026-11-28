use std::cell::RefCell;
use std::rc::Rc;

use crate::media::err_bad_opt;
use crate::modules::build_module;
use crate::modules::cubemap::extract_cubemap;
use crate::modules::gfx;
use crate::modules::lighting::extract_light;
use crate::modules::mesh::extract_model;
use crate::modules::nav::extract_nav;
use crate::modules::scene_util::{
    HandleBank, aabb_to_val, expect_bool, expect_color_u32, expect_f32, expect_rgb_f32, expect_str,
    expect_transform, expect_vec3, extract_handle_id, is_surface_arg, make_handle_def, new_handle,
    vec3_to_val,
};
use crate::modules::terrain::extract_heightmap;
use crate::modules::tiled::extract_map;
use crate::scene::geom::{Transform, Vec3};
use crate::scene::world::{Scene, ScenePlacement};
use crate::value::{StructDef, Value, new_dict_with_capacity};
use crate::vm::VM;

thread_local! {
    static SCENES: RefCell<HandleBank<Rc<RefCell<Scene>>>> = RefCell::new(HandleBank::new());
    static SCENE_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
}

fn scene_def() -> Rc<StructDef> {
    SCENE_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "Scene",
            &[
                ("info", scene_info),
                ("stats", scene_info),
                ("entities", scene_entities),
                ("draw", scene_draw),
                ("close", scene_close),
                ("free", scene_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

pub fn reset_state() {
    SCENES.with(|s| s.borrow_mut().clear());
}

fn extract_scene(val: &Value, ctx: &str) -> Result<Rc<RefCell<Scene>>, String> {
    let id = extract_handle_id(val, "Scene", ctx)?;
    SCENES.with(|b| b.borrow().get(id, "Scene").cloned())
}

pub fn module() -> Value {
    build_module(
        "scene",
        &[
            ("assemble", scene_assemble),
            ("info", scene_info),
            ("stats", scene_info),
            ("entities", scene_entities),
            ("draw", scene_draw),
            ("close", scene_close),
            ("free", scene_close),
        ],
        &[],
    )
}

fn scene_assemble(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("scene.assemble() expects (spec_dict)"));
    }
    let Value::Dict(d) = &args[0] else {
        return Err("E_SCENE_INVALID: scene.assemble() argument must be dict".to_string());
    };
    let map = d.borrow();
    let mut terrain = None;
    let mut tilemap = None;
    let mut nav = None;
    let mut cubemap = None;
    let mut placements = Vec::new();
    let mut lights = Vec::new();
    let mut ambient = [0.25f32, 0.25, 0.25];

    if let Some(v) = map.get("terrain") {
        if !matches!(v, Value::Unit) {
            terrain = Some(extract_heightmap(v, "scene.assemble() terrain")?);
        }
    }
    if let Some(v) = map.get("tilemap") {
        if !matches!(v, Value::Unit) {
            tilemap = Some(extract_map(v, "scene.assemble() tilemap")?);
        }
    }
    if let Some(v) = map.get("nav") {
        if !matches!(v, Value::Unit) {
            nav = Some(extract_nav(v, "scene.assemble() nav")?);
        }
    }
    if let Some(v) = map.get("cubemap") {
        if !matches!(v, Value::Unit) {
            cubemap = Some(extract_cubemap(v, "scene.assemble() cubemap")?);
        }
    }
    if let Some(v) = map.get("ambient") {
        if !matches!(v, Value::Unit) {
            ambient = expect_rgb_f32(v, "scene.assemble() ambient")?;
        }
    }
    if let Some(v) = map.get("lights") {
        if !matches!(v, Value::Unit) {
            let Value::List(l) = v else {
                return Err("E_SCENE_INVALID: 'lights' must be list".to_string());
            };
            for item in l.borrow().iter() {
                lights.push(extract_light(item, "scene.assemble() lights")?);
            }
        }
    }
    if let Some(v) = map.get("placements") {
        if !matches!(v, Value::Unit) {
            let Value::List(l) = v else {
                return Err("E_SCENE_INVALID: 'placements' must be list".to_string());
            };
            for (idx, item) in l.borrow().iter().enumerate() {
                let Value::Dict(pd) = item else {
                    return Err("E_SCENE_INVALID: each placement must be dict".to_string());
                };
                let pm = pd.borrow();
                let model_val = pm
                    .get("model")
                    .ok_or_else(|| "E_SCENE_INVALID: placement missing 'model'".to_string())?;
                let model = extract_model(model_val, "scene.assemble() placement.model")?;
                let name = match pm.get("name") {
                    Some(nv) if !matches!(nv, Value::Unit) => expect_str(nv, "placement.name")?.to_string(),
                    _ => format!("entity_{}", idx),
                };
                let mut tr = match pm.get("transform") {
                    Some(tv) if !matches!(tv, Value::Unit) => expect_transform(tv, "placement.transform")?,
                    _ => Transform::identity(),
                };
                if let Some(pv) = pm.get("position") {
                    if !matches!(pv, Value::Unit) {
                        tr.position = expect_vec3(pv, "placement.position")?;
                    }
                }
                if let Some(sv) = pm.get("scale") {
                    if !matches!(sv, Value::Unit) {
                        tr.scale = expect_vec3(sv, "placement.scale")?;
                    }
                }
                let ground_snap = match pm.get("ground_snap") {
                    Some(gv) if !matches!(gv, Value::Unit) => expect_bool(gv, "placement.ground_snap")?,
                    _ => false,
                };
                let tint_rgb = match pm.get("tint") {
                    Some(cv) if !matches!(cv, Value::Unit) => expect_color_u32(cv, "placement.tint")?,
                    _ => 0xFFD0_B890,
                };
                placements.push(ScenePlacement {
                    name,
                    model,
                    transform: tr,
                    ground_snap,
                    tint_rgb,
                });
            }
        }
    }

    let scene = Scene::assemble(terrain, tilemap, nav, cubemap, placements, lights, ambient)?;
    let id = SCENES.with(|b| b.borrow_mut().insert(Rc::new(RefCell::new(scene)), "Scene"))?;
    Ok(new_handle(&scene_def(), "Scene", id))
}

fn scene_info(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("scene.info() expects (scene)"));
    }
    let sc = extract_scene(&args[0], "scene.info()")?;
    let s = sc.borrow();
    vm.charge_alloc(192)?;
    let mut d = new_dict_with_capacity(8);
    d.insert("entities".to_string(), Value::Int(s.entities.len() as i64));
    d.insert("lights".to_string(), Value::Int(s.lights.len() as i64));
    d.insert("has_terrain".to_string(), Value::Bool(s.terrain.is_some()));
    d.insert("has_tilemap".to_string(), Value::Bool(s.tilemap.is_some()));
    d.insert("has_nav".to_string(), Value::Bool(s.nav.is_some()));
    d.insert("has_cubemap".to_string(), Value::Bool(s.cubemap.is_some()));
    d.insert("ground_bias".to_string(), Value::Float(s.ground_bias as f64));
    d.insert("aabb".to_string(), aabb_to_val(vm, s.world_aabb)?);
    Ok(Value::dict(d))
}

fn scene_entities(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("scene.entities() expects (scene)"));
    }
    let sc = extract_scene(&args[0], "scene.entities()")?;
    let s = sc.borrow();
    vm.charge_alloc(s.entities.len().saturating_mul(160))?;
    let mut out = Vec::with_capacity(s.entities.len());
    for ent in &s.entities {
        let mut d = new_dict_with_capacity(5);
        d.insert("name".to_string(), Value::from_str(&ent.name));
        d.insert("position".to_string(), vec3_to_val(vm, ent.transform.position)?);
        d.insert("scale".to_string(), vec3_to_val(vm, ent.transform.scale)?);
        d.insert(
            "snapped_ground_y".to_string(),
            ent.snapped_ground_y
                .map_or(Value::Unit, |y| Value::Float(y as f64)),
        );
        d.insert("aabb".to_string(), aabb_to_val(vm, ent.world_aabb)?);
        out.push(Value::dict(d));
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn scene_draw(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err(err_bad_opt("scene.draw() expects (scene, [surface], [camera_opts])"));
    }
    let sc = extract_scene(&args[0], "scene.draw()")?;
    let mut target: Option<&Value> = None;
    let mut opts_arg: Option<&Value> = None;
    if let Some(a1) = args.get(1) {
        if is_surface_arg(a1) {
            target = Some(a1);
            opts_arg = args.get(2);
        } else if matches!(a1, Value::Unit) {
            opts_arg = args.get(2);
        } else {
            opts_arg = Some(a1);
        }
    }

    let mut cam_pos = Vec3::new(0.0, 4.0, 12.0);
    let mut yaw = 0.0f32;
    let mut horizon = 64.0f32;
    let mut fov_scale = 0.75f32;
    if let Some(ov) = opts_arg {
        if !matches!(ov, Value::Unit) {
            let Value::Dict(d) = ov else {
                return Err(err_bad_opt("scene.draw() camera_opts must be dict"));
            };
            for (k, v) in d.borrow().iter() {
                match k.as_str() {
                    "position" => cam_pos = expect_vec3(v, "scene.draw() position")?,
                    "cam_x" => cam_pos.x = expect_f32(v, "cam_x")?,
                    "cam_y" => cam_pos.y = expect_f32(v, "cam_y")?,
                    "cam_z" => cam_pos.z = expect_f32(v, "cam_z")?,
                    "yaw" => yaw = expect_f32(v, "yaw")?,
                    "horizon" => horizon = expect_f32(v, "horizon")?,
                    "fov" | "fov_scale" => fov_scale = expect_f32(v, "fov_scale")?,
                    other => return Err(err_bad_opt(format!("scene.draw(): unknown opt '{}'", other))),
                }
            }
        }
    }

    let s = sc.borrow();
    gfx::with_target_pixels_mut(target, true, |buf, depth, bw, bh, clip, _b| {
        s.draw_to_surface(buf, depth, bw, bh, clip, cam_pos, yaw, horizon, fov_scale);
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn scene_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("scene.close() expects (scene)"));
    }
    let id = extract_handle_id(&args[0], "Scene", "scene.close()")?;
    Ok(Value::Bool(SCENES.with(|b| b.borrow_mut().remove(id).is_some())))
}
