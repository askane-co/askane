use std::rc::Rc;

use crate::media::image::{ColorSpace, MediaImage, PixelFormat, SourceFormat};
use crate::media::ops::height_to_normal;
use crate::media::{Digest, MAX_DIM, err_bad_opt, err_limit, temp_budget_guard};
use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "procgen",
        &[
            ("hash2d", pg_hash2d),
            ("noise2d", pg_noise2d),
            ("fbm2d", pg_fbm2d),
            ("heightmap", pg_heightmap),
            ("normal_map", pg_normal_map),
            ("checker", pg_checker),
        ],
        &[],
    )
}

#[inline]
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

#[inline]
fn hash_coords_u32(seed: u64, x: i64, y: i64) -> u32 {
    let hx = splitmix64(seed ^ (x as u64).wrapping_mul(0x9E37_79B1_85EB_CA87));
    let hy = splitmix64(hx ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
    (hy >> 32) as u32
}

/// 16.16 fixed-point deterministic 2D value noise returning [0..=65535].
#[inline]
fn noise2d_q16(seed: u64, x_q16: i64, y_q16: i64) -> u32 {
    let ix = x_q16.div_euclid(65536);
    let iy = y_q16.div_euclid(65536);
    let fx = x_q16.rem_euclid(65536) as u64; // 0..=65535
    let fy = y_q16.rem_euclid(65536) as u64;

    // Smoothstep t^2 * (3 - 2*t) in Q16 integer arithmetic:
    // sx = (3*65536 - 2*fx) * fx^2 / 65536^2
    let smooth = |t: u64| -> u64 {
        let t2 = (t * t) >> 16; // Q16
        let k = 3 * 65536 - 2 * t; // Q16
        (t2 * k) >> 16
    };
    let sx = smooth(fx);
    let sy = smooth(fy);

    let v00 = (hash_coords_u32(seed, ix, iy) >> 16) as u64;
    let v10 = (hash_coords_u32(seed, ix + 1, iy) >> 16) as u64;
    let v01 = (hash_coords_u32(seed, ix, iy + 1) >> 16) as u64;
    let v11 = (hash_coords_u32(seed, ix + 1, iy + 1) >> 16) as u64;

    let top = v00 as i64 + (((v10 as i64 - v00 as i64) * (sx as i64)) >> 16);
    let bot = v01 as i64 + (((v11 as i64 - v01 as i64) * (sx as i64)) >> 16);
    let val = top + (((bot - top) * (sy as i64)) >> 16);
    val.clamp(0, 65535) as u32
}

/// Pure integer Q16 FBM in [0..=65535].
fn fbm2d_q16(
    seed: u64,
    x_q16: i64,
    y_q16: i64,
    octaves: u32,
    lacunarity_q8: i64,
    gain_q16: i64,
) -> u32 {
    let mut sum = 0i128;
    let mut norm = 0i128;
    let mut amp = 65536i128;
    let mut freq_q8 = 256i64;
    for oct in 0..octaves.clamp(1, 12) {
        let sx = (x_q16 * freq_q8) >> 8;
        let sy = (y_q16 * freq_q8) >> 8;
        let n = noise2d_q16(seed.wrapping_add((oct as u64) * 1013), sx, sy) as i128;
        sum += n * amp;
        norm += amp;
        amp = (amp * (gain_q16 as i128)) >> 16;
        freq_q8 = (freq_q8 * lacunarity_q8) >> 8;
        if amp == 0 {
            break;
        }
    }
    if norm == 0 {
        0
    } else {
        (sum / norm).clamp(0, 65535) as u32
    }
}

fn expect_f64(val: &Value, name: &str) -> Result<f64, String> {
    let f = match val {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        other => {
            return Err(err_bad_opt(format!(
                "{} expects number, got '{}'",
                name,
                other.type_name()
            )));
        }
    };
    if !f.is_finite() {
        return Err(err_bad_opt(format!("{} must be finite", name)));
    }
    Ok(f)
}

fn pg_hash2d(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(err_bad_opt("procgen.hash2d() expects (seed, x, y)"));
    }
    let Value::Int(seed) = &args[0] else {
        return Err(err_bad_opt("procgen.hash2d() seed must be int"));
    };
    let Value::Int(x) = &args[1] else {
        return Err(err_bad_opt("procgen.hash2d() x must be int"));
    };
    let Value::Int(y) = &args[2] else {
        return Err(err_bad_opt("procgen.hash2d() y must be int"));
    };
    Ok(Value::Int(hash_coords_u32(*seed as u64, *x, *y) as i64))
}

