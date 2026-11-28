use std::collections::VecDeque;
use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::{MediaBudgetGuard, err_corrupt, err_limit};
use crate::scene::cache::{BinReader, BinWriter};
use crate::scene::geom::{Aabb, Mat4, Quat, Transform, Vec3};

pub const MAX_MESH_VERTICES: usize = 2_000_000;
pub const MAX_MESH_TRIS: usize = 4_000_000;
pub const MAX_SCENE_NODES: usize = 100_000;
pub const MAX_NODE_DEPTH: usize = 128;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceFormat {
    Obj = 1,
    Gltf = 2,
    Glb = 3,
    Generated = 4,
}

impl SourceFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Obj => "obj",
            Self::Gltf => "gltf",
            Self::Glb => "glb",
            Self::Generated => "generated",
        }
    }

    pub fn from_u8(v: u8) -> Result<Self, String> {
        match v {
            1 => Ok(Self::Obj),
            2 => Ok(Self::Gltf),
            3 => Ok(Self::Glb),
            4 => Ok(Self::Generated),
            other => Err(err_corrupt(format!("invalid SourceFormat tag {}", other))),
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaMode {
    Opaque = 0,
    Blend = 1,
    Mask = 2,
}

impl AlphaMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Opaque => "opaque",
            Self::Blend => "blend",
            Self::Mask => "mask",
        }
    }

    pub fn from_u8(v: u8) -> Result<Self, String> {
        match v {
            0 => Ok(Self::Opaque),
            1 => Ok(Self::Blend),
            2 => Ok(Self::Mask),
            other => Err(err_corrupt(format!("invalid AlphaMode tag {}", other))),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaterialTextures {
    pub base_color: Option<String>,
    pub normal: Option<String>,
    pub metallic_roughness: Option<String>,
    pub emissive: Option<String>,
    pub occlusion: Option<String>,
    pub height: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub base_color: Vec3,
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: Vec3,
    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f32,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub double_sided: bool,
    pub textures: MaterialTextures,
    pub unlit: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            base_color: Vec3::one(),
            metallic: 0.0,
            roughness: 1.0,
            emissive: Vec3::zero(),
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            double_sided: false,
            textures: MaterialTextures::default(),
            unlit: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupRange {
    pub first_index: u32,
    pub index_count: u32,
    pub material_index: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeshStats {
    pub vertex_count: usize,
    pub tri_count: usize,
    pub degenerate_tris: usize,
    pub group_count: usize,
}

#[derive(Debug)]
pub struct MeshData {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
    pub groups: Vec<GroupRange>,
    pub splat_weights: Option<Vec<u8>>,
    pub aabb: Aabb,
    pub stats: MeshStats,
    pub source: SourceFormat,
    pub budget: Option<MediaBudgetGuard>,
}

impl MeshData {
    pub fn memory_bytes(&self) -> usize {
        self.positions
            .len()
            .saturating_mul(12 + 12 + 8)
            .saturating_add(self.colors.as_ref().map_or(0, |c| c.len() * 16))
            .saturating_add(self.indices.len() * 4)
            .saturating_add(self.groups.len() * 16)
            .saturating_add(self.splat_weights.as_ref().map_or(0, |w| w.len()))
    }

    pub fn clone_without_budget(&self) -> Self {
        Self {
            name: self.name.clone(),
            positions: self.positions.clone(),
            normals: self.normals.clone(),
            uvs: self.uvs.clone(),
            colors: self.colors.clone(),
            indices: self.indices.clone(),
            groups: self.groups.clone(),
            splat_weights: self.splat_weights.clone(),
            aabb: self.aabb,
            stats: self.stats,
            source: self.source,
            budget: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node3D {
    pub name: String,
    pub transform: Transform,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    pub mesh: Option<u32>,
    pub light: Option<u32>,
    pub layer_mask: u32,
    pub aabb_local: Option<Aabb>,
    pub aabb_world: Aabb,
}

#[derive(Debug)]
pub struct Model {
    pub meshes: Vec<MeshData>,
    pub materials: Vec<Material>,
    pub nodes: Vec<Node3D>,
    pub roots: Vec<u32>,
    pub aabb: Aabb,
    pub warnings: Vec<String>,
    pub source: SourceFormat,
    pub budget: Option<MediaBudgetGuard>,
}

impl Model {
    pub fn single(
        mut mesh: MeshData,
        materials: Vec<Material>,
        warnings: Vec<String>,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        mesh.budget = None;
        let aabb = mesh.aabb;
        let src = mesh.source;
        let node = Node3D {
            name: if mesh.name.is_empty() {
                "root".to_string()
            } else {
                mesh.name.clone()
            },
            transform: Transform::identity(),
            parent: None,
            children: Vec::new(),
            mesh: Some(0),
            light: None,
            layer_mask: 1,
            aabb_local: Some(aabb),
            aabb_world: aabb,
        };
        let mut model = Self {
            meshes: vec![mesh],
            materials,
            nodes: vec![node],
            roots: vec![0],
            aabb,
            warnings,
            source: src,
            budget: None,
        };
        verify_model(&mut model)?;
        let bytes = model.total_memory_bytes();
        model.budget = Some(MediaBudgetGuard::charge(budget_core, bytes)?);
        Ok(model)
    }

    pub fn total_memory_bytes(&self) -> usize {
        self.meshes.iter().map(|m| m.memory_bytes()).sum()
    }

    pub fn total_stats(&self) -> MeshStats {
        let mut acc = MeshStats::default();
        for m in &self.meshes {
            acc.vertex_count = acc.vertex_count.saturating_add(m.stats.vertex_count);
            acc.tri_count = acc.tri_count.saturating_add(m.stats.tri_count);
            acc.degenerate_tris = acc.degenerate_tris.saturating_add(m.stats.degenerate_tris);
            acc.group_count = acc.group_count.saturating_add(m.stats.group_count);
        }
        acc
    }

    pub fn flatten(&self, material_filter: Option<&str>, budget_core: Option<&Arc<BudgetCore>>) -> Result<MeshData, String> {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut colors: Option<Vec<[f32; 4]>> = None;
        let mut indices = Vec::new();
        let mut groups = Vec::new();

        for node in &self.nodes {
            let Some(mi) = node.mesh else {
                continue;
            };
            let Some(mesh) = self.meshes.get(mi as usize) else {
                continue;
            };
            let world_mat = compute_world_matrix(&self.nodes, node);
            for grp in &mesh.groups {
                if let Some(filter) = material_filter {
                    let mat_name = grp
                        .material_index
                        .and_then(|idx| self.materials.get(idx as usize))
                        .map(|m| m.name.as_str())
                        .unwrap_or("default");
                    if mat_name != filter {
                        continue;
                    }
                }
                let first = grp.first_index as usize;
                let count = grp.index_count as usize;
                if first + count > mesh.indices.len() {
                    continue;
                }
                let out_first = indices.len() as u32;
                let base_v = positions.len() as u32;
                if base_v as usize + mesh.positions.len() > MAX_MESH_VERTICES {
                    return Err(format!(
                        "E_MESH_LIMIT: flattened mesh exceeds {} vertices",
                        MAX_MESH_VERTICES
                    ));
                }
                for vi in 0..mesh.positions.len() {
                    let wp = world_mat.transform_point3(Vec3::from_array(mesh.positions[vi]));
                    let wn = world_mat
                        .transform_vector3(Vec3::from_array(mesh.normals[vi]))
                        .normalize();
                    positions.push(wp.to_array());
                    normals.push(wn.to_array());
                    uvs.push(mesh.uvs[vi]);
                    if let Some(src_cols) = &mesh.colors {
                        let dst_cols = colors.get_or_insert_with(Vec::new);
                        dst_cols.push(src_cols[vi]);
                    }
                }
                for &idx in &mesh.indices[first..first + count] {
                    indices.push(base_v + idx);
                }
                if count > 0 {
                    groups.push(GroupRange {
                        first_index: out_first,
                        index_count: count as u32,
                        material_index: grp.material_index,
                    });
                }
            }
        }
        let mut out = MeshData {
            name: "flattened".to_string(),
            positions,
            normals,
            uvs,
            colors,
            indices,
            groups,
            splat_weights: None,
            aabb: Aabb::new(Vec3::zero(), Vec3::zero()),
            stats: MeshStats::default(),
            source: self.source,
            budget: None,
        };
        let mut warns = Vec::new();
        verify_mesh_data(&mut out, &mut warns)?;
        let bytes = out.memory_bytes();
        out.budget = Some(MediaBudgetGuard::charge(budget_core, bytes)?);
        Ok(out)
    }
}

pub fn compute_world_matrix(nodes: &[Node3D], target: &Node3D) -> Mat4 {
    let mut chain = vec![target.transform.to_mat4()];
    let mut cur_parent = target.parent;
    let mut steps = 0usize;
    while let Some(pi) = cur_parent {
        if steps >= MAX_NODE_DEPTH {
            break;
        }
        let Some(pnode) = nodes.get(pi as usize) else {
            break;
        };
        chain.push(pnode.transform.to_mat4());
        cur_parent = pnode.parent;
        steps += 1;
    }
    let mut acc = Mat4::identity();
    for m in chain.into_iter().rev() {
        acc = acc.mul_mat4(&m);
    }
    acc
}

pub fn verify_mesh_data(mesh: &mut MeshData, warnings: &mut Vec<String>) -> Result<(), String> {
    let v_count = mesh.positions.len();
    if v_count > MAX_MESH_VERTICES {
        return Err(format!(
            "E_MESH_LIMIT: vertex count {} exceeds {} limit",
            v_count, MAX_MESH_VERTICES
        ));
    }
    if mesh.normals.len() != v_count || mesh.uvs.len() != v_count {
        return Err("E_MESH_CORRUPT: mesh attribute array lengths do not match".to_string());
    }
    if let Some(cols) = &mesh.colors {
        if cols.len() != v_count {
            return Err("E_MESH_CORRUPT: vertex color array length mismatch".to_string());
        }
        for c in cols {
            if c.iter().any(|x| !x.is_finite()) {
                return Err("E_MESH_CORRUPT: non-finite vertex color".to_string());
            }
        }
    }
    if mesh.indices.len() % 3 != 0 {
        return Err("E_MESH_CORRUPT: index count must be a multiple of 3".to_string());
    }
    let tri_count = mesh.indices.len() / 3;
    if tri_count > MAX_MESH_TRIS {
        return Err(format!(
            "E_MESH_LIMIT: triangle count {} exceeds {} limit",
            tri_count, MAX_MESH_TRIS
        ));
    }
    for &idx in &mesh.indices {
        if (idx as usize) >= v_count {
            return Err(format!(
                "E_MESH_CORRUPT: vertex index {} out of bounds (vertices={})",
                idx, v_count
            ));
        }
    }

    let mut min_p = if v_count > 0 {
        Vec3::from_array(mesh.positions[0])
    } else {
        Vec3::zero()
    };
    let mut max_p = min_p;
    for p in &mesh.positions {
        if p.iter().any(|x| !x.is_finite()) {
            return Err("E_MESH_CORRUPT: non-finite vertex position".to_string());
        }
        let vp = Vec3::from_array(*p);
        min_p = min_p.min(vp);
        max_p = max_p.max(vp);
    }
    let mut renorm_warned = false;
    for n in &mut mesh.normals {
        if n.iter().any(|x| !x.is_finite()) {
            return Err("E_MESH_CORRUPT: non-finite vertex normal".to_string());
        }
        let vn = Vec3::from_array(*n);
        let lsq = vn.len_sq();
        if lsq <= 1e-12 {
            *n = [0.0, 1.0, 0.0];
        } else if (lsq - 1.0).abs() > 1e-3 {
            *n = vn.normalize().to_array();
            if !renorm_warned {
                warnings.push("W0107: auto-normalized non-unit mesh normals".to_string());
                renorm_warned = true;
            }
        }
    }
    for uv in &mesh.uvs {
        if !uv[0].is_finite() || !uv[1].is_finite() {
            return Err("E_MESH_CORRUPT: non-finite UV coordinate".to_string());
        }
    }

    let mut degens = 0usize;
    for tri in mesh.indices.chunks_exact(3) {
        let a = tri[0];
        let b = tri[1];
        let c = tri[2];
        if a == b || b == c || a == c {
            degens += 1;
        } else {
            let p0 = Vec3::from_array(mesh.positions[a as usize]);
            let p1 = Vec3::from_array(mesh.positions[b as usize]);
            let p2 = Vec3::from_array(mesh.positions[c as usize]);
            if p1.sub(p0).cross(p2.sub(p0)).len_sq() <= 1e-18 {
                degens += 1;
            }
        }
    }

    if mesh.groups.is_empty() && !mesh.indices.is_empty() {
        mesh.groups.push(GroupRange {
            first_index: 0,
            index_count: mesh.indices.len() as u32,
            material_index: None,
        });
    } else if !mesh.indices.is_empty() {
        mesh.groups.sort_by_key(|g| g.first_index);
        let mut cursor = 0u32;
        for g in &mesh.groups {
            if g.first_index < cursor {
                return Err("E_MESH_CORRUPT: overlapping mesh material groups".to_string());
            }
            let end = g
                .first_index
                .checked_add(g.index_count)
                .ok_or_else(|| "E_MESH_CORRUPT: group index overflow".to_string())?;
            if (end as usize) > mesh.indices.len() {
                return Err("E_MESH_CORRUPT: group exceeds index buffer".to_string());
            }
            cursor = end;
        }
    }

    mesh.aabb = Aabb::new(min_p, max_p);
    mesh.stats = MeshStats {
        vertex_count: v_count,
        tri_count,
        degenerate_tris: degens,
        group_count: mesh.groups.len(),
    };
    Ok(())
}

pub fn verify_model(model: &mut Model) -> Result<(), String> {
    if model.nodes.len() > MAX_SCENE_NODES {
        return Err(format!(
            "E_MESH_LIMIT: model node count {} exceeds {} limit",
            model.nodes.len(),
            MAX_SCENE_NODES
        ));
    }
    let mut warnings = std::mem::take(&mut model.warnings);
    for mesh in &mut model.meshes {
        verify_mesh_data(mesh, &mut warnings)?;
    }

    for mat in &mut model.materials {
        if !mat.metallic.is_finite() || !(0.0..=1.0).contains(&mat.metallic) {
            mat.metallic = if mat.metallic.is_finite() {
                mat.metallic.clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
        if !mat.roughness.is_finite() || !(0.04..=1.0).contains(&mat.roughness) {
            mat.roughness = if mat.roughness.is_finite() {
                mat.roughness.clamp(0.04, 1.0)
            } else {
                1.0
            };
        }
    }

    // BFS cycle + depth check on DAG
    let n_count = model.nodes.len();
    for (ni, node) in model.nodes.iter().enumerate() {
        if let Some(mi) = node.mesh {
            if (mi as usize) >= model.meshes.len() {
                return Err("E_MESH_CORRUPT: node references out-of-range mesh".to_string());
            }
        }
        if let Some(pi) = node.parent {
            if (pi as usize) >= n_count || (pi as usize) == ni {
                return Err("E_MESH_CYCLE: invalid parent reference".to_string());
            }
        }
        for &ch in &node.children {
            if (ch as usize) >= n_count || (ch as usize) == ni {
                return Err("E_MESH_CYCLE: invalid or self-referential node child".to_string());
            }
        }
    }

    // 3-color DFS/BFS cycle & max depth check starting from every node so unreachable cycles are also caught
    let mut state = vec![0u8; n_count]; // 0 unvisited, 1 visiting, 2 done
    for start in 0..n_count {
        if state[start] != 0 {
            continue;
        }
        let mut stack: Vec<(usize, usize, usize)> = vec![(start, 0, 0)];
        state[start] = 1;
        while let Some((u, child_pos, depth)) = stack.pop() {
            if depth > MAX_NODE_DEPTH {
                return Err(format!(
                    "E_MESH_CYCLE: node hierarchy exceeds max depth ({})",
                    MAX_NODE_DEPTH
                ));
            }
            if child_pos < model.nodes[u].children.len() {
                let v = model.nodes[u].children[child_pos] as usize;
                stack.push((u, child_pos + 1, depth));
                match state[v] {
                    1 => return Err("E_MESH_CYCLE: cyclic node hierarchy detected".to_string()),
                    0 => {
                        state[v] = 1;
                        stack.push((v, 0, depth + 1));
                    }
                    _ => {}
                }
            } else {
                state[u] = 2;
            }
        }
    }

    // Recompute world AABBs deterministically in BFS order from roots
    let mut world_mats = vec![Mat4::identity(); n_count];
    let mut visited_bfs = vec![false; n_count];
    let mut q = VecDeque::new();
    if model.roots.is_empty() && n_count > 0 {
        for (i, n) in model.nodes.iter().enumerate() {
            if n.parent.is_none() {
                model.roots.push(i as u32);
            }
        }
    }
    for &r in &model.roots {
        if (r as usize) >= n_count {
            return Err("E_MESH_CORRUPT: root node index out of range".to_string());
        }
        let ri = r as usize;
        if !visited_bfs[ri] {
            visited_bfs[ri] = true;
            world_mats[ri] = model.nodes[ri].transform.to_mat4();
            q.push_back(ri);
        }
    }
    while let Some(u) = q.pop_front() {
        let pm = world_mats[u];
        let children = model.nodes[u].children.clone();
        for ch in children {
            let ci = ch as usize;
            if !visited_bfs[ci] {
                visited_bfs[ci] = true;
                world_mats[ci] = pm.mul_mat4(&model.nodes[ci].transform.to_mat4());
                q.push_back(ci);
            }
        }
    }

    let mut global_min: Option<Vec3> = None;
    let mut global_max: Option<Vec3> = None;
    for (i, node) in model.nodes.iter_mut().enumerate() {
        let local = node.mesh.map(|mi| model.meshes[mi as usize].aabb);
        node.aabb_local = local;
        let world_bb = match local {
            Some(lb) => lb.transform(&world_mats[i]),
            None => {
                let p = Vec3::new(world_mats[i].m[12], world_mats[i].m[13], world_mats[i].m[14]);
                Aabb::new(p, p)
            }
        };
        node.aabb_world = world_bb;
        global_min = Some(match global_min {
            Some(cur) => cur.min(world_bb.min),
            None => world_bb.min,
        });
        global_max = Some(match global_max {
            Some(cur) => cur.max(world_bb.max),
            None => world_bb.max,
        });
    }
    model.aabb = match (global_min, global_max) {
        (Some(mn), Some(mx)) => Aabb::new(mn, mx),
        _ => Aabb::new(Vec3::zero(), Vec3::zero()),
    };
    model.warnings = warnings;
    Ok(())
}

pub fn encode_model_payload(model: &Model) -> Vec<u8> {
    let mut w = BinWriter::new();
    w.write_u8(model.source as u8);
    w.write_u32(model.meshes.len() as u32);
    for m in &model.meshes {
        w.write_str(&m.name);
        w.write_u8(m.source as u8);
        w.write_u32(m.positions.len() as u32);
        for p in &m.positions {
            w.write_f32(p[0]);
            w.write_f32(p[1]);
            w.write_f32(p[2]);
        }
        for n in &m.normals {
            w.write_f32(n[0]);
            w.write_f32(n[1]);
            w.write_f32(n[2]);
        }
        for uv in &m.uvs {
            w.write_f32(uv[0]);
            w.write_f32(uv[1]);
        }
        match &m.colors {
            Some(cols) => {
                w.write_u8(1);
                for c in cols {
                    w.write_f32(c[0]);
                    w.write_f32(c[1]);
                    w.write_f32(c[2]);
                    w.write_f32(c[3]);
                }
            }
            None => w.write_u8(0),
        }
        w.write_u32(m.indices.len() as u32);
        for &idx in &m.indices {
            w.write_u32(idx);
        }
        w.write_u32(m.groups.len() as u32);
        for g in &m.groups {
            w.write_u32(g.first_index);
            w.write_u32(g.index_count);
            w.write_u32(g.material_index.unwrap_or(u32::MAX));
        }
    }

    w.write_u32(model.materials.len() as u32);
    for mat in &model.materials {
        w.write_str(&mat.name);
        for c in mat.base_color.to_array() {
            w.write_f32(c);
        }
        w.write_f32(mat.metallic);
        w.write_f32(mat.roughness);
        for c in mat.emissive.to_array() {
            w.write_f32(c);
        }
        w.write_u8(mat.alpha_mode as u8);
        w.write_f32(mat.alpha_cutoff);
        w.write_f32(mat.normal_scale);
        w.write_f32(mat.occlusion_strength);
        w.write_u8(if mat.double_sided { 1 } else { 0 });
        w.write_u8(if mat.unlit { 1 } else { 0 });
        for tex in [
            &mat.textures.base_color,
            &mat.textures.normal,
            &mat.textures.metallic_roughness,
            &mat.textures.emissive,
            &mat.textures.occlusion,
            &mat.textures.height,
        ] {
            match tex {
                Some(s) => {
                    w.write_u8(1);
                    w.write_str(s);
                }
                None => w.write_u8(0),
            }
        }
    }

    w.write_u32(model.nodes.len() as u32);
    for nd in &model.nodes {
        w.write_str(&nd.name);
        for v in nd.transform.position.to_array() {
            w.write_f32(v);
        }
        for v in nd.transform.rotation.to_array() {
            w.write_f32(v);
        }
        for v in nd.transform.scale.to_array() {
            w.write_f32(v);
        }
        w.write_u32(nd.parent.unwrap_or(u32::MAX));
        w.write_u32(nd.mesh.unwrap_or(u32::MAX));
        w.write_u32(nd.light.unwrap_or(u32::MAX));
        w.write_u32(nd.layer_mask);
        w.write_u32(nd.children.len() as u32);
        for &c in &nd.children {
            w.write_u32(c);
        }
    }
    w.write_u32(model.roots.len() as u32);
    for &r in &model.roots {
        w.write_u32(r);
    }
    w.buf
}

pub fn decode_model_payload(
    payload: &[u8],
    budget_core: Option<&Arc<BudgetCore>>,
) -> Result<Model, String> {
    let mut r = BinReader::new(payload);
    let src = SourceFormat::from_u8(r.read_u8()?)?;
    let m_count = r.read_u32()? as usize;
    if m_count > 10_000 {
        return Err(err_limit("ASKAS model mesh count exceeds limit"));
    }
    let mut meshes = Vec::with_capacity(m_count);
    for _ in 0..m_count {
        let name = r.read_str()?;
        let m_src = SourceFormat::from_u8(r.read_u8()?)?;
        let vc = r.read_u32()? as usize;
        if vc > MAX_MESH_VERTICES {
            return Err(err_limit("ASKAS mesh vertex count exceeds limit"));
        }
        let mut positions = Vec::with_capacity(vc);
        for _ in 0..vc {
            positions.push([r.read_f32()?, r.read_f32()?, r.read_f32()?]);
        }
        let mut normals = Vec::with_capacity(vc);
        for _ in 0..vc {
            normals.push([r.read_f32()?, r.read_f32()?, r.read_f32()?]);
        }
        let mut uvs = Vec::with_capacity(vc);
        for _ in 0..vc {
            uvs.push([r.read_f32()?, r.read_f32()?]);
        }
        let has_cols = r.read_u8()? != 0;
        let colors = if has_cols {
            let mut cols = Vec::with_capacity(vc);
            for _ in 0..vc {
                cols.push([r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?]);
            }
            Some(cols)
        } else {
            None
        };
        let ic = r.read_u32()? as usize;
        if ic > MAX_MESH_TRIS * 3 {
            return Err(err_limit("ASKAS mesh index count exceeds limit"));
        }
        let mut indices = Vec::with_capacity(ic);
        for _ in 0..ic {
            indices.push(r.read_u32()?);
        }
        let gc = r.read_u32()? as usize;
        if gc > 100_000 {
            return Err(err_limit("ASKAS group count exceeds limit"));
        }
        let mut groups = Vec::with_capacity(gc);
        for _ in 0..gc {
            let fi = r.read_u32()?;
            let cnt = r.read_u32()?;
            let mi = r.read_u32()?;
            groups.push(GroupRange {
                first_index: fi,
                index_count: cnt,
                material_index: if mi == u32::MAX { None } else { Some(mi) },
            });
        }
        meshes.push(MeshData {
            name,
            positions,
            normals,
            uvs,
            colors,
            indices,
            groups,
            splat_weights: None,
            aabb: Aabb::new(Vec3::zero(), Vec3::zero()),
            stats: MeshStats::default(),
            source: m_src,
            budget: None,
        });
    }

    let mat_count = r.read_u32()? as usize;
    if mat_count > 10_000 {
        return Err(err_limit("ASKAS material count exceeds limit"));
    }
    let mut materials = Vec::with_capacity(mat_count);
    for _ in 0..mat_count {
        let name = r.read_str()?;
        let base_color = Vec3::new(r.read_f32()?, r.read_f32()?, r.read_f32()?);
        let metallic = r.read_f32()?;
        let roughness = r.read_f32()?;
        let emissive = Vec3::new(r.read_f32()?, r.read_f32()?, r.read_f32()?);
        let alpha_mode = AlphaMode::from_u8(r.read_u8()?)?;
        let alpha_cutoff = r.read_f32()?;
        let normal_scale = r.read_f32()?;
        let occlusion_strength = r.read_f32()?;
        let double_sided = r.read_u8()? != 0;
        let unlit = r.read_u8()? != 0;
        let mut read_tex = || -> Result<Option<String>, String> {
            if r.read_u8()? == 0 {
                Ok(None)
            } else {
                Ok(Some(r.read_str()?))
            }
        };
        let textures = MaterialTextures {
            base_color: read_tex()?,
            normal: read_tex()?,
            metallic_roughness: read_tex()?,
            emissive: read_tex()?,
            occlusion: read_tex()?,
            height: read_tex()?,
        };
        materials.push(Material {
            name,
            base_color,
            metallic,
            roughness,
            emissive,
            alpha_mode,
            alpha_cutoff,
            normal_scale,
            occlusion_strength,
            double_sided,
            textures,
            unlit,
        });
    }

    let nc = r.read_u32()? as usize;
    if nc > MAX_SCENE_NODES {
        return Err(err_limit("ASKAS node count exceeds limit"));
    }
    let mut nodes = Vec::with_capacity(nc);
    for _ in 0..nc {
        let name = r.read_str()?;
        let pos = Vec3::new(r.read_f32()?, r.read_f32()?, r.read_f32()?);
        let rot = Quat::new(r.read_f32()?, r.read_f32()?, r.read_f32()?, r.read_f32()?);
        let scale = Vec3::new(r.read_f32()?, r.read_f32()?, r.read_f32()?);
        let par = r.read_u32()?;
        let msh = r.read_u32()?;
        let lgt = r.read_u32()?;
        let layer_mask = r.read_u32()?;
        let ch_len = r.read_u32()? as usize;
        if ch_len > MAX_SCENE_NODES {
            return Err(err_corrupt("ASKAS child count exceeds limit"));
        }
        let mut children = Vec::with_capacity(ch_len);
        for _ in 0..ch_len {
            children.push(r.read_u32()?);
        }
        nodes.push(Node3D {
            name,
            transform: Transform {
                position: pos,
                rotation: rot,
                scale,
            },
            parent: if par == u32::MAX { None } else { Some(par) },
            children,
            mesh: if msh == u32::MAX { None } else { Some(msh) },
            light: if lgt == u32::MAX { None } else { Some(lgt) },
            layer_mask,
            aabb_local: None,
            aabb_world: Aabb::new(Vec3::zero(), Vec3::zero()),
        });
    }
    let rc = r.read_u32()? as usize;
    if rc > MAX_SCENE_NODES {
        return Err(err_corrupt("ASKAS root count exceeds limit"));
    }
    let mut roots = Vec::with_capacity(rc);
    for _ in 0..rc {
        roots.push(r.read_u32()?);
    }

    let mut model = Model {
        meshes,
        materials,
        nodes,
        roots,
        aabb: Aabb::new(Vec3::zero(), Vec3::zero()),
        warnings: Vec::new(),
        source: src,
        budget: None,
    };
    verify_model(&mut model)?;
    let bytes = model.total_memory_bytes();
    model.budget = Some(MediaBudgetGuard::charge(budget_core, bytes)?);
    Ok(model)
}
