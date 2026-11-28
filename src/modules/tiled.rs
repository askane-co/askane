use std::cell::RefCell;
use std::rc::Rc;

use crate::media::{MediaBudgetGuard, err_bad_opt};
use crate::modules::build_module;
use crate::modules::gfx;
use crate::modules::hash::sha256_bytes;
use crate::modules::scene_util::{
    HandleBank, expect_f32, expect_str, expect_usize, extract_handle_id, is_surface_arg,
    make_handle_def, new_handle,
};
use crate::modules::terrain::extract_heightmap;
use crate::scene::asset::{AssetRef, read_asset_ref_bytes};
use crate::scene::cache::{BinReader, BinWriter, ParserId, try_load_askas, try_store_askas};
use crate::scene::geom::Vec3;
use crate::scene::tiled::{
    Gid, MAX_TILED_CELLS, SolidGrid, TileChunk, TileLayer, TileMap, Tileset, load_tiled_map,
    render_tilemap_2d,
};
use crate::value::{StructDef, Value, new_dict_with_capacity};
use crate::vm::VM;

thread_local! {
    static MAPS: RefCell<HandleBank<Rc<RefCell<TileMap>>>> = RefCell::new(HandleBank::new());
    static SOLIDS: RefCell<HandleBank<Rc<SolidGrid>>> = RefCell::new(HandleBank::new());
    static MAP_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
    static SOLID_DEF: RefCell<Option<Rc<StructDef>>> = const { RefCell::new(None) };
}

