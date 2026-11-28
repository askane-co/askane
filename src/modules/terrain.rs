use std::cell::RefCell;
use std::rc::Rc;

use crate::media::err_bad_opt;
use crate::media::image::PixelFormat;
use crate::modules::build_module;
use crate::modules::gfx;
use crate::modules::hash::sha256_bytes;
use crate::modules::mesh::register_mesh;
use crate::modules::scene_util::{
    HandleBank, aabb_to_val, expect_f32, expect_str, expect_usize, extract_handle_id,
    is_surface_arg, make_handle_def, new_handle, vec3_to_val,
};
use crate::modules::tiled::register_solid_grid;
use crate::scene::asset::read_path_checked_temp;
use crate::scene::cache::{BinReader, BinWriter, ParserId, try_load_askas, try_store_askas};
use crate::scene::terrain::{
    HeightMap, MAX_TERRAIN_CELLS, MAX_TERRAIN_DIM, SplatMap, load_heightmap, mode7_draw_surface,
};
use crate::value::{StructDef, Value, new_dict_with_capacity};
use crate::vm::VM;

thread_local! {
    static HEIGHTMAPS: RefCell<HandleBank<Rc<HeightMap>>> = RefCell::new(HandleBank::new());
    static SPLATS: RefCell<HandleBank<Rc<SplatMap>>> = RefCell::new(HandleBank::new());
    static HM_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
    static SPLAT_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
}

