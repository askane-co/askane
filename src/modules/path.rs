use std::path::{Component, Path, PathBuf};

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "path",
        &[
            ("join", path_join),
            ("parent", path_parent),
            ("name", path_name),
            ("stem", path_stem),
            ("extension", path_extension),
            ("is_absolute", path_is_absolute),
            ("normalize", path_normalize),
        ],
        &[],
    )
}

fn expect_str<'a>(v: &'a Value, fn_name: &str) -> Result<&'a str, String> {
    match v {
        Value::Str(s) => Ok(s.as_str()),
        other => Err(format!(
            "path.{}() expects a string, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn path_join(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() {
        return Err("path.join() expects at least 1 argument".to_string());
    }
    let mut buf = PathBuf::new();
    for arg in args {
        buf.push(expect_str(arg, "join")?);
    }
    Ok(Value::Str(buf.to_string_lossy().to_string()))
}

fn path_parent(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("path.parent() expects 1 argument (path)".to_string());
    }
    let p = Path::new(expect_str(&args[0], "parent")?);
    match p.parent() {
        Some(parent) => Ok(Value::Str(parent.to_string_lossy().to_string())),
        None => Ok(Value::Unit),
    }
}

fn path_name(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("path.name() expects 1 argument (path)".to_string());
    }
    let p = Path::new(expect_str(&args[0], "name")?);
    match p.file_name() {
        Some(name) => Ok(Value::Str(name.to_string_lossy().to_string())),
        None => Ok(Value::Unit),
    }
}

fn path_stem(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("path.stem() expects 1 argument (path)".to_string());
    }
    let p = Path::new(expect_str(&args[0], "stem")?);
    match p.file_stem() {
        Some(stem) => Ok(Value::Str(stem.to_string_lossy().to_string())),
        None => Ok(Value::Unit),
    }
}

fn path_extension(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("path.extension() expects 1 argument (path)".to_string());
    }
    let p = Path::new(expect_str(&args[0], "extension")?);
    match p.extension() {
        Some(ext) => Ok(Value::Str(ext.to_string_lossy().to_string())),
        None => Ok(Value::Unit),
    }
}

fn path_is_absolute(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("path.is_absolute() expects 1 argument (path)".to_string());
    }
    let p = Path::new(expect_str(&args[0], "is_absolute")?);
    Ok(Value::Bool(p.is_absolute()))
}

fn path_normalize(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("path.normalize() expects 1 argument (path)".to_string());
    }
    let p = Path::new(expect_str(&args[0], "normalize")?);
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(comp);
                }
            }
            other => out.push(other),
        }
    }
    Ok(Value::Str(out.to_string_lossy().to_string()))
}
