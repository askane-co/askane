use std::collections::HashMap;

use crate::media::temp_budget_guard;
use crate::modules::base64::decode_str as b64_decode;
use crate::modules::json::parse_limited;
use crate::scene::asset::{AssetRef, MAX_ASSET_BYTES, read_asset_ref_bytes};
use crate::scene::geom::{Aabb, Mat4, Quat, Transform, Vec3};
use crate::scene::model::{
    AlphaMode, GroupRange, MAX_MESH_TRIS, MAX_MESH_VERTICES, MAX_SCENE_NODES, Material,
    MeshData, MeshStats, Model, Node3D, SourceFormat, verify_model,
};
use crate::value::{DictMap, Value};
use crate::vm::VM;

const GLTF_MAX_JSON_NODES: usize = 100_000;
const GLTF_MAX_JSON_DEPTH: usize = 128;

fn as_dict<'a>(v: &'a Value, ctx: &str) -> Result<std::cell::Ref<'a, DictMap>, String> {
    let Value::Dict(d) = v else {
        return Err(format!("E_MESH_CORRUPT: {} expected object dict", ctx));
    };
    Ok(d.borrow())
}

fn as_list<'a>(v: &'a Value, ctx: &str) -> Result<std::cell::Ref<'a, Vec<Value>>, String> {
    let Value::List(l) = v else {
        return Err(format!("E_MESH_CORRUPT: {} expected array", ctx));
    };
    Ok(l.borrow())
}

fn val_usize(v: &Value, ctx: &str) -> Result<usize, String> {
    match v {
        Value::Int(i) if *i >= 0 => Ok(*i as usize),
        Value::Float(f) if f.is_finite() && *f >= 0.0 && f.fract() == 0.0 => Ok(*f as usize),
        _ => Err(format!("E_MESH_CORRUPT: {} expects non-negative int", ctx)),
    }
}

fn val_f32(v: &Value, ctx: &str) -> Result<f32, String> {
    let f = match v {
        Value::Int(i) => *i as f32,
        Value::Float(fl) => *fl as f32,
        _ => return Err(format!("E_MESH_CORRUPT: {} expects number", ctx)),
    };
    if !f.is_finite() {
        return Err(format!("E_MESH_CORRUPT: {} non-finite float", ctx));
    }
    Ok(f)
}

fn read_vec3(v: &Value, ctx: &str) -> Result<[f32; 3], String> {
    let l = as_list(v, ctx)?;
    if l.len() != 3 {
        return Err(format!("E_MESH_CORRUPT: {} expected 3 elements", ctx));
    }
    Ok([
        val_f32(&l[0], ctx)?,
        val_f32(&l[1], ctx)?,
        val_f32(&l[2], ctx)?,
    ])
}

fn read_vec4(v: &Value, ctx: &str) -> Result<[f32; 4], String> {
    let l = as_list(v, ctx)?;
    if l.len() != 4 {
        return Err(format!("E_MESH_CORRUPT: {} expected 4 elements", ctx));
    }
    Ok([
        val_f32(&l[0], ctx)?,
        val_f32(&l[1], ctx)?,
        val_f32(&l[2], ctx)?,
        val_f32(&l[3], ctx)?,
    ])
}

#[derive(Clone, Copy)]
struct BufferViewMeta {
    buffer: usize,
    offset: usize,
    length: usize,
    stride: Option<usize>,
}

#[derive(Clone)]
struct AccessorMeta {
    view: Option<usize>,
    offset: usize,
    component_type: u32,
    count: usize,
    acc_type: String,
    normalized: bool,
}

fn elem_components(t: &str) -> Result<usize, String> {
    match t {
        "SCALAR" => Ok(1),
        "VEC2" => Ok(2),
        "VEC3" => Ok(3),
        "VEC4" => Ok(4),
        "MAT2" => Ok(4),
        "MAT3" => Ok(9),
        "MAT4" => Ok(16),
        other => Err(format!("E_MESH_CORRUPT: unknown accessor type '{}'", other)),
    }
}

fn comp_byte_size(ct: u32) -> Result<usize, String> {
    match ct {
        5120 | 5121 => Ok(1),
        5122 | 5123 => Ok(2),
        5125 | 5126 => Ok(4),
        other => Err(format!("E_MESH_CORRUPT: unknown accessor componentType {}", other)),
    }
}

