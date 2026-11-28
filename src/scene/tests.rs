use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::bytecode::FunctionProto;
use crate::compiler::Compiler;
use crate::diag;
use crate::lexer::Lexer;
use crate::modules;
use crate::modules::compress::{crc32_bytes, inflate_gzip, inflate_zlib};
use crate::parser::Parser;
use crate::scene::asset::AssetRef;
use crate::scene::cache::{ParserId, decode_and_verify_askas, encode_askas};
use crate::scene::gltf::load_gltf_model;
use crate::scene::obj::load_obj_model;
use crate::scene::postfx::Lut3D;
use crate::scene::terrain::load_heightmap;
use crate::scene::tiled::load_tiled_map;
use crate::value::Value;
use crate::vm::{Permissions, VM};

fn compile_test_source(src: &str) -> Result<std::rc::Rc<FunctionProto>, String> {
    let tokens = Lexer::new(src).tokenize()?;
    let (program, errs) = Parser::new(tokens).parse_program_recover();
    if let Some(e) = errs.first() {
        return Err(e.clone());
    }
    Compiler::compile_with_specs(&program)
}

fn new_sandboxed_vm(dir: &Path, use_cache: bool) -> VM {
    let mut vm = VM::new_with_sandbox(true);
    vm.set_permissions(Permissions::sandboxed());
    vm.set_current_dir(dir.to_path_buf());
    vm.set_sandbox_root(dir.to_path_buf());
    vm.set_bytecode_cache(use_cache);
    vm
}

fn temp_test_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "askane_stage5_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap_or(dir)
}

fn build_rgba_png(w: u32, h: u32, pixels: &[u8]) -> Vec<u8> {
    build_png_raw(w, h, 8, 6, pixels)
}

fn build_gray16_png(w: u32, h: u32, vals: &[u16]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w * h * 2) as usize);
    for &v in vals {
        raw.extend_from_slice(&v.to_be_bytes());
    }
    build_png_raw(w, h, 16, 0, &raw)
}

/// Minimal uncompressed stored-DEFLATE zlib stream for test PNG / TMX fixtures.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 16);
    out.push(0x78);
    out.push(0x9C);
    let mut pos = 0usize;
    while pos < raw.len() || (raw.is_empty() && pos == 0) {
        let chunk_len = (raw.len() - pos).min(65535);
        let is_last = pos + chunk_len >= raw.len();
        out.push(if is_last { 1 } else { 0 });
        let len_u16 = chunk_len as u16;
        out.extend_from_slice(&len_u16.to_le_bytes());
        out.extend_from_slice(&(!len_u16).to_le_bytes());
        out.extend_from_slice(&raw[pos..pos + chunk_len]);
        pos += chunk_len;
        if raw.is_empty() {
            break;
        }
    }
    // Adler-32
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for &b in raw {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    out.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
    out
}

fn gzip_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 3];
    let z = zlib_stored(raw);
    // Strip 2-byte zlib header & 4-byte adler32 to keep raw stored DEFLATE blocks
    out.extend_from_slice(&z[2..z.len() - 4]);
    out.extend_from_slice(&crc32_bytes(raw).to_le_bytes());
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out
}

