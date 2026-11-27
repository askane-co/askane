use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::media::atlas::{atlas_to_value, pack_atlas, resolve_atlas_frame};
use crate::media::image::{
    ColorSpace, DecodeOpts, Filter, MediaImage, PixelFormat, SourceFormat, TextureData, WrapMode,
    decode_image_bytes, parse_decode_opts,
};
use crate::media::ops::{
    convert_format, crop, flip, height_to_normal, premultiply, resize, rotate, to_linear, to_srgb,
};
use crate::media::sample::{sample_image_nearest, sample_texture_uv};
use crate::media::{MAX_DIM, err_bad_opt, err_limit};
use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "image",
        &[
            ("new", img_new),
            ("from_bytes", img_from_bytes),
            ("decode", img_decode),
            ("load", img_load),
            ("info", img_info),
            ("meta", img_info),
            ("digest", img_digest),
            ("width", img_width),
            ("height", img_height),
            ("format", img_format),
            ("color_space", img_color_space),
            ("frame_count", img_frame_count),
            ("frame", img_frame),
            ("get_pixel", img_get_pixel),
            ("to_bytes", img_to_bytes),
            ("convert", img_convert),
            ("to_linear", img_to_linear),
            ("to_srgb", img_to_srgb),
            ("premultiply", img_premultiply),
            ("crop", img_crop),
            ("resize", img_resize),
            ("flip", img_flip),
            ("flip_x", img_flip_x),
            ("flip_y", img_flip_y),
            ("rotate", img_rotate),
            ("mipmaps", img_mipmaps),
            ("mip_count", img_mip_count),
            ("mip_level", img_mip_level),
            ("mip_digest", img_mip_digest),
            ("height_to_normal", img_height_to_normal),
            ("texture", img_texture),
            ("sample", img_sample),
            ("atlas", img_atlas),
            ("atlas_from_paths", img_atlas_from_paths),
            ("sprite_frame", img_sprite_frame),
        ],
        &[],
    )
}

