use std::sync::{Arc, OnceLock};

use crate::budget::BudgetCore;
use crate::media::audio::synth::lut_cos;
use crate::media::{MediaBudgetGuard, err_bad_opt, err_limit};
use crate::scene::geom::Vec3;
use crate::scene::tiled::SolidGrid;
use crate::surface::ClipRect;

pub const MAX_LIGHTS: usize = 256;
pub const MAX_LIGHTMAP_DIM: usize = 4096;
const FALLOFF_LUT_LEN: usize = 256;
const TILE_SIZE: usize = 16;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    Point2D = 0,
    Point3D = 1,
    Directional = 2,
    Spot = 3,
}

impl LightKind {
    #[allow(dead_code)]
    pub fn from_name(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "point2d" | "point_2d" | "2d" => Ok(Self::Point2D),
            "point" | "point3d" | "point_3d" | "3d" => Ok(Self::Point3D),
            "directional" | "dir" | "sun" => Ok(Self::Directional),
            "spot" | "spotlight" => Ok(Self::Spot),
            other => Err(err_bad_opt(format!(
                "unknown light kind '{}' (expected point2d, point3d, directional, or spot)",
                other
            ))),
        }
    }

    #[allow(dead_code)]
    pub fn name(self) -> &'static str {
        match self {
            Self::Point2D => "point2d",
            Self::Point3D => "point3d",
            Self::Directional => "directional",
            Self::Spot => "spot",
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Falloff {
    Linear = 0,
    Smooth = 1,
    Quadratic = 2,
}

impl Falloff {
    pub fn from_name(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "linear" => Ok(Self::Linear),
            "smooth" | "smoothstep" => Ok(Self::Smooth),
            "quadratic" | "quad" | "inv_sq" => Ok(Self::Quadratic),
            other => Err(err_bad_opt(format!(
                "unknown light falloff '{}' (expected linear, smooth, or quadratic)",
                other
            ))),
        }
    }

    #[allow(dead_code)]
    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Smooth => "smooth",
            Self::Quadratic => "quadratic",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Light {
    pub kind: LightKind,
    pub pos: Vec3,
    pub dir: Vec3,
    pub color: [f32; 3],
    pub intensity: f32,
    pub radius: f32,
    pub falloff: Falloff,
    pub inner_cos: f32,
    pub outer_cos: f32,
    pub cast_shadows: bool,
}

impl Light {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: LightKind,
        pos: Vec3,
        dir: Vec3,
        color: [f32; 3],
        intensity: f32,
        radius: f32,
        falloff: Falloff,
        inner_deg: f32,
        outer_deg: f32,
        cast_shadows: bool,
    ) -> Result<Self, String> {
        if !pos.is_finite() || !dir.is_finite() {
            return Err(err_bad_opt("light position and direction must be finite"));
        }
        if color.iter().any(|c| !c.is_finite() || *c < 0.0 || *c > 16.0) {
            return Err(err_bad_opt("light color channels must be finite in 0.0..=16.0"));
        }
        if !intensity.is_finite() || intensity < 0.0 || intensity > 64.0 {
            return Err(err_bad_opt("light intensity must be finite in 0.0..=64.0"));
        }
        if !radius.is_finite() || radius <= 0.0 || radius > 16384.0 {
            return Err(err_bad_opt("light radius must be finite in (0.0, 16384.0]"));
        }
        let d_norm = if dir.len_sq() > 1e-12 {
            dir.normalize()
        } else {
            Vec3::new(0.0, -1.0, 0.0)
        };
        let in_d = inner_deg.clamp(0.0, 179.0);
        let out_d = outer_deg.clamp(in_d + 0.1, 180.0);
        let inner_cos = lut_cos(in_d / 360.0);
        let outer_cos = lut_cos(out_d / 360.0);
        Ok(Self {
            kind,
            pos,
            dir: d_norm,
            color,
            intensity,
            radius,
            falloff,
            inner_cos,
            outer_cos,
            cast_shadows,
        })
    }
}

/// Precomputed 256-entry falloff tables indexed by quantized `d^2 / r^2`, eliminating per-pixel `sqrt`.
static FALLOFF_LUTS: OnceLock<[[f32; FALLOFF_LUT_LEN]; 3]> = OnceLock::new();

