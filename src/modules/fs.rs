use std::fs::{self, OpenOptions};
use std::io::Write;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "fs",
        &[
            ("read", fs_read),
            ("write", fs_write),
            ("append", fs_append),
            ("exists", fs_exists),
            ("remove", fs_remove),
        ],
        &[],
    )
}

fn expect_str<'a>(v: &'a Value, fn_name: &str) -> Result<&'a str, String> {
    match v {
        Value::Str(s) => Ok(s.as_str()),
        other => Err(format!(
            "fs.{}() expects a string, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn fs_read(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("fs.read() expects 1 argument (path)".to_string());
    }
    let path = expect_str(&args[0], "read")?;
    let path_buf = vm.resolve_path(path)?;
    let content =
        fs::read_to_string(&path_buf).map_err(|e| format!("fs.read('{}'): {}", path, e))?;
    Ok(Value::Str(content))
}

fn fs_write(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.write() expects 2 arguments (path, content)".to_string());
    }
    let path = expect_str(&args[0], "write")?;
    let content = expect_str(&args[1], "write")?;
    let path_buf = vm.resolve_path(path)?;
    fs::write(&path_buf, content).map_err(|e| format!("fs.write('{}'): {}", path, e))?;
    Ok(Value::Unit)
}

fn fs_append(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.append() expects 2 arguments (path, content)".to_string());
    }
    let path = expect_str(&args[0], "append")?;
    let content = expect_str(&args[1], "append")?;
    let path_buf = vm.resolve_path(path)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path_buf)
        .map_err(|e| format!("fs.append('{}'): {}", path, e))?;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("fs.append('{}'): {}", path, e))?;
    Ok(Value::Unit)
}

fn fs_exists(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("fs.exists() expects 1 argument (path)".to_string());
    }
    let path = expect_str(&args[0], "exists")?;
    Ok(Value::Bool(vm.resolve_path(path).is_ok_and(|p| p.exists())))
}

fn fs_remove(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("fs.remove() expects 1 argument (path)".to_string());
    }
    let path = expect_str(&args[0], "remove")?;
    let path_buf = vm.resolve_path(path)?;
    fs::remove_file(&path_buf).map_err(|e| format!("fs.remove('{}'): {}", path, e))?;
    Ok(Value::Unit)
}
