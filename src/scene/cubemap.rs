use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::MediaBudgetGuard;
use crate::media::image::{DecodeOpts, decode_image_bytes};
use crate::scene::asset::{AssetRef, read_asset_ref_bytes};
use crate::scene::geom::Vec3;
use crate::vm::VM;

pub const MAX_CUBEMAP_FACE_DIM: u32 = 4096;

#[derive(Debug)]
pub struct Cubemap {
    pub size: u32,
    /// Six square faces in glTF/OpenGL order: `[+X, -X, +Y, -Y, +Z, -Z]`, pixels stored as `0xAARRGGBB`.
    pub faces: [Vec<u32>; 6],
    #[allow(dead_code)]
    pub budget: Option<MediaBudgetGuard>,
}

impl Cubemap {
    pub fn from_faces(
        size: u32,
        faces: [Vec<u32>; 6],
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        if size == 0 || size > MAX_CUBEMAP_FACE_DIM || !size.is_power_of_two() {
            return Err(format!(
                "E_CUBEMAP_INVALID: cubemap face size {} must be a power of two in 1..={}",
                size, MAX_CUBEMAP_FACE_DIM
            ));
        }
        let expected = (size as usize)
            .checked_mul(size as usize)
            .ok_or_else(|| "E_CUBEMAP_INVALID: cubemap texel count overflow".to_string())?;
        for (i, face) in faces.iter().enumerate() {
            if face.len() != expected {
                return Err(format!(
                    "E_CUBEMAP_INVALID: cubemap face {} has {} texels, expected {}",
                    i,
                    face.len(),
                    expected
                ));
            }
        }
        let total_bytes = expected.saturating_mul(6).saturating_mul(4);
        let guard = MediaBudgetGuard::charge(budget_core, total_bytes)?;
        Ok(Self {
            size,
            faces,
            budget: Some(guard),
        })
    }

    pub fn load_paths(vm: &mut VM, paths: &[&str; 6]) -> Result<Self, String> {
        let mut decoded_faces: Vec<Vec<u32>> = Vec::with_capacity(6);
        let mut expected_size: Option<u32> = None;

        for (i, raw_path) in paths.iter().enumerate() {
            let aref = AssetRef::parse_allow_sandbox_root(raw_path, vm)
                .map_err(|e| format!("E_CUBEMAP_INVALID: face {} '{}': {}", i, raw_path, e))?;
            let (bytes, _, _) = read_asset_ref_bytes(vm, &aref)
                .map_err(|e| format!("E_CUBEMAP_INVALID: face {} '{}': {}", i, raw_path, e))?;
            let opts = DecodeOpts::strict_default();
            let img = decode_image_bytes(&bytes, &opts, Some(vm.budget_core()), None)
                .map_err(|e| format!("E_CUBEMAP_INVALID: face {} '{}': {}", i, raw_path, e))?;
            if img.width != img.height {
                return Err(format!(
                    "E_CUBEMAP_INVALID: face {} is non-square ({}x{})",
                    i, img.width, img.height
                ));
            }
            if !img.width.is_power_of_two() || img.width > MAX_CUBEMAP_FACE_DIM {
                return Err(format!(
                    "E_CUBEMAP_INVALID: face {} size {} must be power-of-two <= {}",
                    i, img.width, MAX_CUBEMAP_FACE_DIM
                ));
            }
            if let Some(sz) = expected_size {
                if img.width != sz {
                    return Err(format!(
                        "E_CUBEMAP_INVALID: face {} size {} does not match face 0 size {}",
                        i, img.width, sz
                    ));
                }
            } else {
                expected_size = Some(img.width);
            }
            decoded_faces.push(img.to_argb_u32_vec());
        }

        let size = expected_size.unwrap_or(1);
        let faces_arr: [Vec<u32>; 6] = decoded_faces
            .try_into()
            .map_err(|_| "E_CUBEMAP_INVALID: expected exactly 6 cubemap faces".to_string())?;
        Self::from_faces(size, faces_arr, Some(vm.budget_core()))
    }

    /// Samples the cubemap along direction vector `dir` with bilinear filtering per face.
    /// Returns `[r, g, b, a]` in `0.0..=1.0`.
    pub fn sample_dir(&self, dir: Vec3) -> [f32; 4] {
        if !dir.is_finite() {
            return [0.0, 0.0, 0.0, 1.0];
        }
        let ax = dir.x.abs();
        let ay = dir.y.abs();
        let az = dir.z.abs();
        let (face_idx, u_raw, v_raw, ma) = if ax >= ay && ax >= az {
            if dir.x >= 0.0 {
                (0usize, -dir.z, -dir.y, ax) // +X
            } else {
                (1usize, dir.z, -dir.y, ax) // -X
            }
        } else if ay >= ax && ay >= az {
            if dir.y >= 0.0 {
                (2usize, dir.x, dir.z, ay) // +Y
            } else {
                (3usize, dir.x, -dir.z, ay) // -Y
            }
        } else if dir.z >= 0.0 {
            (4usize, dir.x, -dir.y, az) // +Z
        } else {
            (5usize, -dir.x, -dir.y, az) // -Z
        };

        let inv_ma = if ma > 1e-8 { 1.0 / ma } else { 0.0 };
        let u = (0.5 * (u_raw * inv_ma + 1.0)).clamp(0.0, 1.0);
        let v = (0.5 * (v_raw * inv_ma + 1.0)).clamp(0.0, 1.0);

        let s = self.size as usize;
        let fx = u * ((s - 1) as f32);
        let fy = v * ((s - 1) as f32);
        let x0 = (fx.floor() as usize).min(s - 1);
        let y0 = (fy.floor() as usize).min(s - 1);
        let x1 = (x0 + 1).min(s - 1);
        let y1 = (y0 + 1).min(s - 1);
        let tx = fx - (x0 as f32);
        let ty = fy - (y0 as f32);

        let face = &self.faces[face_idx];
        let unpack = |px: u32| -> [f32; 4] {
            [
                ((px >> 16) & 0xFF) as f32 / 255.0,
                ((px >> 8) & 0xFF) as f32 / 255.0,
                (px & 0xFF) as f32 / 255.0,
                ((px >> 24) & 0xFF) as f32 / 255.0,
            ]
        };
        let c00 = unpack(face[y0 * s + x0]);
        let c10 = unpack(face[y0 * s + x1]);
        let c01 = unpack(face[y1 * s + x0]);
        let c11 = unpack(face[y1 * s + x1]);

        let mut out = [0.0f32; 4];
        for i in 0..4 {
            let top = c00[i] + (c10[i] - c00[i]) * tx;
            let bot = c01[i] + (c11[i] - c01[i]) * tx;
            out[i] = (top + (bot - top) * ty).clamp(0.0, 1.0);
        }
        out
    }

    pub fn sample_argb(&self, dir: Vec3) -> u32 {
        let [r, g, b, a] = self.sample_dir(dir);
        let ur = (r * 255.0).round().clamp(0.0, 255.0) as u32;
        let ug = (g * 255.0).round().clamp(0.0, 255.0) as u32;
        let ub = (b * 255.0).round().clamp(0.0, 255.0) as u32;
        let ua = (a * 255.0).round().clamp(0.0, 255.0) as u32;
        (ua << 24) | (ur << 16) | (ug << 8) | ub
    }
}
