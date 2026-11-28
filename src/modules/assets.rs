use std::cell::RefCell;
use std::rc::Rc;

use crate::media::audio::wav::decode_wav_sound;
use crate::media::cache::{
    bundle_list_entries, cache_stats, load_image_cached, load_texture_cached, mount_bundle_bytes,
    mount_bundle_file, read_asset_bytes, reset_cache, run_cache_gc, set_cache_budget,
    unmount_bundle,
};
use crate::media::err_bad_opt;
use crate::media::image::{ColorSpace, DecodeOpts, Filter, WrapMode, parse_decode_opts};
use crate::modules::audio::parse_loop_override_opt;
use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "assets",
        &[
            ("load_image", assets_load_image),
            ("load_texture", assets_load_texture),
            ("load_sound", assets_load_sound),
            ("sound", assets_load_sound),
            ("read_bytes", assets_read_bytes),
            ("read_text", assets_read_text),
            ("exists", assets_exists),
            ("mount", assets_mount),
            ("mount_bytes", assets_mount_bytes),
            ("unmount", assets_unmount),
            ("entries", assets_entries),
            ("list", assets_entries),
            ("set_budget", assets_set_budget),
            ("gc", assets_gc),
            ("stats", assets_stats),
            ("clear", assets_clear),
            ("reset", assets_clear),
        ],
        &[],
    )
}

fn expect_str<'a>(val: &'a Value, ctx: &str) -> Result<&'a str, String> {
    val.as_str().ok_or_else(|| {
        err_bad_opt(format!(
            "{} expects str, got '{}'",
            ctx,
            val.type_name()
        ))
    })
}

fn assets_load_image(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("assets.load_image() expects (path, [opts])"));
    }
    let path = expect_str(&args[0], "assets.load_image()")?;
    let opts = parse_decode_opts(args.get(1), "assets.load_image()")?;
    let (img, _) = load_image_cached(vm, path, &opts, false)?;
    Ok(Value::Image(img))
}

struct TexLoadOpts {
    decode: DecodeOpts,
    wrap_u: Option<WrapMode>,
    wrap_v: Option<WrapMode>,
    filter: Option<Filter>,
    srgb: Option<bool>,
}

fn parse_texture_load_opts(val: Option<&Value>) -> Result<TexLoadOpts, String> {
    let mut out = TexLoadOpts {
        decode: DecodeOpts::strict_default(),
        wrap_u: None,
        wrap_v: None,
        filter: None,
        srgb: None,
    };
    let Some(v) = val else {
        return Ok(out);
    };
    if matches!(v, Value::Unit) {
        return Ok(out);
    }
    let Value::Dict(dict) = v else {
        return Err(err_bad_opt(format!(
            "assets.load_texture() opts must be dict, got '{}'",
            v.type_name()
        )));
    };
    let mut decode_sub = crate::value::new_dict_with_capacity(6);
    for (k, fv) in dict.borrow().iter() {
        match k.as_str() {
            "format" | "color_space" | "mipmaps" | "strict_crc" | "max_frames" | "max_dim" => {
                decode_sub.insert(k.clone(), fv.clone());
            }
            "wrap" => {
                let s = fv.as_str().ok_or_else(|| {
                    err_bad_opt("assets.load_texture() option 'wrap' must be str")
                })?;
                let w = WrapMode::from_name(s)?;
                out.wrap_u = Some(w);
                out.wrap_v = Some(w);
            }
            "wrap_u" => {
                let s = fv.as_str().ok_or_else(|| {
                    err_bad_opt("assets.load_texture() option 'wrap_u' must be str")
                })?;
                out.wrap_u = Some(WrapMode::from_name(s)?);
            }
            "wrap_v" => {
                let s = fv.as_str().ok_or_else(|| {
                    err_bad_opt("assets.load_texture() option 'wrap_v' must be str")
                })?;
                out.wrap_v = Some(WrapMode::from_name(s)?);
            }
            "filter" => {
                let s = fv.as_str().ok_or_else(|| {
                    err_bad_opt("assets.load_texture() option 'filter' must be str")
                })?;
                out.filter = Some(Filter::from_name(s)?);
            }
            "srgb" => match fv {
                Value::Bool(b) => out.srgb = Some(*b),
                _ => {
                    return Err(err_bad_opt(
                        "assets.load_texture() option 'srgb' must be bool",
                    ));
                }
            },
            unknown => {
                return Err(err_bad_opt(format!(
                    "assets.load_texture(): unknown option '{}'",
                    unknown
                )));
            }
        }
    }
    if !decode_sub.is_empty() {
        out.decode = parse_decode_opts(Some(&Value::dict(decode_sub)), "assets.load_texture()")?;
    }
    if out.srgb.is_none() {
        if let Some(cs) = out.decode.color_space {
            out.srgb = Some(cs == ColorSpace::Srgb);
        }
    }
    Ok(out)
}

