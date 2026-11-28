use std::collections::HashMap;
use std::sync::Arc;

use crate::media::temp_budget_guard;
use crate::scene::asset::{AssetRef, read_asset_ref_bytes};
use crate::scene::geom::{Aabb, Vec3};
use crate::scene::model::{
    AlphaMode, GroupRange, MAX_MESH_TRIS, MAX_MESH_VERTICES, Material, MeshData, MeshStats, Model,
    SourceFormat,
};
use crate::vm::VM;

const MAX_OBJ_LINE_BYTES: usize = 1024 * 1024;
const MAX_OBJ_POLY_VERTS: usize = 64;

fn parse_finite_f32(s: &str, ctx: &str) -> Result<f32, String> {
    let f: f32 = s
        .parse()
        .map_err(|_| format!("E_MESH_CORRUPT: invalid {} float '{}'", ctx, s))?;
    if !f.is_finite() {
        return Err(format!("E_MESH_CORRUPT: non-finite {} float '{}'", ctx, s));
    }
    Ok(f)
}

fn resolve_obj_index(raw_i: i64, total: usize, kind: &str) -> Result<usize, String> {
    if raw_i == 0 {
        return Err(format!("E_MESH_CORRUPT: OBJ {} index cannot be 0", kind));
    }
    if raw_i > 0 {
        let idx = (raw_i as usize) - 1;
        if idx >= total {
            return Err(format!(
                "E_MESH_CORRUPT: OBJ {} index {} out of range ({})",
                kind, raw_i, total
            ));
        }
        Ok(idx)
    } else {
        let back = (-raw_i) as usize;
        if back > total {
            return Err(format!(
                "E_MESH_CORRUPT: OBJ negative {} index {} out of range ({})",
                kind, raw_i, total
            ));
        }
        Ok(total - back)
    }
}