fn falloff_luts() -> &'static [[f32; FALLOFF_LUT_LEN]; 3] {
    FALLOFF_LUTS.get_or_init(|| {
        let mut luts = [[0.0f32; FALLOFF_LUT_LEN]; 3];
        for i in 0..FALLOFF_LUT_LEN {
            let d2_norm = (i as f64) / ((FALLOFF_LUT_LEN - 1) as f64);
            let d = d2_norm.sqrt();
            let t = (1.0 - d).clamp(0.0, 1.0);
            luts[Falloff::Linear as usize][i] = t as f32;
            luts[Falloff::Smooth as usize][i] = (t * t * (3.0 - 2.0 * t)) as f32;
            luts[Falloff::Quadratic as usize][i] = (t * t) as f32;
        }
        luts
    })
}

#[inline]
fn eval_falloff_sq(falloff: Falloff, d2_norm: f32) -> f32 {
    if d2_norm >= 1.0 {
        return 0.0;
    }
    if d2_norm <= 0.0 {
        return 1.0;
    }
    let table = &falloff_luts()[falloff as usize];
    let scaled = d2_norm * ((FALLOFF_LUT_LEN - 1) as f32);
    let idx = (scaled as usize).min(FALLOFF_LUT_LEN - 2);
    let frac = scaled - (idx as f32);
    let a = table[idx];
    let b = table[idx + 1];
    a + (b - a) * frac
}

/// 2D grid DDA raymarch against `SolidGrid`. Returns `false` if blocked by a solid tile before reaching target.
pub fn ray_visible_2d(
    grid: &SolidGrid,
    ox_world: f32,
    oy_world: f32,
    tx_world: f32,
    ty_world: f32,
) -> bool {
    let tw = grid.tile_w.max(1) as f32;
    let th = grid.tile_h.max(1) as f32;
    let x0 = ox_world / tw;
    let y0 = oy_world / th;
    let x1 = tx_world / tw;
    let y1 = ty_world / th;

    let mut cx = x0.floor() as i32;
    let mut cy = y0.floor() as i32;
    let end_x = x1.floor() as i32;
    let end_y = y1.floor() as i32;
    if cx == end_x && cy == end_y {
        return true;
    }

    let dx = x1 - x0;
    let dy = y1 - y0;
    let step_x = if dx > 0.0 { 1 } else if dx < 0.0 { -1 } else { 0 };
    let step_y = if dy > 0.0 { 1 } else if dy < 0.0 { -1 } else { 0 };

    let t_delta_x = if dx.abs() > 1e-6 { (1.0 / dx).abs() } else { f32::INFINITY };
    let t_delta_y = if dy.abs() > 1e-6 { (1.0 / dy).abs() } else { f32::INFINITY };

    let mut t_max_x = if step_x > 0 {
        ((cx as f32 + 1.0) - x0) * t_delta_x
    } else if step_x < 0 {
        (x0 - (cx as f32)) * t_delta_x
    } else {
        f32::INFINITY
    };
    let mut t_max_y = if step_y > 0 {
        ((cy as f32 + 1.0) - y0) * t_delta_y
    } else if step_y < 0 {
        (y0 - (cy as f32)) * t_delta_y
    } else {
        f32::INFINITY
    };

    let max_steps = grid.width.saturating_add(grid.height).min(4096);
    for _ in 0..max_steps {
        if t_max_x < t_max_y {
            cx += step_x;
            t_max_x += t_delta_x;
        } else {
            cy += step_y;
            t_max_y += t_delta_y;
        }
        if cx == end_x && cy == end_y {
            return true;
        }
        if grid.is_solid(cx, cy) {
            return false;
        }
    }
    true
}

#[derive(Debug)]
pub struct Lightmap {
    pub canvas_w: usize,
    pub canvas_h: usize,
    pub scale: usize,
    pub map_w: usize,
    pub map_h: usize,
    pub ambient: [f32; 3],
    pub texels: Vec<[f32; 3]>,
    #[allow(dead_code)]
    pub budget: Option<MediaBudgetGuard>,
}