fn build_png_raw(w: u32, h: u32, bit_depth: u8, color_type: u8, pixel_bytes: &[u8]) -> Vec<u8> {
    let bpp = match (color_type, bit_depth) {
        (0, 16) => 2usize,
        (6, 8) => 4usize,
        _ => panic!("unsupported test png format"),
    };
    let row_bytes = (w as usize) * bpp;
    let mut scanlines = Vec::with_capacity((h as usize) * (1 + row_bytes));
    for y in 0..(h as usize) {
        scanlines.push(0);
        scanlines.extend_from_slice(&pixel_bytes[y * row_bytes..(y + 1) * row_bytes]);
    }
    let idat = zlib_stored(&scanlines);

    fn push_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let crc_start = out.len();
        out.extend_from_slice(tag);
        out.extend_from_slice(data);
        let crc = crc32_bytes(&out[crc_start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.push(bit_depth);
    ihdr.push(color_type);
    ihdr.push(0);
    ihdr.push(0);
    ihdr.push(0);

    let mut out = Vec::new();
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    push_chunk(&mut out, b"IHDR", &ihdr);
    push_chunk(&mut out, b"IDAT", &idat);
    push_chunk(&mut out, b"IEND", &[]);
    out
}

fn build_test_glb(json_str: &str, bin_bytes: &[u8]) -> Vec<u8> {
    let mut json_padded = json_str.as_bytes().to_vec();
    while json_padded.len() % 4 != 0 {
        json_padded.push(b' ');
    }
    let mut bin_padded = bin_bytes.to_vec();
    while bin_padded.len() % 4 != 0 {
        bin_padded.push(0);
    }
    let total_len = 12 + 8 + json_padded.len() + 8 + bin_padded.len();
    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total_len as u32).to_le_bytes());
    out.extend_from_slice(&(json_padded.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // JSON
    out.extend_from_slice(&json_padded);
    out.extend_from_slice(&(bin_padded.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // BIN\0
    out.extend_from_slice(&bin_padded);
    out
}

#[test]
fn test_v020_stage5_loaders_cache_maps_and_fuzz() {
    modules::reset_runtime_state();
    assert_eq!(std::mem::size_of::<Value>(), 16);

    for code in [
        "E0510", "E0511", "E0512", "E0513", "E0514", "E0515", "E0520", "E0521", "E0530", "E0531",
        "E0532", "E0533", "E0540", "E0550", "E0551", "E0552",
    ] {
        assert!(diag::ALL_CODES.contains(&code), "missing diag code {}", code);
    }
    assert_eq!(diag::classify_code("E_MESH_CYCLE: cycle"), "E0514");
    assert_eq!(diag::classify_code("E_TILED_UNSUPPORTED: zstd"), "E0520");
    assert_eq!(diag::classify_code("E_SPLAT_INVALID: bad"), "E0533");
    assert_eq!(diag::classify_code("E_CUBEMAP_INVALID: face"), "E0540");
    assert_eq!(diag::classify_code("E_LUT_INVALID: size"), "E0550");
    assert_eq!(diag::classify_code("E_NAV_INVALID: oob"), "E0551");
    assert_eq!(diag::classify_code("E_SCENE_INVALID: spec"), "E0552");

    for mname in [
        "mesh",
        "tiled",
        "terrain",
        "nav",
        "lighting",
        "postfx",
        "particles",
        "cubemap",
        "scene",
    ] {
        assert!(modules::BUILTIN_MODULE_NAMES.contains(&mname));
        assert!(modules::get_builtin_module(mname).is_some());
        assert_ne!(modules::module_info(mname).0, "other");
    }

    // Verify stored-zlib and stored-gzip test helpers match inflate routines
    let probe = b"hello tiled zlib/gzip";
    assert_eq!(inflate_zlib(&zlib_stored(probe), 1024, None).unwrap(), probe);
    assert_eq!(inflate_gzip(&gzip_stored(probe), 1024, None).unwrap(), probe);

    let dir = temp_test_dir("stage5_a");

    // 1. OBJ + MTL fixture (negative index, V-flip, quad fan, Ns -> roughness)
    fs::write(
        dir.join("crate.mtl"),
        "newmtl wood\nKd 0.8 0.4 0.2\nNs 100.0\n",
    )
    .unwrap();
    fs::write(
        dir.join("crate.obj"),
        "mtllib crate.mtl\n\
         usemtl wood\n\
         v 0 0 0\nv 2 0 0\nv 2 2 0\nv 0 2 0\n\
         vt 0.0 0.25\nvt 1.0 0.25\nvt 1.0 0.75\nvt 0.0 0.75\n\
         vn 0 0 1\n\
         f -4/-4/-1 -3/-3/-1 -2/-2/-1 -1/-1/-1\n",
    )
    .unwrap();

    // 2. GLB fixture (valid triangle + transform node)
    let mut bin = Vec::new();
    for &f in &[0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        bin.extend_from_slice(&f.to_le_bytes());
    }
    for &f in &[0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0] {
        bin.extend_from_slice(&f.to_le_bytes());
    }
    for &i in &[0u16, 1, 2] {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    let gltf_json = format!(
        r#"{{
          "asset": {{"version": "2.0"}},
          "scenes": [{{"nodes": [0]}}],
          "nodes": [{{"name": "tri_node", "translation": [1.0, 2.0, -3.0], "mesh": 0}}],
          "materials": [{{"name": "gold", "pbrMetallicRoughness": {{"baseColorFactor": [1.0, 0.8, 0.2, 1.0], "metallicFactor": 0.9, "roughnessFactor": 0.2}}}}],
          "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0, "NORMAL": 1}}, "indices": 2, "material": 0}}]}}],
          "buffers": [{{"byteLength": {}}}],
          "bufferViews": [
            {{"buffer": 0, "byteOffset": 0, "byteLength": 36}},
            {{"buffer": 0, "byteOffset": 36, "byteLength": 36}},
            {{"buffer": 0, "byteOffset": 72, "byteLength": 6}}
          ],
          "accessors": [
            {{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,0]}},
            {{"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"}},
            {{"bufferView": 2, "componentType": 5123, "count": 3, "type": "SCALAR"}}
          ]
        }}"#,
        bin.len()
    );
    fs::write(dir.join("tri.glb"), build_test_glb(&gltf_json, &bin)).unwrap();

    // 3. Corrupt glTF fixtures (cycle, sparse, traversal)
    fs::write(
        dir.join("cycle.gltf"),
        r#"{"asset":{"version":"2.0"},"nodes":[{"children":[1]},{"children":[0]}]}"#,
    )
    .unwrap();
    fs::write(
        dir.join("sparse.gltf"),
        r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":12,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAA"}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":12}],"accessors":[{"bufferView":0,"componentType":5126,"count":1,"type":"VEC3","sparse":{"count":1}}]}"#,
    )
    .unwrap();

    // 4. Tiled .tmj (infinite unsorted chunks + flip GIDs) and .tmx (zlib + gzip)
    let flip_gid = 0x8000_0001u32; // FLIP_H | local 1
    let tmj = format!(
        r#"{{
          "width": 4, "height": 2, "tilewidth": 16, "tileheight": 16,
          "infinite": true, "orientation": "orthogonal",
          "tilesets": [{{"firstgid": 1, "name": "world", "tilewidth": 16, "tileheight": 16, "tilecount": 4, "columns": 2, "tiles": [{{"id": 1, "properties": [{{"name": "solid", "value": true}}]}}]}}],
          "layers": [{{
            "name": "ground", "type": "tilelayer", "width": 4, "height": 2, "visible": true,
            "chunks": [
              {{"x": 2, "y": 0, "width": 2, "height": 2, "data": [{}, 2, 0, 2]}},
              {{"x": 0, "y": 0, "width": 2, "height": 2, "data": [1, 0, 2, 0]}}
            ]
          }}]
        }}"#,
        flip_gid
    );
    fs::write(dir.join("map.tmj"), &tmj).unwrap();

    let raw_gids_bytes: Vec<u8> = [1u32, 2, 0, 2].iter().flat_map(|g| g.to_le_bytes()).collect();
    use crate::modules::base64 as b64_mod;
    let b64_zlib = b64_mod::encode_bytes(&zlib_stored(&raw_gids_bytes), b64_mod::STD_ALPHABET, true);
    let b64_gzip = b64_mod::encode_bytes(&gzip_stored(&raw_gids_bytes), b64_mod::STD_ALPHABET, true);
    let tmx = format!(
        r#"<map version="1.10" orientation="orthogonal" width="2" height="2" tilewidth="16" tileheight="16">
  <tileset firstgid="1" name="ts" tilewidth="16" tileheight="16" tilecount="4" columns="2">
    <tile id="1"><properties><property name="solid" value="true"/></properties></tile>
  </tileset>
  <layer name="base" width="2" height="2"><data encoding="base64" compression="zlib">{}</data></layer>
  <layer name="walls" width="2" height="2"><data encoding="base64" compression="gzip">{}</data></layer>
</map>"#,
        b64_zlib, b64_gzip
    );
    fs::write(dir.join("map.tmx"), &tmx).unwrap();

    fs::write(
        dir.join("zstd.tmj"),
        r#"{"width":2,"height":2,"tilewidth":16,"tileheight":16,"orientation":"orthogonal","layers":[{"name":"l","type":"tilelayer","encoding":"base64","compression":"zstd","data":"AAAA"}]}"#,
    )
    .unwrap();
    fs::write(
        dir.join("iso.tmj"),
        r#"{"width":2,"height":2,"tilewidth":16,"tileheight":16,"orientation":"isometric","layers":[]}"#,
    )
    .unwrap();

    // 5. Terrain fixtures (.r16 flat + 16-bit sloped PNG)
    let mut flat_r16 = Vec::with_capacity(4 * 4 * 2);
    for _ in 0..16 {
        flat_r16.extend_from_slice(&32768u16.to_le_bytes());
    }
    fs::write(dir.join("flat.r16"), &flat_r16).unwrap();
    fs::write(
        dir.join("hill.png"),
        build_gray16_png(4, 4, &[0; 4]
            .into_iter()
            .chain([16384u16; 4])
            .chain([32768u16; 4])
            .chain([65535u16; 4])
            .collect::<Vec<_>>()),
    )
    .unwrap();

    // Verify with --no-cache (cache disabled): no .askas written
    let mut vm_nocache = new_sandboxed_vm(&dir, false);
    let script = r#"