fn assets_load_texture(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("assets.load_texture() expects (path, [opts])"));
    }
    let path = expect_str(&args[0], "assets.load_texture()")?;
    let opts = parse_texture_load_opts(args.get(1))?;
    let tex = load_texture_cached(
        vm,
        path,
        &opts.decode,
        opts.wrap_u,
        opts.wrap_v,
        opts.filter,
        opts.srgb,
    )?;
    Ok(Value::Texture(tex))
}

fn assets_load_sound(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("assets.load_sound() expects (path, [opts])"));
    }
    let path = expect_str(&args[0], "assets.load_sound()")?;
    let loop_override = parse_loop_override_opt(args.get(1))?;
    let (bytes, _, _) = read_asset_bytes(vm, path)?;
    let snd = decode_wav_sound(
        &bytes,
        loop_override,
        Some(vm.budget_core()),
        Some(&|| vm.check_timeout()),
    )?;
    Ok(Value::Sound(Rc::new(snd)))
}

fn assets_read_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("assets.read_bytes() expects (path)"));
    }
    let path = expect_str(&args[0], "assets.read_bytes()")?;
    let (bytes, _, _) = read_asset_bytes(vm, path)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}

fn assets_read_text(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("assets.read_text() expects (path)"));
    }
    let path = expect_str(&args[0], "assets.read_text()")?;
    let (bytes, _, _) = read_asset_bytes(vm, path)?;
    let s = String::from_utf8(bytes)
        .map_err(|_| crate::media::err_corrupt(format!("asset '{}' is not valid UTF-8", path)))?;
    Ok(Value::from_str(s))
}

fn assets_exists(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("assets.exists() expects (path)"));
    }
    let path = expect_str(&args[0], "assets.exists()")?;
    if let Ok(norm) = crate::media::cache::normalize_vpath(path) {
        if bundle_list_entries().iter().any(|e| e == &norm) {
            return Ok(Value::Bool(true));
        }
    }
    let Ok(resolved) = vm.resolve_read_path(path) else {
        return Ok(Value::Bool(false));
    };
    Ok(Value::Bool(resolved.is_file()))
}

fn assets_mount(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("assets.mount() expects (zip_path, [prefix])"));
    }
    let path = expect_str(&args[0], "assets.mount()")?;
    let prefix = match args.get(1) {
        None | Some(Value::Unit) => None,
        Some(v) => Some(expect_str(v, "assets.mount() prefix")?),
    };
    let id = mount_bundle_file(vm, path, prefix)?;
    Ok(Value::Int(id))
}

fn assets_mount_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("assets.mount_bytes() expects (zip_bytes, [prefix])"));
    }
    let raw = args[0].as_bytes_cow("assets.mount_bytes()")?;
    let prefix = match args.get(1) {
        None | Some(Value::Unit) => None,
        Some(v) => Some(expect_str(v, "assets.mount_bytes() prefix")?),
    };
    let id = mount_bundle_bytes(vm, raw.into_owned(), prefix)?;
    Ok(Value::Int(id))
}

fn assets_unmount(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("assets.unmount() expects (bundle_id_or_prefix)"));
    }
    let removed = unmount_bundle(&args[0])?;
    Ok(Value::Bool(removed))
}

fn assets_entries(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err(err_bad_opt("assets.entries() expects 0 arguments"));
    }
    let list: Vec<Value> = bundle_list_entries()
        .into_iter()
        .map(Value::from_str)
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(list))))
}

fn assets_set_budget(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("assets.set_budget() expects (max_bytes)"));
    }
    let Value::Int(bytes) = &args[0] else {
        return Err(err_bad_opt("assets.set_budget() expects integer bytes"));
    };
    if *bytes < 0 {
        return Err(err_bad_opt("assets.set_budget() cannot be negative"));
    }
    set_cache_budget(*bytes as usize)?;
    Ok(Value::Unit)
}

fn assets_gc(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err(err_bad_opt("assets.gc() expects 0 arguments"));
    }
    let evicted = run_cache_gc();
    Ok(Value::Int(evicted as i64))
}

fn assets_stats(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err(err_bad_opt("assets.stats() expects 0 arguments"));
    }
    let (used_bytes, max_bytes, count, hits, misses, evictions, bundles) = cache_stats();
    let mut map = crate::value::new_dict_with_capacity(7);
    map.insert("used_bytes".to_string(), Value::Int(used_bytes as i64));
    map.insert("max_bytes".to_string(), Value::Int(max_bytes as i64));
    map.insert("count".to_string(), Value::Int(count as i64));
    map.insert("hits".to_string(), Value::Int(hits as i64));
    map.insert("misses".to_string(), Value::Int(misses as i64));
    map.insert("evictions".to_string(), Value::Int(evictions as i64));
    map.insert("bundles".to_string(), Value::Int(bundles as i64));
    Ok(Value::dict(map))
}

fn assets_clear(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err(err_bad_opt("assets.clear() expects 0 arguments"));
    }
    reset_cache();
    Ok(Value::Unit)
}
