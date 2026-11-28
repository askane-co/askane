use std::cell::RefCell;
use std::rc::Rc;

use crate::media::{MediaBudgetGuard, err_bad_opt};
use crate::modules::build_module;
use crate::modules::hash::sha256_bytes;
use crate::modules::scene_util::{
    HandleBank, aabb_to_val, expect_str, extract_handle_id, make_handle_def, new_handle, vec3_to_val,
};
use crate::scene::asset::{AssetRef, read_asset_ref_bytes};
use crate::scene::cache::{ParserId, try_load_askas, try_store_askas};
use crate::scene::gltf::load_gltf_model;
use crate::scene::model::{
    MeshData, Model, decode_model_payload, encode_model_payload, verify_mesh_data, verify_model,
};
use crate::scene::obj::load_obj_model;
use crate::value::{StructDef, Value, new_dict_with_capacity};
use crate::vm::VM;

thread_local! {
    static MODELS: RefCell<HandleBank<Rc<Model>>> = RefCell::new(HandleBank::new());
    static MESHES: RefCell<HandleBank<Rc<MeshData>>> = RefCell::new(HandleBank::new());
    static MODEL_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
    static MESH_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
}

fn model_def() -> Rc<StructDef> {
    MODEL_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "Model",
            &[
                ("stats", mesh_stats),
                ("aabb", mesh_aabb),
                ("nodes", mesh_nodes),
                ("materials", mesh_materials),
                ("warnings", mesh_warnings),
                ("flatten", mesh_flatten),
                ("verify", mesh_verify),
                ("close", mesh_close),
                ("free", mesh_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

fn mesh_def() -> Rc<StructDef> {
    MESH_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "Mesh",
            &[
                ("stats", mesh_stats),
                ("aabb", mesh_aabb),
                ("verify", mesh_verify),
                ("close", mesh_close),
                ("free", mesh_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

pub fn reset_state() {
    MODELS.with(|m| m.borrow_mut().clear());
    MESHES.with(|m| m.borrow_mut().clear());
}

pub(crate) fn get_model_rc(id: u64) -> Result<Rc<Model>, String> {
    MODELS.with(|b| b.borrow().get(id, "Model").cloned())
}

pub(crate) fn extract_model(val: &Value, ctx: &str) -> Result<Rc<Model>, String> {
    let id = extract_handle_id(val, "Model", ctx)?;
    get_model_rc(id)
}

pub(crate) fn register_mesh(mut mesh: MeshData, vm: &mut VM) -> Result<Value, String> {
    if mesh.budget.is_none() {
        let bytes = mesh.memory_bytes();
        mesh.budget = Some(MediaBudgetGuard::charge(Some(vm.budget_core()), bytes)?);
    }
    let id = MESHES.with(|b| b.borrow_mut().insert(Rc::new(mesh), "Mesh"))?;
    Ok(new_handle(&mesh_def(), "Mesh", id))
}

pub fn module() -> Value {
    build_module(
        "mesh",
        &[
            ("load", mesh_load),
            ("stats", mesh_stats),
            ("aabb", mesh_aabb),
            ("nodes", mesh_nodes),
            ("materials", mesh_materials),
            ("warnings", mesh_warnings),
            ("flatten", mesh_flatten),
            ("verify", mesh_verify),
            ("close", mesh_close),
            ("free", mesh_close),
        ],
        &[],
    )
}

fn is_model_handle(val: &Value) -> Option<u64> {
    extract_handle_id(val, "Model", "mesh").ok()
}

fn is_mesh_handle(val: &Value) -> Option<u64> {
    extract_handle_id(val, "Mesh", "mesh").ok()
}

fn mesh_load(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("mesh.load() expects (path, [opts])"));
    }
    let raw_path = expect_str(&args[0], "mesh.load()")?;
    let mut use_cache = true;
    let mut format_hint: Option<String> = None;
    if let Some(arg1) = args.get(1) {
        if !matches!(arg1, Value::Unit) {
            let Value::Dict(d) = arg1 else {
                return Err(err_bad_opt("mesh.load() opts must be dict"));
            };
            for (k, v) in d.borrow().iter() {
                match k.as_str() {
                    "cache" => match v {
                        Value::Bool(b) => use_cache = *b,
                        _ => return Err(err_bad_opt("mesh.load() 'cache' opt must be bool")),
                    },
                    "format" => {
                        format_hint = Some(expect_str(v, "mesh.load() format")?.to_ascii_lowercase());
                    }
                    other => {
                        return Err(err_bad_opt(format!("mesh.load(): unknown option '{}'", other)));
                    }
                }
            }
        }
    }

    let aref = AssetRef::parse_allow_sandbox_root(raw_path, vm)?;
    let (bytes, _, source_key) = read_asset_ref_bytes(vm, &aref)?;
    let raw_hash = sha256_bytes(&bytes);
    let mut cache_key = Vec::with_capacity(64 + source_key.len());
    cache_key.extend_from_slice(b"ask.mesh.v1|");
    cache_key.extend_from_slice(source_key.as_bytes());
    cache_key.push(b'|');
    cache_key.extend_from_slice(&raw_hash);

    let ext_low = format_hint.unwrap_or_else(|| aref.as_str().to_ascii_lowercase());
    let is_glb = ext_low.ends_with(".glb") || ext_low == "glb" || (bytes.len() >= 4 && &bytes[0..4] == b"glTF");
    let is_gltf = ext_low.ends_with(".gltf") || ext_low == "gltf";
    let is_obj = ext_low.ends_with(".obj") || ext_low == "obj";
    if !is_glb && !is_gltf && !is_obj {
        return Err(format!(
            "E_MESH_UNSUPPORTED: unsupported mesh format '{}' (expected .obj, .gltf, or .glb)",
            raw_path
        ));
    }

    let budget_arc = std::sync::Arc::clone(vm.budget_core());
    if use_cache {
        if let Some(model) = try_load_askas(vm, &cache_key, ParserId::Mesh, |payload| {
            decode_model_payload(payload, Some(&budget_arc))
        }) {
            let id = MODELS.with(|b| b.borrow_mut().insert(Rc::new(model), "Model"))?;
            return Ok(new_handle(&model_def(), "Model", id));
        }
    }

    let model = if is_glb {
        load_gltf_model(vm, &aref, &bytes, true)?
    } else if is_gltf {
        load_gltf_model(vm, &aref, &bytes, false)?
    } else {
        load_obj_model(vm, &aref, &bytes)?
    };

    if use_cache {
        let encoded = encode_model_payload(&model);
        try_store_askas(vm, &cache_key, ParserId::Mesh, &encoded);
    }

    let id = MODELS.with(|b| b.borrow_mut().insert(Rc::new(model), "Model"))?;
    Ok(new_handle(&model_def(), "Model", id))
}

fn mesh_stats(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("mesh.stats() expects (model_or_mesh)"));
    }
    if let Some(id) = is_model_handle(&args[0]) {
        let model = get_model_rc(id)?;
        let st = model.total_stats();
        vm.charge_alloc(256)?;
        let mut d = new_dict_with_capacity(10);
        d.insert("vertices".to_string(), Value::Int(st.vertex_count as i64));
        d.insert("tris".to_string(), Value::Int(st.tri_count as i64));
        d.insert("triangles".to_string(), Value::Int(st.tri_count as i64));
        d.insert("degenerate_tris".to_string(), Value::Int(st.degenerate_tris as i64));
        d.insert("groups".to_string(), Value::Int(st.group_count as i64));
        d.insert("nodes".to_string(), Value::Int(model.nodes.len() as i64));
        d.insert("meshes".to_string(), Value::Int(model.meshes.len() as i64));
        d.insert("materials".to_string(), Value::Int(model.materials.len() as i64));
        d.insert("source".to_string(), Value::from_str(model.source.name()));
        d.insert("aabb".to_string(), aabb_to_val(vm, model.aabb)?);
        return Ok(Value::dict(d));
    }
    if let Some(id) = is_mesh_handle(&args[0]) {
        let mesh = MESHES.with(|b| b.borrow().get(id, "Mesh").cloned())?;
        vm.charge_alloc(192)?;
        let mut d = new_dict_with_capacity(8);
        d.insert("vertices".to_string(), Value::Int(mesh.stats.vertex_count as i64));
        d.insert("tris".to_string(), Value::Int(mesh.stats.tri_count as i64));
        d.insert("triangles".to_string(), Value::Int(mesh.stats.tri_count as i64));
        d.insert("degenerate_tris".to_string(), Value::Int(mesh.stats.degenerate_tris as i64));
        d.insert("groups".to_string(), Value::Int(mesh.stats.group_count as i64));
        d.insert("source".to_string(), Value::from_str(mesh.source.name()));
        d.insert("aabb".to_string(), aabb_to_val(vm, mesh.aabb)?);
        return Ok(Value::dict(d));
    }
    Err(err_bad_opt("mesh.stats() expects a Model or Mesh handle"))
}

fn mesh_aabb(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("mesh.aabb() expects (model_or_mesh)"));
    }
    if let Some(id) = is_model_handle(&args[0]) {
        let model = get_model_rc(id)?;
        return aabb_to_val(vm, model.aabb);
    }
    if let Some(id) = is_mesh_handle(&args[0]) {
        let mesh = MESHES.with(|b| b.borrow().get(id, "Mesh").cloned())?;
        return aabb_to_val(vm, mesh.aabb);
    }
    Err(err_bad_opt("mesh.aabb() expects a Model or Mesh handle"))
}