pub(crate) fn extract_image<'a>(val: &'a Value, ctx: &str) -> Result<Rc<MediaImage>, String> {
    match val {
        Value::Image(im) => Ok(Rc::clone(im)),
        Value::Texture(tex) => Ok(Rc::clone(&tex.image)),
        other => Err(err_bad_opt(format!(
            "{} expects image or texture, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

fn expect_u32_dim(val: &Value, name: &str) -> Result<u32, String> {
    match val {
        Value::Int(n) if *n >= 1 && (*n as u64) <= MAX_DIM as u64 => Ok(*n as u32),
        Value::Int(n) => Err(err_limit(format!(
            "{} dimension {} out of 1..={} bounds",
            name, n, MAX_DIM
        ))),
        other => Err(err_bad_opt(format!(
            "{} expects integer, got '{}'",
            name,
            other.type_name()
        ))),
    }
}

fn expect_i64(val: &Value, name: &str) -> Result<i64, String> {
    match val {
        Value::Int(n) => Ok(*n),
        other => Err(err_bad_opt(format!(
            "{} expects integer, got '{}'",
            name,
            other.type_name()
        ))),
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

fn parse_fill_pixel(val: &Value, fmt: PixelFormat) -> Result<[u8; 4], String> {
    match val {
        Value::Int(c) => {
            let u = *c as u32;
            let a = if (u >> 24) == 0 && (u & 0x00FF_FFFF) != 0 {
                255
            } else {
                ((u >> 24) & 0xFF) as u8
            };
            Ok([
                ((u >> 16) & 0xFF) as u8,
                ((u >> 8) & 0xFF) as u8,
                (u & 0xFF) as u8,
                a,
            ])
        }
        Value::List(list) => {
            let items = list.borrow();
            let mut comps = [0u8; 4];
            comps[3] = 255;
            if items.is_empty() || items.len() > 4 {
                return Err(err_bad_opt("fill color list must have 1..=4 byte ints"));
            }
            for (i, item) in items.iter().enumerate() {
                match item {
                    Value::Int(v) if (0..=255).contains(v) => comps[i] = *v as u8,
                    _ => {
                        return Err(err_bad_opt("fill color channel must be int 0..=255"));
                    }
                }
            }
            if items.len() == 1 && fmt == PixelFormat::Rgb8 {
                comps[1] = comps[0];
                comps[2] = comps[0];
            } else if items.len() == 1 && fmt == PixelFormat::Rgba8 {
                comps[1] = comps[0];
                comps[2] = comps[0];
            }
            Ok(comps)
        }
        other => Err(err_bad_opt(format!(
            "fill color must be packed int or [r,g,b,a] list, got '{}'",
            other.type_name()
        ))),
    }
}

fn img_new(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 4 {
        return Err(err_bad_opt(
            "image.new() expects (width, height, [format_or_opts], [fill])",
        ));
    }
    let w = expect_u32_dim(&args[0], "image.new width")?;
    let h = expect_u32_dim(&args[1], "image.new height")?;
    let mut fmt = PixelFormat::Rgba8;
    let mut cs = ColorSpace::Srgb;
    let mut fill_px: Option<[u8; 4]> = None;

    if let Some(arg2) = args.get(2) {
        match arg2 {
            Value::Unit => {}
            Value::Str(s) => fmt = PixelFormat::from_name(s)?,
            Value::Dict(d) => {
                let map = d.borrow();
                for (k, v) in map.iter() {
                    match k.as_str() {
                        "format" => {
                            let s = v.as_str().ok_or_else(|| {
                                err_bad_opt("image.new option 'format' must be str")
                            })?;
                            fmt = PixelFormat::from_name(s)?;
                        }
                        "color_space" => {
                            let s = v.as_str().ok_or_else(|| {
                                err_bad_opt("image.new option 'color_space' must be str")
                            })?;
                            cs = ColorSpace::from_name(s)?;
                        }
                        "fill" => fill_px = Some(parse_fill_pixel(v, fmt)?),
                        other => {
                            return Err(err_bad_opt(format!(
                                "image.new(): unknown option '{}'",
                                other
                            )));
                        }
                    }
                }
            }
            other => {
                return Err(err_bad_opt(format!(
                    "image.new() 3rd argument must be format str or opts dict, got '{}'",
                    other.type_name()
                )));
            }
        }
    }
    if let Some(arg3) = args.get(3) {
        if !matches!(arg3, Value::Unit) {
            fill_px = Some(parse_fill_pixel(arg3, fmt)?);
        }
    }

    let len = fmt.byte_len(w, h)?;
    let mut pixels = vec![0u8; len];
    if let Some(px) = fill_px {
        let ch = fmt.channels();
        for dst in pixels.chunks_exact_mut(ch) {
            dst.copy_from_slice(&px[..ch]);
        }
    }
    let im = MediaImage::from_vec(
        w,
        h,
        fmt,
        cs,
        SourceFormat::Raw,
        pixels,
        None,
        None,
        Some(vm.budget_core()),
    )?;
    Ok(Value::Image(im))
}

fn img_from_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 4 || args.len() > 5 {
        return Err(err_bad_opt(
            "image.from_bytes() expects (bytes, width, height, format, [color_space_or_opts])",
        ));
    }
    let raw = args[0].as_bytes_cow("image.from_bytes()")?;
    let w = expect_u32_dim(&args[1], "image.from_bytes width")?;
    let h = expect_u32_dim(&args[2], "image.from_bytes height")?;
    let sfmt = args[3]
        .as_str()
        .ok_or_else(|| err_bad_opt("image.from_bytes() format must be a string"))?;
    let fmt = PixelFormat::from_name(sfmt)?;
    let mut cs = ColorSpace::Srgb;
    if let Some(opt) = args.get(4) {
        match opt {
            Value::Unit => {}
            Value::Str(s) => cs = ColorSpace::from_name(s)?,
            Value::Dict(d) => {
                for (k, v) in d.borrow().iter() {
                    match k.as_str() {
                        "color_space" => {
                            let s = v.as_str().ok_or_else(|| {
                                err_bad_opt("image.from_bytes option 'color_space' must be str")
                            })?;
                            cs = ColorSpace::from_name(s)?;
                        }
                        other => {
                            return Err(err_bad_opt(format!(
                                "image.from_bytes(): unknown option '{}'",
                                other
                            )));
                        }
                    }
                }
            }
            other => {
                return Err(err_bad_opt(format!(
                    "image.from_bytes() color_space must be str or dict, got '{}'",
                    other.type_name()
                )));
            }
        }
    }
    let im = MediaImage::from_vec(
        w,
        h,
        fmt,
        cs,
        SourceFormat::Raw,
        raw.into_owned(),
        None,
        None,
        Some(vm.budget_core()),
    )?;
    Ok(Value::Image(im))
}

fn img_decode(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("image.decode() expects (bytes, [opts])"));
    }
    let bytes = args[0].as_bytes_cow("image.decode()")?;
    let opts = parse_decode_opts(args.get(1), "image.decode()")?;
    let deadline = || vm.check_timeout();
    let im = decode_image_bytes(&bytes, &opts, Some(vm.budget_core()), Some(&deadline))?;
    Ok(Value::Image(im))
}

fn img_load(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("image.load() expects (path, [opts])"));
    }
    let path = args[0]
        .as_str()
        .ok_or_else(|| err_bad_opt("image.load() path must be str"))?;
    let opts = parse_decode_opts(args.get(1), "image.load()")?;
    let (im, _) = crate::media::cache::load_image_cached(vm, path, &opts, false)?;
    Ok(Value::Image(im))
}

fn img_info(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.info() expects 1 argument (img)"));
    }
    let im = extract_image(&args[0], "image.info()")?;
    let meta = im.meta();
    let mut map = crate::value::new_dict_with_capacity(14);
    map.insert("width".to_string(), Value::Int(im.width as i64));
    map.insert("height".to_string(), Value::Int(im.height as i64));
    map.insert("format".to_string(), Value::from_str(im.format.name()));
    map.insert(
        "color_space".to_string(),
        Value::from_str(meta.color_space.name()),
    );
    map.insert(
        "source_format".to_string(),
        Value::from_str(meta.source_format.name()),
    );
    map.insert("has_alpha".to_string(), Value::Bool(meta.has_alpha));
    map.insert("channels".to_string(), Value::Int(im.format.channels() as i64));
    map.insert("byte_len".to_string(), Value::Int(im.byte_len() as i64));
    map.insert("mip_count".to_string(), Value::Int(meta.mip_count as i64));
    map.insert("frame_count".to_string(), Value::Int(meta.frame_count as i64));
    map.insert("is_animated".to_string(), Value::Bool(meta.is_animated));
    map.insert("loop_count".to_string(), Value::Int(meta.loop_count as i64));
    map.insert("digest".to_string(), Value::from_str(im.digest.to_hex()));
    let delays: Vec<Value> = im.delays_ms.iter().map(|&d| Value::Int(d as i64)).collect();
    map.insert(
        "delays_ms".to_string(),
        Value::List(Rc::new(RefCell::new(delays))),
    );
    Ok(Value::dict(map))
}

fn img_digest(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.digest() expects 1 argument (img or tex)"));
    }
    match &args[0] {
        Value::Image(im) => Ok(Value::from_str(im.digest.to_hex())),
        Value::Texture(t) => Ok(Value::from_str(t.digest.to_hex())),
        other => Err(err_bad_opt(format!(
            "image.digest() expects image or texture, got '{}'",
            other.type_name()
        ))),
    }
}

