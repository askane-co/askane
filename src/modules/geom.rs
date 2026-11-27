use std::rc::Rc;

use crate::modules::build_module;
use crate::scene::geom::{Aabb, Frustum, Mat4, Plane, Quat, Ray, Transform, Vec2, Vec3, Vec4};
use crate::scene::units::{deg_to_rad, ensure_finite_f32, rad_to_deg};
use crate::value::{new_dict_with_capacity, Value};
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "geom",
        &[
            ("vec2", geom_vec2),
            ("vec3", geom_vec3),
            ("vec4", geom_vec4),
            ("quat", geom_quat),
            ("quat_identity", geom_quat_identity),
            ("quat_axis_angle", geom_quat_axis_angle),
            ("quat_euler", geom_quat_euler),
            ("mat4", geom_mat4),
            ("mat4_identity", geom_mat4_identity),
            ("mat4_translate", geom_mat4_translate),
            ("mat4_scale", geom_mat4_scale),
            ("mat4_rotate_x", geom_mat4_rotate_x),
            ("mat4_rotate_y", geom_mat4_rotate_y),
            ("mat4_rotate_z", geom_mat4_rotate_z),
            ("mat4_from_quat", geom_mat4_from_quat),
            ("mat4_trs", geom_mat4_trs),
            ("mat4_look_at", geom_mat4_look_at),
            ("mat4_perspective", geom_mat4_perspective),
            ("mat4_ortho", geom_mat4_ortho),
            ("mat4_mul", geom_mat4_mul),
            ("mat4_vec4", geom_mat4_vec4),
            ("mat4_point", geom_mat4_point),
            ("mat4_dir", geom_mat4_dir),
            ("mat4_transpose", geom_mat4_transpose),
            ("mat4_det", geom_mat4_det),
            ("mat4_inverse", geom_mat4_inverse),
            ("x", geom_x),
            ("y", geom_y),
            ("z", geom_z),
            ("w", geom_w),
            ("to_list", geom_to_list),
            ("rot90", geom_rot90),
            ("rotate_vec2", geom_rotate_vec2),
            ("rotate_vec3", geom_rotate_vec3),
            ("deg_to_rad", geom_deg_to_rad),
            ("rad_to_deg", geom_rad_to_deg),
            ("dot", geom_dot),
            ("cross", geom_cross),
            ("len", geom_len),
            ("len_sq", geom_len_sq),
            ("dist", geom_dist),
            ("normalize", geom_normalize),
            ("lerp", geom_lerp),
            ("slerp", geom_slerp),
            ("conjugate", geom_conjugate),
            ("reflect", geom_reflect),
            ("project", geom_project),
            ("transform", geom_transform),
            ("transform_to_mat4", geom_transform_to_mat4),
            ("transform_point", geom_transform_point),
            ("transform_combine", geom_transform_combine),
            ("aabb", geom_aabb),
            ("aabb_contains", geom_aabb_contains),
            ("aabb_intersects", geom_aabb_intersects),
            ("aabb_center", geom_aabb_center),
            ("aabb_size", geom_aabb_size),
            ("aabb_expand", geom_aabb_expand),
            ("aabb_transform", geom_aabb_transform),
            ("ray", geom_ray),
            ("ray_at", geom_ray_at),
            ("ray_aabb", geom_ray_aabb),
            ("ray_plane", geom_ray_plane),
            ("plane", geom_plane),
            ("plane_from_point", geom_plane_from_point),
            ("plane_distance", geom_plane_distance),
            ("frustum_from_mat4", geom_frustum_from_mat4),
            ("frustum_contains_point", geom_frustum_contains_point),
            ("frustum_intersects_aabb", geom_frustum_intersects_aabb),
        ],
        &[],
    )
}