impl Lightmap {
    pub fn new(
        canvas_w: usize,
        canvas_h: usize,
        scale: usize,
        ambient: [f32; 3],
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        let scale = scale.clamp(1, 4);
        if canvas_w == 0 || canvas_h == 0 || canvas_w > MAX_LIGHTMAP_DIM || canvas_h > MAX_LIGHTMAP_DIM {
            return Err(err_limit(format!(
                "lightmap canvas dimensions {}x{} out of 1..={}",
                canvas_w, canvas_h, MAX_LIGHTMAP_DIM
            )));
        }
        if ambient.iter().any(|c| !c.is_finite() || *c < 0.0 || *c > 4.0) {
            return Err(err_bad_opt("lightmap ambient color must be finite in 0.0..=4.0"));
        }
        let map_w = canvas_w.div_ceil(scale).max(1);
        let map_h = canvas_h.div_ceil(scale).max(1);
        let count = map_w.saturating_mul(map_h);
        let bytes = count.saturating_mul(12);
        let guard = MediaBudgetGuard::charge(budget_core, bytes)?;
        let texels = vec![ambient; count];
        Ok(Self {
            canvas_w,
            canvas_h,
            scale,
            map_w,
            map_h,
            ambient,
            texels,
            budget: Some(guard),
        })
    }

    pub fn clear(&mut self, ambient: Option<[f32; 3]>) -> Result<(), String> {
        if let Some(a) = ambient {
            if a.iter().any(|c| !c.is_finite() || *c < 0.0 || *c > 4.0) {
                return Err(err_bad_opt("lightmap ambient color must be finite in 0.0..=4.0"));
            }
            self.ambient = a;
        }
        self.texels.fill(self.ambient);
        Ok(())
    }