fn img_width(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.width() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.width()")?;
    Ok(Value::Int(im.width as i64))
}

fn img_height(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.height() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.height()")?;
    Ok(Value::Int(im.height as i64))
}

fn img_format(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.format() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.format()")?;
    Ok(Value::from_str(im.format.name()))
}

fn img_color_space(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.color_space() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.color_space()")?;
    Ok(Value::from_str(im.color_space.name()))
}

fn img_frame_count(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.frame_count() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.frame_count()")?;
    Ok(Value::Int(im.frame_count() as i64))
}

fn img_frame(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(err_bad_opt("image.frame() expects (img, index)"));
    }
    let im = extract_image(&args[0], "image.frame()")?;
    let idx = expect_i64(&args[1], "image.frame index")?;
    if idx < 0 || (idx as usize) >= im.frame_count() {
        return Err(err_bad_opt(format!(
            "image.frame() index {} out of bounds 0..{}",
            idx,
            im.frame_count()
        )));
    }
    let bytes = im
        .frame_data(idx as usize)
        .ok_or_else(|| err_bad_opt("frame not found"))?
        .to_vec();
    let frame_img = MediaImage::from_vec(
        im.width,
        im.height,
        im.format,
        im.color_space,
        SourceFormat::Derived,
        bytes,
        Some(im.has_alpha),
        None,
        Some(vm.budget_core()),
    )?;
    Ok(Value::Image(frame_img))
}

