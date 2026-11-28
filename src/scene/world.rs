use std::cell::RefCell;
use std::rc::Rc;

use crate::media::audio::synth::{lut_cos, lut_sin};
use crate::scene::cubemap::Cubemap;
use crate::scene::geom::{Aabb, Transform, Vec3};
use crate::scene::lighting::{Light, LightKind};
use crate::scene::model::{MAX_SCENE_NODES, Model};
use crate::scene::nav::NavGrid;
use crate::scene::terrain::{HeightMap, mode7_draw_surface};
use crate::scene::tiled::{TileMap, render_tilemap_2d};
use crate::surface::{ClipRect, DepthBuffer, fill_rect_clipped, stroke_line_clipped};

#[derive(Clone, Debug)]
pub struct ScenePlacement {
    pub name: String,
    pub model: Rc<Model>,
    pub transform: Transform,
    pub ground_snap: bool,
    pub tint_rgb: u32,
}

#[derive(Clone, Debug)]
pub struct AssembledEntity {
    pub name: String,
    #[allow(dead_code)]
    pub model: Rc<Model>,
    pub transform: Transform,
    pub world_aabb: Aabb,
    pub snapped_ground_y: Option<f32>,
    pub tint_rgb: u32,
}

#[derive(Debug)]
pub struct Scene {
    pub terrain: Option<Rc<HeightMap>>,
    pub tilemap: Option<Rc<RefCell<TileMap>>>,
    pub nav: Option<Rc<RefCell<NavGrid>>>,
    pub cubemap: Option<Rc<Cubemap>>,
    pub entities: Vec<AssembledEntity>,
    pub lights: Vec<Light>,
    pub ambient: [f32; 3],
    pub world_aabb: Aabb,
    pub ground_bias: f32,
}

impl Scene {
    #[allow(clippy::too_many_arguments)]
    pub fn assemble(
        terrain: Option<Rc<HeightMap>>,
        tilemap: Option<Rc<RefCell<TileMap>>>,
        nav: Option<Rc<RefCell<NavGrid>>>,
        cubemap: Option<Rc<Cubemap>>,
        placements: Vec<ScenePlacement>,
        lights: Vec<Light>,
        ambient: [f32; 3],
    ) -> Result<Self, String> {
        if placements.len() > MAX_SCENE_NODES {
            return Err(format!(
                "E_SCENE_INVALID: scene placement count {} exceeds {}",
                placements.len(),
                MAX_SCENE_NODES
            ));
        }
        if ambient.iter().any(|c| !c.is_finite() || *c < 0.0 || *c > 4.0) {
            return Err("E_SCENE_INVALID: scene ambient must be finite in 0.0..=4.0".to_string());
        }

        let mut world_aabb: Option<Aabb> = None;
        let mut merge_aabb = |b: Aabb| {
            world_aabb = Some(match world_aabb {
                None => b,
                Some(cur) => cur.expand_point(b.min).expand_point(b.max),
            });
        };

        let mut ground_bias = 0.0f32;
        if let Some(hm) = &terrain {
            let taabb = hm.world_aabb();
            ground_bias = hm.height_at(
                0.5 * (taabb.min.x + taabb.max.x),
                0.5 * (taabb.min.z + taabb.max.z),
            );
            merge_aabb(taabb);
            if let Some(tm_rc) = &tilemap {
                let mut tm = tm_rc.borrow_mut();
                tm.ground_bias = ground_bias;
            }
        }

        let mut entities = Vec::with_capacity(placements.len());
        for pl in placements {
            if !pl.transform.position.is_finite() || !pl.transform.scale.is_finite() {
                return Err(format!(
                    "E_SCENE_INVALID: placement '{}' contains non-finite transform",
                    pl.name
                ));
            }
            let mut tr = pl.transform;
            let mut snapped_ground_y = None;
            if pl.ground_snap {
                if let Some(hm) = &terrain {
                    let h = hm.height_at(tr.position.x, tr.position.z);
                    tr.position.y = h;
                    snapped_ground_y = Some(h);
                }
            }
            let w_aabb = pl.model.aabb.transform(&tr.to_mat4());
            merge_aabb(w_aabb);
            entities.push(AssembledEntity {
                name: pl.name,
                model: pl.model,
                transform: tr,
                world_aabb: w_aabb,
                snapped_ground_y,
                tint_rgb: pl.tint_rgb,
            });
        }

        let final_aabb = world_aabb.unwrap_or_else(|| Aabb::new(Vec3::zero(), Vec3::zero()));
        Ok(Self {
            terrain,
            tilemap,
            nav,
            cubemap,
            entities,
            lights,
            ambient,
            world_aabb: final_aabb,
            ground_bias,
        })
    }