fn hm_def() -> Rc<StructDef> {
    HM_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "HeightMap",
            &[
                ("info", terrain_info),
                ("height", terrain_height),
                ("height_at", terrain_height),
                ("normal", terrain_normal),
                ("normal_at", terrain_normal),
                ("slope", terrain_slope),
                ("to_solid", terrain_to_solid),
                ("to_mesh", terrain_to_mesh),
                ("mode7_draw", terrain_mode7_draw),
                ("close", terrain_close),
                ("free", terrain_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

fn splat_def() -> Rc<StructDef> {
    SPLAT_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "SplatMap",
            &[
                ("weights_at", terrain_splat_weights),
                ("weights", terrain_splat_weights),
                ("close", terrain_close),
                ("free", terrain_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

pub fn reset_state() {
    HEIGHTMAPS.with(|h| h.borrow_mut().clear());
    SPLATS.with(|s| s.borrow_mut().clear());
}

pub(crate) fn extract_heightmap(val: &Value, ctx: &str) -> Result<Rc<HeightMap>, String> {
    let id = extract_handle_id(val, "HeightMap", ctx)?;
    HEIGHTMAPS.with(|b| b.borrow().get(id, "HeightMap").cloned())
}

pub(crate) fn extract_splat(val: &Value, ctx: &str) -> Result<Rc<SplatMap>, String> {
    let id = extract_handle_id(val, "SplatMap", ctx)?;
    SPLATS.with(|b| b.borrow().get(id, "SplatMap").cloned())
}

pub fn module() -> Value {
    build_module(
        "terrain",
        &[
            ("load", terrain_load),
            ("from_image", terrain_from_image),
            ("info", terrain_info),
            ("height", terrain_height),
            ("height_at", terrain_height),
            ("normal", terrain_normal),
            ("normal_at", terrain_normal),
            ("slope", terrain_slope),
            ("to_solid", terrain_to_solid),
            ("to_mesh", terrain_to_mesh),
            ("splat_from_image", terrain_splat_from_image),
            ("splat_weights", terrain_splat_weights),
            ("mode7_draw", terrain_mode7_draw),
            ("close", terrain_close),
            ("free", terrain_close),
        ],
        &[],
    )
}

fn encode_terrain_payload(hm: &HeightMap) -> Vec<u8> {
    let mut w = BinWriter::new();
    w.write_u32(hm.width as u32);
    w.write_u32(hm.height as u32);
    w.write_f32(hm.cell_size);
    w.write_f32(hm.height_scale);
    for &h in &hm.heights {
        w.write_f32(h);
    }
    w.buf
}

fn decode_terrain_payload(
    payload: &[u8],
    budget_core: Option<&std::sync::Arc<crate::budget::BudgetCore>>,
) -> Result<HeightMap, String> {
    let mut r = BinReader::new(payload);
    let w = r.read_u32()? as usize;
    let h = r.read_u32()? as usize;
    if w == 0 || h == 0 || w > MAX_TERRAIN_DIM || h > MAX_TERRAIN_DIM {
        return Err("E_TERRAIN_LIMIT: ASKAS heightmap dimensions invalid".to_string());
    }
    let cells = w.saturating_mul(h);
    if cells > MAX_TERRAIN_CELLS {
        return Err("E_TERRAIN_LIMIT: ASKAS heightmap exceeds 4M cell limit".to_string());
    }
    let cell_size = r.read_f32()?;
    let height_scale = r.read_f32()?;
    let mut heights = Vec::with_capacity(cells);
    for _ in 0..cells {
        heights.push(r.read_f32()?);
    }
    HeightMap::from_vec(w, h, cell_size, height_scale, heights, budget_core)
}

fn terrain_load(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("terrain.load() expects (path, [opts])"));
    }
    let raw_path = expect_str(&args[0], "terrain.load()")?;
    let mut width_opt = None;
    let mut height_opt = None;
    let mut cell_size = 1.0f32;
    let mut height_scale = 1.0f32;
    let mut use_cache = true;

    if let Some(arg1) = args.get(1) {
        if !matches!(arg1, Value::Unit) {
            let Value::Dict(d) = arg1 else {
                return Err(err_bad_opt("terrain.load() opts must be dict"));
            };
            for (k, v) in d.borrow().iter() {
                match k.as_str() {
                    "width" => width_opt = Some(expect_usize(v, "terrain.load() width")?),
                    "height" => height_opt = Some(expect_usize(v, "terrain.load() height")?),
                    "cell_size" => cell_size = expect_f32(v, "terrain.load() cell_size")?,
                    "height_scale" => height_scale = expect_f32(v, "terrain.load() height_scale")?,
                    "cache" => match v {
                        Value::Bool(b) => use_cache = *b,
                        _ => return Err(err_bad_opt("terrain.load() 'cache' must be bool")),
                    },
                    other => return Err(format!("E_TERRAIN_BAD_OPT: unknown terrain option '{}'", other)),
                }
            }
        }
    }

    if !cell_size.is_finite() || cell_size <= 0.0 {
        return Err("E_TERRAIN_BAD_OPT: cell_size must be > 0".to_string());
    }
    if !height_scale.is_finite() || height_scale == 0.0 {
        return Err("E_TERRAIN_BAD_OPT: height_scale must be non-zero finite".to_string());
    }

    let (_, bytes, _, source_key) = read_path_checked_temp(vm, raw_path)?;
    let mut cache_key = Vec::with_capacity(96 + source_key.len());
    cache_key.extend_from_slice(b"ask.terrain.v1|");
    cache_key.extend_from_slice(source_key.as_bytes());
    cache_key.extend_from_slice(&(width_opt.unwrap_or(0) as u64).to_le_bytes());
    cache_key.extend_from_slice(&(height_opt.unwrap_or(0) as u64).to_le_bytes());
    cache_key.extend_from_slice(&cell_size.to_bits().to_le_bytes());
    cache_key.extend_from_slice(&height_scale.to_bits().to_le_bytes());
    cache_key.extend_from_slice(&sha256_bytes(&bytes));

    let budget_arc = std::sync::Arc::clone(vm.budget_core());
    if use_cache {
        if let Some(hm) = try_load_askas(vm, &cache_key, ParserId::Terrain, |payload| {
            decode_terrain_payload(payload, Some(&budget_arc))
        }) {
            let id = HEIGHTMAPS.with(|b| b.borrow_mut().insert(Rc::new(hm), "HeightMap"))?;
            return Ok(new_handle(&hm_def(), "HeightMap", id));
        }
    }

    let hm = load_heightmap(vm, raw_path, width_opt, height_opt, cell_size, height_scale)?;
    if use_cache {
        let payload = encode_terrain_payload(&hm);
        try_store_askas(vm, &cache_key, ParserId::Terrain, &payload);
    }
    let id = HEIGHTMAPS.with(|b| b.borrow_mut().insert(Rc::new(hm), "HeightMap"))?;
    Ok(new_handle(&hm_def(), "HeightMap", id))
}

fn terrain_from_image(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err(err_bad_opt("terrain.from_image() expects (img, [cell_size, height_scale])"));
    }
    let Value::Image(im) = &args[0] else {
        return Err("E_TERRAIN_BAD_OPT: terrain.from_image() expects Image".to_string());
    };
    let cell_size = match args.get(1) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "terrain.from_image() cell_size")?,
        _ => 1.0,
    };
    let height_scale = match args.get(2) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "terrain.from_image() height_scale")?,
        _ => 1.0,
    };
    let w = im.width as usize;
    let h = im.height as usize;
    let cells = w.saturating_mul(h);
    let mut heights = Vec::with_capacity(cells);
    match im.format {
        PixelFormat::R8 => {
            for &v in im.data.iter() {
                heights.push((v as f32 / 255.0) * height_scale);
            }
        }
        PixelFormat::Rg8 => {
            for px in im.data.chunks_exact(2) {
                heights.push((px[0] as f32 / 255.0) * height_scale);
            }
        }
        PixelFormat::Rgb8 => {
            for px in im.data.chunks_exact(3) {
                heights.push((px[0] as f32 / 255.0) * height_scale);
            }
        }
        PixelFormat::Rgba8 => {
            for px in im.data.chunks_exact(4) {
                heights.push((px[0] as f32 / 255.0) * height_scale);
            }
        }
    }
    let hm = HeightMap::from_vec(w, h, cell_size, height_scale, heights, Some(vm.budget_core()))?;
    let id = HEIGHTMAPS.with(|b| b.borrow_mut().insert(Rc::new(hm), "HeightMap"))?;
    Ok(new_handle(&hm_def(), "HeightMap", id))
}