fn as_f32(v: &Value, ctx: &str) -> Result<f32, String> {
    match v {
        Value::Int(i) => ensure_finite_f32(*i as f32, ctx),
        Value::Float(f) => ensure_finite_f32(*f as f32, ctx),
        other => Err(format!(
            "{} expects a number, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn as_f64(v: &Value, ctx: &str) -> Result<f64, String> {
    match v {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) => crate::scene::units::ensure_finite_f64(*f, ctx),
        other => Err(format!(
            "{} expects a number, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn as_i32(v: &Value, ctx: &str) -> Result<i32, String> {
    match v {
        Value::Int(i) => i32::try_from(*i).map_err(|_| format!("{} out of i32 range", ctx)),
        other => Err(format!(
            "{} expects an integer, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn as_vec2(v: &Value, ctx: &str) -> Result<Vec2, String> {
    match v {
        Value::Vec2(x, y) => Ok(Vec2::new(*x, *y)),
        other => Err(format!(
            "{} expects vec2, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn as_vec3(v: &Value, ctx: &str) -> Result<Vec3, String> {
    match v {
        Value::Vec3(a) => Ok(Vec3::from_array(**a)),
        other => Err(format!(
            "{} expects vec3, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn as_vec4(v: &Value, ctx: &str) -> Result<Vec4, String> {
    match v {
        Value::Vec4(a) => Ok(Vec4::from_array(**a)),
        other => Err(format!(
            "{} expects vec4, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn as_quat(v: &Value, ctx: &str) -> Result<Quat, String> {
    match v {
        Value::Quat(a) => Ok(Quat::from_array(**a)),
        other => Err(format!(
            "{} expects quat, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn as_mat4(v: &Value, ctx: &str) -> Result<Mat4, String> {
    match v {
        Value::Mat4(a) => Ok(Mat4::from_array(**a)),
        other => Err(format!(
            "{} expects mat4, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

#[inline]
fn val_vec3(vm: &mut VM, v: Vec3) -> Result<Value, String> {
    vm.charge_alloc(16)?;
    Ok(Value::Vec3(Rc::new(v.to_array())))
}

#[inline]
fn val_vec4(vm: &mut VM, v: Vec4) -> Result<Value, String> {
    vm.charge_alloc(24)?;
    Ok(Value::Vec4(Rc::new(v.to_array())))
}

#[inline]
fn val_quat(vm: &mut VM, q: Quat) -> Result<Value, String> {
    vm.charge_alloc(24)?;
    Ok(Value::Quat(Rc::new(q.to_array())))
}

#[inline]
fn val_mat4(vm: &mut VM, m: Mat4) -> Result<Value, String> {
    vm.charge_alloc(64)?;
    Ok(Value::Mat4(Rc::new(m.to_array())))
}

fn dict_get<'a>(map: &'a crate::value::DictMap, key: &str, ctx: &str) -> Result<&'a Value, String> {
    map.get(key)
        .ok_or_else(|| format!("{} missing field '{}'", ctx, key))
}

fn as_transform(v: &Value, ctx: &str) -> Result<Transform, String> {
    let Value::Dict(d) = v else {
        return Err(format!("{} expects transform dict, got '{}'", ctx, v.type_name()));
    };
    let m = d.borrow();
    Ok(Transform {
        position: as_vec3(dict_get(&m, "position", ctx)?, ctx)?,
        rotation: as_quat(dict_get(&m, "rotation", ctx)?, ctx)?,
        scale: as_vec3(dict_get(&m, "scale", ctx)?, ctx)?,
    })
}

fn transform_to_val(vm: &mut VM, tr: Transform) -> Result<Value, String> {
    vm.charge_alloc(128)?;
    let mut map = new_dict_with_capacity(3);
    map.insert("position".to_string(), val_vec3(vm, tr.position)?);
    map.insert("rotation".to_string(), val_quat(vm, tr.rotation)?);
    map.insert("scale".to_string(), val_vec3(vm, tr.scale)?);
    Ok(Value::dict(map))
}

fn as_aabb(v: &Value, ctx: &str) -> Result<Aabb, String> {
    let Value::Dict(d) = v else {
        return Err(format!("{} expects aabb dict, got '{}'", ctx, v.type_name()));
    };
    let m = d.borrow();
    Ok(Aabb {
        min: as_vec3(dict_get(&m, "min", ctx)?, ctx)?,
        max: as_vec3(dict_get(&m, "max", ctx)?, ctx)?,
    })
}

fn aabb_to_val(vm: &mut VM, b: Aabb) -> Result<Value, String> {
    vm.charge_alloc(96)?;
    let mut map = new_dict_with_capacity(2);
    map.insert("min".to_string(), val_vec3(vm, b.min)?);
    map.insert("max".to_string(), val_vec3(vm, b.max)?);
    Ok(Value::dict(map))
}

fn as_ray(v: &Value, ctx: &str) -> Result<Ray, String> {
    let Value::Dict(d) = v else {
        return Err(format!("{} expects ray dict, got '{}'", ctx, v.type_name()));
    };
    let m = d.borrow();
    Ok(Ray {
        origin: as_vec3(dict_get(&m, "origin", ctx)?, ctx)?,
        dir: as_vec3(dict_get(&m, "dir", ctx)?, ctx)?,
    })
}

fn as_plane(v: &Value, ctx: &str) -> Result<Plane, String> {
    let Value::Dict(d) = v else {
        return Err(format!("{} expects plane dict, got '{}'", ctx, v.type_name()));
    };
    let m = d.borrow();
    Ok(Plane {
        normal: as_vec3(dict_get(&m, "normal", ctx)?, ctx)?,
        d: as_f32(dict_get(&m, "d", ctx)?, ctx)?,
    })
}

fn plane_to_val(vm: &mut VM, p: Plane) -> Result<Value, String> {
    vm.charge_alloc(64)?;
    let mut map = new_dict_with_capacity(2);
    map.insert("normal".to_string(), val_vec3(vm, p.normal)?);
    map.insert("d".to_string(), Value::Float(p.d as f64));
    Ok(Value::dict(map))
}

fn as_frustum(v: &Value, ctx: &str) -> Result<Frustum, String> {
    let Value::Dict(d) = v else {
        return Err(format!("{} expects frustum dict, got '{}'", ctx, v.type_name()));
    };
    let m = d.borrow();
    let Value::List(list) = dict_get(&m, "planes", ctx)? else {
        return Err(format!("{} frustum.planes must be list", ctx));
    };
    let items = list.borrow();
    if items.len() != 6 {
        return Err(format!("{} frustum requires 6 planes", ctx));
    }
    let mut planes = [Plane {
        normal: Vec3::unit_y(),
        d: 0.0,
    }; 6];
    for (i, item) in items.iter().enumerate() {
        planes[i] = as_plane(item, ctx)?;
    }
    Ok(Frustum { planes })
}

fn geom_vec2(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.vec2() expects 2 arguments (x, y)".to_string());
    }
    Ok(Value::Vec2(
        as_f32(&args[0], "geom.vec2 x")?,
        as_f32(&args[1], "geom.vec2 y")?,
    ))
}

fn geom_vec3(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("geom.vec3() expects 3 arguments (x, y, z)".to_string());
    }
    val_vec3(
        vm,
        Vec3::new(
            as_f32(&args[0], "geom.vec3 x")?,
            as_f32(&args[1], "geom.vec3 y")?,
            as_f32(&args[2], "geom.vec3 z")?,
        ),
    )
}

fn geom_vec4(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 4 {
        return Err("geom.vec4() expects 4 arguments (x, y, z, w)".to_string());
    }
    val_vec4(
        vm,
        Vec4::new(
            as_f32(&args[0], "geom.vec4 x")?,
            as_f32(&args[1], "geom.vec4 y")?,
            as_f32(&args[2], "geom.vec4 z")?,
            as_f32(&args[3], "geom.vec4 w")?,
        ),
    )
}

fn geom_quat(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 4 {
        return Err("geom.quat() expects 4 arguments (x, y, z, w)".to_string());
    }
    val_quat(
        vm,
        Quat::new(
            as_f32(&args[0], "geom.quat x")?,
            as_f32(&args[1], "geom.quat y")?,
            as_f32(&args[2], "geom.quat z")?,
            as_f32(&args[3], "geom.quat w")?,
        ),
    )
}

fn geom_quat_identity(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("geom.quat_identity() expects 0 arguments".to_string());
    }
    val_quat(vm, Quat::identity())
}

fn geom_quat_axis_angle(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.quat_axis_angle() expects (axis_vec3, radians)".to_string());
    }
    let axis = as_vec3(&args[0], "geom.quat_axis_angle")?;
    let rad = as_f32(&args[1], "geom.quat_axis_angle radians")?;
    val_quat(vm, Quat::from_axis_angle(axis, rad))
}

fn geom_quat_euler(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("geom.quat_euler() expects (yaw, pitch, roll)".to_string());
    }
    val_quat(
        vm,
        Quat::from_euler_yxz(
            as_f32(&args[0], "geom.quat_euler yaw")?,
            as_f32(&args[1], "geom.quat_euler pitch")?,
            as_f32(&args[2], "geom.quat_euler roll")?,
        ),
    )
}

fn geom_mat4(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.mat4() expects 1 list of 16 numbers".to_string());
    }
    let Value::List(list) = &args[0] else {
        return Err("geom.mat4() expects list of 16 numbers".to_string());
    };
    let items = list.borrow();
    if items.len() != 16 {
        return Err("geom.mat4() expects exactly 16 elements".to_string());
    }
    let mut arr = [0.0f32; 16];
    for (i, item) in items.iter().enumerate() {
        arr[i] = as_f32(item, "geom.mat4 element")?;
    }
    val_mat4(vm, Mat4::from_array(arr))
}

fn geom_mat4_identity(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("geom.mat4_identity() expects 0 arguments".to_string());
    }
    val_mat4(vm, Mat4::identity())
}

fn geom_mat4_translate(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.mat4_translate() expects (vec3)".to_string());
    }
    val_mat4(vm, Mat4::translate(as_vec3(&args[0], "geom.mat4_translate")?))
}

fn geom_mat4_scale(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.mat4_scale() expects (vec3)".to_string());
    }
    val_mat4(vm, Mat4::scale(as_vec3(&args[0], "geom.mat4_scale")?))
}

fn geom_mat4_rotate_x(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.mat4_rotate_x() expects (radians)".to_string());
    }
    val_mat4(vm, Mat4::rotate_x(as_f32(&args[0], "geom.mat4_rotate_x")?))
}

fn geom_mat4_rotate_y(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.mat4_rotate_y() expects (radians)".to_string());
    }
    val_mat4(vm, Mat4::rotate_y(as_f32(&args[0], "geom.mat4_rotate_y")?))
}

fn geom_mat4_rotate_z(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.mat4_rotate_z() expects (radians)".to_string());
    }
    val_mat4(vm, Mat4::rotate_z(as_f32(&args[0], "geom.mat4_rotate_z")?))
}

fn geom_mat4_from_quat(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.mat4_from_quat() expects (quat)".to_string());
    }
    val_mat4(vm, Mat4::from_quat(as_quat(&args[0], "geom.mat4_from_quat")?))
}

fn geom_mat4_trs(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("geom.mat4_trs() expects (pos_vec3, rot_quat, scale_vec3)".to_string());
    }
    val_mat4(
        vm,
        Mat4::from_trs(
            as_vec3(&args[0], "geom.mat4_trs pos")?,
            as_quat(&args[1], "geom.mat4_trs rot")?,
            as_vec3(&args[2], "geom.mat4_trs scale")?,
        ),
    )
}

fn geom_mat4_look_at(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("geom.mat4_look_at() expects (eye, target, up)".to_string());
    }
    val_mat4(
        vm,
        Mat4::look_at_rh(
            as_vec3(&args[0], "geom.mat4_look_at eye")?,
            as_vec3(&args[1], "geom.mat4_look_at target")?,
            as_vec3(&args[2], "geom.mat4_look_at up")?,
        ),
    )
}

fn geom_mat4_perspective(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 4 {
        return Err("geom.mat4_perspective() expects (fov_y_rad, aspect, near, far)".to_string());
    }
    let m = Mat4::perspective_rh_zo(
        as_f32(&args[0], "geom.mat4_perspective fov_y")?,
        as_f32(&args[1], "geom.mat4_perspective aspect")?,
        as_f32(&args[2], "geom.mat4_perspective near")?,
        as_f32(&args[3], "geom.mat4_perspective far")?,
    )?;
    val_mat4(vm, m)
}

fn geom_mat4_ortho(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 6 {
        return Err("geom.mat4_ortho() expects (left, right, bottom, top, near, far)".to_string());
    }
    let m = Mat4::orthographic_rh_zo(
        as_f32(&args[0], "geom.mat4_ortho left")?,
        as_f32(&args[1], "geom.mat4_ortho right")?,
        as_f32(&args[2], "geom.mat4_ortho bottom")?,
        as_f32(&args[3], "geom.mat4_ortho top")?,
        as_f32(&args[4], "geom.mat4_ortho near")?,
        as_f32(&args[5], "geom.mat4_ortho far")?,
    )?;
    val_mat4(vm, m)
}

fn geom_mat4_mul(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.mat4_mul() expects (mat4, mat4)".to_string());
    }
    let a = as_mat4(&args[0], "geom.mat4_mul")?;
    let b = as_mat4(&args[1], "geom.mat4_mul")?;
    val_mat4(vm, a.mul_mat4(&b))
}

fn geom_mat4_vec4(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.mat4_vec4() expects (mat4, vec4)".to_string());
    }
    let m = as_mat4(&args[0], "geom.mat4_vec4")?;
    let v = as_vec4(&args[1], "geom.mat4_vec4")?;
    val_vec4(vm, m.mul_vec4(v))
}

fn geom_mat4_point(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.mat4_point() expects (mat4, vec3)".to_string());
    }
    let m = as_mat4(&args[0], "geom.mat4_point")?;
    let p = as_vec3(&args[1], "geom.mat4_point")?;
    val_vec3(vm, m.transform_point3(p))
}

fn geom_mat4_dir(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.mat4_dir() expects (mat4, vec3)".to_string());
    }
    let m = as_mat4(&args[0], "geom.mat4_dir")?;
    let d = as_vec3(&args[1], "geom.mat4_dir")?;
    val_vec3(vm, m.transform_vector3(d))
}

fn geom_mat4_transpose(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.mat4_transpose() expects (mat4)".to_string());
    }
    val_mat4(vm, as_mat4(&args[0], "geom.mat4_transpose")?.transpose())
}

fn geom_mat4_det(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.mat4_det() expects (mat4)".to_string());
    }
    Ok(Value::Float(as_mat4(&args[0], "geom.mat4_det")?.determinant() as f64))
}

fn geom_mat4_inverse(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.mat4_inverse() expects (mat4)".to_string());
    }
    match as_mat4(&args[0], "geom.mat4_inverse")?.inverse() {
        Some(inv) => val_mat4(vm, inv),
        None => Ok(Value::Unit),
    }
}