fn pg_noise2d(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err(err_bad_opt("procgen.noise2d() expects (seed, x, y, [scale])"));
    }
    let Value::Int(seed) = &args[0] else {
        return Err(err_bad_opt("procgen.noise2d() seed must be int"));
    };
    let x = expect_f64(&args[1], "x")?;
    let y = expect_f64(&args[2], "y")?;
    let scale = match args.get(3) {
        Some(v) if !matches!(v, Value::Unit) => expect_f64(v, "scale")?,
        _ => 1.0,
    };
    let x_q16 = (x * scale * 65536.0).round() as i64;
    let y_q16 = (y * scale * 65536.0).round() as i64;
    let v = noise2d_q16(*seed as u64, x_q16, y_q16);
    Ok(Value::Float((v as f64) / 65535.0))
}

fn pg_fbm2d(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 6 {
        return Err(err_bad_opt(
            "procgen.fbm2d() expects (seed, x, y, [octaves, lacunarity, gain])",
        ));
    }
    let Value::Int(seed) = &args[0] else {
        return Err(err_bad_opt("procgen.fbm2d() seed must be int"));
    };
    let x = expect_f64(&args[1], "x")?;
    let y = expect_f64(&args[2], "y")?;
    let octaves = match args.get(3) {
        Some(Value::Int(o)) if (1..=12).contains(o) => *o as u32,
        None | Some(Value::Unit) => 4,
        _ => return Err(err_bad_opt("procgen.fbm2d() octaves must be int in 1..=12")),
    };
    let lacunarity = match args.get(4) {
        Some(v) if !matches!(v, Value::Unit) => expect_f64(v, "lacunarity")?,
        _ => 2.0,
    };
    let gain = match args.get(5) {
        Some(v) if !matches!(v, Value::Unit) => expect_f64(v, "gain")?,
        _ => 0.5,
    };
    let x_q16 = (x * 65536.0).round() as i64;
    let y_q16 = (y * 65536.0).round() as i64;
    let lac_q8 = (lacunarity.clamp(0.5, 8.0) * 256.0).round() as i64;
    let gain_q16 = (gain.clamp(0.0, 1.0) * 65536.0).round() as i64;
    let v = fbm2d_q16(*seed as u64, x_q16, y_q16, octaves, lac_q8, gain_q16);
    Ok(Value::Float((v as f64) / 65535.0))
}

struct HeightmapOpts {
    seed: u64,
    scale_q16: i64,
    octaves: u32,
    lacunarity_q8: i64,
    gain_q16: i64,
    strength: f64,
}