fn mesh_nodes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("mesh.nodes() expects (model)"));
    }
    let model = extract_model(&args[0], "mesh.nodes()")?;
    vm.charge_alloc(model.nodes.len().saturating_mul(160))?;
    let mut out = Vec::with_capacity(model.nodes.len());
    for nd in &model.nodes {
        let mut d = new_dict_with_capacity(7);
        d.insert("name".to_string(), Value::from_str(&nd.name));
        d.insert(
            "parent".to_string(),
            nd.parent.map_or(Value::Unit, |p| Value::Int(p as i64)),
        );
        d.insert(
            "mesh".to_string(),
            nd.mesh.map_or(Value::Unit, |m| Value::Int(m as i64)),
        );
        d.insert("position".to_string(), vec3_to_val(vm, nd.transform.position)?);
        d.insert(
            "rotation".to_string(),
            Value::Quat(Rc::new(nd.transform.rotation.to_array())),
        );
        d.insert("scale".to_string(), vec3_to_val(vm, nd.transform.scale)?);
        d.insert("aabb".to_string(), aabb_to_val(vm, nd.aabb_world)?);
        out.push(Value::dict(d));
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn mesh_materials(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("mesh.materials() expects (model)"));
    }
    let model = extract_model(&args[0], "mesh.materials()")?;
    vm.charge_alloc(model.materials.len().saturating_mul(192))?;
    let mut out = Vec::with_capacity(model.materials.len());
    for mat in &model.materials {
        let mut tex = new_dict_with_capacity(6);
        tex.insert(
            "base_color".to_string(),
            mat.textures.base_color.as_ref().map_or(Value::Unit, |s| Value::from_str(s)),
        );
        tex.insert(
            "normal".to_string(),
            mat.textures.normal.as_ref().map_or(Value::Unit, |s| Value::from_str(s)),
        );
        tex.insert(
            "metallic_roughness".to_string(),
            mat.textures.metallic_roughness.as_ref().map_or(Value::Unit, |s| Value::from_str(s)),
        );
        tex.insert(
            "emissive".to_string(),
            mat.textures.emissive.as_ref().map_or(Value::Unit, |s| Value::from_str(s)),
        );
        tex.insert(
            "occlusion".to_string(),
            mat.textures.occlusion.as_ref().map_or(Value::Unit, |s| Value::from_str(s)),
        );
        tex.insert(
            "height".to_string(),
            mat.textures.height.as_ref().map_or(Value::Unit, |s| Value::from_str(s)),
        );

        let mut d = new_dict_with_capacity(8);
        d.insert("name".to_string(), Value::from_str(&mat.name));
        d.insert("base_color".to_string(), vec3_to_val(vm, mat.base_color)?);
        d.insert("metallic".to_string(), Value::Float(mat.metallic as f64));
        d.insert("roughness".to_string(), Value::Float(mat.roughness as f64));
        d.insert("alpha_mode".to_string(), Value::from_str(mat.alpha_mode.name()));
        d.insert("double_sided".to_string(), Value::Bool(mat.double_sided));
        d.insert("unlit".to_string(), Value::Bool(mat.unlit));
        d.insert("textures".to_string(), Value::dict(tex));
        out.push(Value::dict(d));
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn mesh_warnings(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("mesh.warnings() expects (model)"));
    }
    let model = extract_model(&args[0], "mesh.warnings()")?;
    vm.charge_alloc(model.warnings.len().saturating_mul(64))?;
    let list: Vec<Value> = model.warnings.iter().map(|w| Value::from_str(w)).collect();
    Ok(Value::List(Rc::new(RefCell::new(list))))
}