fn map_def() -> Rc<StructDef> {
    MAP_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "TileMap",
            &[
                ("info", tiled_info),
                ("stats", tiled_info),
                ("layers", tiled_layers),
                ("tile_at", tiled_tile_at),
                ("solid_grid", tiled_solid_grid),
                ("place_on_terrain", tiled_place_on_terrain),
                ("draw", tiled_draw),
                ("close", tiled_close),
                ("free", tiled_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

fn solid_def() -> Rc<StructDef> {
    SOLID_DEF.with(|d| {
        let mut opt = d.borrow_mut();
        if let Some(def) = opt.as_ref() {
            return Rc::clone(def);
        }
        let def = make_handle_def(
            "SolidGrid",
            &[
                ("is_solid", tiled_is_solid),
                ("info", tiled_solid_info),
                ("close", tiled_close),
                ("free", tiled_close),
            ],
        );
        *opt = Some(Rc::clone(&def));
        def
    })
}

pub fn reset_state() {
    MAPS.with(|m| m.borrow_mut().clear());
    SOLIDS.with(|s| s.borrow_mut().clear());
}

pub(crate) fn extract_map(val: &Value, ctx: &str) -> Result<Rc<RefCell<TileMap>>, String> {
    let id = extract_handle_id(val, "TileMap", ctx)?;
    MAPS.with(|b| b.borrow().get(id, "TileMap").cloned())
}

pub(crate) fn extract_solid(val: &Value, ctx: &str) -> Result<Rc<SolidGrid>, String> {
    let id = extract_handle_id(val, "SolidGrid", ctx)?;
    SOLIDS.with(|b| b.borrow().get(id, "SolidGrid").cloned())
}

pub(crate) fn register_solid_grid(grid: SolidGrid) -> Result<Value, String> {
    let id = SOLIDS.with(|b| b.borrow_mut().insert(Rc::new(grid), "SolidGrid"))?;
    Ok(new_handle(&solid_def(), "SolidGrid", id))
}

pub fn module() -> Value {
    build_module(
        "tiled",
        &[
            ("load", tiled_load),
            ("info", tiled_info),
            ("stats", tiled_info),
            ("layers", tiled_layers),
            ("tile_at", tiled_tile_at),
            ("solid_grid", tiled_solid_grid),
            ("is_solid", tiled_is_solid),
            ("place_on_terrain", tiled_place_on_terrain),
            ("draw", tiled_draw),
            ("close", tiled_close),
            ("free", tiled_close),
        ],
        &[],
    )
}

fn encode_tilemap_payload(tmap: &TileMap) -> Vec<u8> {
    let mut w = BinWriter::new();
    w.write_u32(tmap.width as u32);
    w.write_u32(tmap.height as u32);
    w.write_u32(tmap.tile_width);
    w.write_u32(tmap.tile_height);
    w.write_u8(if tmap.infinite { 1 } else { 0 });
    w.write_str(&tmap.orientation);

    w.write_u32(tmap.tilesets.len() as u32);
    for ts in &tmap.tilesets {
        w.write_str(&ts.name);
        w.write_u32(ts.first_gid);
        w.write_u32(ts.tile_width);
        w.write_u32(ts.tile_height);
        w.write_u32(ts.tile_count);
        w.write_u32(ts.columns);
        match &ts.image {
            Some(s) => {
                w.write_u8(1);
                w.write_str(s);
            }
            None => w.write_u8(0),
        }
        w.write_u32(ts.solid_tiles.len() as u32);
        for &b in &ts.solid_tiles {
            w.write_u8(if b { 1 } else { 0 });
        }
    }

    w.write_u32(tmap.layers.len() as u32);
    for lay in &tmap.layers {
        w.write_str(&lay.name);
        w.write_str(&lay.layer_type);
        w.write_u32(lay.width as u32);
        w.write_u32(lay.height as u32);
        w.write_f32(lay.offset_x);
        w.write_f32(lay.offset_y);
        w.write_f32(lay.parallax_x);
        w.write_f32(lay.parallax_y);
        w.write_u8(if lay.visible { 1 } else { 0 });
        w.write_f32(lay.opacity);
        w.write_u8(if lay.is_collision { 1 } else { 0 });
        w.write_u32(lay.gids.len() as u32);
        for g in &lay.gids {
            w.write_u32(g.0);
        }
        w.write_u32(lay.chunks.len() as u32);
        for ch in &lay.chunks {
            w.write_u32(ch.x as u32);
            w.write_u32(ch.y as u32);
            w.write_u32(ch.width as u32);
            w.write_u32(ch.height as u32);
            w.write_u32(ch.gids.len() as u32);
            for g in &ch.gids {
                w.write_u32(g.0);
            }
        }
    }
    w.buf
}

fn decode_tilemap_payload(
    payload: &[u8],
    budget_core: Option<&std::sync::Arc<crate::budget::BudgetCore>>,
) -> Result<TileMap, String> {
    let mut r = BinReader::new(payload);
    let width = r.read_u32()? as usize;
    let height = r.read_u32()? as usize;
    let tile_width = r.read_u32()?;
    let tile_height = r.read_u32()?;
    let infinite = r.read_u8()? != 0;
    let orientation = r.read_str()?;

    let ts_len = r.read_u32()? as usize;
    if ts_len > 10_000 {
        return Err("E_TILED_CORRUPT: ASKAS tileset count exceeds limit".to_string());
    }
    let mut tilesets = Vec::with_capacity(ts_len);
    for _ in 0..ts_len {
        let name = r.read_str()?;
        let first_gid = r.read_u32()?;
        let tw = r.read_u32()?;
        let th = r.read_u32()?;
        let tc = r.read_u32()?;
        let cols = r.read_u32()?;
        let image = if r.read_u8()? != 0 {
            Some(r.read_str()?)
        } else {
            None
        };
        let sc = r.read_u32()? as usize;
        if sc > 1_000_000 {
            return Err("E_TILED_CORRUPT: ASKAS solid tile count exceeds limit".to_string());
        }
        let mut solid_tiles = Vec::with_capacity(sc);
        for _ in 0..sc {
            solid_tiles.push(r.read_u8()? != 0);
        }
        tilesets.push(Tileset {
            name,
            first_gid,
            tile_width: tw,
            tile_height: th,
            tile_count: tc,
            columns: cols,
            image,
            solid_tiles,
        });
    }

    let l_len = r.read_u32()? as usize;
    if l_len > 4096 {
        return Err("E_TILED_CORRUPT: ASKAS layer count exceeds limit".to_string());
    }
    let mut layers = Vec::with_capacity(l_len);
    let mut total_cells = 0usize;
    for _ in 0..l_len {
        let name = r.read_str()?;
        let layer_type = r.read_str()?;
        let lw = r.read_u32()? as usize;
        let lh = r.read_u32()? as usize;
        let offset_x = r.read_f32()?;
        let offset_y = r.read_f32()?;
        let parallax_x = r.read_f32()?;
        let parallax_y = r.read_f32()?;
        let visible = r.read_u8()? != 0;
        let opacity = r.read_f32()?;
        let is_collision = r.read_u8()? != 0;
        let gcount = r.read_u32()? as usize;
        total_cells = total_cells.saturating_add(gcount);
        if total_cells > MAX_TILED_CELLS {
            return Err("E_TILED_CORRUPT: ASKAS tilemap cell count exceeds 4M limit".to_string());
        }
        let mut gids = Vec::with_capacity(gcount);
        for _ in 0..gcount {
            gids.push(Gid(r.read_u32()?));
        }
        let ccount = r.read_u32()? as usize;
        if ccount > 100_000 {
            return Err("E_TILED_CORRUPT: ASKAS chunk count exceeds limit".to_string());
        }
        let mut chunks = Vec::with_capacity(ccount);
        for _ in 0..ccount {
            let cx = r.read_u32()? as i32;
            let cy = r.read_u32()? as i32;
            let cw = r.read_u32()? as usize;
            let ch = r.read_u32()? as usize;
            let cg = r.read_u32()? as usize;
            total_cells = total_cells.saturating_add(cg);
            if total_cells > MAX_TILED_CELLS {
                return Err("E_TILED_CORRUPT: ASKAS tilemap chunk cells exceed 4M limit".to_string());
            }
            let mut cgids = Vec::with_capacity(cg);
            for _ in 0..cg {
                cgids.push(Gid(r.read_u32()?));
            }
            chunks.push(TileChunk {
                x: cx,
                y: cy,
                width: cw,
                height: ch,
                gids: cgids,
            });
        }
        layers.push(TileLayer {
            name,
            layer_type,
            width: lw,
            height: lh,
            offset_x,
            offset_y,
            parallax_x,
            parallax_y,
            visible,
            opacity,
            is_collision,
            gids,
            chunks,
        });
    }

    let mut tmap = TileMap {
        width,
        height,
        tile_width,
        tile_height,
        infinite,
        orientation,
        layers,
        tilesets,
        ground_bias: 0.0,
        world_origin: Vec3::zero(),
        budget: None,
    };
    let bytes = tmap.total_gid_bytes();
    tmap.budget = Some(MediaBudgetGuard::charge(budget_core, bytes)?);
    Ok(tmap)
}

fn tiled_load(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("tiled.load() expects (path, [opts])"));
    }
    let raw_path = expect_str(&args[0], "tiled.load()")?;
    let mut use_cache = true;
    if let Some(arg1) = args.get(1) {
        if !matches!(arg1, Value::Unit) {
            let Value::Dict(d) = arg1 else {
                return Err(err_bad_opt("tiled.load() opts must be dict"));
            };
            for (k, v) in d.borrow().iter() {
                match k.as_str() {
                    "cache" => match v {
                        Value::Bool(b) => use_cache = *b,
                        _ => return Err(err_bad_opt("tiled.load() 'cache' must be bool")),
                    },
                    other => return Err(err_bad_opt(format!("tiled.load(): unknown opt '{}'", other))),
                }
            }
        }
    }

    let aref = AssetRef::parse_allow_sandbox_root(raw_path, vm)?;
    let (bytes, _, source_key) = read_asset_ref_bytes(vm, &aref)?;
    let mut cache_key = Vec::with_capacity(64 + source_key.len());
    cache_key.extend_from_slice(b"ask.tiled.v1|");
    cache_key.extend_from_slice(source_key.as_bytes());
    cache_key.push(b'|');
    cache_key.extend_from_slice(&sha256_bytes(&bytes));

    let budget_arc = std::sync::Arc::clone(vm.budget_core());
    if use_cache {
        if let Some(tmap) = try_load_askas(vm, &cache_key, ParserId::Tiled, |payload| {
            decode_tilemap_payload(payload, Some(&budget_arc))
        }) {
            let id = MAPS.with(|b| b.borrow_mut().insert(Rc::new(RefCell::new(tmap)), "TileMap"))?;
            return Ok(new_handle(&map_def(), "TileMap", id));
        }
    }

    let tmap = load_tiled_map(vm, raw_path)?;
    if use_cache {
        let payload = encode_tilemap_payload(&tmap);
        try_store_askas(vm, &cache_key, ParserId::Tiled, &payload);
    }
    let id = MAPS.with(|b| b.borrow_mut().insert(Rc::new(RefCell::new(tmap)), "TileMap"))?;
    Ok(new_handle(&map_def(), "TileMap", id))
}

fn tiled_info(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("tiled.info() expects (tilemap)"));
    }
    let map_rc = extract_map(&args[0], "tiled.info()")?;
    let m = map_rc.borrow();
    vm.charge_alloc(192)?;
    let mut d = new_dict_with_capacity(9);
    d.insert("width".to_string(), Value::Int(m.width as i64));
    d.insert("height".to_string(), Value::Int(m.height as i64));
    d.insert("tile_width".to_string(), Value::Int(m.tile_width as i64));
    d.insert("tile_height".to_string(), Value::Int(m.tile_height as i64));
    d.insert("infinite".to_string(), Value::Bool(m.infinite));
    d.insert("orientation".to_string(), Value::from_str(&m.orientation));
    d.insert("layers".to_string(), Value::Int(m.layers.len() as i64));
    d.insert("tilesets".to_string(), Value::Int(m.tilesets.len() as i64));
    d.insert("ground_bias".to_string(), Value::Float(m.ground_bias as f64));
    Ok(Value::dict(d))
}