fn parse_heightmap_opts(val: Option<&Value>, allow_strength: bool, ctx: &str) -> Result<HeightmapOpts, String> {
    let mut out = HeightmapOpts {
        seed: 1337,
        scale_q16: 65536 / 8, // 0.125 per pixel -> smooth terrain
        octaves: 4,
        lacunarity_q8: 512,   // 2.0
        gain_q16: 32768,      // 0.5
        strength: 2.0,
    };
    let Some(v) = val else {
        return Ok(out);
    };
    match v {
        Value::Unit => Ok(out),
        Value::Int(s) => {
            out.seed = *s as u64;
            Ok(out)
        }
        Value::Dict(d) => {
            for (k, fv) in d.borrow().iter() {
                match k.as_str() {
                    "seed" => match fv {
                        Value::Int(s) => out.seed = *s as u64,
                        _ => return Err(err_bad_opt(format!("{} 'seed' must be int", ctx))),
                    },
                    "scale" => {
                        let s = expect_f64(fv, "scale")?;
                        if !(0.0001..=64.0).contains(&s) {
                            return Err(err_bad_opt("scale must be in 0.0001..=64.0"));
                        }
                        out.scale_q16 = (s * 65536.0).round() as i64;
                    }
                    "octaves" => match fv {
                        Value::Int(o) if (1..=12).contains(o) => out.octaves = *o as u32,
                        _ => return Err(err_bad_opt("octaves must be int in 1..=12")),
                    },
                    "lacunarity" => {
                        let l = expect_f64(fv, "lacunarity")?;
                        out.lacunarity_q8 = (l.clamp(0.5, 8.0) * 256.0).round() as i64;
                    }
                    "gain" => {
                        let g = expect_f64(fv, "gain")?;
                        out.gain_q16 = (g.clamp(0.0, 1.0) * 65536.0).round() as i64;
                    }
                    "strength" if allow_strength => {
                        out.strength = expect_f64(fv, "strength")?;
                    }
                    unknown => {
                        return Err(err_bad_opt(format!("{}: unknown option '{}'", ctx, unknown)));
                    }
                }
            }
            Ok(out)
        }
        other => Err(err_bad_opt(format!(
            "{} expects seed int or opts dict, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

fn build_heightmap(
    vm: &mut VM,
    w: u32,
    h: u32,
    opts: &HeightmapOpts,
) -> Result<Rc<MediaImage>, String> {
    let bytes = PixelFormat::R8.byte_len(w, h)?;
    let _tmp = temp_budget_guard(Some(vm.budget_core()), bytes)?;
    let mut out = Vec::with_capacity(bytes);
    for y in 0..h {
        if (y & 31) == 0 {
            vm.check_timeout()?;
        }
        let py_q16 = (y as i64) * opts.scale_q16;
        for x in 0..w {
            let px_q16 = (x as i64) * opts.scale_q16;
            let val_q16 = fbm2d_q16(
                opts.seed,
                px_q16,
                py_q16,
                opts.octaves,
                opts.lacunarity_q8,
                opts.gain_q16,
            );
            out.push((val_q16 >> 8).min(255) as u8);
        }
    }
    drop(_tmp);
    let digest = Digest::from_parts(&[
        b"procgen:heightmap.v1|",
        &w.to_le_bytes(),
        &h.to_le_bytes(),
        &opts.seed.to_le_bytes(),
        &opts.scale_q16.to_le_bytes(),
        &opts.octaves.to_le_bytes(),
        &opts.lacunarity_q8.to_le_bytes(),
        &opts.gain_q16.to_le_bytes(),
        &out,
    ]);
    MediaImage::from_vec(
        w,
        h,
        PixelFormat::R8,
        ColorSpace::Linear,
        SourceFormat::Procgen,
        out,
        Some(false),
        Some(digest),
        Some(vm.budget_core()),
    )
}

fn pg_heightmap(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err(err_bad_opt(
            "procgen.heightmap() expects (width, height, [seed_or_opts])",
        ));
    }
    let Value::Int(wi) = &args[0] else {
        return Err(err_bad_opt("procgen.heightmap() width must be int"));
    };
    let Value::Int(hi) = &args[1] else {
        return Err(err_bad_opt("procgen.heightmap() height must be int"));
    };
    if *wi < 1 || *hi < 1 || (*wi as u64) > MAX_DIM as u64 || (*hi as u64) > MAX_DIM as u64 {
        return Err(err_limit(format!(
            "procgen.heightmap() dimensions {}x{} out of 1..={}",
            wi, hi, MAX_DIM
        )));
    }
    let opts = parse_heightmap_opts(args.get(2), false, "procgen.heightmap()")?;
    let im = build_heightmap(vm, *wi as u32, *hi as u32, &opts)?;
    Ok(Value::Image(im))
}

fn pg_normal_map(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err(err_bad_opt(
            "procgen.normal_map() expects (height_image, [strength]) or (width, height, [opts])",
        ));
    }
    match &args[0] {
        Value::Image(im) => {
            if args.len() > 2 {
                return Err(err_bad_opt(
                    "procgen.normal_map(img, [strength]) takes at most 2 args",
                ));
            }
            let strength = match args.get(1) {
                Some(v) if !matches!(v, Value::Unit) => expect_f64(v, "strength")?,
                _ => 2.0,
            };
            let norm = height_to_normal(im, strength, Some(vm.budget_core()))?;
            Ok(Value::Image(norm))
        }
        Value::Int(wi) => {
            if args.len() < 2 {
                return Err(err_bad_opt(
                    "procgen.normal_map(width, height, [opts]) requires height",
                ));
            }
            let Value::Int(hi) = &args[1] else {
                return Err(err_bad_opt("procgen.normal_map() height must be int"));
            };
            if *wi < 1 || *hi < 1 || (*wi as u64) > MAX_DIM as u64 || (*hi as u64) > MAX_DIM as u64
            {
                return Err(err_limit(format!(
                    "procgen.normal_map() dimensions {}x{} out of 1..={}",
                    wi, hi, MAX_DIM
                )));
            }
            let opts = parse_heightmap_opts(args.get(2), true, "procgen.normal_map()")?;
            let hm = build_heightmap(vm, *wi as u32, *hi as u32, &opts)?;
            let norm = height_to_normal(&hm, opts.strength, Some(vm.budget_core()))?;
            Ok(Value::Image(norm))
        }
        other => Err(err_bad_opt(format!(
            "procgen.normal_map() expects image or width int, got '{}'",
            other.type_name()
        ))),
    }
}

fn pg_checker(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 5 {
        return Err(err_bad_opt(
            "procgen.checker() expects (width, height, [tile_size, color_a, color_b])",
        ));
    }
    let Value::Int(wi) = &args[0] else {
        return Err(err_bad_opt("procgen.checker() width must be int"));
    };
    let Value::Int(hi) = &args[1] else {
        return Err(err_bad_opt("procgen.checker() height must be int"));
    };
    if *wi < 1 || *hi < 1 || (*wi as u64) > MAX_DIM as u64 || (*hi as u64) > MAX_DIM as u64 {
        return Err(err_limit("procgen.checker() dimensions out of bounds"));
    }
    let w = *wi as u32;
    let h = *hi as u32;
    let tile = match args.get(2) {
        Some(Value::Int(t)) if *t >= 1 && *t <= 1024 => *t as u32,
        None | Some(Value::Unit) => 8,
        _ => return Err(err_bad_opt("procgen.checker() tile_size must be in 1..=1024")),
    };
    let unpack_rgb = |v: &Value, default_px: [u8; 4]| -> Result<[u8; 4], String> {
        if matches!(v, Value::Unit) {
            return Ok(default_px);
        }
        let Value::Int(c) = v else {
            return Err(err_bad_opt("procgen.checker() color must be packed int"));
        };
        let u = *c as u32;
        let a = if (u >> 24) == 0 { 255 } else { ((u >> 24) & 0xFF) as u8 };
        Ok([
            ((u >> 16) & 0xFF) as u8,
            ((u >> 8) & 0xFF) as u8,
            (u & 0xFF) as u8,
            a,
        ])
    };
    let ca = match args.get(3) {
        Some(v) => unpack_rgb(v, [255, 255, 255, 255])?,
        None => [255, 255, 255, 255],
    };
    let cb = match args.get(4) {
        Some(v) => unpack_rgb(v, [40, 40, 40, 255])?,
        None => [40, 40, 40, 255],
    };

    let bytes = PixelFormat::Rgba8.byte_len(w, h)?;
    let _tmp = temp_budget_guard(Some(vm.budget_core()), bytes)?;
    let mut out = Vec::with_capacity(bytes);
    for y in 0..h {
        let ty = y / tile;
        for x in 0..w {
            let tx = x / tile;
            if ((tx ^ ty) & 1) == 0 {
                out.extend_from_slice(&ca);
            } else {
                out.extend_from_slice(&cb);
            }
        }
    }
    drop(_tmp);
    let im = MediaImage::from_vec(
        w,
        h,
        PixelFormat::Rgba8,
        ColorSpace::Srgb,
        SourceFormat::Procgen,
        out,
        Some(true),
        None,
        Some(vm.budget_core()),
    )?;
    Ok(Value::Image(im))
}