fn img_get_pixel(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err(err_bad_opt("image.get_pixel() expects (img, x, y)"));
    }
    let im = extract_image(&args[0], "image.get_pixel()")?;
    let x = expect_i64(&args[1], "x")?;
    let y = expect_i64(&args[2], "y")?;
    if x < 0 || y < 0 || x >= im.width as i64 || y >= im.height as i64 {
        return Err(err_bad_opt(format!(
            "image.get_pixel({}, {}) out of bounds {}x{}",
            x, y, im.width, im.height
        )));
    }
    let rgba = crate::media::sample::read_rgba_texel(
        &im.data,
        im.width,
        im.height,
        im.format,
        x as i32,
        y as i32,
    );
    let list = vec![
        Value::Int(rgba[0] as i64),
        Value::Int(rgba[1] as i64),
        Value::Int(rgba[2] as i64),
        Value::Int(rgba[3] as i64),
    ];
    Ok(Value::List(Rc::new(RefCell::new(list))))
}

fn img_to_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.to_bytes() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.to_bytes()")?;
    vm.charge_alloc(im.data.len())?;
    Ok(Value::Bytes(Rc::new(im.data.to_vec())))
}

fn img_convert(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(err_bad_opt("image.convert() expects (img, format)"));
    }
    let im = extract_image(&args[0], "image.convert()")?;
    let sfmt = args[1]
        .as_str()
        .ok_or_else(|| err_bad_opt("image.convert() format must be str"))?;
    let target = PixelFormat::from_name(sfmt)?;
    let out = convert_format(&im, target, Some(vm.budget_core()))?;
    Ok(Value::Image(out))
}

fn img_to_linear(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.to_linear() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.to_linear()")?;
    let out = to_linear(&im, Some(vm.budget_core()))?;
    Ok(Value::Image(out))
}

fn img_to_srgb(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.to_srgb() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.to_srgb()")?;
    let out = to_srgb(&im, Some(vm.budget_core()))?;
    Ok(Value::Image(out))
}

fn img_premultiply(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.premultiply() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.premultiply()")?;
    let out = premultiply(&im, Some(vm.budget_core()))?;
    Ok(Value::Image(out))
}

fn img_crop(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 5 {
        return Err(err_bad_opt("image.crop() expects (img, x, y, w, h)"));
    }
    let im = extract_image(&args[0], "image.crop()")?;
    let x = expect_i64(&args[1], "crop x")?;
    let y = expect_i64(&args[2], "crop y")?;
    let w = expect_i64(&args[3], "crop w")?;
    let h = expect_i64(&args[4], "crop h")?;
    let out = crop(&im, x, y, w, h, Some(vm.budget_core()))?;
    Ok(Value::Image(out))
}