fn parse_mtl_text(
    src: &str,
    mtl_ref: &AssetRef,
    materials: &mut Vec<Material>,
    mat_by_name: &mut HashMap<String, usize>,
) -> Result<(), String> {
    let mut cur_idx: Option<usize> = None;
    for (line_no, raw_line) in src.lines().enumerate() {
        if raw_line.len() > MAX_OBJ_LINE_BYTES {
            return Err(format!(
                "E_MESH_LIMIT: MTL line {} exceeds 1 MiB limit",
                line_no + 1
            ));
        }
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(kw) = parts.next() else {
            continue;
        };
        match kw {
            "newmtl" => {
                let name = parts.collect::<Vec<_>>().join(" ");
                let name = if name.is_empty() {
                    format!("mat_{}", materials.len())
                } else {
                    name
                };
                let idx = if let Some(&existing) = mat_by_name.get(&name) {
                    existing
                } else {
                    let i = materials.len();
                    let mut m = Material::default();
                    m.name = name.clone();
                    materials.push(m);
                    mat_by_name.insert(name, i);
                    i
                };
                cur_idx = Some(idx);
            }
            _ if cur_idx.is_none() => {}
            "Kd" => {
                let r = parse_finite_f32(parts.next().unwrap_or("1"), "Kd.r")?;
                let g = parse_finite_f32(parts.next().unwrap_or("1"), "Kd.g")?;
                let b = parse_finite_f32(parts.next().unwrap_or("1"), "Kd.b")?;
                materials[cur_idx.unwrap()].base_color =
                    Vec3::new(r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
            }
            "Ke" => {
                let r = parse_finite_f32(parts.next().unwrap_or("0"), "Ke.r")?;
                let g = parse_finite_f32(parts.next().unwrap_or("0"), "Ke.g")?;
                let b = parse_finite_f32(parts.next().unwrap_or("0"), "Ke.b")?;
                materials[cur_idx.unwrap()].emissive = Vec3::new(r.max(0.0), g.max(0.0), b.max(0.0));
            }
            "Ns" => {
                let ns = parse_finite_f32(parts.next().unwrap_or("0"), "Ns")?.clamp(0.0, 10_000.0);
                // Convert Blinn-Phong exponent Ns into PBR perceptual roughness [0.04..1.0].
                let roughness = (2.0f32 / (ns + 2.0)).sqrt().clamp(0.04, 1.0);
                let mat = &mut materials[cur_idx.unwrap()];
                mat.roughness = roughness;
                mat.metallic = 0.0;
            }
            "d" => {
                let d = parse_finite_f32(parts.next().unwrap_or("1"), "d")?;
                if d < 0.999 {
                    materials[cur_idx.unwrap()].alpha_mode = AlphaMode::Blend;
                }
            }
            "Tr" => {
                let tr = parse_finite_f32(parts.next().unwrap_or("0"), "Tr")?;
                if tr > 0.001 {
                    materials[cur_idx.unwrap()].alpha_mode = AlphaMode::Blend;
                }
            }
            "illum" => {
                if let Some("0") = parts.next() {
                    let mat = &mut materials[cur_idx.unwrap()];
                    mat.unlit = true;
                }
            }
            "map_Kd" | "map_Bump" | "bump" | "norm" | "map_Ks" | "map_Ke" => {
                let rest = parts.collect::<Vec<_>>().join(" ");
                let rel_path = rest.split_whitespace().last().unwrap_or("").trim();
                if !rel_path.is_empty() {
                    let resolved = mtl_ref.join_relative(rel_path)?;
                    let tex = &mut materials[cur_idx.unwrap()].textures;
                    match kw {
                        "map_Kd" => tex.base_color = Some(resolved.as_str().to_string()),
                        "map_Bump" | "bump" | "norm" => {
                            tex.normal = Some(resolved.as_str().to_string())
                        }
                        "map_Ks" => tex.metallic_roughness = Some(resolved.as_str().to_string()),
                        "map_Ke" => tex.emissive = Some(resolved.as_str().to_string()),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn load_obj_model(vm: &mut VM, obj_ref: &AssetRef, raw_bytes: &[u8]) -> Result<Model, String> {
    let text = std::str::from_utf8(raw_bytes)
        .map_err(|_| "E_MESH_CORRUPT: OBJ file is not valid UTF-8".to_string())?;
    let budget_core = Arc::clone(vm.budget_core());
    let _tmp = temp_budget_guard(Some(&budget_core), raw_bytes.len())?;

    let mut raw_pos: Vec<[f32; 3]> = Vec::new();
    let mut raw_uv: Vec<[f32; 2]> = Vec::new();
    let mut raw_norm: Vec<[f32; 3]> = Vec::new();

    let mut materials: Vec<Material> = vec![Material::default()];
    let mut mat_by_name: HashMap<String, usize> = HashMap::new();
    mat_by_name.insert("default".to_string(), 0);

    let mut out_pos: Vec<[f32; 3]> = Vec::new();
    let mut out_norm: Vec<[f32; 3]> = Vec::new();
    let mut out_uv: Vec<[f32; 2]> = Vec::new();
    let mut out_indices: Vec<u32> = Vec::new();
    let mut groups: Vec<GroupRange> = Vec::new();
    let mut dedup: HashMap<(u32, u32, u32), u32> = HashMap::new();
    let mut mesh_name = "obj_mesh".to_string();
    let mut warnings: Vec<String> = Vec::new();

    let mut cur_group_start: u32 = 0;
    let mut cur_group_mat: Option<u32> = Some(0);

    let flush_group = |groups: &mut Vec<GroupRange>,
                       cur_start: &mut u32,
                       total_indices: usize,
                       mat: Option<u32>| {
        let len = (total_indices as u32).saturating_sub(*cur_start);
        if len > 0 {
            groups.push(GroupRange {
                first_index: *cur_start,
                index_count: len,
                material_index: mat,
            });
            *cur_start = total_indices as u32;
        }
    };

    for (line_idx, raw_line) in text.lines().enumerate() {
        if raw_line.len() > MAX_OBJ_LINE_BYTES {
            return Err(format!(
                "E_MESH_LIMIT: OBJ line {} exceeds 1 MiB limit",
                line_idx + 1
            ));
        }
        if (line_idx & 2047) == 0 {
            vm.check_timeout()?;
        }
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(kw) = parts.next() else {
            continue;
        };
        match kw {
            "o" | "g" => {
                if let Some(n) = parts.next() {
                    if !n.is_empty() && mesh_name == "obj_mesh" {
                        mesh_name = n.to_string();
                    }
                }
            }
            "v" => {
                let x = parse_finite_f32(
                    parts.next().ok_or("E_MESH_CORRUPT: missing v.x")?,
                    "v.x",
                )?;
                let y = parse_finite_f32(
                    parts.next().ok_or("E_MESH_CORRUPT: missing v.y")?,
                    "v.y",
                )?;
                let z = parse_finite_f32(
                    parts.next().ok_or("E_MESH_CORRUPT: missing v.z")?,
                    "v.z",
                )?;
                raw_pos.push([x, y, z]);
                if raw_pos.len() > MAX_MESH_VERTICES {
                    return Err("E_MESH_LIMIT: OBJ vertex count exceeds 2M limit".to_string());
                }
            }
            "vt" => {
                let u = parse_finite_f32(
                    parts.next().ok_or("E_MESH_CORRUPT: missing vt.u")?,
                    "vt.u",
                )?;
                let v = parse_finite_f32(parts.next().unwrap_or("0"), "vt.v")?;
                // Canonical Askane UV origin: V=0 top, so flip OBJ bottom-left V at parse time.
                raw_uv.push([u, 1.0 - v]);
            }
            "vn" => {
                let nx = parse_finite_f32(
                    parts.next().ok_or("E_MESH_CORRUPT: missing vn.x")?,
                    "vn.x",
                )?;
                let ny = parse_finite_f32(
                    parts.next().ok_or("E_MESH_CORRUPT: missing vn.y")?,
                    "vn.y",
                )?;
                let nz = parse_finite_f32(
                    parts.next().ok_or("E_MESH_CORRUPT: missing vn.z")?,
                    "vn.z",
                )?;
                raw_norm.push([nx, ny, nz]);
            }
            "mtllib" => {
                let mtl_rel = parts.collect::<Vec<_>>().join(" ");
                let mtl_rel = mtl_rel.trim();
                if !mtl_rel.is_empty() {
                    let mtl_ref = obj_ref.join_relative(mtl_rel)?;
                    let (mtl_bytes, _, _) = read_asset_ref_bytes(vm, &mtl_ref)?;
                    let mtl_str = std::str::from_utf8(&mtl_bytes)
                        .map_err(|_| "E_MESH_CORRUPT: MTL file is not valid UTF-8".to_string())?;
                    parse_mtl_text(mtl_str, &mtl_ref, &mut materials, &mut mat_by_name)?;
                }
            }
            "usemtl" => {
                let name = parts.collect::<Vec<_>>().join(" ");
                let name = name.trim();
                let next_mat = *mat_by_name.get(name).unwrap_or(&0) as u32;
                if Some(next_mat) != cur_group_mat {
                    flush_group(&mut groups, &mut cur_group_start, out_indices.len(), cur_group_mat);
                    cur_group_mat = Some(next_mat);
                }
            }
            "f" => {
                let mut face_verts: Vec<(usize, Option<usize>, Option<usize>)> = Vec::with_capacity(4);
                for tok in parts {
                    if face_verts.len() >= MAX_OBJ_POLY_VERTS {
                        return Err(format!(
                            "E_MESH_LIMIT: OBJ polygon exceeds {} vertices",
                            MAX_OBJ_POLY_VERTS
                        ));
                    }
                    let segs: Vec<&str> = tok.split('/').collect();
                    if segs.len() > 3 || segs[0].is_empty() {
                        return Err(format!("E_MESH_CORRUPT: invalid OBJ face token '{}'", tok));
                    }
                    let vi_raw: i64 = segs[0]
                        .parse()
                        .map_err(|_| format!("E_MESH_CORRUPT: invalid face index '{}'", segs[0]))?;
                    let vi = resolve_obj_index(vi_raw, raw_pos.len(), "position")?;
                    let vti = if segs.len() >= 2 && !segs[1].is_empty() {
                        let t_raw: i64 = segs[1].parse().map_err(|_| {
                            format!("E_MESH_CORRUPT: invalid UV index '{}'", segs[1])
                        })?;
                        Some(resolve_obj_index(t_raw, raw_uv.len(), "uv")?)
                    } else {
                        None
                    };
                    let vni = if segs.len() == 3 && !segs[2].is_empty() {
                        let n_raw: i64 = segs[2].parse().map_err(|_| {
                            format!("E_MESH_CORRUPT: invalid normal index '{}'", segs[2])
                        })?;
                        Some(resolve_obj_index(n_raw, raw_norm.len(), "normal")?)
                    } else {
                        None
                    };
                    face_verts.push((vi, vti, vni));
                }
                if face_verts.len() < 3 {
                    return Err("E_MESH_CORRUPT: OBJ face has fewer than 3 vertices".to_string());
                }

                // Fan triangulate around vertex 0
                let n_poly = face_verts.len();
                for k in 1..n_poly - 1 {
                    if out_indices.len() / 3 >= MAX_MESH_TRIS {
                        return Err(
                            "E_MESH_LIMIT: OBJ triangle count exceeds 4M limit".to_string(),
                        );
                    }
                    let tri = [face_verts[0], face_verts[k], face_verts[k + 1]];
                    let tri_missing_n = tri.iter().any(|(_, _, n)| n.is_none());
                    let computed_fn = if tri_missing_n {
                        let p0 = Vec3::from_array(raw_pos[tri[0].0]);
                        let p1 = Vec3::from_array(raw_pos[tri[1].0]);
                        let p2 = Vec3::from_array(raw_pos[tri[2].0]);
                        let cn = p1.sub(p0).cross(p2.sub(p0)).normalize();
                        if cn.len_sq() <= 1e-12 {
                            [0.0, 1.0, 0.0]
                        } else {
                            cn.to_array()
                        }
                    } else {
                        [0.0, 1.0, 0.0]
                    };

                    for &(vi, vti, vni) in &tri {
                        let key = (
                            vi as u32,
                            vti.map(|x| x as u32 + 1).unwrap_or(0),
                            vni.map(|x| x as u32 + 1).unwrap_or(0),
                        );
                        if vni.is_some() {
                            if let Some(&existing) = dedup.get(&key) {
                                out_indices.push(existing);
                                continue;
                            }
                        }
                        if out_pos.len() >= MAX_MESH_VERTICES {
                            return Err(
                                "E_MESH_LIMIT: expanded OBJ vertices exceed 2M limit".to_string(),
                            );
                        }
                        let idx = out_pos.len() as u32;
                        out_pos.push(raw_pos[vi]);
                        out_uv.push(vti.map(|i| raw_uv[i]).unwrap_or([0.0, 0.0]));
                        out_norm.push(vni.map(|i| raw_norm[i]).unwrap_or(computed_fn));
                        if vni.is_some() {
                            dedup.insert(key, idx);
                        }
                        out_indices.push(idx);
                    }
                }
            }
            _ => {}
        }
    }

    flush_group(&mut groups, &mut cur_group_start, out_indices.len(), cur_group_mat);
    if materials.len() == 1 && mat_by_name.len() == 1 && materials[0].textures.base_color.is_none() && out_pos.is_empty() {
        warnings.push("empty OBJ mesh".to_string());
    }

    let mesh = MeshData {
        name: mesh_name,
        positions: out_pos,
        normals: out_norm,
        uvs: out_uv,
        colors: None,
        indices: out_indices,
        groups,
        splat_weights: None,
        aabb: Aabb::new(Vec3::zero(), Vec3::zero()),
        stats: MeshStats::default(),
        source: SourceFormat::Obj,
        budget: None,
    };
    let budget = Arc::clone(vm.budget_core());
    Model::single(mesh, materials, warnings, Some(&budget))
}