fn geom_x(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.x() expects 1 vector/quat".to_string());
    }
    let x = match &args[0] {
        Value::Vec2(x, _) => *x,
        Value::Vec3(a) => a[0],
        Value::Vec4(a) | Value::Quat(a) => a[0],
        other => return Err(format!("geom.x() not supported on '{}'", other.type_name())),
    };
    Ok(Value::Float(x as f64))
}

fn geom_y(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.y() expects 1 vector/quat".to_string());
    }
    let y = match &args[0] {
        Value::Vec2(_, y) => *y,
        Value::Vec3(a) => a[1],
        Value::Vec4(a) | Value::Quat(a) => a[1],
        other => return Err(format!("geom.y() not supported on '{}'", other.type_name())),
    };
    Ok(Value::Float(y as f64))
}

fn geom_z(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.z() expects 1 vec3/vec4/quat".to_string());
    }
    let z = match &args[0] {
        Value::Vec3(a) => a[2],
        Value::Vec4(a) | Value::Quat(a) => a[2],
        other => return Err(format!("geom.z() not supported on '{}'", other.type_name())),
    };
    Ok(Value::Float(z as f64))
}

fn geom_w(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.w() expects 1 vec4/quat".to_string());
    }
    let w = match &args[0] {
        Value::Vec4(a) | Value::Quat(a) => a[3],
        other => return Err(format!("geom.w() not supported on '{}'", other.type_name())),
    };
    Ok(Value::Float(w as f64))
}