fn read_accessor_slice<'a>(
    acc: &AccessorMeta,
    views: &[BufferViewMeta],
    buffers: &'a [Vec<u8>],
) -> Result<(&'a [u8], usize, usize), String> {
    let Some(vi) = acc.view else {
        return Err("E_MESH_CORRUPT: accessor missing bufferView".to_string());
    };
    let view = views
        .get(vi)
        .ok_or_else(|| "E_MESH_CORRUPT: accessor bufferView index out of range".to_string())?;
    let buf = buffers
        .get(view.buffer)
        .ok_or_else(|| "E_MESH_CORRUPT: bufferView buffer index out of range".to_string())?;
    let comps = elem_components(&acc.acc_type)?;
    let csize = comp_byte_size(acc.component_type)?;
    let packed_stride = comps * csize;
    let stride = match view.stride {
        Some(s) => {
            if s == 0 || s > 252 || s < packed_stride {
                return Err(format!("E_MESH_CORRUPT: invalid bufferView byteStride {}", s));
            }
            s
        }
        None => packed_stride,
    };
    let view_end = view
        .offset
        .checked_add(view.length)
        .ok_or_else(|| "E_MESH_CORRUPT: bufferView offset+length overflow".to_string())?;
    if view_end > buf.len() {
        return Err("E_MESH_CORRUPT: bufferView exceeds buffer byteLength".to_string());
    }
    if acc.count == 0 {
        return Ok((&[], stride, comps));
    }
    let span = (acc.count - 1)
        .checked_mul(stride)
        .and_then(|s| s.checked_add(packed_stride))
        .ok_or_else(|| "E_MESH_CORRUPT: accessor byte span overflow".to_string())?;
    let start = view
        .offset
        .checked_add(acc.offset)
        .ok_or_else(|| "E_MESH_CORRUPT: accessor offset overflow".to_string())?;
    let end = start
        .checked_add(span)
        .ok_or_else(|| "E_MESH_CORRUPT: accessor end overflow".to_string())?;
    if end > view_end {
        return Err("E_MESH_CORRUPT: accessor exceeds bufferView bounds".to_string());
    }
    Ok((&buf[start..end], stride, comps))
}

#[inline]
fn read_component_f32(slice: &[u8], ct: u32, norm: bool) -> Result<f32, String> {
    let f = match ct {
        5126 => f32::from_le_bytes(slice[..4].try_into().unwrap()),
        5121 => {
            let u = slice[0] as f32;
            if norm { u / 255.0 } else { u }
        }
        5120 => {
            let s = (slice[0] as i8) as f32;
            if norm { (s / 127.0).max(-1.0) } else { s }
        }
        5123 => {
            let u = u16::from_le_bytes([slice[0], slice[1]]) as f32;
            if norm { u / 65535.0 } else { u }
        }
        5122 => {
            let s = i16::from_le_bytes([slice[0], slice[1]]) as f32;
            if norm { (s / 32767.0).max(-1.0) } else { s }
        }
        other => return Err(format!("E_MESH_CORRUPT: unsupported float componentType {}", other)),
    };
    if !f.is_finite() {
        return Err("E_MESH_CORRUPT: non-finite vertex attribute float".to_string());
    }
    Ok(f)
}

fn read_accessor_f32_vec(
    acc: &AccessorMeta,
    expected_comps: usize,
    views: &[BufferViewMeta],
    buffers: &[Vec<u8>],
) -> Result<Vec<f32>, String> {
    let (data, stride, comps) = read_accessor_slice(acc, views, buffers)?;
    if comps != expected_comps {
        return Err(format!(
            "E_MESH_CORRUPT: expected {} components, got {}",
            expected_comps, comps
        ));
    }
    let csize = comp_byte_size(acc.component_type)?;
    let mut out = Vec::with_capacity(acc.count * comps);
    for i in 0..acc.count {
        let base = i * stride;
        for c in 0..comps {
            let v = read_component_f32(&data[base + c * csize..], acc.component_type, acc.normalized)?;
            out.push(v);
        }
    }
    Ok(out)
}

fn read_accessor_indices(
    acc: &AccessorMeta,
    views: &[BufferViewMeta],
    buffers: &[Vec<u8>],
) -> Result<Vec<u32>, String> {
    if acc.count % 3 != 0 {
        return Err("E_MESH_CORRUPT: index accessor count must be multiple of 3".to_string());
    }
    if acc.count / 3 > MAX_MESH_TRIS {
        return Err("E_MESH_LIMIT: glTF triangle count exceeds 4M limit".to_string());
    }
    let (data, stride, comps) = read_accessor_slice(acc, views, buffers)?;
    if comps != 1 {
        return Err("E_MESH_CORRUPT: index accessor must be SCALAR".to_string());
    }
    let mut out = Vec::with_capacity(acc.count);
    for i in 0..acc.count {
        let off = i * stride;
        let idx = match acc.component_type {
            5121 => data[off] as u32,
            5123 => u16::from_le_bytes([data[off], data[off + 1]]) as u32,
            5125 => u32::from_le_bytes(data[off..off + 4].try_into().unwrap()),
            other => {
                return Err(format!(
                    "E_MESH_CORRUPT: invalid index componentType {}",
                    other
                ));
            }
        };
        out.push(idx);
    }
    Ok(out)
}