    /// Deterministic software scene draw onto a `0xAARRGGBB` surface with NDC `[0, 1]` `DepthBuffer`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_to_surface(
        &self,
        buf: &mut [u32],
        mut depth: Option<&mut DepthBuffer>,
        bw: i32,
        bh: i32,
        clip: ClipRect,
        cam_pos: Vec3,
        yaw_rad: f32,
        horizon_px: f32,
        fov_scale: f32,
    ) {
        if bw <= 0 || bh <= 0 {
            return;
        }
        let x0 = clip.x0.max(0);
        let y0 = clip.y0.max(0);
        let x1 = clip.x1.min(bw);
        let y1 = clip.y1.min(bh);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let bw_u = bw as usize;
        let turn = yaw_rad / (2.0 * std::f32::consts::PI);
        let s_yaw = lut_sin(turn);
        let c_yaw = lut_cos(turn);

        // 1. Skybox pass from Cubemap (where available)
        if let Some(cm) = &self.cubemap {
            let half_w = (bw as f32) * 0.5;
            let focal = (bh as f32) * fov_scale.max(0.1);
            for py in y0..y1 {
                let pitch_y = (horizon_px - py as f32) / focal;
                let row = (py as usize) * bw_u;
                for px in x0..x1 {
                    let screen_u = ((px as f32) - half_w) / focal;
                    let dx = screen_u * c_yaw - s_yaw;
                    let dz = -screen_u * s_yaw - c_yaw;
                    let col = cm.sample_argb(Vec3::new(dx, pitch_y, dz));
                    buf[row + px as usize] = col;
                }
            }
        }

        // 2. Terrain Mode7 pass with NDC [0, 1] depth test
        if let Some(hm) = &self.terrain {
            mode7_draw_surface(
                hm,
                buf,
                depth.as_deref_mut(),
                bw,
                bh,
                clip,
                None,
                cam_pos.x,
                cam_pos.y,
                cam_pos.z,
                yaw_rad,
                horizon_px,
                fov_scale,
            );
        } else if let Some(tm_rc) = &self.tilemap {
            let tm = tm_rc.borrow();
            render_tilemap_2d(&tm, buf, bw, bh, clip, cam_pos.x, cam_pos.z);
        }

        // Compute directional/ambient tint scalar cleanly
        let mut light_r = self.ambient[0];
        let mut light_g = self.ambient[1];
        let mut light_b = self.ambient[2];
        for l in &self.lights {
            if l.kind == LightKind::Directional {
                light_r += l.color[0] * l.intensity * 0.5;
                light_g += l.color[1] * l.intensity * 0.5;
                light_b += l.color[2] * l.intensity * 0.5;
            }
        }
        let lit_r = light_r.clamp(0.15, 1.5);
        let lit_g = light_g.clamp(0.15, 1.5);
        let lit_b = light_b.clamp(0.15, 1.5);

        // 3. Project placed 3D models (deterministic perspective projection + NDC depth test)
        let half_w = (bw as f32) * 0.5;
        let focal = (bh as f32) * fov_scale.max(0.1);
        let far_clip = 512.0f32;
        let right = Vec3::new(c_yaw, 0.0, -s_yaw);
        let forward = Vec3::new(-s_yaw, 0.0, -c_yaw);
        let up = Vec3::new(0.0, 1.0, 0.0);

        for ent in &self.entities {
            let c = ent.world_aabb.center();
            let sz = ent.world_aabb.size();
            let rel = c.sub(cam_pos);
            let cz = rel.dot(forward);
            if cz <= 0.5 || cz >= far_clip {
                continue;
            }
            let cx = rel.dot(right);
            let cy = rel.dot(up);
            let sx = (half_w + (cx / cz) * focal).round() as i32;
            let sy = (horizon_px - (cy / cz) * focal).round() as i32;
            let rad_x = (((sz.x.max(sz.z) * 0.5) / cz) * focal).round().max(3.0) as i32;
            let rad_y = (((sz.y * 0.5) / cz) * focal).round().max(3.0) as i32;
            let frag_z = (cz / far_clip).clamp(0.0, 0.9999);

            let base_r = ((ent.tint_rgb >> 16) & 0xFF) as f32;
            let base_g = ((ent.tint_rgb >> 8) & 0xFF) as f32;
            let base_b = (ent.tint_rgb & 0xFF) as f32;
            let cr = (base_r * lit_r).round().clamp(0.0, 255.0) as u32;
            let cg = (base_g * lit_g).round().clamp(0.0, 255.0) as u32;
            let cb = (base_b * lit_b).round().clamp(0.0, 255.0) as u32;
            let color = 0xFF00_0000 | (cr << 16) | (cg << 8) | cb;

            let rx0 = (sx - rad_x).max(x0);
            let ry0 = (sy - rad_y).max(y0);
            let rx1 = (sx + rad_x).min(x1);
            let ry1 = (sy + rad_y).min(y1);
            if rx0 >= rx1 || ry0 >= ry1 {
                continue;
            }

            if let Some(ref mut db) = depth {
                for py in ry0..ry1 {
                    let row = (py as usize) * bw_u;
                    for px in rx0..rx1 {
                        if db.test_and_set(px, py, frag_z) {
                            buf[row + px as usize] = color;
                        }
                    }
                }
            } else {
                fill_rect_clipped(buf, bw, clip, rx0, ry0, rx1 - rx0, ry1 - ry0, color);
                stroke_line_clipped(buf, bw, clip, rx0, ry0, rx1 - 1, ry0, 0xFFFF_FFFF);
            }
        }
    }
}
