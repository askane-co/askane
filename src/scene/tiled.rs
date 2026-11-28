use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::{MediaBudgetGuard, err_bad_opt};
use crate::modules::base64::decode_str as b64_decode;
use crate::modules::compress::{inflate_gzip, inflate_zlib};
use crate::modules::html::{DomNode, parse_xml};
use crate::modules::json::parse_limited;
use crate::scene::asset::{AssetRef, MAX_ASSET_BYTES, read_asset_ref_bytes};
use crate::scene::geom::Vec3;
use crate::surface::{ClipRect, put_pixel_clipped};
use crate::value::Value;
use crate::vm::VM;

pub const MAX_TILED_DIM: usize = 4096;
pub const MAX_TILED_CELLS: usize = 4_000_000;

/// Tiled GID preserving high flip flags (`FLIPPED_H`, `FLIPPED_V`, `FLIPPED_DIAGONAL`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gid(pub u32);

impl Gid {
    pub const FLIP_H: u32 = 0x8000_0000;
    pub const FLIP_V: u32 = 0x4000_0000;
    pub const FLIP_D: u32 = 0x2000_0000;
    pub const ID_MASK: u32 = 0x0FFF_FFFF;

    #[inline]
    pub const fn local_id(self) -> u32 {
        self.0 & Self::ID_MASK
    }

    #[inline]
    pub const fn flipped_h(self) -> bool {
        (self.0 & Self::FLIP_H) != 0
    }

    #[inline]
    pub const fn flipped_v(self) -> bool {
        (self.0 & Self::FLIP_V) != 0
    }

    #[inline]
    pub const fn flipped_diag(self) -> bool {
        (self.0 & Self::FLIP_D) != 0
    }
}

#[derive(Debug)]
pub struct SolidGrid {
    pub width: usize,
    pub height: usize,
    pub tile_w: u32,
    pub tile_h: u32,
    pub cells: Vec<bool>,
    #[allow(dead_code)]
    pub budget: Option<MediaBudgetGuard>,
}

impl SolidGrid {
    pub fn from_cells(
        width: usize,
        height: usize,
        tile_w: u32,
        tile_h: u32,
        cells: Vec<bool>,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 || cells.len() != width * height {
            return Err("E_TILED_CORRUPT: SolidGrid dimension mismatch".to_string());
        }
        if cells.len() > MAX_TILED_CELLS {
            return Err("E_TILED_CORRUPT: SolidGrid exceeds 4M cell limit".to_string());
        }
        let guard = MediaBudgetGuard::charge(budget_core, cells.len())?;
        Ok(Self {
            width,
            height,
            tile_w: tile_w.max(1),
            tile_h: tile_h.max(1),
            cells,
            budget: Some(guard),
        })
    }

    #[inline]
    pub fn is_solid(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || (x as usize) >= self.width || (y as usize) >= self.height {
            return true;
        }
        self.cells[(y as usize) * self.width + (x as usize)]
    }
}

#[derive(Clone, Debug)]
pub struct TileChunk {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
    pub gids: Vec<Gid>,
}

#[derive(Clone, Debug)]
pub struct TileLayer {
    pub name: String,
    pub layer_type: String,
    pub width: usize,
    pub height: usize,
    pub offset_x: f32,
    pub offset_y: f32,
    pub parallax_x: f32,
    pub parallax_y: f32,
    pub visible: bool,
    pub opacity: f32,
    pub is_collision: bool,
    pub gids: Vec<Gid>,
    pub chunks: Vec<TileChunk>,
}

#[derive(Clone, Debug)]
pub struct Tileset {
    pub name: String,
    pub first_gid: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub tile_count: u32,
    pub columns: u32,
    pub image: Option<String>,
    pub solid_tiles: Vec<bool>,
}

#[derive(Debug)]
pub struct TileMap {
    pub width: usize,
    pub height: usize,
    pub tile_width: u32,
    pub tile_height: u32,
    pub infinite: bool,
    pub orientation: String,
    pub layers: Vec<TileLayer>,
    pub tilesets: Vec<Tileset>,
    pub ground_bias: f32,
    pub world_origin: Vec3,
    pub budget: Option<MediaBudgetGuard>,
}

