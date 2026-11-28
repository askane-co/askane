use std::cell::RefCell;
use std::rc::Rc;

use crate::media::image::{ColorSpace, MediaImage, PixelFormat, SourceFormat};
use crate::media::ops::height_to_normal;
use crate::media::{Digest, MAX_DIM, err_bad_opt, err_limit, temp_budget_guard};
use crate::modules::build_module;
use crate::value::{Value, new_dict_with_capacity};
use crate::vm::VM;

thread_local! {
    static PROCGEN_SEED: RefCell<u64> = const { RefCell::new(1337) };
}

pub fn reset_state() {
    PROCGEN_SEED.with(|s| *s.borrow_mut() = 1337);
}

fn default_seed() -> u64 {
    PROCGEN_SEED.with(|s| *s.borrow())
}

pub fn module() -> Value {
    build_module(
        "procgen",
        &[
            ("seed", pg_seed),
            ("hash2d", pg_hash2d),
            ("noise2d", pg_noise2d),
            ("perlin2", pg_perlin2),
            ("worley2", pg_worley2),
            ("fbm2d", pg_fbm2d),
            ("heightmap", pg_heightmap),
            ("normal_map", pg_normal_map),
            ("checker", pg_checker),
            ("cellular", pg_cellular),
            ("scatter", pg_scatter),
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

fn pg_seed(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("procgen.seed() expects (seed_int)"));
    }
    let Value::Int(s) = &args[0] else {
        return Err(err_bad_opt("procgen.seed() expects integer seed"));
    };
    PROCGEN_SEED.with(|cell| *cell.borrow_mut() = *s as u64);
    Ok(Value::Int(*s))
}

#[inline]
fn grad2_dot(hash: u32, dx_q16: i64, dy_q16: i64) -> i64 {
    match hash & 7 {
        0 => dx_q16 + dy_q16,
        1 => -dx_q16 + dy_q16,
        2 => dx_q16 - dy_q16,
        3 => -dx_q16 - dy_q16,
        4 => dx_q16,
        5 => -dx_q16,
        6 => dy_q16,
        _ => -dy_q16,
    }
}

fn perlin2_q16(seed: u64, x_q16: i64, y_q16: i64) -> f64 {
    let ix = x_q16.div_euclid(65536);
    let iy = y_q16.div_euclid(65536);
    let fx = x_q16.rem_euclid(65536);
    let fy = y_q16.rem_euclid(65536);

    let fade = |t: i64| -> i64 {
        let t2 = (t * t) >> 16;
        let t3 = (t2 * t) >> 16;
        let inner = 6 * t - 15 * 65536;
        let p = ((t * inner) >> 16) + 10 * 65536;
        // fallback clean quintic / smoothstep in Q16:
        let _ = (t3, p);
        let sq = (t * t) >> 16;
        (sq * (3 * 65536 - 2 * t)) >> 16
    };
    let u = fade(fx);
    let v = fade(fy);

    let g00 = grad2_dot(hash_coords_u32(seed, ix, iy), fx, fy);
    let g10 = grad2_dot(hash_coords_u32(seed, ix + 1, iy), fx - 65536, fy);
    let g01 = grad2_dot(hash_coords_u32(seed, ix, iy + 1), fx, fy - 65536);
    let g11 = grad2_dot(hash_coords_u32(seed, ix + 1, iy + 1), fx - 65536, fy - 65536);

    let nx0 = g00 + (((g10 - g00) * u) >> 16);
    let nx1 = g01 + (((g11 - g01) * u) >> 16);
    let n = nx0 + (((nx1 - nx0) * v) >> 16);
    (n as f64 / 65536.0).clamp(-1.0, 1.0)
}

fn pg_perlin2(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err(err_bad_opt("procgen.perlin2() expects (x, y, [seed])"));
    }
    let x = expect_f64(&args[0], "procgen.perlin2() x")?;
    let y = expect_f64(&args[1], "procgen.perlin2() y")?;
    let seed = match args.get(2) {
        Some(Value::Int(s)) => *s as u64,
        None | Some(Value::Unit) => default_seed(),
        _ => return Err(err_bad_opt("procgen.perlin2() seed must be int")),
    };
    let x_q16 = (x * 65536.0).round() as i64;
    let y_q16 = (y * 65536.0).round() as i64;
    Ok(Value::Float(perlin2_q16(seed, x_q16, y_q16)))
}

fn pg_worley2(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err(err_bad_opt("procgen.worley2() expects (x, y, [seed])"));
    }
    let x = expect_f64(&args[0], "procgen.worley2() x")?;
    let y = expect_f64(&args[1], "procgen.worley2() y")?;
    let seed = match args.get(2) {
        Some(Value::Int(s)) => *s as u64,
        None | Some(Value::Unit) => default_seed(),
        _ => return Err(err_bad_opt("procgen.worley2() seed must be int")),
    };
    let ix = x.floor() as i64;
    let iy = y.floor() as i64;
    let mut min_d2 = f64::INFINITY;
    for oy in -1..=1 {
        for ox in -1..=1 {
            let cx = ix + ox;
            let cy = iy + oy;
            let h = hash_coords_u32(seed, cx, cy);
            let px = cx as f64 + ((h & 0xFFFF) as f64) / 65535.0;
            let py = cy as f64 + (((h >> 16) & 0xFFFF) as f64) / 65535.0;
            let dx = x - px;
            let dy = y - py;
            let d2 = dx * dx + dy * dy;
            if d2 < min_d2 {
                min_d2 = d2;
            }
        }
    }
    Ok(Value::Float(min_d2.sqrt()))
}