fn geom_to_list(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.to_list() expects 1 geom value".to_string());
    }
    let floats: Vec<Value> = match &args[0] {
        Value::Vec2(x, y) => vec![Value::Float(*x as f64), Value::Float(*y as f64)],
        Value::Vec3(a) => a.iter().map(|&f| Value::Float(f as f64)).collect(),
        Value::Vec4(a) | Value::Quat(a) => a.iter().map(|&f| Value::Float(f as f64)).collect(),
        Value::Mat4(a) => a.iter().map(|&f| Value::Float(f as f64)).collect(),
        other => {
            return Err(format!(
                "geom.to_list() not supported on '{}'",
                other.type_name()
            ));
        }
    };
    vm.charge_alloc(floats.len() * std::mem::size_of::<Value>())?;
    Ok(Value::List(Rc::new(std::cell::RefCell::new(floats))))
}

fn geom_rot90(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.rot90() expects (vec2, quarter_turns)".to_string());
    }
    let v = as_vec2(&args[0], "geom.rot90")?;
    let steps = as_i32(&args[1], "geom.rot90 steps")?;
    let r = v.rot90(steps);
    Ok(Value::Vec2(r.x, r.y))
}

fn geom_rotate_vec2(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.rotate_vec2() expects (vec2, radians)".to_string());
    }
    let v = as_vec2(&args[0], "geom.rotate_vec2")?;
    let rad = as_f32(&args[1], "geom.rotate_vec2 radians")?;
    let r = v.rotate(rad);
    Ok(Value::Vec2(r.x, r.y))
}