fn terrain_info(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("terrain.info() expects (heightmap)"));
    }
    let hm = extract_heightmap(&args[0], "terrain.info()")?;
    vm.charge_alloc(128)?;
    let mut d = new_dict_with_capacity(5);
    d.insert("width".to_string(), Value::Int(hm.width as i64));
    d.insert("height".to_string(), Value::Int(hm.height as i64));
    d.insert("cell_size".to_string(), Value::Float(hm.cell_size as f64));
    d.insert("height_scale".to_string(), Value::Float(hm.height_scale as f64));
    d.insert("aabb".to_string(), aabb_to_val(vm, hm.world_aabb())?);
    Ok(Value::dict(d))
}

fn terrain_height(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(err_bad_opt("terrain.height() expects (heightmap, wx, wz)"));
    }
    let hm = extract_heightmap(&args[0], "terrain.height()")?;
    let wx = expect_f32(&args[1], "terrain.height() wx")?;
    let wz = expect_f32(&args[2], "terrain.height() wz")?;
    Ok(Value::Float(hm.height_at(wx, wz) as f64))
}

fn terrain_normal(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(err_bad_opt("terrain.normal() expects (heightmap, wx, wz)"));
    }
    let hm = extract_heightmap(&args[0], "terrain.normal()")?;
    let wx = expect_f32(&args[1], "terrain.normal() wx")?;
    let wz = expect_f32(&args[2], "terrain.normal() wz")?;
    vec3_to_val(vm, hm.normal_at(wx, wz))
}

fn terrain_slope(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(err_bad_opt("terrain.slope() expects (heightmap, wx, wz)"));
    }
    let hm = extract_heightmap(&args[0], "terrain.slope()")?;
    let wx = expect_f32(&args[1], "terrain.slope() wx")?;
    let wz = expect_f32(&args[2], "terrain.slope() wz")?;
    Ok(Value::Float(hm.slope_deg(wx, wz) as f64))
}

fn terrain_to_solid(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(err_bad_opt("terrain.to_solid() expects (heightmap, threshold)"));
    }
    let hm = extract_heightmap(&args[0], "terrain.to_solid()")?;
    let threshold = expect_f32(&args[1], "terrain.to_solid() threshold")?;
    let solid = hm.to_solid(threshold, Some(vm.budget_core()))?;
    register_solid_grid(solid)
}

fn terrain_to_mesh(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err(err_bad_opt("terrain.to_mesh() expects (heightmap, [step], [splatmap])"));
    }
    let hm = extract_heightmap(&args[0], "terrain.to_mesh()")?;
    let step = match args.get(1) {
        Some(v) if !matches!(v, Value::Unit) => expect_usize(v, "terrain.to_mesh() step")?.max(1),
        _ => 1,
    };
    let splat_rc = match args.get(2) {
        Some(v) if !matches!(v, Value::Unit) => Some(extract_splat(v, "terrain.to_mesh() splat")?),
        _ => None,
    };
    let mesh = hm.to_mesh(step, splat_rc.as_deref(), Some(vm.budget_core()))?;
    register_mesh(mesh, vm)
}