fn pg_cellular(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 5 {
        return Err(err_bad_opt(
            "procgen.cellular() expects (width, height, [seed], [fill_percent], [steps])",
        ));
    }
    let Value::Int(wi) = &args[0] else {
        return Err(err_bad_opt("procgen.cellular() width must be int"));
    };
    let Value::Int(hi) = &args[1] else {
        return Err(err_bad_opt("procgen.cellular() height must be int"));
    };
    if *wi < 1 || *hi < 1 || *wi > 1024 || *hi > 1024 {
        return Err(err_limit("procgen.cellular() dimensions must be in 1..=1024"));
    }
    let w = *wi as usize;
    let h = *hi as usize;
    let seed = match args.get(2) {
        Some(Value::Int(s)) => *s as u64,
        None | Some(Value::Unit) => default_seed(),
        _ => return Err(err_bad_opt("procgen.cellular() seed must be int")),
    };
    let fill_pct = match args.get(3) {
        Some(Value::Int(p)) if (0..=100).contains(p) => *p as u32,
        None | Some(Value::Unit) => 45,
        _ => return Err(err_bad_opt("procgen.cellular() fill_percent must be 0..=100")),
    };
    let steps = match args.get(4) {
        Some(Value::Int(s)) if (0..=32).contains(s) => *s as usize,
        None | Some(Value::Unit) => 4,
        _ => return Err(err_bad_opt("procgen.cellular() steps must be 0..=32")),
    };

    let mut cur = vec![0u8; w * h];
    let mut next = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                cur[y * w + x] = 1;
            } else {
                let r = hash_coords_u32(seed, x as i64, y as i64) % 100;
                cur[y * w + x] = if r < fill_pct { 1 } else { 0 };
            }
        }
    }

    for _ in 0..steps {
        for y in 0..h {
            for x in 0..w {
                if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                    next[y * w + x] = 1;
                    continue;
                }
                let mut walls = 0u32;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = (x as isize + dx) as usize;
                        let ny = (y as isize + dy) as usize;
                        walls += cur[ny * w + nx] as u32;
                    }
                }
                next[y * w + x] = if walls > 4 || (walls == 4 && cur[y * w + x] == 1) {
                    1
                } else {
                    0
                };
            }
        }
        std::mem::swap(&mut cur, &mut next);
    }

    vm.charge_alloc((w * h).saturating_mul(16).saturating_add(64))?;
    let cells: Vec<Value> = cur.into_iter().map(|c| Value::Int(c as i64)).collect();
    let mut dict = new_dict_with_capacity(3);
    dict.insert("width".to_string(), Value::Int(w as i64));
    dict.insert("height".to_string(), Value::Int(h as i64));
    dict.insert("cells".to_string(), Value::List(Rc::new(RefCell::new(cells))));
    Ok(Value::dict(dict))
}

fn pg_scatter(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 4 || args.len() > 5 {
        return Err(err_bad_opt(
            "procgen.scatter() expects (width, height, count, min_dist, [seed])",
        ));
    }
    let w = expect_f64(&args[0], "procgen.scatter() width")?;
    let h = expect_f64(&args[1], "procgen.scatter() height")?;
    let Value::Int(cnt_i) = &args[2] else {
        return Err(err_bad_opt("procgen.scatter() count must be int"));
    };
    if *cnt_i < 0 || *cnt_i > 100_000 {
        return Err(err_limit("procgen.scatter() count must be in 0..=100_000"));
    }
    let min_dist = expect_f64(&args[3], "procgen.scatter() min_dist")?;
    if w <= 0.0 || h <= 0.0 || min_dist < 0.0 {
        return Err(err_bad_opt("procgen.scatter() bounds must be positive"));
    }
    let seed = match args.get(4) {
        Some(Value::Int(s)) => *s as u64,
        None | Some(Value::Unit) => default_seed(),
        _ => return Err(err_bad_opt("procgen.scatter() seed must be int")),
    };

    let target = *cnt_i as usize;
    let min_d2 = min_dist * min_dist;
    let max_attempts = target.saturating_mul(16).max(64).min(400_000);
    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(target);
    let mut state = splitmix64(seed ^ 0xA5A5_5A5A_1234_5678);

    for _ in 0..max_attempts {
        if pts.len() >= target {
            break;
        }
        state = splitmix64(state);
        let ux = ((state >> 32) as f64) / (u32::MAX as f64);
        state = splitmix64(state);
        let uy = ((state >> 32) as f64) / (u32::MAX as f64);
        let px = ux * w;
        let py = uy * h;
        let mut ok = true;
        for &[ex, ey] in &pts {
            let dx = px - ex;
            let dy = py - ey;
            if dx * dx + dy * dy < min_d2 {
                ok = false;
                break;
            }
        }
        if ok {
            pts.push([px, py]);
        }
    }

    vm.charge_alloc(pts.len().saturating_mul(48))?;
    let vals: Vec<Value> = pts
        .into_iter()
        .map(|[px, py]| {
            Value::List(Rc::new(RefCell::new(vec![
                Value::Float(px),
                Value::Float(py),
            ])))
        })
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(vals))))
}