    pub fn apply_lights(
        &mut self,
        lights: &[Light],
        shadows: Option<(&SolidGrid, f32, f32)>,
        cam_x: f32,
        cam_y: f32,
    ) -> Result<(), String> {
        if lights.len() > MAX_LIGHTS {
            return Err(err_limit(format!(
                "light count {} exceeds {} limit",
                lights.len(),
                MAX_LIGHTS
            )));
        }
        let inv_scale = self.scale as f32;
        let half_step = inv_scale * 0.5;
        let tiles_x = self.map_w.div_ceil(TILE_SIZE);
        let tiles_y = self.map_h.div_ceil(TILE_SIZE);

        let mut dir_indices = Vec::new();
        let mut local_boxes: Vec<(usize, usize, usize, usize, usize)> = Vec::new();
        for (li, l) in lights.iter().enumerate() {
            if l.intensity <= 0.0 {
                continue;
            }
            if l.kind == LightKind::Directional {
                dir_indices.push(li);
                continue;
            }
            let sx = l.pos.x - cam_x;
            let sy = l.pos.y - cam_y;
            let r = l.radius;
            let x0_px = (sx - r).floor() as isize;
            let y0_px = (sy - r).floor() as isize;
            let x1_px = (sx + r).ceil() as isize;
            let y1_px = (sy + r).ceil() as isize;
            let tx0 = (x0_px.max(0) as usize / self.scale) / TILE_SIZE;
            let ty0 = (y0_px.max(0) as usize / self.scale) / TILE_SIZE;
            let tx1 = (x1_px.clamp(0, self.canvas_w as isize) as usize / self.scale) / TILE_SIZE;
            let ty1 = (y1_px.clamp(0, self.canvas_h as isize) as usize / self.scale) / TILE_SIZE;
            if tx0 < tiles_x && ty0 < tiles_y {
                local_boxes.push((li, tx0, ty0, tx1.min(tiles_x - 1), ty1.min(tiles_y - 1)));
            }
        }

        if !dir_indices.is_empty() {
            for &li in &dir_indices {
                let l = &lights[li];
                let cr = l.color[0] * l.intensity;
                let cg = l.color[1] * l.intensity;
                let cb = l.color[2] * l.intensity;
                for px in &mut self.texels {
                    px[0] += cr;
                    px[1] += cg;
                    px[2] += cb;
                }
            }
        }

        for ty in 0..tiles_y {
            let my0 = ty * TILE_SIZE;
            let my1 = (my0 + TILE_SIZE).min(self.map_h);
            for tx in 0..tiles_x {
                let mut active = Vec::new();
                for &(li, bx0, by0, bx1, by1) in &local_boxes {
                    if tx >= bx0 && tx <= bx1 && ty >= by0 && ty <= by1 {
                        active.push(li);
                    }
                }
                if active.is_empty() {
                    continue;
                }
                let mx0 = tx * TILE_SIZE;
                let mx1 = (mx0 + TILE_SIZE).min(self.map_w);

                for my in my0..my1 {
                    let wy = (my as f32) * inv_scale + half_step + cam_y;
                    for mx in mx0..mx1 {
                        let wx = (mx as f32) * inv_scale + half_step + cam_x;
                        let mut acc = self.texels[my * self.map_w + mx];
                        for &li in &active {
                            let l = &lights[li];
                            let dx = wx - l.pos.x;
                            let dy = wy - l.pos.y;
                            let dz = if l.kind == LightKind::Point3D { l.pos.z } else { 0.0 };
                            let d2 = dx * dx + dy * dy + dz * dz;
                            let r2 = l.radius * l.radius;
                            if d2 >= r2 {
                                continue;
                            }
                            let mut atten = eval_falloff_sq(l.falloff, d2 / r2);
                            if l.kind == LightKind::Spot {
                                let d2_len = d2.max(1e-8).sqrt();
                                let cos_a = (dx * l.dir.x + dy * l.dir.y) / d2_len;
                                if cos_a <= l.outer_cos {
                                    continue;
                                }
                                let span = (l.inner_cos - l.outer_cos).max(1e-4);
                                let spot_f = ((cos_a - l.outer_cos) / span).clamp(0.0, 1.0);
                                atten *= spot_f;
                            }
                            if atten <= 0.0 {
                                continue;
                            }
                            if l.cast_shadows {
                                if let Some((grid, ox, oy)) = shadows {
                                    if !ray_visible_2d(grid, l.pos.x - ox, l.pos.y - oy, wx - ox, wy - oy) {
                                        continue;
                                    }
                                }
                            }
                            let k = atten * l.intensity;
                            acc[0] += l.color[0] * k;
                            acc[1] += l.color[1] * k;
                            acc[2] += l.color[2] * k;
                        }
                        self.texels[my * self.map_w + mx] = acc;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn composite_surface(
        &self,
        buf: &mut [u32],
        bw: i32,
        bh: i32,
        clip: ClipRect,
        exposure: f32,
    ) {
        if bw <= 0 || bh <= 0 {
            return;
        }
        let exp = if exposure.is_finite() { exposure.clamp(0.0, 8.0) } else { 1.0 };
        let x0 = clip.x0.max(0);
        let y0 = clip.y0.max(0);
        let x1 = clip.x1.min(bw);
        let y1 = clip.y1.min(bh);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let bw_u = bw as usize;
        let inv_scale = 1.0 / (self.scale as f32);

        for py in y0..y1 {
            let fy = ((py as f32) * inv_scale - 0.5).clamp(0.0, (self.map_h - 1) as f32);
            let iy0 = fy.floor() as usize;
            let iy1 = (iy0 + 1).min(self.map_h - 1);
            let ty = fy - (iy0 as f32);
            let row = (py as usize) * bw_u;

            for px in x0..x1 {
                let fx = ((px as f32) * inv_scale - 0.5).clamp(0.0, (self.map_w - 1) as f32);
                let ix0 = fx.floor() as usize;
                let ix1 = (ix0 + 1).min(self.map_w - 1);
                let tx = fx - (ix0 as f32);

                let c00 = self.texels[iy0 * self.map_w + ix0];
                let c10 = self.texels[iy0 * self.map_w + ix1];
                let c01 = self.texels[iy1 * self.map_w + ix0];
                let c11 = self.texels[iy1 * self.map_w + ix1];

                let lr = (c00[0] + (c10[0] - c00[0]) * tx) * (1.0 - ty)
                    + (c01[0] + (c11[0] - c01[0]) * tx) * ty;
                let lg = (c00[1] + (c10[1] - c00[1]) * tx) * (1.0 - ty)
                    + (c01[1] + (c11[1] - c01[1]) * tx) * ty;
                let lb = (c00[2] + (c10[2] - c00[2]) * tx) * (1.0 - ty)
                    + (c01[2] + (c11[2] - c01[2]) * tx) * ty;

                let dst = buf[row + px as usize];
                let dr = ((dst >> 16) & 0xFF) as f32;
                let dg = ((dst >> 8) & 0xFF) as f32;
                let db = (dst & 0xFF) as f32;
                let nr = (dr * lr * exp).round().clamp(0.0, 255.0) as u32;
                let ng = (dg * lg * exp).round().clamp(0.0, 255.0) as u32;
                let nb = (db * lb * exp).round().clamp(0.0, 255.0) as u32;
                buf[row + px as usize] = (dst & 0xFF00_0000) | (nr << 16) | (ng << 8) | nb;
            }
        }
    }
}