fn geom_rotate_vec3(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.rotate_vec3() expects (quat, vec3)".to_string());
    }
    let q = as_quat(&args[0], "geom.rotate_vec3")?;
    let v = as_vec3(&args[1], "geom.rotate_vec3")?;
    val_vec3(vm, q.rotate_vec3(v))
}

fn geom_deg_to_rad(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.deg_to_rad() expects (degrees)".to_string());
    }
    Ok(Value::Float(deg_to_rad(as_f64(&args[0], "geom.deg_to_rad")?)))
}

fn geom_rad_to_deg(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.rad_to_deg() expects (radians)".to_string());
    }
    Ok(Value::Float(rad_to_deg(as_f64(&args[0], "geom.rad_to_deg")?)))
}

fn geom_dot(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.dot() expects 2 values".to_string());
    }
    let d = match (&args[0], &args[1]) {
        (Value::Vec2(x1, y1), Value::Vec2(x2, y2)) => {
            Vec2::new(*x1, *y1).dot(Vec2::new(*x2, *y2))
        }
        (Value::Vec3(a), Value::Vec3(b)) => Vec3::from_array(**a).dot(Vec3::from_array(**b)),
        (Value::Vec4(a), Value::Vec4(b)) => Vec4::from_array(**a).dot(Vec4::from_array(**b)),
        (Value::Quat(a), Value::Quat(b)) => Quat::from_array(**a).dot(Quat::from_array(**b)),
        (l, r) => {
            return Err(format!(
                "geom.dot() type mismatch '{}' and '{}'",
                l.type_name(),
                r.type_name()
            ));
        }
    };
    Ok(Value::Float(d as f64))
}

