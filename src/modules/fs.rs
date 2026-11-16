use std::cell::RefCell;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::rc::Rc;

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
            ("list_dir", fs_list_dir),
            ("mkdir", fs_mkdir),
            ("is_dir", fs_is_dir),
            ("read_bytes", fs_read_bytes),
            ("write_bytes", fs_write_bytes),
            ("copy", fs_copy),
            ("move", fs_move),
            ("size", fs_size),
            ("glob", fs_glob),
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
    let path_buf = vm.resolve_read_path(path)?;
    let content = crate::read_text_limited(&path_buf, 32 * 1024 * 1024)
        .map_err(|e| format!("fs.read('{}'): {}", path, e))?;
    Ok(Value::Str(content))
}

fn fs_write(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.write() expects 2 arguments (path, content)".to_string());
    }
    let path = expect_str(&args[0], "write")?;
    let content = expect_str(&args[1], "write")?;
    let path_buf = vm.resolve_write_path(path)?;
    fs::write(&path_buf, content).map_err(|e| format!("fs.write('{}'): {}", path, e))?;
    Ok(Value::Unit)
}

fn fs_append(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.append() expects 2 arguments (path, content)".to_string());
    }
    let path = expect_str(&args[0], "append")?;
    let content = expect_str(&args[1], "append")?;
    let path_buf = vm.resolve_write_path(path)?;
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
    Ok(Value::Bool(
        vm.resolve_read_path(path).is_ok_and(|p| p.exists()),
    ))
}

fn fs_remove(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("fs.remove() expects 1 argument (path)".to_string());
    }
    let path = expect_str(&args[0], "remove")?;
    let path_buf = vm.resolve_write_path(path)?;
    fs::remove_file(&path_buf).map_err(|e| format!("fs.remove('{}'): {}", path, e))?;
    Ok(Value::Unit)
}

fn fs_list_dir(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("fs.list_dir() expects 1 argument (path)".to_string());
    }
    let path = expect_str(&args[0], "list_dir")?;
    let path_buf = vm.resolve_read_path(path)?;
    let entries = fs::read_dir(&path_buf).map_err(|e| format!("fs.list_dir('{}'): {}", path, e))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("fs.list_dir('{}'): {}", path, e))?;
        names.push(Value::Str(entry.file_name().to_string_lossy().to_string()));
    }
    Ok(Value::List(Rc::new(RefCell::new(names))))
}

fn fs_mkdir(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("fs.mkdir() expects 1 argument (path)".to_string());
    }
    let path = expect_str(&args[0], "mkdir")?;
    let path_buf = vm.resolve_write_path(path)?;
    fs::create_dir_all(&path_buf).map_err(|e| format!("fs.mkdir('{}'): {}", path, e))?;
    Ok(Value::Unit)
}

fn fs_is_dir(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("fs.is_dir() expects 1 argument (path)".to_string());
    }
    let path = expect_str(&args[0], "is_dir")?;
    Ok(Value::Bool(
        vm.resolve_read_path(path).is_ok_and(|p| p.is_dir()),
    ))
}

fn check_not_symlink(vm: &VM, path_buf: &std::path::Path, raw: &str) -> Result<(), String> {
    if vm.is_sandboxed() {
        if let Ok(meta) = fs::symlink_metadata(path_buf) {
            if meta.file_type().is_symlink() {
                return Err(format!("Refusing to overwrite symlink '{}'", raw));
            }
        }
    }
    Ok(())
}

fn fs_read_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    use std::io::Read;
    if args.len() != 1 {
        return Err("fs.read_bytes() expects 1 argument (path)".to_string());
    }
    let path = expect_str(&args[0], "read_bytes")?;
    let path_buf = vm.resolve_read_path(path)?;
    let file =
        fs::File::open(&path_buf).map_err(|e| format!("fs.read_bytes('{}'): {}", path, e))?;
    if !file
        .metadata()
        .map_err(|e| format!("fs.read_bytes('{}'): {}", path, e))?
        .is_file()
    {
        return Err("Only regular files can be read".to_string());
    }
    const LIMIT: usize = 32 * 1024 * 1024;
    let mut buf = Vec::new();
    file.take(LIMIT as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("fs.read_bytes('{}'): {}", path, e))?;
    if buf.len() > LIMIT {
        return Err(format!("File '{}' exceeds {} byte limit", path, LIMIT));
    }
    let list: Vec<Value> = buf.into_iter().map(|b| Value::Int(b as i64)).collect();
    Ok(Value::List(Rc::new(RefCell::new(list))))
}

