use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::audio::synth::{lut_cos, lut_sin};
use crate::media::formats::png::decode_png_gray_u16;
use crate::media::{MediaBudgetGuard, temp_budget_guard};
use crate::scene::asset::read_path_checked_temp;
use crate::scene::geom::{Aabb, Vec3};
use crate::scene::model::{
    GroupRange, MAX_MESH_TRIS, MAX_MESH_VERTICES, MeshData, MeshStats, SourceFormat,
    verify_mesh_data,
};
use crate::scene::tiled::SolidGrid;
use crate::surface::{ClipRect, DepthBuffer};
use crate::vm::VM;

pub const MAX_TERRAIN_DIM: usize = 4096;
pub const MAX_TERRAIN_CELLS: usize = 4_194_304;
pub const MAX_SPLAT_PIXELS: usize = 16_777_216;

#[derive(Debug)]
pub struct HeightMap {
    pub width: usize,
    pub height: usize,
    pub cell_size: f32,
    pub height_scale: f32,
    pub heights: Vec<f32>,
    #[allow(dead_code)]
    pub budget: Option<MediaBudgetGuard>,
}

impl HeightMap {
    pub fn from_vec(
        width: usize,
        height: usize,
        cell_size: f32,
        height_scale: f32,
        heights: Vec<f32>,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 || width > MAX_TERRAIN_DIM || height > MAX_TERRAIN_DIM {
            return Err(format!(
                "E_TERRAIN_LIMIT: heightmap dimensions {}x{} out of 1..={}",
                width, height, MAX_TERRAIN_DIM
            ));
        }
        let cells = width
            .checked_mul(height)
            .ok_or_else(|| "E_TERRAIN_LIMIT: heightmap cell count overflow".to_string())?;
        if cells > MAX_TERRAIN_CELLS || heights.len() != cells {
            return Err("E_TERRAIN_LIMIT: heightmap exceeds 4M cell limit or size mismatch".to_string());
        }
        if !cell_size.is_finite() || cell_size <= 0.0 {
            return Err("E_TERRAIN_BAD_OPT: heightmap cell_size must be > 0".to_string());
        }
        if !height_scale.is_finite() || height_scale == 0.0 {
            return Err("E_TERRAIN_BAD_OPT: heightmap height_scale must be non-zero finite".to_string());
        }
        if heights.iter().any(|h| !h.is_finite()) {
            return Err("E_TERRAIN_CORRUPT: heightmap contains non-finite value".to_string());
        }
        let byte_len = cells.saturating_mul(std::mem::size_of::<f32>());
        let guard = MediaBudgetGuard::charge(budget_core, byte_len)?;
        Ok(Self {
            width,
            height,
            cell_size,
            height_scale,
            heights,
            budget: Some(guard),
        })
    }

    #[inline]
    pub fn sample_cell(&self, cx: isize, cz: isize) -> f32 {
        let x = cx.clamp(0, self.width as isize - 1) as usize;
        let z = cz.clamp(0, self.height as isize - 1) as usize;
        self.heights[z * self.width + x]
    }

    /// World coordinates `(wx, wz)` -> bilinearly interpolated terrain elevation.
    /// Canonical grid center `(cx, cz)` maps to `wx = (cx + 0.5) * cell_size`, `wz = -(cz + 0.5) * cell_size`.
    pub fn height_at(&self, wx: f32, wz: f32) -> f32 {
        if !wx.is_finite() || !wz.is_finite() {
            return 0.0;
        }
        let gx = (wx / self.cell_size) - 0.5;
        let gz = ((-wz) / self.cell_size) - 0.5;
        let x0 = gx.floor() as isize;
        let z0 = gz.floor() as isize;
        let tx = (gx - x0 as f32).clamp(0.0, 1.0);
        let tz = (gz - z0 as f32).clamp(0.0, 1.0);
        let h00 = self.sample_cell(x0, z0);
        let h10 = self.sample_cell(x0 + 1, z0);
        let h01 = self.sample_cell(x0, z0 + 1);
        let h11 = self.sample_cell(x0 + 1, z0 + 1);
        let top = h00 + (h10 - h00) * tx;
        let bot = h01 + (h11 - h01) * tx;
        top + (bot - top) * tz
    }