fn tiled_solid_info(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("solid_grid.info() expects (solid_grid)"));
    }
    let s = extract_solid(&args[0], "solid_grid.info()")?;
    vm.charge_alloc(128)?;
    let mut d = new_dict_with_capacity(4);
    d.insert("width".to_string(), Value::Int(s.width as i64));
    d.insert("height".to_string(), Value::Int(s.height as i64));
    d.insert("tile_w".to_string(), Value::Int(s.tile_w as i64));
    d.insert("tile_h".to_string(), Value::Int(s.tile_h as i64));
    Ok(Value::dict(d))
}

fn tiled_layers(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("tiled.layers() expects (tilemap)"));
    }
    let map_rc = extract_map(&args[0], "tiled.layers()")?;
    let m = map_rc.borrow();
    vm.charge_alloc(m.layers.len().saturating_mul(160))?;
    let mut out = Vec::with_capacity(m.layers.len());
    for l in &m.layers {
        let mut d = new_dict_with_capacity(9);
        d.insert("name".to_string(), Value::from_str(&l.name));
        d.insert("type".to_string(), Value::from_str(&l.layer_type));
        d.insert("width".to_string(), Value::Int(l.width as i64));
        d.insert("height".to_string(), Value::Int(l.height as i64));
        d.insert("visible".to_string(), Value::Bool(l.visible));
        d.insert("opacity".to_string(), Value::Float(l.opacity as f64));
        d.insert("parallax_x".to_string(), Value::Float(l.parallax_x as f64));
        d.insert("parallax_y".to_string(), Value::Float(l.parallax_y as f64));
        d.insert("is_collision".to_string(), Value::Bool(l.is_collision));
        d.insert("chunk_count".to_string(), Value::Int(l.chunks.len() as i64));
        out.push(Value::dict(d));
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn tiled_tile_at(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 4 {
        return Err(err_bad_opt("tiled.tile_at() expects (tilemap, layer, x, y)"));
    }
    let map_rc = extract_map(&args[0], "tiled.tile_at()")?;
    let m = map_rc.borrow();
    let layer_idx = match &args[1] {
        Value::Int(i) if *i >= 0 => *i as usize,
        Value::Str(name) => m
            .layers
            .iter()
            .position(|l| l.name == name.as_str())
            .ok_or_else(|| err_bad_opt(format!("tiled.tile_at(): layer '{}' not found", name)))?,
        _ => return Err(err_bad_opt("tiled.tile_at() layer must be index int or layer name str")),
    };
    let x = expect_usize(&args[2], "tiled.tile_at() x")?;
    let y = expect_usize(&args[3], "tiled.tile_at() y")?;
    let Some(gid) = m.tile_at(layer_idx, x, y) else {
        return Ok(Value::Unit);
    };
    vm.charge_alloc(96)?;
    let mut d = new_dict_with_capacity(5);
    d.insert("gid".to_string(), Value::Int(gid.0 as i64));
    d.insert("local_id".to_string(), Value::Int(gid.local_id() as i64));
    d.insert("flip_h".to_string(), Value::Bool(gid.flipped_h()));
    d.insert("flip_v".to_string(), Value::Bool(gid.flipped_v()));
    d.insert("flip_diag".to_string(), Value::Bool(gid.flipped_diag()));
    Ok(Value::dict(d))
}

fn tiled_solid_grid(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("tiled.solid_grid() expects (tilemap, [layer_name])"));
    }
    let map_rc = extract_map(&args[0], "tiled.solid_grid()")?;
    let layer_name = match args.get(1) {
        Some(v) if !matches!(v, Value::Unit) => Some(expect_str(v, "tiled.solid_grid()")?),
        _ => None,
    };
    let grid = map_rc
        .borrow()
        .rasterize_solid(layer_name, Some(vm.budget_core()))?;
    register_solid_grid(grid)
}