fn decode_data_uri(uri: &str) -> Result<Vec<u8>, String> {
    let rest = uri
        .strip_prefix("data:")
        .ok_or_else(|| "E_MESH_CORRUPT: invalid data URI".to_string())?;
    let Some((meta, payload)) = rest.split_once(',') else {
        return Err("E_MESH_CORRUPT: malformed data URI".to_string());
    };
    if !meta.to_ascii_lowercase().contains(";base64") {
        return Err("E_MESH_UNSUPPORTED: only base64 data URIs are supported".to_string());
    }
    let bytes = b64_decode(payload, false)
        .map_err(|e| format!("E_MESH_CORRUPT: base64 buffer decode failed: {}", e))?;
    if bytes.len() > MAX_ASSET_BYTES {
        return Err("E_MESH_LIMIT: data URI buffer exceeds 256 MiB limit".to_string());
    }
    Ok(bytes)
}

fn parse_glb_chunks(raw: &[u8]) -> Result<(&str, Option<&[u8]>), String> {
    if raw.len() < 12 {
        return Err("E_MESH_CORRUPT: GLB header truncated".to_string());
    }
    let magic = u32::from_le_bytes(raw[0..4].try_into().unwrap());
    let version = u32::from_le_bytes(raw[4..8].try_into().unwrap());
    let total_len = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as usize;
    if magic != 0x4654_6C67 {
        return Err("E_MESH_CORRUPT: invalid GLB magic".to_string());
    }
    if version != 2 {
        return Err(format!("E_MESH_UNSUPPORTED: unsupported GLB version {}", version));
    }
    if total_len < 12 || total_len > raw.len() || total_len > MAX_ASSET_BYTES {
        return Err("E_MESH_CORRUPT: invalid GLB total length".to_string());
    }
    let mut pos = 12usize;
    let mut json_chunk: Option<&[u8]> = None;
    let mut bin_chunk: Option<&[u8]> = None;
    let mut chunk_idx = 0usize;
    while pos < total_len {
        if pos + 8 > total_len {
            return Err("E_MESH_CORRUPT: truncated GLB chunk header".to_string());
        }
        let chunk_len = u32::from_le_bytes(raw[pos..pos + 4].try_into().unwrap()) as usize;
        let chunk_type = u32::from_le_bytes(raw[pos + 4..pos + 8].try_into().unwrap());
        pos += 8;
        if (chunk_len & 3) != 0 {
            return Err("E_MESH_CORRUPT: GLB chunk length not 4-byte aligned".to_string());
        }
        let end = pos
            .checked_add(chunk_len)
            .ok_or_else(|| "E_MESH_CORRUPT: GLB chunk length overflow".to_string())?;
        if end > total_len {
            return Err("E_MESH_CORRUPT: GLB chunk exceeds total length".to_string());
        }
        let payload = &raw[pos..end];
        if chunk_idx == 0 {
            if chunk_type != 0x4E4F_534A {
                return Err("E_MESH_CORRUPT: GLB first chunk must be JSON".to_string());
            }
            json_chunk = Some(payload);
        } else if chunk_type == 0x004E_4942 {
            if bin_chunk.is_some() {
                return Err("E_MESH_CORRUPT: multiple GLB BIN chunks".to_string());
            }
            bin_chunk = Some(payload);
        }
        pos = end;
        chunk_idx += 1;
    }
    let jbytes = json_chunk.ok_or_else(|| "E_MESH_CORRUPT: GLB missing JSON chunk".to_string())?;
    let jstr = std::str::from_utf8(jbytes)
        .map_err(|_| "E_MESH_CORRUPT: GLB JSON chunk is not valid UTF-8".to_string())?
        .trim_end_matches(|c: char| c == '\0' || c.is_ascii_whitespace());
    Ok((jstr, bin_chunk))
}

fn extension_allowed(ext: &str) -> bool {
    ext == "KHR_texture_transform" || ext.starts_with("KHR_materials_")
}