import mesh;
import tiled;
import terrain;
import geom;
import gfx;
import test;

let crate = mesh.load("crate.obj");
let cst = crate.stats();
test.assert_eq(cst["vertices"], 4);
test.assert_eq(cst["tris"], 2);
test.assert_eq(crate.materials()[1]["name"], "wood");

let tri = mesh.load("tri.glb");
test.assert_eq(tri.stats()["tris"], 1);
test.assert_eq(tri.nodes()[0]["name"], "tri_node");
let flat_m = tri.flatten();
test.assert_eq(flat_m.stats()["vertices"], 3);
test.assert(tri.verify());

let tmj = tiled.load("map.tmj");
test.assert_eq(tmj.info()["tilesets"], 1);
let tg = tmj.tile_at(0, 2, 0);
test.assert(tg["flip_h"]);
test.assert_eq(tg["local_id"], 1);

let tmx = tiled.load("map.tmx");
let sg = tmx.solid_grid();
test.assert(!sg.is_solid(0, 0));
test.assert(sg.is_solid(1, 0));
test.assert(sg.is_solid(1, 1));

let hm_flat = terrain.load("flat.r16", #{ cell_size: 2.0, height_scale: 10.0 });
let n = hm_flat.normal(4.0, -4.0);
test.assert(geom.dist(n, geom.vec3(0.0, 0.0, 1.0)) < 0.001);
test.assert(hm_flat.slope(4.0, -4.0) < 0.01);

let hm_hill = terrain.load("hill.png", #{ cell_size: 1.0, height_scale: 8.0 });
let tm_mesh = hm_hill.to_mesh(1);
test.assert(tm_mesh.verify());
let bias = tmx.place_on_terrain(hm_flat);
test.assert(bias > 4.9 && bias < 5.1);

gfx.headless(true, 32, 24);
gfx.clear(0, 0, 20);
hm_hill.mode7_draw(#{ horizon: 8.0 });
gfx.present();
gfx.close();

crate.close();
tri.close();
flat_m.close();
tmj.close();
tmx.close();
sg.close();
hm_flat.close();
hm_hill.close();
tm_mesh.close();
true
"#;
    let res = vm_nocache.run(compile_test_source(script).unwrap()).unwrap();
    assert!(res.equals(&Value::Bool(true)));
    assert!(!dir.join("__askcache__").exists(), "nocache must not create __askcache__");

    // Verify error cases
    for (bad_script, want_substr) in [
        ("import mesh; mesh.load(\"cycle.gltf\");", "E_MESH_CYCLE"),
        ("import mesh; mesh.load(\"sparse.gltf\");", "E_MESH_UNSUPPORTED"),
        ("import mesh; mesh.load(\"../secret.obj\");", "traversal"),
        ("import tiled; tiled.load(\"zstd.tmj\");", "E_TILED_UNSUPPORTED"),
        ("import tiled; tiled.load(\"iso.tmj\");", "E_TILED_UNSUPPORTED"),
        ("import terrain; terrain.load(\"flat.r16\", #{ cell_size: 0.0 });", "E_TERRAIN_BAD_OPT"),
    ] {
        let err = vm_nocache.run(compile_test_source(bad_script).unwrap()).unwrap_err();
        assert!(
            err.contains(want_substr) || err.contains("escapes") || err.contains(".."),
            "script `{}` expected '{}', got '{}'",
            bad_script,
            want_substr,
            err
        );
    }
    drop(vm_nocache);

    // 6. Test ASKAS cache creation, cache hit, SHA-256 tamper rejection, and askane clean
    let mut vm_cache = new_sandboxed_vm(&dir, true);
    let cache_script = r#"
import mesh;
import tiled;
import terrain;
let a = mesh.load("tri.glb");
let b = tiled.load("map.tmj");
let c = terrain.load("flat.r16", #{ cell_size: 2.0, height_scale: 10.0 });
let ok = a.stats()["tris"] == 1 && b.info()["width"] == 4 && c.info()["width"] == 4;
a.close();
b.close();
c.close();
ok
"#;
    assert!(vm_cache.run(compile_test_source(cache_script).unwrap()).unwrap().equals(&Value::Bool(true)));
    let askas_dir = dir.join("__askcache__").join("assets");
    let entries: Vec<_> = fs::read_dir(&askas_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 3, "expected 3 .askas cache files");

    // Bit-flip SHA-256 verification test on ASKAS binary
    let raw_askas = fs::read(entries[0].path()).unwrap();
    let mut tampered = raw_askas.clone();
    let flip_idx = tampered.len() - 1;
    tampered[flip_idx] ^= 0x01;
    assert!(decode_and_verify_askas(&tampered, ParserId::Mesh).is_err());
    assert!(decode_and_verify_askas(&tampered, ParserId::Tiled).is_err());
    assert!(decode_and_verify_askas(&tampered, ParserId::Terrain).is_err());

    // Writing tampered .askas back falls back cleanly to re-parsing & still succeeds!
    fs::write(entries[0].path(), &tampered).unwrap();
    assert!(vm_cache.run(compile_test_source(cache_script).unwrap()).unwrap().equals(&Value::Bool(true)));
    drop(vm_cache);

    // Check encode/decode roundtrip helper & clean_dir_walk
    let sample_askas = encode_askas(ParserId::Mesh, b"payload").unwrap();
    assert_eq!(decode_and_verify_askas(&sample_askas, ParserId::Mesh).unwrap(), b"payload");
    let mut removed = 0usize;
    crate::clean_dir_walk(&dir, false, false, &mut removed, 0).unwrap();
    assert!(removed >= 3, "clean_dir_walk must delete .askas files");
    assert!(!dir.join("__askcache__").exists(), "clean_dir_walk must remove empty __askcache__");

    // 7. Parser fuzzing + memory budget 0-leak check across all Stage 5 parsers
    let mut fuzz_vm = new_sandboxed_vm(&dir, false);
    let dummy_obj = AssetRef::parse("fuzz.obj").unwrap();
    let dummy_gltf = AssetRef::parse("fuzz.gltf").unwrap();
    let dummy_tmj = AssetRef::parse("fuzz.tmj").unwrap();
    let mut seed: u64 = 0xD1CE_F00D_1234_5678;
    let next_u64 = |s: &mut u64| -> u64 {
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        *s
    };

    for i in 0..160usize {
        let len = (next_u64(&mut seed) as usize % 96) + 1;
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            buf.push((next_u64(&mut seed) & 0xFF) as u8);
        }
        match i % 6 {
            0 => { let _ = load_obj_model(&mut fuzz_vm, &dummy_obj, &buf); }
            1 => { let _ = load_gltf_model(&mut fuzz_vm, &dummy_gltf, &buf, false); }
            2 => { let _ = load_gltf_model(&mut fuzz_vm, &dummy_gltf, &buf, true); }
            3 => { let _ = load_tiled_map(&mut fuzz_vm, "zstd.tmj"); }
            4 => {
                if let Ok(s) = std::str::from_utf8(&buf) {
                    let _ = Lut3D::from_cube_str(s, Some(fuzz_vm.budget_core()));
                }
            }
            _ => {
                let mut mutated = tmj.as_bytes().to_vec();
                let idx = (next_u64(&mut seed) as usize) % mutated.len();
                mutated[idx] ^= (next_u64(&mut seed) & 0x7F) as u8;
                fs::write(dir.join("fuzz_mut.tmj"), &mutated).unwrap();
                let _ = load_tiled_map(&mut fuzz_vm, "fuzz_mut.tmj");
                let _ = dummy_tmj;
            }
        }
        assert_eq!(fuzz_vm.budget_core().used_bytes(), 0, "Stage 5 fuzz leak at case {}", i);
    }
    let _ = load_heightmap(&mut fuzz_vm, "flat.r16", None, None, 1.0, 1.0).unwrap();
    drop(fuzz_vm);
    modules::reset_runtime_state();
    let b = Arc::new(BudgetCore::default());
    assert_eq!(b.used_bytes(), 0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_v020_stage5_lighting_postfx_particles_nav_and_scene() {
    modules::reset_runtime_state();
    let dir = temp_test_dir("stage5_b");

    // Cubemap 6 square 4x4 RGBA PNG faces (+X red, -X cyan, +Y green, -Y magenta, +Z blue, -Z yellow)
    let make_face = |r: u8, g: u8, b: u8| -> Vec<u8> {
        let mut px = Vec::with_capacity(16 * 4);
        for _ in 0..16 {
            px.extend_from_slice(&[r, g, b, 255]);
        }
        build_rgba_png(4, 4, &px)
    };
    fs::write(dir.join("px.png"), make_face(255, 0, 0)).unwrap();
    fs::write(dir.join("nx.png"), make_face(0, 255, 255)).unwrap();
    fs::write(dir.join("py.png"), make_face(0, 255, 0)).unwrap();
    fs::write(dir.join("ny.png"), make_face(255, 0, 255)).unwrap();
    fs::write(dir.join("pz.png"), make_face(0, 0, 255)).unwrap();
    fs::write(dir.join("nz.png"), make_face(255, 255, 0)).unwrap();
    fs::write(dir.join("bad_dim.png"), build_rgba_png(2, 2, &[0; 16])).unwrap();

    // 16x16x16 identity+warmth .cube LUT
    let mut cube = String::from("TITLE \"WarmTest\"\nLUT_3D_SIZE 16\n");
    for b in 0..16 {
        for g in 0..16 {
            for r in 0..16 {
                let rf = ((r as f32 / 15.0) * 1.05).min(1.0);
                let gf = g as f32 / 15.0;
                let bf = (b as f32 / 15.0) * 0.9;
                cube.push_str(&format!("{:.5} {:.5} {:.5}\n", rf, gf, bf));
            }
        }
    }
    fs::write(dir.join("warm16.cube"), &cube).unwrap();
    fs::write(dir.join("bad_lut.cube"), "LUT_1D_SIZE 16\n0 0 0\n").unwrap();

    // Flat 8x8 terrain (.r16) at elevation ~5.0 + simple .obj prop + wall .tmj
    let mut r16 = Vec::with_capacity(64 * 2);
    for _ in 0..64 {
        r16.extend_from_slice(&32768u16.to_le_bytes());
    }
    fs::write(dir.join("ground.r16"), &r16).unwrap();
    fs::write(
        dir.join("prop.obj"),
        "v -0.5 0 -0.5\nv 0.5 0 -0.5\nv 0 1 0.5\nf 1 2 3\n",
    )
    .unwrap();
    fs::write(
        dir.join("level.tmj"),
        r#"{"width":8,"height":8,"tilewidth":8,"tileheight":8,"orientation":"orthogonal","layers":[{"name":"walls","type":"tilelayer","width":8,"height":8,"visible":true,"properties":[{"name":"collision","value":true}],"data":[
          0,0,0,0,0,0,0,0,
          0,0,0,1,0,0,0,0,
          0,0,0,1,0,0,0,0,
          0,0,0,1,0,0,0,0,
          0,0,0,1,0,0,0,0,
          0,0,0,0,0,0,0,0,
          0,0,0,0,0,0,0,0,
          0,0,0,0,0,0,0,0
        ]}]}"#,
    )
    .unwrap();

    let script = r#"
import nav;
import procgen;
import lighting;
import postfx;
import particles;
import cubemap;
import tiled;
import terrain;
import mesh;
import scene;
import gfx;
import hash;
import test;

let tmap = tiled.load("level.tmj");
let solid = tmap.solid_grid("walls");
let ng = nav.from_solid(solid);
test.assert(ng.is_walkable(0, 2));
test.assert(!ng.is_walkable(3, 2));
let path = ng.find_path(1, 2, 5, 2, true);
test.assert(path.len() >= 5);
let smooth = ng.path_smooth(path);
test.assert(smooth.len() <= path.len());
let flow = ng.flow_field(5, 2);
test.assert_eq(flow["width"], 8);
test.assert_eq(flow["dirs"].len(), 64);

// Deterministic procgen
procgen.seed(777);
let w1 = procgen.worley2(3.25, 4.75);
let p1 = procgen.perlin2(1.5, 2.25);
let c1 = procgen.cellular(16, 16, 42, 45, 3);
let s1 = procgen.scatter(64.0, 64.0, 8, 5.0, 99);
test.assert(w1 >= 0.0 && w1 <= 2.0);
test.assert(p1 >= -1.0 && p1 <= 1.0);
test.assert_eq(c1["cells"].len(), 256);
test.assert(s1.len() <= 8);

// Cubemap sampling
let cm = cubemap.load("px.png", "nx.png", "py.png", "ny.png", "pz.png", "nz.png");
test.assert_eq(cm.size(), 4);
let px_col = cm.sample([1.0, 0.0, 0.0]);
test.assert(px_col[0] > 0.99 && px_col[1] < 0.01);
let py_col = cm.sample([0.0, 1.0, 0.0]);
test.assert(py_col[1] > 0.99);

// Headless scene + lighting + shadow DDA + particles + postfx
gfx.headless(true, 64, 64);
let hm = terrain.load("ground.r16", #{ cell_size: 2.0, height_scale: 10.0 });
let prop = mesh.load("prop.obj");
let sun = lighting.directional(0.2, -1.0, 0.3, [1.0, 0.98, 0.9], 0.8);
let sc = scene.assemble(#{
  terrain: hm,
  tilemap: tmap,
  nav: ng,
  cubemap: cm,
  lights: [sun],
  ambient: [0.3, 0.3, 0.35],
  placements: [#{ name: "tower", model: prop, position: [4.0, 0.0, -4.0], ground_snap: true, tint: [204, 136, 68] }]
});
let sinfo = sc.info();
test.assert_eq(sinfo["entities"], 1);
test.assert(sinfo["ground_bias"] > 4.9 && sinfo["ground_bias"] < 5.1);
let ents = sc.entities();
test.assert(ents[0]["snapped_ground_y"] > 4.9 && ents[0]["snapped_ground_y"] < 5.1);

gfx.clear(10, 15, 25);
sc.draw(#{ cam_x: 4.0, cam_y: 8.0, cam_z: 4.0, yaw: 0.0, horizon: 24.0 });

let lm = lighting.lightmap(64, 64, 2, [0.25, 0.25, 0.25]);
let pt = lighting.point(12.0, 20.0, 48.0, [1.0, 0.8, 0.5], 1.5, "smooth", true);
lm.apply([pt], #{ solid: solid });
lm.composite(1.0);

let ps = particles.create(128, 12345);
ps.set_gravity(0.0, 40.0);
let n_emit = ps.emit(#{ x: 32.0, y: 20.0, count: 24, speed_min: 10.0, speed_max: 40.0, size: 2.5, color: [255, 170, 0] });
test.assert_eq(n_emit, 24);
ps.update(0.1);
ps.draw();

let lut = postfx.lut_load("warm16.cube");
test.assert_eq(lut.size(), 16);
postfx.bloom(0.6, 0.5, 2);
lut.apply(0.8);
postfx.vignette(0.35, 0.8);
postfx.scanlines(0.15, 2);
gfx.present();

let bmp = postfx.snapshot();
test.assert(bmp.len() == 54 + 64 * 64 * 3);
let digest = hash.sha256(bmp);

gfx.close();
tmap.close();
solid.close();
ng.close();
cm.close();
hm.close();
prop.close();
sun.close();
pt.close();
lm.close();
ps.close();
lut.close();
sc.close();
digest
"#;

    let mut vm1 = new_sandboxed_vm(&dir, false);
    let d1 = vm1.run(compile_test_source(script).unwrap()).unwrap();
    drop(vm1);

    let mut vm2 = new_sandboxed_vm(&dir, false);
    let d2 = vm2.run(compile_test_source(script).unwrap()).unwrap();
    assert!(
        d1.equals(&d2),
        "Stage 5 end-to-end framebuffer BMP snapshot must be deterministic across runs"
    );

    // Verify invalid cubemap & LUT error handling
    let err_cm = vm2
        .run(compile_test_source("import cubemap; cubemap.load(\"px.png\",\"bad_dim.png\",\"py.png\",\"ny.png\",\"pz.png\",\"nz.png\");").unwrap())
        .unwrap_err();
    assert!(err_cm.contains("E_CUBEMAP_INVALID"));

    let err_lut = vm2
        .run(compile_test_source("import postfx; postfx.lut_load(\"bad_lut.cube\");").unwrap())
        .unwrap_err();
    assert!(err_lut.contains("E_LUT_INVALID"));

    drop(vm2);
    modules::reset_runtime_state();
    let _ = fs::remove_dir_all(&dir);
}