fn tiled_is_solid(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(err_bad_opt("tiled.is_solid() expects (solid_grid, x, y)"));
    }
    let grid = extract_solid(&args[0], "tiled.is_solid()")?;
    let Value::Int(x) = &args[1] else {
        return Err(err_bad_opt("tiled.is_solid() x must be int"));
    };
    let Value::Int(y) = &args[2] else {
        return Err(err_bad_opt("tiled.is_solid() y must be int"));
    };
    Ok(Value::Bool(grid.is_solid(*x as i32, *y as i32)))
}

fn tiled_place_on_terrain(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err(err_bad_opt("tiled.place_on_terrain() expects (tilemap, heightmap, [bias])"));
    }
    let map_rc = extract_map(&args[0], "tiled.place_on_terrain()")?;
    let hm = extract_heightmap(&args[1], "tiled.place_on_terrain()")?;
    let extra = match args.get(2) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "tiled.place_on_terrain() bias")?,
        _ => 0.0,
    };
    let taabb = hm.world_aabb();
    let mid_x = 0.5 * (taabb.min.x + taabb.max.x);
    let mid_z = 0.5 * (taabb.min.z + taabb.max.z);
    let ground_y = hm.height_at(mid_x, mid_z) + extra;
    let mut m = map_rc.borrow_mut();
    m.ground_bias = ground_y;
    m.world_origin = Vec3::new(taabb.min.x, ground_y, taabb.max.z);
    Ok(Value::Float(ground_y as f64))
}