fn terrain_splat_from_image(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("terrain.splat_from_image() expects (img)"));
    }
    let Value::Image(im) = &args[0] else {
        return Err("E_SPLAT_INVALID: terrain.splat_from_image() expects Image".to_string());
    };
    let mut data = Vec::with_capacity((im.width as usize) * (im.height as usize) * 4);
    for px in im.to_argb_u32_vec() {
        let a = ((px >> 24) & 0xFF) as u8;
        let r = ((px >> 16) & 0xFF) as u8;
        let g = ((px >> 8) & 0xFF) as u8;
        let b = (px & 0xFF) as u8;
        data.extend_from_slice(&[r, g, b, a]);
    }
    let sm = SplatMap::new(im.width as usize, im.height as usize, data, Some(vm.budget_core()))?;
    let id = SPLATS.with(|b| b.borrow_mut().insert(Rc::new(sm), "SplatMap"))?;
    Ok(new_handle(&splat_def(), "SplatMap", id))
}

fn terrain_splat_weights(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(err_bad_opt("terrain.splat_weights() expects (splatmap, u, v)"));
    }
    let sm = extract_splat(&args[0], "terrain.splat_weights()")?;
    let u = expect_f32(&args[1], "terrain.splat_weights() u")?;
    let v = expect_f32(&args[2], "terrain.splat_weights() v")?;
    let w = sm.weights_at(u, v);
    Ok(Value::List(Rc::new(RefCell::new(vec![
        Value::Float(w[0] as f64),
        Value::Float(w[1] as f64),
        Value::Float(w[2] as f64),
        Value::Float(w[3] as f64),
    ]))))
}

fn terrain_mode7_draw(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err(err_bad_opt("terrain.mode7_draw() expects (heightmap, [surface], [opts_dict])"));
    }
    let hm = extract_heightmap(&args[0], "terrain.mode7_draw()")?;
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

    let mut cam_x = (hm.width as f32) * hm.cell_size * 0.5;
    let mut cam_y = hm.height_scale.abs().max(4.0);
    let mut cam_z = 0.0f32;
    let mut yaw = 0.0f32;
    let mut horizon = 48.0f32;
    let mut fov_scale = 0.75f32;
    let mut tex_img: Option<Rc<crate::media::image::MediaImage>> = None;

    if let Some(ov) = opts_arg {
        if !matches!(ov, Value::Unit) {
            let Value::Dict(d) = ov else {
                return Err(err_bad_opt("terrain.mode7_draw() opts must be dict"));
            };
            for (k, v) in d.borrow().iter() {
                match k.as_str() {
                    "cam_x" | "x" => cam_x = expect_f32(v, "mode7 cam_x")?,
                    "cam_y" | "y" => cam_y = expect_f32(v, "mode7 cam_y")?,
                    "cam_z" | "z" => cam_z = expect_f32(v, "mode7 cam_z")?,
                    "yaw" => yaw = expect_f32(v, "mode7 yaw")?,
                    "horizon" => horizon = expect_f32(v, "mode7 horizon")?,
                    "fov" | "fov_scale" => fov_scale = expect_f32(v, "mode7 fov_scale")?,
                    "texture" => match v {
                        Value::Image(im) => tex_img = Some(Rc::clone(im)),
                        Value::Texture(tx) => tex_img = Some(Rc::clone(&tx.image)),
                        _ => return Err(err_bad_opt("mode7 'texture' must be Image or Texture")),
                    },
                    other => return Err(err_bad_opt(format!("terrain.mode7_draw(): unknown opt '{}'", other))),
                }
            }
        }
    }

    let tex_argb = tex_img
        .as_ref()
        .map(|im| (im.to_argb_u32_vec(), im.width as usize, im.height as usize));

    gfx::with_target_pixels_mut(target, true, |buf, depth, bw, bh, clip, _b| {
        let tex_slice = tex_argb.as_ref().map(|(v, w, h)| (v.as_slice(), *w, *h));
        mode7_draw_surface(
            &hm,
            buf,
            depth,
            bw,
            bh,
            clip,
            tex_slice,
            cam_x,
            cam_y,
            cam_z,
            yaw,
            horizon,
            fov_scale,
        );
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn terrain_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("terrain.close() expects (handle)"));
    }
    if let Ok(id) = extract_handle_id(&args[0], "HeightMap", "terrain.close()") {
        return Ok(Value::Bool(HEIGHTMAPS.with(|b| b.borrow_mut().remove(id).is_some())));
    }
    if let Ok(id) = extract_handle_id(&args[0], "SplatMap", "terrain.close()") {
        return Ok(Value::Bool(SPLATS.with(|b| b.borrow_mut().remove(id).is_some())));
    }
    Err(err_bad_opt("terrain.close() expects HeightMap or SplatMap handle"))
}