    /// Canonical surface normal `normalize(-hx, -hy, 1)` in V-down UV frame so flat surface returns `(0, 0, 1)`.
    pub fn normal_at(&self, wx: f32, wz: f32) -> Vec3 {
        let step = self.cell_size;
        let h_l = self.height_at(wx - step, wz);
        let h_r = self.height_at(wx + step, wz);
        let h_u = self.height_at(wx, wz + step);
        let h_d = self.height_at(wx, wz - step);
        let denom = (2.0 * step).max(1e-6);
        let hx = (h_r - h_l) / denom;
        let hy = (h_d - h_u) / denom;
        Vec3::new(-hx, -hy, 1.0).normalize()
    }

    pub fn slope_deg(&self, wx: f32, wz: f32) -> f32 {
        let n = self.normal_at(wx, wz);
        let c = n.z.clamp(-1.0, 1.0);
        (c.acos() * (180.0 / std::f32::consts::PI)).clamp(0.0, 180.0)
    }

    pub fn world_aabb(&self) -> Aabb {
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for &h in &self.heights {
            if h < min_y {
                min_y = h;
            }
            if h > max_y {
                max_y = h;
            }
        }
        if !min_y.is_finite() {
            min_y = 0.0;
            max_y = 0.0;
        }
        let max_x = (self.width as f32) * self.cell_size;
        let min_z = -((self.height as f32) * self.cell_size);
        Aabb::new(Vec3::new(0.0, min_y, min_z), Vec3::new(max_x, max_y, 0.0))
    }

    pub fn to_solid(&self, threshold: f32, budget_core: Option<&Arc<BudgetCore>>) -> Result<SolidGrid, String> {
        let cells: Vec<bool> = self.heights.iter().map(|&h| h >= threshold).collect();
        SolidGrid::from_cells(
            self.width,
            self.height,
            self.cell_size.max(1.0) as u32,
            self.cell_size.max(1.0) as u32,
            cells,
            budget_core,
        )
    }

    pub fn to_mesh(
        &self,
        step: usize,
        splat: Option<&SplatMap>,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<MeshData, String> {
        let step = step.max(1);
        let cols = (self.width - 1) / step + 1;
        let rows = (self.height - 1) / step + 1;
        if cols < 2 || rows < 2 {
            return Err("E_TERRAIN_BAD_OPT: terrain too small for mesh generation".to_string());
        }
        let v_count = cols * rows;
        let tri_count = (cols - 1) * (rows - 1) * 2;
        if v_count > MAX_MESH_VERTICES || tri_count > MAX_MESH_TRIS {
            return Err("E_TERRAIN_LIMIT: generated terrain mesh exceeds vertex/tri limits".to_string());
        }

        let mut positions = Vec::with_capacity(v_count);
        let mut normals = Vec::with_capacity(v_count);
        let mut uvs = Vec::with_capacity(v_count);
        let mut splat_weights = splat.map(|_| Vec::with_capacity(v_count * 4));

        for r in 0..rows {
            let cz = (r * step).min(self.height - 1);
            let v = (cz as f32) / ((self.height - 1).max(1) as f32);
            let wz = -((cz as f32 + 0.5) * self.cell_size);
            for c in 0..cols {
                let cx = (c * step).min(self.width - 1);
                let u = (cx as f32) / ((self.width - 1).max(1) as f32);
                let wx = (cx as f32 + 0.5) * self.cell_size;
                let h = self.heights[cz * self.width + cx];
                let n = self.normal_at(wx, wz);
                // Convert surface UV normal into world Y-up frame for 3D mesh
                let world_n = Vec3::new(n.x, n.z, -n.y).normalize();
                positions.push([wx, h, wz]);
                normals.push(world_n.to_array());
                uvs.push([u, v]);
                if let (Some(sm), Some(sw)) = (splat, splat_weights.as_mut()) {
                    let w = sm.sample_rgba_u8(u, v);
                    sw.extend_from_slice(&w);
                }
            }
        }

        let mut indices = Vec::with_capacity(tri_count * 3);
        for r in 0..(rows - 1) {
            for c in 0..(cols - 1) {
                let i0 = (r * cols + c) as u32;
                let i1 = (r * cols + c + 1) as u32;
                let i2 = ((r + 1) * cols + c) as u32;
                let i3 = ((r + 1) * cols + c + 1) as u32;
                indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
            }
        }

        let mut mesh = MeshData {
            name: "terrain_mesh".to_string(),
            positions,
            normals,
            uvs,
            colors: None,
            indices,
            groups: vec![GroupRange {
                first_index: 0,
                index_count: (tri_count * 3) as u32,
                material_index: Some(0),
            }],
            splat_weights,
            aabb: Aabb::new(Vec3::zero(), Vec3::zero()),
            stats: MeshStats::default(),
            source: SourceFormat::Generated,
            budget: None,
        };
        let mut warns = Vec::new();
        verify_mesh_data(&mut mesh, &mut warns)?;
        let mem = mesh.memory_bytes();
        mesh.budget = Some(MediaBudgetGuard::charge(budget_core, mem)?);
        Ok(mesh)
    }
}

#[derive(Debug)]
pub struct SplatMap {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
    #[allow(dead_code)]
    pub budget: Option<MediaBudgetGuard>,
}

impl SplatMap {
    pub fn new(
        width: usize,
        height: usize,
        data: Vec<u8>,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        let px = width
            .checked_mul(height)
            .ok_or_else(|| "E_SPLAT_INVALID: splatmap dimensions overflow".to_string())?;
        if px == 0 || px > MAX_SPLAT_PIXELS || data.len() != px * 4 {
            return Err("E_SPLAT_INVALID: splatmap exceeds 16M pixel cap or byte length mismatch".to_string());
        }
        let guard = MediaBudgetGuard::charge(budget_core, data.len())?;
        Ok(Self {
            width,
            height,
            data,
            budget: Some(guard),
        })
    }