fn geom_cross(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.cross() expects 2 vectors".to_string());
    }
    match (&args[0], &args[1]) {
        (Value::Vec2(x1, y1), Value::Vec2(x2, y2)) => {
            Ok(Value::Float(Vec2::new(*x1, *y1).cross(Vec2::new(*x2, *y2)) as f64))
        }
        (Value::Vec3(a), Value::Vec3(b)) => {
            val_vec3(vm, Vec3::from_array(**a).cross(Vec3::from_array(**b)))
        }
        (l, r) => Err(format!(
            "geom.cross() type mismatch '{}' and '{}'",
            l.type_name(),
            r.type_name()
        )),
    }
}

fn geom_len_sq(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.len_sq() expects 1 vector/quat".to_string());
    }
    let sq = match &args[0] {
        Value::Vec2(x, y) => Vec2::new(*x, *y).len_sq(),
        Value::Vec3(a) => Vec3::from_array(**a).len_sq(),
        Value::Vec4(a) => Vec4::from_array(**a).len_sq(),
        Value::Quat(a) => {
            let q = Quat::from_array(**a);
            q.dot(q)
        }
        other => return Err(format!("geom.len_sq() not supported on '{}'", other.type_name())),
    };
    Ok(Value::Float(sq as f64))
}

fn geom_len(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.len() expects 1 vector/quat".to_string());
    }
    let Value::Float(sq) = geom_len_sq(_vm, args)? else {
        unreachable!();
    };
    Ok(Value::Float(sq.sqrt()))
}

fn geom_dist(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.dist() expects 2 vectors".to_string());
    }
    let d = match (&args[0], &args[1]) {
        (Value::Vec2(x1, y1), Value::Vec2(x2, y2)) => {
            Vec2::new(*x1, *y1).dist(Vec2::new(*x2, *y2))
        }
        (Value::Vec3(a), Value::Vec3(b)) => Vec3::from_array(**a).dist(Vec3::from_array(**b)),
        (Value::Vec4(a), Value::Vec4(b)) => {
            Vec4::from_array(**a).sub(Vec4::from_array(**b)).len()
        }
        (l, r) => {
            return Err(format!(
                "geom.dist() type mismatch '{}' and '{}'",
                l.type_name(),
                r.type_name()
            ));
        }
    };
    Ok(Value::Float(d as f64))
}

fn geom_normalize(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.normalize() expects 1 vector/quat".to_string());
    }
    match &args[0] {
        Value::Vec2(x, y) => {
            let n = Vec2::new(*x, *y).normalize();
            Ok(Value::Vec2(n.x, n.y))
        }
        Value::Vec3(a) => val_vec3(vm, Vec3::from_array(**a).normalize()),
        Value::Vec4(a) => val_vec4(vm, Vec4::from_array(**a).normalize()),
        Value::Quat(a) => val_quat(vm, Quat::from_array(**a).normalize()),
        other => Err(format!(
            "geom.normalize() not supported on '{}'",
            other.type_name()
        )),
    }
}

fn geom_lerp(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("geom.lerp() expects (a, b, t)".to_string());
    }
    let t = as_f32(&args[2], "geom.lerp t")?;
    match (&args[0], &args[1]) {
        (Value::Vec2(x1, y1), Value::Vec2(x2, y2)) => {
            let r = Vec2::new(*x1, *y1).lerp(Vec2::new(*x2, *y2), t);
            Ok(Value::Vec2(r.x, r.y))
        }
        (Value::Vec3(a), Value::Vec3(b)) => {
            val_vec3(vm, Vec3::from_array(**a).lerp(Vec3::from_array(**b), t))
        }
        (Value::Vec4(a), Value::Vec4(b)) => {
            val_vec4(vm, Vec4::from_array(**a).lerp(Vec4::from_array(**b), t))
        }
        (l, r) => Err(format!(
            "geom.lerp() type mismatch '{}' and '{}'",
            l.type_name(),
            r.type_name()
        )),
    }
}