fn fs_write_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.write_bytes() expects 2 arguments (path, byte_list)".to_string());
    }
    let path = expect_str(&args[0], "write_bytes")?;
    let list = match &args[1] {
        Value::List(l) => l,
        other => {
            return Err(format!(
                "fs.write_bytes() expects a byte list, got '{}'",
                other.type_name()
            ));
        }
    };
    let borrowed = list.borrow();
    if borrowed.len() > 32 * 1024 * 1024 {
        return Err("fs.write_bytes() exceeds 32 MiB limit".to_string());
    }
    let mut bytes = Vec::with_capacity(borrowed.len());
    for item in borrowed.iter() {
        match item {
            Value::Int(b) if (0..=255).contains(b) => bytes.push(*b as u8),
            _ => return Err("fs.write_bytes() list items must be integers 0..=255".to_string()),
        }
    }
    let path_buf = vm.resolve_write_path(path)?;
    check_not_symlink(vm, &path_buf, path)?;
    fs::write(&path_buf, bytes).map_err(|e| format!("fs.write_bytes('{}'): {}", path, e))?;
    Ok(Value::Unit)
}

fn fs_copy(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.copy() expects 2 arguments (src, dst)".to_string());
    }
    let src = expect_str(&args[0], "copy")?;
    let dst = expect_str(&args[1], "copy")?;
    let src_buf = vm.resolve_read_path(src)?;
    let dst_buf = vm.resolve_write_path(dst)?;
    check_not_symlink(vm, &dst_buf, dst)?;
    let bytes = fs::copy(&src_buf, &dst_buf)
        .map_err(|e| format!("fs.copy('{}', '{}'): {}", src, dst, e))?;
    Ok(Value::Int(bytes as i64))
}

fn fs_move(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.move() expects 2 arguments (src, dst)".to_string());
    }
    let src = expect_str(&args[0], "move")?;
    let dst = expect_str(&args[1], "move")?;
    let src_buf = vm.resolve_write_path(src)?;
    let dst_buf = vm.resolve_write_path(dst)?;
    check_not_symlink(vm, &dst_buf, dst)?;
    fs::rename(&src_buf, &dst_buf).map_err(|e| format!("fs.move('{}', '{}'): {}", src, dst, e))?;
    Ok(Value::Unit)
}

fn fs_size(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("fs.size() expects 1 argument (path)".to_string());
    }
    let path = expect_str(&args[0], "size")?;
    let path_buf = vm.resolve_read_path(path)?;
    let meta = fs::metadata(&path_buf).map_err(|e| format!("fs.size('{}'): {}", path, e))?;
    Ok(Value::Int(meta.len() as i64))
}

fn collect_glob(
    vm: &VM,
    dir: &std::path::Path,
    base: &std::path::Path,
    pattern: &str,
    out: &mut Vec<Value>,
    depth: usize,
) -> Result<(), String> {
    if depth > 16 || out.len() >= 5000 {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        if out.len() >= 5000 {
            break;
        }
        let path = entry.path();
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if meta.file_type().is_symlink() {
                continue;
            }
            let rel = path.strip_prefix(base).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if crate::modules::glob::glob_matches(pattern, &rel_str)
                && vm.resolve_read_path(&rel_str).is_ok()
            {
                out.push(Value::Str(rel_str));
            }
            if meta.is_dir() {
                collect_glob(vm, &path, base, pattern, out, depth + 1)?;
            }
        }
    }
    Ok(())
}

fn fs_glob(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("fs.glob() expects 1 string argument (pattern)".to_string());
    }
    let pattern = expect_str(&args[0], "glob")?;
    let root = vm.sandbox_root().clone();
    let mut out = Vec::new();
    collect_glob(vm, &root, &root, pattern, &mut out, 0)?;
    Ok(Value::List(Rc::new(RefCell::new(out))))
}