fn img_resize(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err(err_bad_opt("image.resize() expects (img, w, h, [filter])"));
    }
    let im = extract_image(&args[0], "image.resize()")?;
    let w = expect_u32_dim(&args[1], "resize w")?;
    let h = expect_u32_dim(&args[2], "resize h")?;
    let filter = match args.get(3) {
        Some(Value::Str(s)) => s.as_str(),
        Some(Value::Unit) | None => "bilinear",
        Some(other) => {
            return Err(err_bad_opt(format!(
                "image.resize() filter must be str, got '{}'",
                other.type_name()
            )));
        }
    };
    let deadline = || vm.check_timeout();
    let out = resize(&im, w, h, filter, Some(vm.budget_core()), Some(&deadline))?;
    Ok(Value::Image(out))
}

fn img_flip(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err(err_bad_opt("image.flip() expects (img, [horiz], [vert])"));
    }
    let im = extract_image(&args[0], "image.flip()")?;
    let h = match args.get(1) {
        Some(Value::Bool(b)) => *b,
        None | Some(Value::Unit) => true,
        Some(other) => {
            return Err(err_bad_opt(format!(
                "image.flip() horizontal must be bool, got '{}'",
                other.type_name()
            )));
        }
    };
    let v = match args.get(2) {
        Some(Value::Bool(b)) => *b,
        None | Some(Value::Unit) => false,
        Some(other) => {
            return Err(err_bad_opt(format!(
                "image.flip() vertical must be bool, got '{}'",
                other.type_name()
            )));
        }
    };
    let out = flip(&im, h, v, Some(vm.budget_core()))?;
    Ok(Value::Image(out))
}

fn img_flip_x(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.flip_x() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.flip_x()")?;
    Ok(Value::Image(flip(&im, true, false, Some(vm.budget_core()))?))
}

fn img_flip_y(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.flip_y() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.flip_y()")?;
    Ok(Value::Image(flip(&im, false, true, Some(vm.budget_core()))?))
}

fn img_rotate(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(err_bad_opt("image.rotate() expects (img, degrees)"));
    }
    let im = extract_image(&args[0], "image.rotate()")?;
    let deg = expect_i64(&args[1], "rotate degrees")?;
    let out = rotate(&im, deg, Some(vm.budget_core()))?;
    Ok(Value::Image(out))
}

fn img_mipmaps(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.mipmaps() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.mipmaps()")?;
    let deadline = || vm.check_timeout();
    let chain = im.ensure_mipmaps(Some(vm.budget_core()), Some(&deadline))?;
    Ok(Value::Int(chain.level_count() as i64))
}

fn img_mip_count(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("image.mip_count() expects 1 argument"));
    }
    let im = extract_image(&args[0], "image.mip_count()")?;
    Ok(Value::Int(im.mip_count() as i64))
}

fn img_mip_level(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(err_bad_opt("image.mip_level() expects (img, level)"));
    }
    let im = extract_image(&args[0], "image.mip_level()")?;
    let lvl_idx = expect_i64(&args[1], "mip level")?;
    if lvl_idx < 0 {
        return Err(err_bad_opt("mip level cannot be negative"));
    }
    let deadline = || vm.check_timeout();
    let chain = im.ensure_mipmaps(Some(vm.budget_core()), Some(&deadline))?;
    let (lw, lh, slice) = chain
        .level_slice(lvl_idx as usize)
        .ok_or_else(|| err_bad_opt(format!("mip level {} out of range 0..{}", lvl_idx, chain.level_count())))?;
    let lvl_img = MediaImage::from_vec(
        lw,
        lh,
        im.format,
        im.color_space,
        SourceFormat::Derived,
        slice.to_vec(),
        Some(im.has_alpha),
        None,
        Some(vm.budget_core()),
    )?;
    Ok(Value::Image(lvl_img))
}