fn geom_slerp(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("geom.slerp() expects (q1, q2, t)".to_string());
    }
    let a = as_quat(&args[0], "geom.slerp")?;
    let b = as_quat(&args[1], "geom.slerp")?;
    let t = as_f32(&args[2], "geom.slerp t")?;
    val_quat(vm, a.slerp(b, t))
}

fn geom_conjugate(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.conjugate() expects (quat)".to_string());
    }
    val_quat(vm, as_quat(&args[0], "geom.conjugate")?.conjugate())
}

fn geom_reflect(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.reflect() expects (v, normal)".to_string());
    }
    match (&args[0], &args[1]) {
        (Value::Vec2(x1, y1), Value::Vec2(x2, y2)) => {
            let r = Vec2::new(*x1, *y1).reflect(Vec2::new(*x2, *y2));
            Ok(Value::Vec2(r.x, r.y))
        }
        (Value::Vec3(a), Value::Vec3(b)) => {
            val_vec3(vm, Vec3::from_array(**a).reflect(Vec3::from_array(**b)))
        }
        (l, r) => Err(format!(
            "geom.reflect() type mismatch '{}' and '{}'",
            l.type_name(),
            r.type_name()
        )),
    }
}

fn geom_project(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.project() expects (a, onto)".to_string());
    }
    match (&args[0], &args[1]) {
        (Value::Vec2(x1, y1), Value::Vec2(x2, y2)) => {
            let r = Vec2::new(*x1, *y1).project(Vec2::new(*x2, *y2));
            Ok(Value::Vec2(r.x, r.y))
        }
        (Value::Vec3(a), Value::Vec3(b)) => {
            val_vec3(vm, Vec3::from_array(**a).project(Vec3::from_array(**b)))
        }
        (l, r) => Err(format!(
            "geom.project() type mismatch '{}' and '{}'",
            l.type_name(),
            r.type_name()
        )),
    }
}

fn geom_transform(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let tr = match args.len() {
        0 => Transform::identity(),
        3 => Transform {
            position: as_vec3(&args[0], "geom.transform pos")?,
            rotation: as_quat(&args[1], "geom.transform rot")?,
            scale: as_vec3(&args[2], "geom.transform scale")?,
        },
        _ => return Err("geom.transform() expects 0 or 3 arguments (pos, rot, scale)".to_string()),
    };
    transform_to_val(vm, tr)
}

fn geom_transform_to_mat4(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.transform_to_mat4() expects (transform)".to_string());
    }
    val_mat4(vm, as_transform(&args[0], "geom.transform_to_mat4")?.to_mat4())
}

fn geom_transform_point(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.transform_point() expects (transform, vec3)".to_string());
    }
    let tr = as_transform(&args[0], "geom.transform_point")?;
    let p = as_vec3(&args[1], "geom.transform_point")?;
    val_vec3(vm, tr.transform_point(p))
}

fn geom_transform_combine(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.transform_combine() expects (parent, child)".to_string());
    }
    let parent = as_transform(&args[0], "geom.transform_combine")?;
    let child = as_transform(&args[1], "geom.transform_combine")?;
    transform_to_val(vm, parent.combine(child))
}

fn geom_aabb(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.aabb() expects (min_vec3, max_vec3)".to_string());
    }
    let min = as_vec3(&args[0], "geom.aabb min")?;
    let max = as_vec3(&args[1], "geom.aabb max")?;
    aabb_to_val(vm, Aabb::new(min, max))
}

fn geom_aabb_contains(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.aabb_contains() expects (aabb, vec3)".to_string());
    }
    Ok(Value::Bool(
        as_aabb(&args[0], "geom.aabb_contains")?
            .contains_point(as_vec3(&args[1], "geom.aabb_contains")?),
    ))
}

fn geom_aabb_intersects(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.aabb_intersects() expects (aabb, aabb)".to_string());
    }
    Ok(Value::Bool(
        as_aabb(&args[0], "geom.aabb_intersects")?
            .intersects(as_aabb(&args[1], "geom.aabb_intersects")?),
    ))
}

fn geom_aabb_center(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.aabb_center() expects (aabb)".to_string());
    }
    val_vec3(vm, as_aabb(&args[0], "geom.aabb_center")?.center())
}