    pub fn sample_rgba_u8(&self, u: f32, v: f32) -> [u8; 4] {
        let x = ((u.clamp(0.0, 1.0) * (self.width - 1) as f32).round() as usize).min(self.width - 1);
        let y = ((v.clamp(0.0, 1.0) * (self.height - 1) as f32).round() as usize).min(self.height - 1);
        let idx = (y * self.width + x) * 4;
        [
            self.data[idx],
            self.data[idx + 1],
            self.data[idx + 2],
            self.data[idx + 3],
        ]
    }

    pub fn weights_at(&self, u: f32, v: f32) -> [f32; 4] {
        let raw = self.sample_rgba_u8(u, v);
        let sum = (raw[0] as f32 + raw[1] as f32 + raw[2] as f32 + raw[3] as f32).max(1.0);
        [
            raw[0] as f32 / sum,
            raw[1] as f32 / sum,
            raw[2] as f32 / sum,
            raw[3] as f32 / sum,
        ]
    }
}

pub fn load_heightmap(
    vm: &mut VM,
    raw_path: &str,
    width_opt: Option<usize>,
    height_opt: Option<usize>,
    cell_size: f32,
    height_scale: f32,
) -> Result<HeightMap, String> {
    let (aref, bytes, _, _) = read_path_checked_temp(vm, raw_path)?;
    let _tmp = temp_budget_guard(Some(vm.budget_core()), bytes.len())?;
    let low = aref.as_str().to_ascii_lowercase();
    let budget = Arc::clone(vm.budget_core());

    if low.ends_with(".png") {
        let (pw, ph, u16s) =
            decode_png_gray_u16(&bytes, true, Some(&budget), Some(&|| vm.check_timeout()))?;
        let heights: Vec<f32> = u16s
            .into_iter()
            .map(|v| (v as f32 / 65535.0) * height_scale)
            .collect();
        return HeightMap::from_vec(
            pw as usize,
            ph as usize,
            cell_size,
            height_scale,
            heights,
            Some(&budget),
        );
    }

    if low.ends_with(".r16") || low.ends_with(".raw") {
        if bytes.len() % 2 != 0 || bytes.is_empty() {
            return Err("E_TERRAIN_CORRUPT: .r16 file byte length must be positive and even".to_string());
        }
        let cells = bytes.len() / 2;
        if cells > MAX_TERRAIN_CELLS {
            return Err("E_TERRAIN_LIMIT: heightmap exceeds 4M cell limit".to_string());
        }
        let (w, h) = match (width_opt, height_opt) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) if w > 0 => (w, cells / w),
            _ => {
                let side = (cells as f64).sqrt() as usize;
                if side * side != cells {
                    return Err("E_TERRAIN_BAD_OPT: non-square .r16 heightmap requires width and height options".to_string());
                }
                (side, side)
            }
        };
        if w * h != cells {
            return Err("E_TERRAIN_CORRUPT: .r16 heightmap dimensions do not match byte size".to_string());
        }
        let heights: Vec<f32> = bytes
            .chunks_exact(2)
            .map(|c| {
                let v = u16::from_le_bytes([c[0], c[1]]);
                (v as f32 / 65535.0) * height_scale
            })
            .collect();
        return HeightMap::from_vec(w, h, cell_size, height_scale, heights, Some(&budget));
    }

    Err(format!(
        "E_TERRAIN_BAD_OPT: unsupported terrain heightmap extension '{}' (expected .r16, .raw, or .png)",
        raw_path
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn mode7_draw_surface(
    hm: &HeightMap,
    buf: &mut [u32],
    mut depth: Option<&mut DepthBuffer>,
    bw: i32,
    bh: i32,
    clip: ClipRect,
    tex_pixels: Option<(&[u32], usize, usize)>,
    cam_x: f32,
    cam_y: f32,
    cam_z: f32,
    yaw_rad: f32,
    horizon_px: f32,
    fov_scale: f32,
) {
    if bw <= 0 || bh <= 0 {
        return;
    }
    let y0 = clip.y0.max(0);
    let y1 = clip.y1.min(bh);
    let x0 = clip.x0.max(0);
    let x1 = clip.x1.min(bw);
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    // Use LUT-based deterministic trig for pixel-exact reproducibility across Windows/Linux/macOS.
    let turn = yaw_rad / (2.0 * std::f32::consts::PI);
    let s_yaw = lut_sin(turn);
    let c_yaw = lut_cos(turn);

    let cam_h = cam_y.abs().max(0.5);
    let world_span_x = ((hm.width as f32) * hm.cell_size).max(1.0);
    let world_span_z = ((hm.height as f32) * hm.cell_size).max(1.0);
    let far_clip = 512.0f32;
    let half_w = (bw as f32) * 0.5;
    let focal = (bh as f32) * fov_scale.max(0.1);
    let bw_u = bw as usize;

    for py in y0..y1 {
        let dy = (py as f32) - horizon_px;
        if dy <= 0.5 {
            continue;
        }
        let dist = (cam_h * focal) / dy;
        if !dist.is_finite() || dist > far_clip {
            continue;
        }
        let frag_z = (dist / far_clip).clamp(0.0, 0.9999);
        let dir_fwd_x = -s_yaw;
        let dir_fwd_z = -c_yaw;
        let dir_right_x = c_yaw;
        let dir_right_z = -s_yaw;

        let step_world = dist / focal;
        let base_wx = cam_x + dir_fwd_x * dist;
        let base_wz = cam_z + dir_fwd_z * dist;

        let row = &mut buf[(py as usize) * bw_u..(py as usize + 1) * bw_u];
        for px in x0..x1 {
            if let Some(ref mut db) = depth {
                if !db.test_and_set(px, py, frag_z) {
                    continue;
                }
            }
            let screen_u = (px as f32) - half_w;
            let wx = base_wx + dir_right_x * (screen_u * step_world);
            let wz = base_wz + dir_right_z * (screen_u * step_world);

            let col = if let Some((tpx, tw, th)) = tex_pixels {
                let u = (wx / world_span_x).rem_euclid(1.0);
                let v = ((-wz) / world_span_z).rem_euclid(1.0);
                let tx = ((u * (tw as f32)) as usize).min(tw - 1);
                let ty = ((v * (th as f32)) as usize).min(th - 1);
                tpx[ty * tw + tx]
            } else {
                let elev = (hm.height_at(wx, wz) / hm.height_scale.abs().max(1e-4)).clamp(0.0, 1.0);
                let grid_x = (wx / hm.cell_size).floor() as i32;
                let grid_z = ((-wz) / hm.cell_size).floor() as i32;
                let checker = ((grid_x ^ grid_z) & 1) as u32;
                let base = (40.0 + elev * 180.0) as u32;
                let r = (base + checker * 14).min(255);
                let g = (base + 30 - checker * 8).min(255);
                let b = ((base / 2) + 24).min(255);
                0xFF00_0000 | (r << 16) | (g << 8) | b
            };
            row[px as usize] = col;
        }
    }
}