fn mesh_flatten(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("mesh.flatten() expects (model, [material_name])"));
    }
    let model = extract_model(&args[0], "mesh.flatten()")?;
    let mat_filter = match args.get(1) {
        Some(v) if !matches!(v, Value::Unit) => Some(expect_str(v, "mesh.flatten()")?),
        _ => None,
    };
    let flattened = model.flatten(mat_filter, Some(vm.budget_core()))?;
    register_mesh(flattened, vm)
}

fn mesh_verify(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("mesh.verify() expects (model_or_mesh)"));
    }
    if let Some(id) = is_model_handle(&args[0]) {
        let model = get_model_rc(id)?;
        let mut cloned = Model {
            meshes: model.meshes.iter().map(|m| m.clone_without_budget()).collect(),
            materials: model.materials.clone(),
            nodes: model.nodes.clone(),
            roots: model.roots.clone(),
            aabb: model.aabb,
            warnings: Vec::new(),
            source: model.source,
            budget: None,
        };
        verify_model(&mut cloned)?;
        return Ok(Value::Bool(true));
    }
    if let Some(id) = is_mesh_handle(&args[0]) {
        let mesh = MESHES.with(|b| b.borrow().get(id, "Mesh").cloned())?;
        let mut cloned = mesh.clone_without_budget();
        let mut warns = Vec::new();
        verify_mesh_data(&mut cloned, &mut warns)?;
        return Ok(Value::Bool(true));
    }
    Err(err_bad_opt("mesh.verify() expects a Model or Mesh handle"))
}

fn mesh_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("mesh.close() expects (handle)"));
    }
    if let Some(id) = is_model_handle(&args[0]) {
        let removed = MODELS.with(|b| b.borrow_mut().remove(id).is_some());
        return Ok(Value::Bool(removed));
    }
    if let Some(id) = is_mesh_handle(&args[0]) {
        let removed = MESHES.with(|b| b.borrow_mut().remove(id).is_some());
        return Ok(Value::Bool(removed));
    }
    Err(err_bad_opt("mesh.close() expects a Model or Mesh handle"))
}