fn img_mip_digest(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(err_bad_opt("image.mip_digest() expects (img, level)"));
    }
    let im = extract_image(&args[0], "image.mip_digest()")?;
    let lvl_idx = expect_i64(&args[1], "mip level")?;
    if lvl_idx < 0 {
        return Err(err_bad_opt("mip level cannot be negative"));
    }
    let deadline = || vm.check_timeout();
    let chain = im.ensure_mipmaps(Some(vm.budget_core()), Some(&deadline))?;
    let (lw, lh, slice) = chain
        .level_slice(lvl_idx as usize)
        .ok_or_else(|| err_bad_opt(format!("mip level {} out of range 0..{}", lvl_idx, chain.level_count())))?;
    let digest = MediaImage::compute_raw_digest(lw, lh, im.format, im.color_space, slice);
    Ok(Value::from_str(digest.to_hex()))
}

fn img_height_to_normal(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("image.height_to_normal() expects (img, [strength])"));
    }
    let im = extract_image(&args[0], "image.height_to_normal()")?;
    let strength = match args.get(1) {
        Some(v) if !matches!(v, Value::Unit) => expect_f64(v, "strength")?,
        _ => 2.0,
    };
    let out = height_to_normal(&im, strength, Some(vm.budget_core()))?;
    Ok(Value::Image(out))
}

fn img_texture(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("image.texture() expects (img, [opts])"));
    }
    let im = extract_image(&args[0], "image.texture()")?;
    let mut wrap_u = WrapMode::Clamp;
    let mut wrap_v = WrapMode::Clamp;
    let mut filter = Filter::Nearest;
    let mut srgb = im.color_space == ColorSpace::Srgb;

    if let Some(opt_val) = args.get(1) {
        if !matches!(opt_val, Value::Unit) {
            let Value::Dict(dict) = opt_val else {
                return Err(err_bad_opt(format!(
                    "image.texture() opts must be dict, got '{}'",
                    opt_val.type_name()
                )));
            };
            for (k, v) in dict.borrow().iter() {
                match k.as_str() {
                    "wrap" => {
                        let s = v.as_str().ok_or_else(|| {
                            err_bad_opt("image.texture() option 'wrap' must be str")
                        })?;
                        let w = WrapMode::from_name(s)?;
                        wrap_u = w;
                        wrap_v = w;
                    }
                    "wrap_u" => {
                        let s = v.as_str().ok_or_else(|| {
                            err_bad_opt("image.texture() option 'wrap_u' must be str")
                        })?;
                        wrap_u = WrapMode::from_name(s)?;
                    }
                    "wrap_v" => {
                        let s = v.as_str().ok_or_else(|| {
                            err_bad_opt("image.texture() option 'wrap_v' must be str")
                        })?;
                        wrap_v = WrapMode::from_name(s)?;
                    }
                    "filter" => {
                        let s = v.as_str().ok_or_else(|| {
                            err_bad_opt("image.texture() option 'filter' must be str")
                        })?;
                        filter = Filter::from_name(s)?;
                    }
                    "srgb" => match v {
                        Value::Bool(b) => srgb = *b,
                        _ => {
                            return Err(err_bad_opt("image.texture() option 'srgb' must be bool"));
                        }
                    },
                    unknown => {
                        return Err(err_bad_opt(format!(
                            "image.texture(): unknown option '{}'",
                            unknown
                        )));
                    }
                }
            }
        }
    }
    let deadline = || vm.check_timeout();
    let tex = TextureData::new(
        im,
        wrap_u,
        wrap_v,
        filter,
        srgb,
        Some(vm.budget_core()),
        Some(&deadline),
    )?;
    Ok(Value::Texture(tex))
}