fn tiled_draw(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 4 {
        return Err(err_bad_opt("tiled.draw() expects (tilemap, [surface], [cam_x, cam_y])"));
    }
    let map_rc = extract_map(&args[0], "tiled.draw()")?;
    let mut target: Option<&Value> = None;
    let mut idx = 1usize;
    if let Some(v) = args.get(1) {
        if is_surface_arg(v) {
            target = Some(v);
            idx = 2;
        } else if matches!(v, Value::Unit) && args.len() > 2 {
            idx = 2;
        }
    }
    let cam_x = match args.get(idx) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "tiled.draw() cam_x")?,
        _ => 0.0,
    };
    let cam_y = match args.get(idx + 1) {
        Some(v) if !matches!(v, Value::Unit) => expect_f32(v, "tiled.draw() cam_y")?,
        _ => 0.0,
    };
    let map_ref = map_rc.borrow();
    gfx::with_target_pixels_mut(target, false, |buf, _depth, bw, bh, clip, _b| {
        render_tilemap_2d(&map_ref, buf, bw, bh, clip, cam_x, cam_y);
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn tiled_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("tiled.close() expects (handle)"));
    }
    if let Ok(id) = extract_handle_id(&args[0], "TileMap", "tiled.close()") {
        return Ok(Value::Bool(MAPS.with(|b| b.borrow_mut().remove(id).is_some())));
    }
    if let Ok(id) = extract_handle_id(&args[0], "SolidGrid", "tiled.close()") {
        return Ok(Value::Bool(SOLIDS.with(|b| b.borrow_mut().remove(id).is_some())));
    }
    Err(err_bad_opt("tiled.close() expects a TileMap or SolidGrid handle"))
}