impl TileMap {
    pub fn total_gid_bytes(&self) -> usize {
        self.layers
            .iter()
            .map(|l| {
                l.gids.len() * 4 + l.chunks.iter().map(|c| c.gids.len() * 4).sum::<usize>()
            })
            .sum::<usize>()
            .saturating_add(128)
    }

    pub fn tile_at(&self, layer_idx: usize, x: usize, y: usize) -> Option<Gid> {
        let lay = self.layers.get(layer_idx)?;
        if !lay.chunks.is_empty() {
            for ch in &lay.chunks {
                let cx0 = ch.x;
                let cy0 = ch.y;
                let xi = x as i32;
                let yi = y as i32;
                if xi >= cx0
                    && yi >= cy0
                    && (xi - cx0) < (ch.width as i32)
                    && (yi - cy0) < (ch.height as i32)
                {
                    let lx = (xi - cx0) as usize;
                    let ly = (yi - cy0) as usize;
                    return ch.gids.get(ly * ch.width + lx).copied();
                }
            }
            None
        } else if x < lay.width && y < lay.height {
            lay.gids.get(y * lay.width + x).copied()
        } else {
            None
        }
    }

    pub fn rasterize_solid(
        &self,
        layer_name: Option<&str>,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<SolidGrid, String> {
        let w = self.width.max(1);
        let h = self.height.max(1);
        if w * h > MAX_TILED_CELLS {
            return Err("E_TILED_CORRUPT: tilemap grid exceeds 4M cell limit".to_string());
        }
        let mut cells = vec![false; w * h];
        for lay in &self.layers {
            if lay.layer_type != "tilelayer" || !lay.visible {
                continue;
            }
            if let Some(target_name) = layer_name {
                if lay.name != target_name {
                    continue;
                }
            } else if (lay.parallax_x - 1.0).abs() > 1e-3 || (lay.parallax_y - 1.0).abs() > 1e-3 {
                // Skip non-world parallax layers for world collision rasterization.
                continue;
            }
            let force_solid = lay.is_collision || layer_name == Some(lay.name.as_str());
            let mut apply_gid = |gx: i32, gy: i32, gid: Gid| {
                if gx < 0 || gy < 0 || (gx as usize) >= w || (gy as usize) >= h {
                    return;
                }
                let raw_id = gid.local_id();
                if raw_id == 0 {
                    return;
                }
                let idx = (gy as usize) * w + (gx as usize);
                if force_solid {
                    cells[idx] = true;
                } else {
                    // Check tileset collision flag if defined
                    for ts in self.tilesets.iter().rev() {
                        if raw_id >= ts.first_gid {
                            let local = (raw_id - ts.first_gid) as usize;
                            if ts.solid_tiles.get(local).copied().unwrap_or(false) {
                                cells[idx] = true;
                            }
                            break;
                        }
                    }
                }
            };

            if !lay.chunks.is_empty() {
                for ch in &lay.chunks {
                    for ly in 0..ch.height {
                        for lx in 0..ch.width {
                            if let Some(&gid) = ch.gids.get(ly * ch.width + lx) {
                                apply_gid(ch.x + lx as i32, ch.y + ly as i32, gid);
                            }
                        }
                    }
                }
            } else {
                for ly in 0..lay.height {
                    for lx in 0..lay.width {
                        if let Some(&gid) = lay.gids.get(ly * lay.width + lx) {
                            apply_gid(lx as i32, ly as i32, gid);
                        }
                    }
                }
            }
        }
        SolidGrid::from_cells(w, h, self.tile_width, self.tile_height, cells, budget_core)
    }
}

fn decode_tile_data_str(
    vm: &mut VM,
    raw_str: &str,
    encoding: &str,
    compression: &str,
    expected_cells: usize,
) -> Result<Vec<Gid>, String> {
    if expected_cells > MAX_TILED_CELLS {
        return Err("E_TILED_CORRUPT: layer cell count exceeds 4M limit".to_string());
    }
    match encoding {
        "csv" => {
            let mut gids = Vec::with_capacity(expected_cells);
            for tok in raw_str.split(',') {
                let s = tok.trim();
                if s.is_empty() {
                    continue;
                }
                let v: u32 = s
                    .parse()
                    .map_err(|_| format!("E_TILED_CORRUPT: invalid CSV gid '{}'", s))?;
                gids.push(Gid(v));
                if gids.len() > MAX_TILED_CELLS {
                    return Err("E_TILED_CORRUPT: CSV layer exceeds 4M cell limit".to_string());
                }
            }
            Ok(gids)
        }
        "base64" => {
            let compressed = b64_decode(raw_str, false)
                .map_err(|e| format!("E_TILED_CORRUPT: invalid base64 tile data: {}", e))?;
            let max_bytes = expected_cells.max(1).saturating_mul(4).min(MAX_ASSET_BYTES);
            let raw = match compression {
                "" | "none" => compressed,
                "zlib" => inflate_zlib(&compressed, max_bytes, Some(vm))
                    .map_err(|e| format!("E_TILED_CORRUPT: zlib tile layer: {}", e))?,
                "gzip" => inflate_gzip(&compressed, max_bytes, Some(vm))
                    .map_err(|e| format!("E_TILED_CORRUPT: gzip tile layer: {}", e))?,
                "zstd" => {
                    return Err(
                        "E_TILED_UNSUPPORTED: Tiled zstd tile compression is not supported in 0.20 (use zlib, gzip, or csv)"
                            .to_string(),
                    );
                }
                other => {
                    return Err(format!(
                        "E_TILED_UNSUPPORTED: unknown Tiled compression '{}'",
                        other
                    ));
                }
            };
            if raw.len() % 4 != 0 {
                return Err("E_TILED_CORRUPT: decoded base64 tile payload not 4-byte aligned".to_string());
            }
            let count = raw.len() / 4;
            if count > MAX_TILED_CELLS {
                return Err("E_TILED_CORRUPT: decoded tile layer exceeds 4M cells".to_string());
            }
            Ok(raw
                .chunks_exact(4)
                .map(|c| Gid(u32::from_le_bytes(c.try_into().unwrap())))
                .collect())
        }
        other => Err(format!(
            "E_TILED_UNSUPPORTED: unsupported Tiled encoding '{}'",
            other
        )),
    }
}

fn j_usize(v: Option<&Value>, def: usize, ctx: &str) -> Result<usize, String> {
    let Some(val) = v else { return Ok(def); };
    match val {
        Value::Int(i) if *i >= 0 => Ok(*i as usize),
        Value::Float(f) if f.is_finite() && *f >= 0.0 => Ok(*f as usize),
        Value::Unit => Ok(def),
        _ => Err(format!("E_TILED_CORRUPT: {} must be integer", ctx)),
    }
}

fn j_i32(v: Option<&Value>, def: i32, ctx: &str) -> Result<i32, String> {
    let Some(val) = v else { return Ok(def); };
    match val {
        Value::Int(i) => Ok(*i as i32),
        Value::Float(f) if f.is_finite() => Ok(*f as i32),
        Value::Unit => Ok(def),
        _ => Err(format!("E_TILED_CORRUPT: {} must be integer", ctx)),
    }
}

fn j_f32(v: Option<&Value>, def: f32, ctx: &str) -> Result<f32, String> {
    let Some(val) = v else { return Ok(def); };
    match val {
        Value::Int(i) => Ok(*i as f32),
        Value::Float(f) if f.is_finite() => Ok(*f as f32),
        Value::Unit => Ok(def),
        _ => Err(format!("E_TILED_CORRUPT: {} must be number", ctx)),
    }
}

fn parse_tsj_dict(
    map: &crate::value::DictMap,
    base_ref: &AssetRef,
    first_gid: u32,
) -> Result<Tileset, String> {
    let name = map
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("tileset")
        .to_string();
    let tw = j_usize(map.get("tilewidth"), 32, "tilewidth")?.clamp(1, MAX_TILED_DIM) as u32;
    let th = j_usize(map.get("tileheight"), 32, "tileheight")?.clamp(1, MAX_TILED_DIM) as u32;
    let tile_count = j_usize(map.get("tilecount"), 0, "tilecount")? as u32;
    let columns = j_usize(map.get("columns"), 1, "columns")?.min(MAX_TILED_DIM) as u32;
    let image = if let Some(im) = map.get("image").and_then(|v| v.as_str()) {
        if im.is_empty() {
            None
        } else {
            Some(base_ref.join_relative(im)?.as_str().to_string())
        }
    } else {
        None
    };
    let mut solid_tiles = vec![false; tile_count as usize];
    if let Some(Value::List(tiles_val)) = map.get("tiles") {
        for tv in tiles_val.borrow().iter() {
            if let Value::Dict(td) = tv {
                let tmap = td.borrow();
                let id = j_usize(tmap.get("id"), 0, "tile.id")?;
                let mut is_col = false;
                if tmap.contains_key("objectgroup") {
                    is_col = true;
                }
                if let Some(Value::List(props)) = tmap.get("properties") {
                    for pv in props.borrow().iter() {
                        if let Value::Dict(pd) = pv {
                            let pm = pd.borrow();
                            let pname = pm.get("name").and_then(|x| x.as_str()).unwrap_or("");
                            if pname == "solid" || pname == "collision" {
                                if matches!(pm.get("value"), Some(Value::Bool(true))) {
                                    is_col = true;
                                }
                            }
                        }
                    }
                }
                if id >= solid_tiles.len() && id < 65536 {
                    solid_tiles.resize(id + 1, false);
                }
                if id < solid_tiles.len() && is_col {
                    solid_tiles[id] = true;
                }
            }
        }
    }
    Ok(Tileset {
        name,
        first_gid,
        tile_width: tw,
        tile_height: th,
        tile_count,
        columns,
        image,
        solid_tiles,
    })
}

fn parse_tsx_dom(
    node: &DomNode,
    base_ref: &AssetRef,
    first_gid: u32,
) -> Result<Tileset, String> {
    let name = node
        .attrs
        .get("name")
        .cloned()
        .unwrap_or_else(|| "tileset".to_string());
    let tw = node
        .attrs
        .get("tilewidth")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(32)
        .clamp(1, MAX_TILED_DIM as u32);
    let th = node
        .attrs
        .get("tileheight")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(32)
        .clamp(1, MAX_TILED_DIM as u32);
    let tile_count = node
        .attrs
        .get("tilecount")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let columns = node
        .attrs
        .get("columns")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(1)
        .min(MAX_TILED_DIM as u32);
    let mut image = None;
    let mut solid_tiles = vec![false; tile_count as usize];

    for ch in &node.children {
        if ch.tag == "image" {
            if let Some(src) = ch.attrs.get("source") {
                if !src.is_empty() {
                    image = Some(base_ref.join_relative(src)?.as_str().to_string());
                }
            }
        } else if ch.tag == "tile" {
            if let Some(id) = ch.attrs.get("id").and_then(|s| s.parse::<usize>().ok()) {
                let mut is_col = false;
                for sub in &ch.children {
                    if sub.tag == "objectgroup" {
                        is_col = true;
                    } else if sub.tag == "properties" {
                        for prop in &sub.children {
                            if prop.tag == "property" {
                                let pname = prop.attrs.get("name").map(|s| s.as_str()).unwrap_or("");
                                let pval = prop.attrs.get("value").map(|s| s.as_str()).unwrap_or("");
                                if (pname == "solid" || pname == "collision") && pval == "true" {
                                    is_col = true;
                                }
                            }
                        }
                    }
                }
                if id >= solid_tiles.len() && id < 65536 {
                    solid_tiles.resize(id + 1, false);
                }
                if id < solid_tiles.len() && is_col {
                    solid_tiles[id] = true;
                }
            }
        }
    }

    Ok(Tileset {
        name,
        first_gid,
        tile_width: tw,
        tile_height: th,
        tile_count,
        columns,
        image,
        solid_tiles,
    })
}

pub fn load_tmj_map(vm: &mut VM, map_ref: &AssetRef, raw_bytes: &[u8]) -> Result<TileMap, String> {
    let text = std::str::from_utf8(raw_bytes)
        .map_err(|_| "E_TILED_CORRUPT: TMJ file is not valid UTF-8".to_string())?;
    let val = parse_limited(text, 100_000, 128, 4 * 1024 * 1024, Some(&|| vm.check_timeout()))?;
    let Value::Dict(root_rc) = val else {
        return Err("E_TILED_CORRUPT: TMJ root must be object".to_string());
    };
    let root = root_rc.borrow();

    let orientation = root
        .get("orientation")
        .and_then(|v| v.as_str())
        .unwrap_or("orthogonal")
        .to_string();
    if orientation != "orthogonal" {
        return Err(format!(
            "E_TILED_UNSUPPORTED: Tiled orientation '{}' is not supported in 0.20 (orthogonal only)",
            orientation
        ));
    }

    let width = j_usize(root.get("width"), 0, "width")?;
    let height = j_usize(root.get("height"), 0, "height")?;
    if width > MAX_TILED_DIM || height > MAX_TILED_DIM {
        return Err("E_TILED_CORRUPT: map dimensions exceed 4096".to_string());
    }
    let tile_width = j_usize(root.get("tilewidth"), 32, "tilewidth")?.clamp(1, MAX_TILED_DIM) as u32;
    let tile_height = j_usize(root.get("tileheight"), 32, "tileheight")?.clamp(1, MAX_TILED_DIM) as u32;
    let infinite = matches!(root.get("infinite"), Some(Value::Bool(true)));

    let mut tilesets = Vec::new();
    if let Some(Value::List(ts_list)) = root.get("tilesets") {
        for ts_val in ts_list.borrow().iter() {
            let Value::Dict(ts_rc) = ts_val else {
                return Err("E_TILED_CORRUPT: tileset entry must be object".to_string());
            };
            let ts_map = ts_rc.borrow();
            let first_gid = j_usize(ts_map.get("firstgid"), 1, "tileset.firstgid")? as u32;
            if let Some(src_path) = ts_map.get("source").and_then(|v| v.as_str()) {
                let child = map_ref.join_relative(src_path)?;
                let (ext_bytes, _, _) = read_asset_ref_bytes(vm, &child)?;
                let ext_txt = std::str::from_utf8(&ext_bytes)
                    .map_err(|_| "E_TILED_CORRUPT: external tileset is not valid UTF-8".to_string())?;
                if child.as_str().to_ascii_lowercase().ends_with(".tsx") {
                    let dom = parse_xml(vm, ext_txt)?;
                    let ts_node = dom
                        .children
                        .iter()
                        .find(|c| c.tag == "tileset")
                        .ok_or_else(|| "E_TILED_CORRUPT: TSX missing <tileset> root".to_string())?;
                    tilesets.push(parse_tsx_dom(ts_node, &child, first_gid)?);
                } else {
                    let ext_val = parse_limited(
                        ext_txt,
                        100_000,
                        128,
                        4 * 1024 * 1024,
                        Some(&|| vm.check_timeout()),
                    )?;
                    let Value::Dict(ext_rc) = ext_val else {
                        return Err("E_TILED_CORRUPT: TSJ root must be object".to_string());
                    };
                    tilesets.push(parse_tsj_dict(&ext_rc.borrow(), &child, first_gid)?);
                }
            } else {
                tilesets.push(parse_tsj_dict(&ts_map, map_ref, first_gid)?);
            }
        }
    }
    tilesets.sort_by_key(|t| t.first_gid);

    let mut layers = Vec::new();
    if let Some(Value::List(l_list)) = root.get("layers") {
        for lv in l_list.borrow().iter() {
            let Value::Dict(ld_rc) = lv else {
                return Err("E_TILED_CORRUPT: layer entry must be object".to_string());
            };
            let ld = ld_rc.borrow();
            let name = ld
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("layer")
                .to_string();
            let layer_type = ld
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("tilelayer")
                .to_string();
            let lw = j_usize(ld.get("width"), width, "layer.width")?;
            let lh = j_usize(ld.get("height"), height, "layer.height")?;
            let offset_x = j_f32(ld.get("offsetx"), 0.0, "layer.offsetx")?;
            let offset_y = j_f32(ld.get("offsety"), 0.0, "layer.offsety")?;
            let parallax_x = j_f32(ld.get("parallaxx"), 1.0, "layer.parallaxx")?;
            let parallax_y = j_f32(ld.get("parallaxy"), 1.0, "layer.parallaxy")?;
            let visible = !matches!(ld.get("visible"), Some(Value::Bool(false)));
            let opacity = j_f32(ld.get("opacity"), 1.0, "layer.opacity")?;
            let encoding = ld
                .get("encoding")
                .and_then(|v| v.as_str())
                .unwrap_or("json");
            let compression = ld
                .get("compression")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_collision = name.to_ascii_lowercase().contains("collision")
                || name.to_ascii_lowercase().contains("solid");

            let mut gids = Vec::new();
            let mut chunks = Vec::new();

            if let Some(Value::List(chunk_list)) = ld.get("chunks") {
                for chv in chunk_list.borrow().iter() {
                    let Value::Dict(ch_rc) = chv else {
                        return Err("E_TILED_CORRUPT: chunk must be object".to_string());
                    };
                    let chd = ch_rc.borrow();
                    let cx = j_i32(chd.get("x"), 0, "chunk.x")?;
                    let cy = j_i32(chd.get("y"), 0, "chunk.y")?;
                    let cw = j_usize(chd.get("width"), 16, "chunk.width")?.clamp(1, 1024);
                    let ch_h = j_usize(chd.get("height"), 16, "chunk.height")?.clamp(1, 1024);
                    let cgids = match chd.get("data") {
                        Some(Value::List(arr)) => arr
                            .borrow()
                            .iter()
                            .map(|x| j_usize(Some(x), 0, "chunk.data[]").map(|u| Gid(u as u32)))
                            .collect::<Result<Vec<_>, _>>()?,
                        Some(Value::Str(s)) => {
                            decode_tile_data_str(vm, s, encoding, compression, cw * ch_h)?
                        }
                        _ => Vec::new(),
                    };
                    chunks.push(TileChunk {
                        x: cx,
                        y: cy,
                        width: cw,
                        height: ch_h,
                        gids: cgids,
                    });
                }
                chunks.sort_by_key(|c| (c.y, c.x));
            } else if layer_type == "tilelayer" {
                match ld.get("data") {
                    Some(Value::List(arr)) => {
                        let items = arr.borrow();
                        if items.len() > MAX_TILED_CELLS {
                            return Err("E_TILED_CORRUPT: layer data exceeds 4M cells".to_string());
                        }
                        gids.reserve(items.len());
                        for item in items.iter() {
                            gids.push(Gid(j_usize(Some(item), 0, "layer.data[]")? as u32));
                        }
                    }
                    Some(Value::Str(s)) => {
                        gids = decode_tile_data_str(vm, s, encoding, compression, lw * lh)?;
                    }
                    _ => {}
                }
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
    tmap.budget = Some(MediaBudgetGuard::charge(Some(vm.budget_core()), bytes)?);
    Ok(tmap)
}

pub fn load_tmx_map(vm: &mut VM, map_ref: &AssetRef, raw_bytes: &[u8]) -> Result<TileMap, String> {
    let text = std::str::from_utf8(raw_bytes)
        .map_err(|_| "E_TILED_CORRUPT: TMX file is not valid UTF-8".to_string())?;
    let dom = parse_xml(vm, text)?;
    let map_node = dom
        .children
        .iter()
        .find(|c| c.tag == "map")
        .ok_or_else(|| "E_TILED_CORRUPT: TMX missing <map> root".to_string())?;

    let orientation = map_node
        .attrs
        .get("orientation")
        .cloned()
        .unwrap_or_else(|| "orthogonal".to_string());
    if orientation != "orthogonal" {
        return Err(format!(
            "E_TILED_UNSUPPORTED: Tiled orientation '{}' is not supported in 0.20 (orthogonal only)",
            orientation
        ));
    }

    let width = map_node
        .attrs
        .get("width")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let height = map_node
        .attrs
        .get("height")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    if width > MAX_TILED_DIM || height > MAX_TILED_DIM {
        return Err("E_TILED_CORRUPT: TMX dimensions exceed 4096".to_string());
    }
    let tile_width = map_node
        .attrs
        .get("tilewidth")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(32)
        .clamp(1, MAX_TILED_DIM as u32);
    let tile_height = map_node
        .attrs
        .get("tileheight")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(32)
        .clamp(1, MAX_TILED_DIM as u32);
    let infinite = map_node.attrs.get("infinite").is_some_and(|s| s == "1" || s == "true");

    let mut tilesets = Vec::new();
    let mut layers = Vec::new();

    for ch in &map_node.children {
        if ch.tag == "tileset" {
            let first_gid = ch
                .attrs
                .get("firstgid")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(1);
            if let Some(src_path) = ch.attrs.get("source") {
                let child = map_ref.join_relative(src_path)?;
                let (ext_bytes, _, _) = read_asset_ref_bytes(vm, &child)?;
                let ext_txt = std::str::from_utf8(&ext_bytes)
                    .map_err(|_| "E_TILED_CORRUPT: external TSX is not valid UTF-8".to_string())?;
                if child.as_str().to_ascii_lowercase().ends_with(".tsj") {
                    let ext_val = parse_limited(
                        ext_txt,
                        100_000,
                        128,
                        4 * 1024 * 1024,
                        Some(&|| vm.check_timeout()),
                    )?;
                    let Value::Dict(ext_rc) = ext_val else {
                        return Err("E_TILED_CORRUPT: TSJ root must be object".to_string());
                    };
                    tilesets.push(parse_tsj_dict(&ext_rc.borrow(), &child, first_gid)?);
                } else {
                    let tsx_dom = parse_xml(vm, ext_txt)?;
                    let ts_node = tsx_dom
                        .children
                        .iter()
                        .find(|c| c.tag == "tileset")
                        .ok_or_else(|| "E_TILED_CORRUPT: TSX missing <tileset>".to_string())?;
                    tilesets.push(parse_tsx_dom(ts_node, &child, first_gid)?);
                }
            } else {
                tilesets.push(parse_tsx_dom(ch, map_ref, first_gid)?);
            }
        } else if ch.tag == "layer" {
            let name = ch
                .attrs
                .get("name")
                .cloned()
                .unwrap_or_else(|| "layer".to_string());
            let lw = ch
                .attrs
                .get("width")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(width);
            let lh = ch
                .attrs
                .get("height")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(height);
            let offset_x = ch
                .attrs
                .get("offsetx")
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(0.0);
            let offset_y = ch
                .attrs
                .get("offsety")
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(0.0);
            let parallax_x = ch
                .attrs
                .get("parallaxx")
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(1.0);
            let parallax_y = ch
                .attrs
                .get("parallaxy")
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(1.0);
            let visible = ch.attrs.get("visible").is_none_or(|s| s != "0" && s != "false");
            let opacity = ch
                .attrs
                .get("opacity")
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(1.0);
            let is_collision = name.to_ascii_lowercase().contains("collision")
                || name.to_ascii_lowercase().contains("solid");

            let mut gids = Vec::new();
            let mut chunks = Vec::new();
            for data_node in &ch.children {
                if data_node.tag != "data" {
                    continue;
                }
                let encoding = data_node
                    .attrs
                    .get("encoding")
                    .map(|s| s.as_str())
                    .unwrap_or("xml");
                let compression = data_node
                    .attrs
                    .get("compression")
                    .map(|s| s.as_str())
                    .unwrap_or("");
                let sub_chunks: Vec<&DomNode> = data_node
                    .children
                    .iter()
                    .filter(|c| c.tag == "chunk")
                    .collect();
                if !sub_chunks.is_empty() {
                    for ch_n in sub_chunks {
                        let cx = ch_n.attrs.get("x").and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
                        let cy = ch_n.attrs.get("y").and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
                        let cw = ch_n
                            .attrs
                            .get("width")
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(16)
                            .clamp(1, 1024);
                        let ch_h = ch_n
                            .attrs
                            .get("height")
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(16)
                            .clamp(1, 1024);
                        let cgids = if encoding == "xml" {
                            ch_n.children
                                .iter()
                                .filter(|c| c.tag == "tile")
                                .map(|t| {
                                    Gid(t
                                        .attrs
                                        .get("gid")
                                        .and_then(|s| s.parse::<u32>().ok())
                                        .unwrap_or(0))
                                })
                                .collect()
                        } else {
                            decode_tile_data_str(vm, &ch_n.text, encoding, compression, cw * ch_h)?
                        };
                        chunks.push(TileChunk {
                            x: cx,
                            y: cy,
                            width: cw,
                            height: ch_h,
                            gids: cgids,
                        });
                    }
                    chunks.sort_by_key(|c| (c.y, c.x));
                } else if encoding == "xml" {
                    for tile_n in &data_node.children {
                        if tile_n.tag == "tile" {
                            let g = tile_n
                                .attrs
                                .get("gid")
                                .and_then(|s| s.parse::<u32>().ok())
                                .unwrap_or(0);
                            gids.push(Gid(g));
                        }
                    }
                } else {
                    gids = decode_tile_data_str(vm, &data_node.text, encoding, compression, lw * lh)?;
                }
            }

            layers.push(TileLayer {
                name,
                layer_type: "tilelayer".to_string(),
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
    }

    tilesets.sort_by_key(|t| t.first_gid);
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
    tmap.budget = Some(MediaBudgetGuard::charge(Some(vm.budget_core()), bytes)?);
    Ok(tmap)
}

pub fn load_tiled_map(vm: &mut VM, raw_path: &str) -> Result<TileMap, String> {
    let aref = AssetRef::parse_allow_sandbox_root(raw_path, vm)?;
    let (bytes, _, _) = read_asset_ref_bytes(vm, &aref)?;
    let low = aref.as_str().to_ascii_lowercase();
    if low.ends_with(".tmx") {
        load_tmx_map(vm, &aref, &bytes)
    } else if low.ends_with(".tmj") || low.ends_with(".json") {
        load_tmj_map(vm, &aref, &bytes)
    } else {
        // Autodetect XML vs JSON
        let trimmed = bytes
            .iter()
            .copied()
            .find(|b| !b.is_ascii_whitespace())
            .unwrap_or(0);
        if trimmed == b'<' {
            load_tmx_map(vm, &aref, &bytes)
        } else if trimmed == b'{' {
            load_tmj_map(vm, &aref, &bytes)
        } else {
            Err(err_bad_opt(format!(
                "tiled.load() unsupported format '{}' (expected .tmj or .tmx)",
                raw_path
            )))
        }
    }
}

/// Deterministic tilemap visualization draw onto an `0xAARRGGBB` surface.
pub fn render_tilemap_2d(
    tmap: &TileMap,
    buf: &mut [u32],
    bw: i32,
    bh: i32,
    clip: ClipRect,
    cam_x: f32,
    cam_y: f32,
) {
    let tw = tmap.tile_width as i32;
    let th = tmap.tile_height as i32;
    if bw <= 0 || bh <= 0 || tw <= 0 || th <= 0 {
        return;
    }
    for (li, lay) in tmap.layers.iter().enumerate() {
        if lay.layer_type != "tilelayer" || !lay.visible {
            continue;
        }
        let ox = (lay.offset_x - cam_x * lay.parallax_x).round() as i32;
        let oy = (lay.offset_y - cam_y * lay.parallax_y).round() as i32;
        let mut draw_cell = |tx: i32, ty: i32, gid: Gid| {
            let lid = gid.local_id();
            if lid == 0 {
                return;
            }
            let px0 = tx * tw + ox;
            let py0 = ty * th + oy;
            if px0 >= clip.x1 || py0 >= clip.y1 || px0 + tw <= clip.x0 || py0 + th <= clip.y0 {
                return;
            }
            // Deterministic tile palette color based on layer & local GID
            let h = lid.wrapping_mul(2654435761).wrapping_add((li as u32) * 1013);
            let r = 60 + ((h >> 16) & 0x7F);
            let g = 60 + ((h >> 8) & 0x7F);
            let b = 60 + (h & 0x7F);
            let col = 0xFF00_0000 | (r << 16) | (g << 8) | b;
            let x_start = px0.max(clip.x0);
            let y_start = py0.max(clip.y0);
            let x_end = (px0 + tw).min(clip.x1);
            let y_end = (py0 + th).min(clip.y1);
            for py in y_start..y_end {
                for px in x_start..x_end {
                    put_pixel_clipped(buf, bw, clip, px, py, col);
                }
            }
        };

        if !lay.chunks.is_empty() {
            for ch in &lay.chunks {
                for cy in 0..ch.height {
                    for cx in 0..ch.width {
                        if let Some(&gid) = ch.gids.get(cy * ch.width + cx) {
                            draw_cell(ch.x + cx as i32, ch.y + cy as i32, gid);
                        }
                    }
                }
            }
        } else {
            for cy in 0..lay.height {
                for cx in 0..lay.width {
                    if let Some(&gid) = lay.gids.get(cy * lay.width + cx) {
                        draw_cell(cx as i32, cy as i32, gid);
                    }
                }
            }
        }
    }
}