pub fn load_gltf_model(
    vm: &mut VM,
    gltf_ref: &AssetRef,
    raw_bytes: &[u8],
    is_glb: bool,
) -> Result<Model, String> {
    let budget_core = std::sync::Arc::clone(vm.budget_core());
    let _tmp = temp_budget_guard(Some(&budget_core), raw_bytes.len())?;
    let (json_str, bin_chunk) = if is_glb {
        parse_glb_chunks(raw_bytes)?
    } else {
        let s = std::str::from_utf8(raw_bytes)
            .map_err(|_| "E_MESH_CORRUPT: glTF file is not valid UTF-8".to_string())?;
        (s, None)
    };

    let root_val = parse_limited(
        json_str,
        GLTF_MAX_JSON_NODES,
        GLTF_MAX_JSON_DEPTH,
        4 * 1024 * 1024,
        Some(&|| vm.check_timeout()),
    )?;
    let root = as_dict(&root_val, "glTF root")?;

    let mut warnings = Vec::new();
    if let Some(exts) = root.get("extensionsUsed") {
        for e in as_list(exts, "extensionsUsed")?.iter() {
            if let Some(s) = e.as_str() {
                if !extension_allowed(s) {
                    warnings.push(format!("W0109: ignoring optional glTF extension '{}'", s));
                }
            }
        }
    }
    if let Some(exts_req) = root.get("extensionsRequired") {
        for e in as_list(exts_req, "extensionsRequired")?.iter() {
            if let Some(s) = e.as_str() {
                if !extension_allowed(s) {
                    return Err(format!(
                        "E_MESH_UNSUPPORTED: required glTF extension '{}' is not supported",
                        s
                    ));
                }
            }
        }
    }

    // Load buffers
    let mut buffers: Vec<Vec<u8>> = Vec::new();
    if let Some(bufs_val) = root.get("buffers") {
        for (bi, bval) in as_list(bufs_val, "buffers")?.iter().enumerate() {
            let bd = as_dict(bval, "buffer")?;
            let decl_len = val_usize(
                bd.get("byteLength")
                    .ok_or_else(|| "E_MESH_CORRUPT: glTF buffer missing byteLength".to_string())?,
                "buffer.byteLength",
            )?;
            if decl_len > MAX_ASSET_BYTES {
                return Err("E_MESH_LIMIT: glTF buffer declared byteLength exceeds 256 MiB".to_string());
            }
            let bytes = if let Some(uri_val) = bd.get("uri") {
                let uri = uri_val
                    .as_str()
                    .ok_or_else(|| "E_MESH_CORRUPT: buffer.uri must be str".to_string())?;
                if uri.starts_with("data:") {
                    decode_data_uri(uri)?
                } else {
                    let child = gltf_ref.join_relative(uri)?;
                    read_asset_ref_bytes(vm, &child)?.0
                }
            } else if bi == 0 {
                let Some(bin) = bin_chunk else {
                    return Err("E_MESH_CORRUPT: glTF buffer[0] has no uri or GLB BIN chunk".to_string());
                };
                bin.to_vec()
            } else {
                return Err(format!("E_MESH_CORRUPT: glTF buffer[{}] missing uri", bi));
            };
            if bytes.len() < decl_len {
                return Err("E_MESH_CORRUPT: glTF buffer smaller than declared byteLength".to_string());
            }
            buffers.push(bytes);
        }
    }

    // Parse bufferViews
    let mut views: Vec<BufferViewMeta> = Vec::new();
    if let Some(bv_val) = root.get("bufferViews") {
        for v in as_list(bv_val, "bufferViews")?.iter() {
            let d = as_dict(v, "bufferView")?;
            let buffer = val_usize(
                d.get("buffer").ok_or("E_MESH_CORRUPT: bufferView missing buffer")?,
                "bufferView.buffer",
            )?;
            let offset = match d.get("byteOffset") {
                Some(x) => val_usize(x, "bufferView.byteOffset")?,
                None => 0,
            };
            let length = val_usize(
                d.get("byteLength").ok_or("E_MESH_CORRUPT: bufferView missing byteLength")?,
                "bufferView.byteLength",
            )?;
            let stride = match d.get("byteStride") {
                Some(x) => Some(val_usize(x, "bufferView.byteStride")?),
                None => None,
            };
            views.push(BufferViewMeta {
                buffer,
                offset,
                length,
                stride,
            });
        }
    }

    // Parse accessors
    let mut accessors: Vec<AccessorMeta> = Vec::new();
    if let Some(acc_val) = root.get("accessors") {
        for a in as_list(acc_val, "accessors")?.iter() {
            let d = as_dict(a, "accessor")?;
            if d.contains_key("sparse") {
                return Err(
                    "E_MESH_UNSUPPORTED: glTF sparse accessors are not supported in 0.20 (export dense buffers)"
                        .to_string(),
                );
            }
            let view = match d.get("bufferView") {
                Some(x) => Some(val_usize(x, "accessor.bufferView")?),
                None => None,
            };
            let offset = match d.get("byteOffset") {
                Some(x) => val_usize(x, "accessor.byteOffset")?,
                None => 0,
            };
            let ct = val_usize(
                d.get("componentType").ok_or("E_MESH_CORRUPT: accessor missing componentType")?,
                "accessor.componentType",
            )? as u32;
            let count = val_usize(
                d.get("count").ok_or("E_MESH_CORRUPT: accessor missing count")?,
                "accessor.count",
            )?;
            let acc_type = d
                .get("type")
                .and_then(|v| v.as_str())
                .ok_or("E_MESH_CORRUPT: accessor missing type")?
                .to_string();
            let normalized = matches!(d.get("normalized"), Some(Value::Bool(true)));
            accessors.push(AccessorMeta {
                view,
                offset,
                component_type: ct,
                count,
                acc_type,
                normalized,
            });
        }
    }

    // Resolve images/textures -> Option<String> URIs
    let mut image_uris: Vec<Option<String>> = Vec::new();
    if let Some(imgs_val) = root.get("images") {
        for im in as_list(imgs_val, "images")?.iter() {
            let d = as_dict(im, "image")?;
            let resolved = if let Some(u) = d.get("uri").and_then(|x| x.as_str()) {
                if u.starts_with("data:") {
                    Some(u.to_string())
                } else {
                    Some(gltf_ref.join_relative(u)?.as_str().to_string())
                }
            } else {
                None
            };
            image_uris.push(resolved);
        }
    }
    let mut tex_sources: Vec<Option<String>> = Vec::new();
    if let Some(texs_val) = root.get("textures") {
        for t in as_list(texs_val, "textures")?.iter() {
            let d = as_dict(t, "texture")?;
            let src_uri = match d.get("source") {
                Some(x) => {
                    let idx = val_usize(x, "texture.source")?;
                    image_uris.get(idx).cloned().flatten()
                }
                None => None,
            };
            tex_sources.push(src_uri);
        }
    }
    let lookup_tex_uri = |tex_ref: Option<&Value>| -> Result<Option<String>, String> {
        let Some(tv) = tex_ref else {
            return Ok(None);
        };
        let td = as_dict(tv, "textureInfo")?;
        let idx = val_usize(
            td.get("index").ok_or("E_MESH_CORRUPT: textureInfo missing index")?,
            "textureInfo.index",
        )?;
        Ok(tex_sources.get(idx).cloned().flatten())
    };

    // Parse materials
    let mut materials: Vec<Material> = Vec::new();
    if let Some(mats_val) = root.get("materials") {
        for (mi, mv) in as_list(mats_val, "materials")?.iter().enumerate() {
            let d = as_dict(mv, "material")?;
            let mut mat = Material::default();
            mat.name = d
                .get("name")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("gltf_mat_{}", mi));
            if let Some(pbr_val) = d.get("pbrMetallicRoughness") {
                let pbr = as_dict(pbr_val, "pbrMetallicRoughness")?;
                if let Some(bc) = pbr.get("baseColorFactor") {
                    let c = read_vec4(bc, "baseColorFactor")?;
                    mat.base_color = Vec3::new(c[0], c[1], c[2]);
                }
                if let Some(mf) = pbr.get("metallicFactor") {
                    mat.metallic = val_f32(mf, "metallicFactor")?;
                }
                if let Some(rf) = pbr.get("roughnessFactor") {
                    mat.roughness = val_f32(rf, "roughnessFactor")?;
                }
                mat.textures.base_color = lookup_tex_uri(pbr.get("baseColorTexture"))?;
                mat.textures.metallic_roughness =
                    lookup_tex_uri(pbr.get("metallicRoughnessTexture"))?;
            }
            if let Some(em) = d.get("emissiveFactor") {
                let c = read_vec3(em, "emissiveFactor")?;
                mat.emissive = Vec3::from_array(c);
            }
            mat.textures.normal = lookup_tex_uri(d.get("normalTexture"))?;
            if let Some(nt) = d.get("normalTexture") {
                let ntd = as_dict(nt, "normalTexture")?;
                if let Some(sc) = ntd.get("scale") {
                    mat.normal_scale = val_f32(sc, "normalTexture.scale")?;
                }
            }
            mat.textures.occlusion = lookup_tex_uri(d.get("occlusionTexture"))?;
            if let Some(ot) = d.get("occlusionTexture") {
                let otd = as_dict(ot, "occlusionTexture")?;
                if let Some(st) = otd.get("strength") {
                    mat.occlusion_strength = val_f32(st, "occlusionTexture.strength")?;
                }
            }
            mat.textures.emissive = lookup_tex_uri(d.get("emissiveTexture"))?;
            if let Some(am) = d.get("alphaMode").and_then(|x| x.as_str()) {
                mat.alpha_mode = match am {
                    "OPAQUE" => AlphaMode::Opaque,
                    "BLEND" => AlphaMode::Blend,
                    "MASK" => AlphaMode::Mask,
                    other => {
                        return Err(format!("E_MESH_UNSUPPORTED: unknown alphaMode '{}'", other));
                    }
                };
            }
            if let Some(ac) = d.get("alphaCutoff") {
                mat.alpha_cutoff = val_f32(ac, "alphaCutoff")?;
            }
            if let Some(Value::Bool(ds)) = d.get("doubleSided") {
                mat.double_sided = *ds;
            }
            if let Some(Value::Dict(ext_d)) = d.get("extensions") {
                if ext_d.borrow().contains_key("KHR_materials_unlit") {
                    mat.unlit = true;
                }
            }
            materials.push(mat);
        }
    }
    if materials.is_empty() {
        materials.push(Material::default());
    }

    // Parse meshes (merge primitives per glTF mesh as Groups)
    let mut meshes: Vec<MeshData> = Vec::new();
    if let Some(meshes_val) = root.get("meshes") {
        for (mi, mv) in as_list(meshes_val, "meshes")?.iter().enumerate() {
            let md = as_dict(mv, "mesh")?;
            let mname = md
                .get("name")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("mesh_{}", mi));
            let prims = as_list(
                md.get("primitives").ok_or("E_MESH_CORRUPT: glTF mesh missing primitives")?,
                "mesh.primitives",
            )?;

            let mut positions: Vec<[f32; 3]> = Vec::new();
            let mut normals: Vec<[f32; 3]> = Vec::new();
            let mut uvs: Vec<[f32; 2]> = Vec::new();
            let mut colors: Option<Vec<[f32; 4]>> = None;
            let mut indices: Vec<u32> = Vec::new();
            let mut groups: Vec<GroupRange> = Vec::new();

            for pv in prims.iter() {
                let pd = as_dict(pv, "primitive")?;
                let mode = match pd.get("mode") {
                    Some(x) => val_usize(x, "primitive.mode")?,
                    None => 4, // TRIANGLES
                };
                if mode != 4 {
                    return Err(format!(
                        "E_MESH_UNSUPPORTED: primitive mode {} (only TRIANGLES=4 supported in 0.20)",
                        mode
                    ));
                }
                let attrs = as_dict(
                    pd.get("attributes").ok_or("E_MESH_CORRUPT: primitive missing attributes")?,
                    "primitive.attributes",
                )?;
                let pos_idx = val_usize(
                    attrs.get("POSITION").ok_or("E_MESH_CORRUPT: primitive missing POSITION")?,
                    "POSITION accessor",
                )?;
                let pos_acc = accessors
                    .get(pos_idx)
                    .ok_or("E_MESH_CORRUPT: POSITION accessor out of bounds")?;
                let pos_f32 = read_accessor_f32_vec(pos_acc, 3, &views, &buffers)?;
                let prim_vcount = pos_acc.count;
                let base_v = positions.len();
                if base_v + prim_vcount > MAX_MESH_VERTICES {
                    return Err("E_MESH_LIMIT: glTF mesh vertex count exceeds 2M limit".to_string());
                }
                for p in pos_f32.chunks_exact(3) {
                    positions.push([p[0], p[1], p[2]]);
                }

                let norm_f32 = if let Some(nv) = attrs.get("NORMAL") {
                    let ni = val_usize(nv, "NORMAL accessor")?;
                    let nacc = accessors
                        .get(ni)
                        .ok_or("E_MESH_CORRUPT: NORMAL accessor out of bounds")?;
                    if nacc.count != prim_vcount {
                        return Err("E_MESH_CORRUPT: NORMAL count mismatch".to_string());
                    }
                    read_accessor_f32_vec(nacc, 3, &views, &buffers)?
                } else {
                    vec![0.0; prim_vcount * 3]
                };
                let mut prim_normals: Vec<[f32; 3]> = norm_f32
                    .chunks_exact(3)
                    .map(|n| [n[0], n[1], n[2]])
                    .collect();

                if let Some(uvv) = attrs.get("TEXCOORD_0") {
                    let uvi = val_usize(uvv, "TEXCOORD_0 accessor")?;
                    let uvacc = accessors
                        .get(uvi)
                        .ok_or("E_MESH_CORRUPT: TEXCOORD_0 accessor out of bounds")?;
                    if uvacc.count != prim_vcount {
                        return Err("E_MESH_CORRUPT: TEXCOORD_0 count mismatch".to_string());
                    }
                    let uv_f32 = read_accessor_f32_vec(uvacc, 2, &views, &buffers)?;
                    for u in uv_f32.chunks_exact(2) {
                        uvs.push([u[0], u[1]]);
                    }
                } else {
                    uvs.resize(uvs.len() + prim_vcount, [0.0, 0.0]);
                }

                if let Some(cv) = attrs.get("COLOR_0") {
                    let ci = val_usize(cv, "COLOR_0 accessor")?;
                    let cacc = accessors
                        .get(ci)
                        .ok_or("E_MESH_CORRUPT: COLOR_0 accessor out of bounds")?;
                    if cacc.count != prim_vcount {
                        return Err("E_MESH_CORRUPT: COLOR_0 count mismatch".to_string());
                    }
                    let dst_cols = colors.get_or_insert_with(|| vec![[1.0, 1.0, 1.0, 1.0]; base_v]);
                    let ccomps = elem_components(&cacc.acc_type)?;
                    let cf32 = read_accessor_f32_vec(cacc, ccomps, &views, &buffers)?;
                    for c in cf32.chunks_exact(ccomps) {
                        let rgba = if ccomps == 4 {
                            [c[0], c[1], c[2], c[3]]
                        } else {
                            [c[0], c[1], c[2], 1.0]
                        };
                        dst_cols.push(rgba);
                    }
                } else if let Some(dst_cols) = &mut colors {
                    dst_cols.resize(dst_cols.len() + prim_vcount, [1.0, 1.0, 1.0, 1.0]);
                }

                let prim_indices = if let Some(iv) = pd.get("indices") {
                    let ii = val_usize(iv, "primitive.indices")?;
                    let iacc = accessors
                        .get(ii)
                        .ok_or("E_MESH_CORRUPT: indices accessor out of bounds")?;
                    let raw_idx = read_accessor_indices(iacc, &views, &buffers)?;
                    for &k in &raw_idx {
                        if (k as usize) >= prim_vcount {
                            return Err("E_MESH_CORRUPT: glTF primitive index out of bounds".to_string());
                        }
                    }
                    raw_idx
                } else {
                    if prim_vcount % 3 != 0 {
                        return Err("E_MESH_CORRUPT: unindexed primitive count not multiple of 3".to_string());
                    }
                    (0..prim_vcount as u32).collect()
                };

                if !attrs.contains_key("NORMAL") {
                    for tri in prim_indices.chunks_exact(3) {
                        let a = tri[0] as usize;
                        let b = tri[1] as usize;
                        let c = tri[2] as usize;
                        let p0 = Vec3::from_array(positions[base_v + a]);
                        let p1 = Vec3::from_array(positions[base_v + b]);
                        let p2 = Vec3::from_array(positions[base_v + c]);
                        let n = p1.sub(p0).cross(p2.sub(p0)).normalize();
                        let na = if n.len_sq() > 1e-12 { n.to_array() } else { [0.0, 1.0, 0.0] };
                        prim_normals[a] = na;
                        prim_normals[b] = na;
                        prim_normals[c] = na;
                    }
                }
                normals.extend_from_slice(&prim_normals);

                let first_index = indices.len() as u32;
                let index_count = prim_indices.len() as u32;
                if (indices.len() + prim_indices.len()) / 3 > MAX_MESH_TRIS {
                    return Err("E_MESH_LIMIT: glTF mesh triangle count exceeds 4M limit".to_string());
                }
                for idx in prim_indices {
                    indices.push((base_v as u32) + idx);
                }
                let mat_idx = match pd.get("material") {
                    Some(x) => {
                        let i = val_usize(x, "primitive.material")?;
                        if i >= materials.len() {
                            return Err("E_MESH_CORRUPT: material index out of bounds".to_string());
                        }
                        Some(i as u32)
                    }
                    None => Some(0),
                };
                if index_count > 0 {
                    groups.push(GroupRange {
                        first_index,
                        index_count,
                        material_index: mat_idx,
                    });
                }
            }

            meshes.push(MeshData {
                name: mname,
                positions,
                normals,
                uvs,
                colors,
                indices,
                groups,
                splat_weights: None,
                aabb: Aabb::new(Vec3::zero(), Vec3::zero()),
                stats: MeshStats::default(),
                source: if is_glb { SourceFormat::Glb } else { SourceFormat::Gltf },
                budget: None,
            });
        }
    }

    // Parse nodes
    let mut nodes: Vec<Node3D> = Vec::new();
    let mut parent_map: HashMap<usize, usize> = HashMap::new();
    if let Some(nodes_val) = root.get("nodes") {
        let nl = as_list(nodes_val, "nodes")?;
        if nl.len() > MAX_SCENE_NODES {
            return Err("E_MESH_LIMIT: glTF node count exceeds 100000 limit".to_string());
        }
        for (ni, nv) in nl.iter().enumerate() {
            let nd = as_dict(nv, "node")?;
            let name = nd
                .get("name")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("node_{}", ni));
            let transform = if let Some(mat_val) = nd.get("matrix") {
                let ml = as_list(mat_val, "node.matrix")?;
                if ml.len() != 16 {
                    return Err("E_MESH_CORRUPT: node.matrix must have 16 elements".to_string());
                }
                let mut arr = [0.0f32; 16];
                for i in 0..16 {
                    arr[i] = val_f32(&ml[i], "node.matrix")?;
                }
                Transform::from_mat4(&Mat4::from_array(arr))?
            } else {
                let t = match nd.get("translation") {
                    Some(v) => Vec3::from_array(read_vec3(v, "node.translation")?),
                    None => Vec3::zero(),
                };
                let r = match nd.get("rotation") {
                    Some(v) => {
                        let q = read_vec4(v, "node.rotation")?;
                        Quat::new(q[0], q[1], q[2], q[3]).normalize()
                    }
                    None => Quat::identity(),
                };
                let s = match nd.get("scale") {
                    Some(v) => Vec3::from_array(read_vec3(v, "node.scale")?),
                    None => Vec3::one(),
                };
                Transform {
                    position: t,
                    rotation: r,
                    scale: s,
                }
            };
            let mesh = match nd.get("mesh") {
                Some(x) => Some(val_usize(x, "node.mesh")? as u32),
                None => None,
            };
            let mut children = Vec::new();
            if let Some(ch_val) = nd.get("children") {
                for cv in as_list(ch_val, "node.children")?.iter() {
                    let ci = val_usize(cv, "node.children[]")?;
                    if parent_map.contains_key(&ci) {
                        warnings.push("W0106: glTF child node shared across parents".to_string());
                    } else {
                        parent_map.insert(ci, ni);
                    }
                    children.push(ci as u32);
                }
            }
            nodes.push(Node3D {
                name,
                transform,
                parent: None,
                children,
                mesh,
                light: None,
                layer_mask: 1,
                aabb_local: None,
                aabb_world: Aabb::new(Vec3::zero(), Vec3::zero()),
            });
        }
        for (child, par) in parent_map {
            if let Some(node) = nodes.get_mut(child) {
                node.parent = Some(par as u32);
            }
        }
    }

    let mut roots: Vec<u32> = Vec::new();
    if let Some(scenes_val) = root.get("scenes") {
        let scenes = as_list(scenes_val, "scenes")?;
        let active_idx = match root.get("scene") {
            Some(x) => val_usize(x, "scene")?,
            None => 0,
        };
        if let Some(sv) = scenes.get(active_idx) {
            let sd = as_dict(sv, "scene")?;
            if let Some(rn) = sd.get("nodes") {
                for r in as_list(rn, "scene.nodes")?.iter() {
                    roots.push(val_usize(r, "scene.nodes[]")? as u32);
                }
            }
        }
    }

    let mut model = Model {
        meshes,
        materials,
        nodes,
        roots,
        aabb: Aabb::new(Vec3::zero(), Vec3::zero()),
        warnings,
        source: if is_glb { SourceFormat::Glb } else { SourceFormat::Gltf },
        budget: None,
    };
    verify_model(&mut model)?;
    let bytes = model.total_memory_bytes();
    model.budget = Some(crate::media::MediaBudgetGuard::charge(Some(vm.budget_core()), bytes)?);
    Ok(model)
}
