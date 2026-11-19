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

const MAX_FS_BYTES: usize = 32 * 1024 * 1024;

fn check_safe_write_target(vm: &VM, path_buf: &std::path::Path, raw: &str) -> Result<(), String> {
    if vm.is_sandboxed() {
        if let Ok(meta) = fs::symlink_metadata(path_buf) {
            if meta.file_type().is_symlink() {
                return Err(format!("Refusing to overwrite symlink '{}'", raw));
            }
        }
        if let Some(ext) = path_buf.extension().and_then(|e| e.to_str()) {
            let lower = ext.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "exe"
                    | "dll"
                    | "bat"
                    | "cmd"
                    | "ps1"
                    | "vbs"
                    | "vbe"
                    | "js"
                    | "jse"
                    | "wsf"
                    | "wsh"
                    | "scr"
                    | "com"
                    | "msi"
                    | "hta"
                    | "pif"
            ) {
                return Err(format!(
                    "Writing executable or script extension '.{}' is blocked in sandbox mode",
                    lower
                ));
            }
        }
    }
    Ok(())
}

fn fs_read(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("fs.read() expects 1 argument (path)".to_string());
    }
    let path = expect_str(&args[0], "read")?;
    let path_buf = vm.resolve_read_path(path)?;
    if let Ok(meta) = fs::metadata(&path_buf) {
        if meta.len() <= MAX_FS_BYTES as u64 {
            let size = meta.len() as usize;
            vm.charge_io(size)?;
            vm.charge_alloc(size)?;
        }
    }
    let content = crate::read_text_limited(&path_buf, MAX_FS_BYTES)
        .map_err(|e| format!("fs.read('{}'): {}", path, e))?;
    Ok(Value::Str(content))
}

fn fs_write(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.write() expects 2 arguments (path, content)".to_string());
    }
    let path = expect_str(&args[0], "write")?;
    let content = expect_str(&args[1], "write")?;
    if content.len() > MAX_FS_BYTES {
        return Err("fs.write() exceeds 32 MiB limit".to_string());
    }
    let path_buf = vm.resolve_write_path(path)?;
    check_safe_write_target(vm, &path_buf, path)?;
    vm.charge_io(content.len())?;
    fs::write(&path_buf, content).map_err(|e| format!("fs.write('{}'): {}", path, e))?;
    Ok(Value::Unit)
}

fn fs_append(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.append() expects 2 arguments (path, content)".to_string());
    }
    let path = expect_str(&args[0], "append")?;
    let content = expect_str(&args[1], "append")?;
    if content.len() > MAX_FS_BYTES {
        return Err("fs.append() exceeds 32 MiB limit".to_string());
    }
    let path_buf = vm.resolve_write_path(path)?;
    check_safe_write_target(vm, &path_buf, path)?;
    if let Ok(meta) = fs::metadata(&path_buf) {
        if meta.len().saturating_add(content.len() as u64) > MAX_FS_BYTES as u64 {
            return Err("fs.append() target file exceeds 32 MiB limit".to_string());
        }
    }
    vm.charge_io(content.len())?;
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
    let path_buf = vm.resolve_write_link_or_path(path)?;
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
    check_safe_write_target(vm, path_buf, raw)
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
    let meta = file
        .metadata()
        .map_err(|e| format!("fs.read_bytes('{}'): {}", path, e))?;
    if !meta.is_file() {
        return Err("Only regular files can be read".to_string());
    }
    if meta.len() > MAX_FS_BYTES as u64 {
        return Err(format!(
            "File '{}' exceeds {} byte limit",
            path, MAX_FS_BYTES
        ));
    }
    let expected = meta.len() as usize;
    vm.charge_io(expected)?;
    vm.charge_alloc(expected)?;
    let mut buf = Vec::new();
    file.take(MAX_FS_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("fs.read_bytes('{}'): {}", path, e))?;
    if buf.len() > MAX_FS_BYTES {
        return Err(format!(
            "File '{}' exceeds {} byte limit",
            path, MAX_FS_BYTES
        ));
    }
    Ok(Value::Bytes(Rc::new(buf)))
}

fn fs_write_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.write_bytes() expects 2 arguments (path, bytes)".to_string());
    }
    let path = expect_str(&args[0], "write_bytes")?;
    let bytes = args[1].as_bytes_cow("fs.write_bytes()")?;
    if bytes.len() > MAX_FS_BYTES {
        return Err("fs.write_bytes() exceeds 32 MiB limit".to_string());
    }
    let path_buf = vm.resolve_write_path(path)?;
    check_not_symlink(vm, &path_buf, path)?;
    vm.charge_io(bytes.len())?;
    fs::write(&path_buf, bytes.as_ref())
        .map_err(|e| format!("fs.write_bytes('{}'): {}", path, e))?;
    Ok(Value::Unit)
}

fn fs_copy(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.copy() expects 2 arguments (src, dst)".to_string());
    }
    let src = expect_str(&args[0], "copy")?;
    let dst = expect_str(&args[1], "copy")?;
    let src_buf = vm.resolve_read_path(src)?;
    let meta =
        fs::metadata(&src_buf).map_err(|e| format!("fs.copy('{}', '{}'): {}", src, dst, e))?;
    if !meta.is_file() {
        return Err(format!(
            "fs.copy('{}'): only regular files can be copied",
            src
        ));
    }
    if meta.len() > MAX_FS_BYTES as u64 {
        return Err("fs.copy() exceeds 32 MiB limit".to_string());
    }
    let dst_buf = vm.resolve_write_path(dst)?;
    check_not_symlink(vm, &dst_buf, dst)?;
    vm.charge_io(meta.len() as usize)?;
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
    let src_buf = vm.resolve_write_link_or_path(src)?;
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

fn strip_verbatim(path: &std::path::Path) -> std::borrow::Cow<'_, str> {
    let s = path.to_string_lossy();
    #[cfg(windows)]
    if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
        return std::borrow::Cow::Owned(format!("\\\\{}", rest));
    } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
        return std::borrow::Cow::Owned(rest.to_string());
    }
    s
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
            let check_target = strip_verbatim(&path);
            if crate::modules::glob::glob_matches(pattern, &rel_str)
                && vm.resolve_read_path(&check_target).is_ok()
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
    let root = fs::canonicalize(vm.sandbox_root()).unwrap_or_else(|_| vm.sandbox_root().clone());
    let mut out = Vec::new();
    collect_glob(vm, &root, &root, pattern, &mut out, 0)?;
    Ok(Value::List(Rc::new(RefCell::new(out))))
}