fn img_sample(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err(err_bad_opt("image.sample() expects (tex_or_img, u, v, [lod])"));
    }
    let u = expect_f64(&args[1], "sample u")?;
    let v = expect_f64(&args[2], "sample v")?;
    let lod = match args.get(3) {
        Some(lv) if !matches!(lv, Value::Unit) => expect_f64(lv, "sample lod")?,
        _ => 0.0,
    };
    let rgba = match &args[0] {
        Value::Texture(t) => sample_texture_uv(t, u, v, lod)?,
        Value::Image(im) => {
            let u_q16 = (u.clamp(-4096.0, 4096.0) * 65536.0).round() as i64;
            let v_q16 = (v.clamp(-4096.0, 4096.0) * 65536.0).round() as i64;
            sample_image_nearest(im, u_q16, v_q16, WrapMode::Clamp, WrapMode::Clamp)
        }
        other => {
            return Err(err_bad_opt(format!(
                "image.sample() expects texture or image, got '{}'",
                other.type_name()
            )));
        }
    };
    Ok(Value::List(Rc::new(RefCell::new(vec![
        Value::Int(rgba[0] as i64),
        Value::Int(rgba[1] as i64),
        Value::Int(rgba[2] as i64),
        Value::Int(rgba[3] as i64),
    ]))))
}

fn parse_atlas_opts(val: Option<&Value>) -> Result<(u32, u32), String> {
    let mut padding = 1u32;
    let mut max_size = 4096u32;
    if let Some(ov) = val {
        if !matches!(ov, Value::Unit) {
            let Value::Dict(d) = ov else {
                return Err(err_bad_opt("atlas opts must be dict"));
            };
            for (k, v) in d.borrow().iter() {
                match k.as_str() {
                    "padding" => match v {
                        Value::Int(p) if (0..=128).contains(p) => padding = *p as u32,
                        _ => return Err(err_bad_opt("atlas padding must be int 0..=128")),
                    },
                    "max_size" => match v {
                        Value::Int(m) if *m >= 1 && (*m as u64) <= MAX_DIM as u64 => {
                            max_size = *m as u32;
                        }
                        _ => return Err(err_bad_opt(format!("atlas max_size must be in 1..={}", MAX_DIM))),
                    },
                    other => {
                        return Err(err_bad_opt(format!("atlas(): unknown option '{}'", other)));
                    }
                }
            }
        }
    }
    Ok((padding, max_size))
}

fn img_atlas(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("image.atlas() expects (frames_dict_or_list, [opts])"));
    }
    let (padding, max_size) = parse_atlas_opts(args.get(1))?;
    let mut items: Vec<(String, Rc<MediaImage>)> = Vec::new();
    match &args[0] {
        Value::Dict(d) => {
            let map = d.borrow();
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (name, val) in entries {
                let im = extract_image(val, "image.atlas() entry")?;
                items.push((name.clone(), im));
            }
        }
        Value::List(l) => {
            for (idx, item) in l.borrow().iter().enumerate() {
                match item {
                    Value::Image(im) => items.push((idx.to_string(), Rc::clone(im))),
                    Value::List(pair) => {
                        let pb = pair.borrow();
                        if pb.len() != 2 {
                            return Err(err_bad_opt("atlas entry pair must have [name, image]"));
                        }
                        let name = pb[0]
                            .as_str()
                            .ok_or_else(|| err_bad_opt("atlas entry name must be str"))?
                            .to_string();
                        let im = extract_image(&pb[1], "image.atlas() entry")?;
                        items.push((name, im));
                    }
                    other => {
                        return Err(err_bad_opt(format!(
                            "image.atlas() list item must be image or [name, image], got '{}'",
                            other.type_name()
                        )));
                    }
                }
            }
        }
        other => {
            return Err(err_bad_opt(format!(
                "image.atlas() expects dict or list, got '{}'",
                other.type_name()
            )));
        }
    }
    let deadline = || vm.check_timeout();
    let atlas = pack_atlas(items, padding, max_size, Some(vm.budget_core()), Some(&deadline))?;
    Ok(atlas_to_value(&atlas))
}

