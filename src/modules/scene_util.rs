use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::media::{err_bad_opt, err_limit};
use crate::modules::NativeFnPtr;
use crate::scene::asset::MAX_ASSET_HANDLES;
use crate::scene::geom::{Aabb, Mat4, Quat, Transform, Vec3};
use crate::value::{FastMap, StructDef, StructInstance, Value, new_dict_with_capacity};
use crate::vm::VM;

pub(crate) struct HandleBank<T> {
    next_id: u64,
    items: HashMap<u64, T>,
}

impl<T> HandleBank<T> {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            items: HashMap::new(),
        }
    }

    pub fn insert(&mut self, item: T, kind: &str) -> Result<u64, String> {
        if self.items.len() >= MAX_ASSET_HANDLES {
            return Err(err_limit(format!(
                "{} handle limit ({}) exceeded; close unused handles first",
                kind, MAX_ASSET_HANDLES
            )));
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1).max(1);
        self.items.insert(id, item);
        Ok(id)
    }

    pub fn get(&self, id: u64, kind: &str) -> Result<&T, String> {
        self.items
            .get(&id)
            .ok_or_else(|| err_bad_opt(format!("invalid or closed {} handle #{}", kind, id)))
    }

    pub fn remove(&mut self, id: u64) -> Option<T> {
        self.items.remove(&id)
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

pub(crate) fn make_handle_def(
    name: &str,
    methods: &[(&'static str, NativeFnPtr)],
) -> Rc<StructDef> {
    let field_names = vec!["id".to_string(), "kind".to_string()];
    let mut field_index = FastMap::default();
    field_index.insert("id".to_string(), 0usize);
    field_index.insert("kind".to_string(), 1usize);
    let field_defaults = vec![None, None];
    let mut method_map = new_dict_with_capacity(methods.len());
    for &(mname, func) in methods {
        method_map.insert(mname.to_string(), Value::native_fn(mname, func));
    }
    Rc::new(StructDef {
        ic_id: crate::bytecode::next_ic_id(),
        name: name.to_string(),
        field_names,
        field_index,
        field_defaults,
        methods: method_map,
    })
}

pub(crate) fn new_handle(def: &Rc<StructDef>, kind: &str, id: u64) -> Value {
    Value::StructInstance(Rc::new(StructInstance {
        def: Rc::clone(def),
        fields: RefCell::new(vec![Value::Int(id as i64), Value::from_str(kind)]),
    }))
}

pub(crate) fn extract_handle_id(val: &Value, expected_kind: &str, ctx: &str) -> Result<u64, String> {
    match val {
        Value::StructInstance(inst) => {
            let fields = inst.fields.borrow();
            let Some(Value::Int(id)) = fields.first() else {
                return Err(err_bad_opt(format!("{} invalid {} handle", ctx, expected_kind)));
            };
            let Some(Value::Str(k)) = fields.get(1) else {
                return Err(err_bad_opt(format!("{} invalid {} handle", ctx, expected_kind)));
            };
            if k.as_str() != expected_kind || *id <= 0 {
                return Err(err_bad_opt(format!(
                    "{} expected {} handle, got '{}'",
                    ctx, expected_kind, k
                )));
            }
            Ok(*id as u64)
        }
        other => Err(err_bad_opt(format!(
            "{} expected {} handle, got '{}'",
            ctx,
            expected_kind,
            other.type_name()
        ))),
    }
}

pub(crate) fn expect_str<'a>(val: &'a Value, ctx: &str) -> Result<&'a str, String> {
    val.as_str()
        .ok_or_else(|| err_bad_opt(format!("{} expects str, got '{}'", ctx, val.type_name())))
}

pub(crate) fn expect_f32(val: &Value, ctx: &str) -> Result<f32, String> {
    let f = match val {
        Value::Int(i) => *i as f32,
        Value::Float(v) => *v as f32,
        other => {
            return Err(err_bad_opt(format!(
                "{} expects number, got '{}'",
                ctx,
                other.type_name()
            )));
        }
    };
    if !f.is_finite() {
        return Err(err_bad_opt(format!("{} must be finite", ctx)));
    }
    Ok(f)
}

pub(crate) fn expect_i32(val: &Value, ctx: &str) -> Result<i32, String> {
    match val {
        Value::Int(i) => i32::try_from(*i).map_err(|_| err_bad_opt(format!("{} out of i32 range", ctx))),
        other => Err(err_bad_opt(format!(
            "{} expects int, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

pub(crate) fn expect_usize(val: &Value, ctx: &str) -> Result<usize, String> {
    match val {
        Value::Int(i) if *i >= 0 => Ok(*i as usize),
        other => Err(err_bad_opt(format!(
            "{} expects non-negative int, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

pub(crate) fn expect_bool(val: &Value, ctx: &str) -> Result<bool, String> {
    match val {
        Value::Bool(b) => Ok(*b),
        other => Err(err_bad_opt(format!(
            "{} expects bool, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

pub(crate) fn expect_vec3(val: &Value, ctx: &str) -> Result<Vec3, String> {
    match val {
        Value::Vec3(a) => {
            let v = Vec3::from_array(**a);
            if !v.is_finite() {
                return Err(err_bad_opt(format!("{} contains non-finite vec3", ctx)));
            }
            Ok(v)
        }
        Value::List(l) => {
            let b = l.borrow();
            if b.len() != 3 {
                return Err(err_bad_opt(format!("{} expected [x, y, z] of length 3", ctx)));
            }
            Ok(Vec3::new(
                expect_f32(&b[0], ctx)?,
                expect_f32(&b[1], ctx)?,
                expect_f32(&b[2], ctx)?,
            ))
        }
        other => Err(err_bad_opt(format!(
            "{} expects vec3 or [x,y,z] list, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

pub(crate) fn expect_quat(val: &Value, ctx: &str) -> Result<Quat, String> {
    match val {
        Value::Quat(a) => Ok(Quat::from_array(**a).normalize()),
        Value::List(l) => {
            let b = l.borrow();
            if b.len() != 4 {
                return Err(err_bad_opt(format!("{} expected [x, y, z, w] quat", ctx)));
            }
            Ok(Quat::new(
                expect_f32(&b[0], ctx)?,
                expect_f32(&b[1], ctx)?,
                expect_f32(&b[2], ctx)?,
                expect_f32(&b[3], ctx)?,
            )
            .normalize())
        }
        other => Err(err_bad_opt(format!(
            "{} expects quat or [x,y,z,w] list, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

pub(crate) fn expect_transform(val: &Value, ctx: &str) -> Result<Transform, String> {
    match val {
        Value::Mat4(m) => Transform::from_mat4(&Mat4::from_array(**m)),
        Value::Dict(d) => {
            let m = d.borrow();
            let mut tr = Transform::identity();
            if let Some(p) = m.get("position") {
                tr.position = expect_vec3(p, ctx)?;
            }
            if let Some(r) = m.get("rotation") {
                tr.rotation = expect_quat(r, ctx)?;
            }
            if let Some(s) = m.get("scale") {
                tr.scale = expect_vec3(s, ctx)?;
            }
            Ok(tr)
        }
        other => Err(err_bad_opt(format!(
            "{} expects transform dict or mat4, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

pub(crate) fn expect_rgb_f32(val: &Value, ctx: &str) -> Result<[f32; 3], String> {
    match val {
        Value::Int(c) => {
            let u = *c as u32;
            Ok([
                ((u >> 16) & 0xFF) as f32 / 255.0,
                ((u >> 8) & 0xFF) as f32 / 255.0,
                (u & 0xFF) as f32 / 255.0,
            ])
        }
        Value::Vec3(a) => Ok(**a),
        Value::List(l) => {
            let b = l.borrow();
            if b.len() != 3 {
                return Err(err_bad_opt(format!("{} color list must have 3 floats", ctx)));
            }
            Ok([
                expect_f32(&b[0], ctx)?,
                expect_f32(&b[1], ctx)?,
                expect_f32(&b[2], ctx)?,
            ])
        }
        other => Err(err_bad_opt(format!(
            "{} expects RGB color ([r,g,b], vec3, or int), got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

pub(crate) fn expect_color_u32(val: &Value, ctx: &str) -> Result<u32, String> {
    match val {
        Value::Int(c) => {
            let u = *c as u32;
            if (u >> 24) == 0 {
                Ok(0xFF00_0000 | (u & 0x00FF_FFFF))
            } else {
                Ok(u)
            }
        }
        Value::List(l) => {
            let b = l.borrow();
            if b.len() < 3 || b.len() > 4 {
                return Err(err_bad_opt(format!("{} color list expects 3 or 4 ints", ctx)));
            }
            let r = (expect_f32(&b[0], ctx)?).clamp(0.0, 255.0) as u32;
            let g = (expect_f32(&b[1], ctx)?).clamp(0.0, 255.0) as u32;
            let b_ch = (expect_f32(&b[2], ctx)?).clamp(0.0, 255.0) as u32;
            let a = if b.len() == 4 {
                (expect_f32(&b[3], ctx)?).clamp(0.0, 255.0) as u32
            } else {
                255
            };
            Ok((a << 24) | (r << 16) | (g << 8) | b_ch)
        }
        other => Err(err_bad_opt(format!(
            "{} expects color int or [r,g,b(,a)], got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

pub(crate) fn vec3_to_val(vm: &mut VM, v: Vec3) -> Result<Value, String> {
    vm.charge_alloc(16)?;
    Ok(Value::Vec3(Rc::new(v.to_array())))
}

pub(crate) fn aabb_to_val(vm: &mut VM, aabb: Aabb) -> Result<Value, String> {
    vm.charge_alloc(96)?;
    let mut map = new_dict_with_capacity(4);
    map.insert("min".to_string(), vec3_to_val(vm, aabb.min)?);
    map.insert("max".to_string(), vec3_to_val(vm, aabb.max)?);
    map.insert("center".to_string(), vec3_to_val(vm, aabb.center())?);
    map.insert("size".to_string(), vec3_to_val(vm, aabb.size())?);
    Ok(Value::dict(map))
}

pub(crate) fn is_surface_arg(val: &Value) -> bool {
    match val {
        Value::StructInstance(inst) => {
            let f = inst.fields.borrow();
            matches!(f.get(1), Some(Value::Str(s)) if s.as_str() == "Surface")
        }
        _ => false,
    }
}