fn geom_aabb_size(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.aabb_size() expects (aabb)".to_string());
    }
    val_vec3(vm, as_aabb(&args[0], "geom.aabb_size")?.size())
}

fn geom_aabb_expand(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.aabb_expand() expects (aabb, vec3)".to_string());
    }
    aabb_to_val(
        vm,
        as_aabb(&args[0], "geom.aabb_expand")?
            .expand_point(as_vec3(&args[1], "geom.aabb_expand")?),
    )
}

fn geom_aabb_transform(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.aabb_transform() expects (aabb, mat4)".to_string());
    }
    let b = as_aabb(&args[0], "geom.aabb_transform")?;
    let m = as_mat4(&args[1], "geom.aabb_transform")?;
    aabb_to_val(vm, b.transform(&m))
}

fn geom_ray(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.ray() expects (origin_vec3, dir_vec3)".to_string());
    }
    let ray = Ray::new(
        as_vec3(&args[0], "geom.ray origin")?,
        as_vec3(&args[1], "geom.ray dir")?,
    );
    vm.charge_alloc(96)?;
    let mut map = new_dict_with_capacity(2);
    map.insert("origin".to_string(), val_vec3(vm, ray.origin)?);
    map.insert("dir".to_string(), val_vec3(vm, ray.dir)?);
    Ok(Value::dict(map))
}

fn geom_ray_at(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.ray_at() expects (ray, t)".to_string());
    }
    val_vec3(
        vm,
        as_ray(&args[0], "geom.ray_at")?.at(as_f32(&args[1], "geom.ray_at t")?),
    )
}

fn geom_ray_aabb(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.ray_aabb() expects (ray, aabb)".to_string());
    }
    match as_ray(&args[0], "geom.ray_aabb")?.intersect_aabb(as_aabb(&args[1], "geom.ray_aabb")?) {
        Some(t) => Ok(Value::Float(t as f64)),
        None => Ok(Value::Unit),
    }
}

fn geom_ray_plane(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.ray_plane() expects (ray, plane)".to_string());
    }
    match as_ray(&args[0], "geom.ray_plane")?.intersect_plane(as_plane(&args[1], "geom.ray_plane")?) {
        Some(t) => Ok(Value::Float(t as f64)),
        None => Ok(Value::Unit),
    }
}

fn geom_plane(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.plane() expects (normal_vec3, d)".to_string());
    }
    let n = as_vec3(&args[0], "geom.plane normal")?.normalize();
    let d = as_f32(&args[1], "geom.plane d")?;
    plane_to_val(vm, Plane { normal: n, d })
}

fn geom_plane_from_point(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.plane_from_point() expects (normal_vec3, point_vec3)".to_string());
    }
    plane_to_val(
        vm,
        Plane::from_normal_point(
            as_vec3(&args[0], "geom.plane_from_point normal")?,
            as_vec3(&args[1], "geom.plane_from_point point")?,
        ),
    )
}

fn geom_plane_distance(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.plane_distance() expects (plane, vec3)".to_string());
    }
    Ok(Value::Float(
        as_plane(&args[0], "geom.plane_distance")?
            .signed_distance(as_vec3(&args[1], "geom.plane_distance")?) as f64,
    ))
}

fn geom_frustum_from_mat4(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("geom.frustum_from_mat4() expects (vp_mat4)".to_string());
    }
    let fr = Frustum::from_mat4_zo(&as_mat4(&args[0], "geom.frustum_from_mat4")?);
    vm.charge_alloc(512)?;
    let mut planes = Vec::with_capacity(6);
    for pl in fr.planes {
        planes.push(plane_to_val(vm, pl)?);
    }
    let mut map = new_dict_with_capacity(1);
    map.insert(
        "planes".to_string(),
        Value::List(Rc::new(std::cell::RefCell::new(planes))),
    );
    Ok(Value::dict(map))
}

fn geom_frustum_contains_point(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.frustum_contains_point() expects (frustum, vec3)".to_string());
    }
    Ok(Value::Bool(
        as_frustum(&args[0], "geom.frustum_contains_point")?
            .contains_point(as_vec3(&args[1], "geom.frustum_contains_point")?),
    ))
}

fn geom_frustum_intersects_aabb(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("geom.frustum_intersects_aabb() expects (frustum, aabb)".to_string());
    }
    Ok(Value::Bool(
        as_frustum(&args[0], "geom.frustum_intersects_aabb")?
            .intersects_aabb(as_aabb(&args[1], "geom.frustum_intersects_aabb")?),
    ))
}