fn img_atlas_from_paths(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt(
            "image.atlas_from_paths() expects (paths_dict_or_list, [opts])",
        ));
    }
    let (padding, max_size) = parse_atlas_opts(args.get(1))?;
    let mut specs: Vec<(String, String)> = Vec::new();
    match &args[0] {
        Value::Dict(d) => {
            let map = d.borrow();
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (name, val) in entries {
                let p = val
                    .as_str()
                    .ok_or_else(|| err_bad_opt("image.atlas_from_paths() dict values must be path strs"))?;
                specs.push((name.clone(), p.to_string()));
            }
        }
        Value::List(l) => {
            let mut used = HashMap::<String, usize>::new();
            for item in l.borrow().iter() {
                match item {
                    Value::Str(p) => {
                        let clean = p.split(['/', '\\']).last().unwrap_or(p.as_str());
                        let stem = clean.rsplit_once('.').map(|(s, _)| s).unwrap_or(clean).to_string();
                        let count = used.entry(stem.clone()).or_insert(0);
                        let name = if *count == 0 {
                            stem
                        } else {
                            format!("{}_{}", stem, *count)
                        };
                        *count += 1;
                        specs.push((name, p.to_string()));
                    }
                    Value::List(pair) => {
                        let pb = pair.borrow();
                        if pb.len() != 2 {
                            return Err(err_bad_opt("atlas path pair must be [name, path]"));
                        }
                        let name = pb[0]
                            .as_str()
                            .ok_or_else(|| err_bad_opt("atlas name must be str"))?
                            .to_string();
                        let p = pb[1]
                            .as_str()
                            .ok_or_else(|| err_bad_opt("atlas path must be str"))?
                            .to_string();
                        specs.push((name, p));
                    }
                    other => {
                        return Err(err_bad_opt(format!(
                            "image.atlas_from_paths() expects path str or [name, path], got '{}'",
                            other.type_name()
                        )));
                    }
                }
            }
        }
        other => {
            return Err(err_bad_opt(format!(
                "image.atlas_from_paths() expects dict or list, got '{}'",
                other.type_name()
            )));
        }
    }

    let opts = DecodeOpts::strict_default();
    let mut items = Vec::with_capacity(specs.len());
    for (name, p) in specs {
        let (im, _) = crate::media::cache::load_image_cached(vm, &p, &opts, false)?;
        items.push((name, im));
    }
    let deadline = || vm.check_timeout();
    let atlas = pack_atlas(items, padding, max_size, Some(vm.budget_core()), Some(&deadline))?;
    Ok(atlas_to_value(&atlas))
}

fn img_sprite_frame(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    match args.len() {
        2 => {
            let (im, frame) = resolve_atlas_frame(&args[0], &args[1])?;
            let sub = crop(
                &im,
                frame.x as i64,
                frame.y as i64,
                frame.w as i64,
                frame.h as i64,
                Some(vm.budget_core()),
            )?;
            Ok(Value::Image(sub))
        }
        4 => {
            let im = extract_image(&args[0], "image.sprite_frame()")?;
            let idx = expect_i64(&args[1], "frame_idx")?;
            let cols = expect_i64(&args[2], "cols")?;
            let rows = expect_i64(&args[3], "rows")?;
            if !(1..=4096).contains(&cols) || !(1..=4096).contains(&rows) {
                return Err(err_bad_opt("image.sprite_frame() cols/rows must be in 1..=4096"));
            }
            let total = cols * rows;
            if idx < 0 || idx >= total {
                return Err(err_bad_opt(format!(
                    "image.sprite_frame() index {} out of bounds 0..{}",
                    idx, total
                )));
            }
            let fw = (im.width as i64) / cols;
            let fh = (im.height as i64) / rows;
            if fw <= 0 || fh <= 0 {
                return Err(err_bad_opt(
                    "image.sprite_frame() grid exceeds image dimensions",
                ));
            }
            let cx = (idx % cols) * fw;
            let cy = (idx / cols) * fh;
            let sub = crop(&im, cx, cy, fw, fh, Some(vm.budget_core()))?;
            Ok(Value::Image(sub))
        }
        _ => Err(err_bad_opt(
            "image.sprite_frame() expects (atlas, key) or (img, idx, cols, rows)",
        )),
    }
}
