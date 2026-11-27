use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::modules::{NativeFnPtr, build_module};
use crate::value::{FastMap, StructDef, StructInstance, Value, new_dict_with_capacity};
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
            ("remove_dir", fs_remove_dir),
            ("list_dir", fs_list_dir),
            ("walk", fs_walk),
            ("mkdir", fs_mkdir),
            ("is_dir", fs_is_dir),
            ("read_bytes", fs_read_bytes),
            ("write_bytes", fs_write_bytes),
            ("read_lines", fs_read_lines),
            ("write_lines", fs_write_lines),
            ("open_lines", fs_open_lines),
            ("open_writer", fs_open_writer),
            ("write_stream", fs_open_writer),
            ("temp_file", fs_temp_file),
            ("copy", fs_copy),
            ("copy_dir", fs_copy_dir),
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

pub(crate) fn check_safe_write_target(
    vm: &VM,
    path_buf: &std::path::Path,
    raw: &str,
) -> Result<(), String> {
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
    Ok(Value::from_str(content))
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
    for (i, entry) in entries.enumerate() {
        if (i & 255) == 0 {
            vm.check_timeout()?;
            vm.charge_work(1)?;
        }
        if names.len() >= 100_000 {
            return Err("fs.list_dir() directory entry limit (100000) exceeded".to_string());
        }
        let entry = entry.map_err(|e| format!("fs.list_dir('{}'): {}", path, e))?;
        let s = entry.file_name().to_string_lossy().to_string();
        vm.charge_alloc(s.len().saturating_add(std::mem::size_of::<Value>()))?;
        names.push(Value::from_str(s));
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
    vm: &mut VM,
    dir: &std::path::Path,
    base: &std::path::Path,
    pattern: &str,
    out: &mut Vec<Value>,
    depth: usize,
) -> Result<(), String> {
    if depth > 16 || out.len() >= 5000 {
        return Ok(());
    }
    vm.check_timeout()?;
    vm.charge_work(1)?;
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for (idx, entry) in entries.flatten().enumerate() {
        if (idx & 255) == 0 {
            vm.check_timeout()?;
            vm.charge_work(1)?;
        }
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
                vm.charge_alloc(rel_str.len().saturating_add(std::mem::size_of::<Value>()))?;
                out.push(Value::from_str(rel_str));
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

const MAX_LINE_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_LINES: usize = 200_000;
const HARD_MAX_LINES: usize = 1_000_000;
const MAX_DIR_DEPTH_HARD: usize = 32;
const MAX_WALK_ENTRIES_HARD: usize = 100_000;
static TEMP_FILE_SEQ: AtomicU64 = AtomicU64::new(1);

struct FsReaderHandle {
    reader: Option<BufReader<File>>,
    bytes_read: usize,
    closed: bool,
}

struct FsWriterHandle {
    writer: Option<BufWriter<File>>,
    bytes_written: usize,
    closed: bool,
}

struct FsStreamRegistry {
    next_id: u64,
    readers: HashMap<u64, FsReaderHandle>,
    writers: HashMap<u64, FsWriterHandle>,
    reader_def: Rc<StructDef>,
    writer_def: Rc<StructDef>,
}

fn make_handle_def(name: &str, methods_spec: &[(&'static str, NativeFnPtr)]) -> Rc<StructDef> {
    let field_names = vec!["__id".to_string()];
    let mut field_index = FastMap::default();
    field_index.insert("__id".to_string(), 0);
    let mut methods = new_dict_with_capacity(methods_spec.len());
    for &(mname, func) in methods_spec {
        methods.insert(mname.to_string(), Value::native_fn(mname, func));
    }
    Rc::new(StructDef {
        ic_id: crate::bytecode::next_ic_id(),
        name: name.to_string(),
        field_names,
        field_index,
        field_defaults: vec![None],
        methods,
    })
}

thread_local! {
    static FS_STREAMS: RefCell<FsStreamRegistry> = RefCell::new(FsStreamRegistry {
        next_id: 1,
        readers: HashMap::new(),
        writers: HashMap::new(),
        reader_def: make_handle_def("FsLineReader", &[
            ("next", fs_reader_next),
            ("close", fs_reader_close),
        ]),
        writer_def: make_handle_def("FsStreamWriter", &[
            ("write", fs_writer_write),
            ("write_line", fs_writer_write_line),
            ("flush", fs_writer_flush),
            ("close", fs_writer_close),
        ]),
    });
}

pub fn reset_state() {
    FS_STREAMS.with(|reg| {
        let mut reg = reg.borrow_mut();
        reg.readers.clear();
        for (_, mut w) in reg.writers.drain() {
            if let Some(ref mut bw) = w.writer {
                let _ = bw.flush();
            }
        }
    });
}

fn handle_id(v: &Value, expected: &str, method: &str) -> Result<u64, String> {
    let Value::StructInstance(inst) = v else {
        return Err(format!("fs.{}() expects a {} handle", method, expected));
    };
    if inst.def.name != expected {
        return Err(format!(
            "fs.{}() expects a {} handle, got '{}'",
            method, expected, inst.def.name
        ));
    }
    match inst.fields.borrow().first() {
        Some(Value::Int(id)) if *id > 0 => Ok(*id as u64),
        _ => Err(format!("Invalid {} handle", expected)),
    }
}

fn read_one_bounded_line(
    reader: &mut BufReader<File>,
    total_read: &mut usize,
    ctx: &str,
) -> Result<Option<String>, String> {
    let mut raw = Vec::new();
    let mut chunk_reader = std::io::Read::take(reader, (MAX_LINE_BYTES + 1) as u64);
    let n = chunk_reader
        .read_until(b'\n', &mut raw)
        .map_err(|e| format!("{}: {}", ctx, e))?;
    if n == 0 {
        return Ok(None);
    }
    if raw.len() > MAX_LINE_BYTES && !raw.ends_with(b"\n") {
        return Err(format!(
            "{}: line exceeds {} byte limit",
            ctx, MAX_LINE_BYTES
        ));
    }
    *total_read = total_read
        .checked_add(n)
        .ok_or_else(|| format!("{}: total size overflow", ctx))?;
    if *total_read > MAX_FS_BYTES {
        return Err(format!(
            "{}: total read exceeds {} byte limit",
            ctx, MAX_FS_BYTES
        ));
    }
    if raw.last() == Some(&b'\n') {
        raw.pop();
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
    }
    let line = String::from_utf8(raw).map_err(|_| format!("{}: invalid UTF-8 in line", ctx))?;
    Ok(Some(line))
}

fn fs_read_lines(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("fs.read_lines() expects 1 or 2 arguments (path, [max_lines])".to_string());
    }
    let path = expect_str(&args[0], "read_lines")?;
    let max_lines = if args.len() == 2 {
        match &args[1] {
            Value::Int(n) if *n >= 0 => (*n as usize).min(HARD_MAX_LINES),
            other => {
                return Err(format!(
                    "fs.read_lines() max_lines must be a non-negative int, got '{}'",
                    other.type_name()
                ));
            }
        }
    } else {
        DEFAULT_MAX_LINES
    };
    let path_buf = vm.resolve_read_path(path)?;
    let file = File::open(&path_buf).map_err(|e| format!("fs.read_lines('{}'): {}", path, e))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("fs.read_lines('{}'): {}", path, e))?;
    if !meta.is_file() {
        return Err("Only regular files can be read".to_string());
    }
    let mut reader = BufReader::new(file);
    let mut lines = Vec::new();
    let mut total_read = 0usize;
    let ctx = format!("fs.read_lines('{}')", path);
    loop {
        if lines.len() >= max_lines {
            break;
        }
        if (lines.len() & 255) == 0 {
            vm.check_timeout()?;
        }
        let Some(line) = read_one_bounded_line(&mut reader, &mut total_read, &ctx)? else {
            break;
        };
        vm.charge_io(line.len().max(1))?;
        vm.charge_alloc(line.len().saturating_add(std::mem::size_of::<Value>()))?;
        lines.push(Value::from_str(line));
    }
    Ok(Value::List(Rc::new(RefCell::new(lines))))
}

fn fs_write_lines(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("fs.write_lines() expects 2 arguments (path, lines)".to_string());
    }
    let path = expect_str(&args[0], "write_lines")?;
    let Value::List(list) = &args[1] else {
        return Err(format!(
            "fs.write_lines() expects a list of strings, got '{}'",
            args[1].type_name()
        ));
    };
    let items = list.borrow().clone();
    let path_buf = vm.resolve_write_path(path)?;
    check_safe_write_target(vm, &path_buf, path)?;
    let mut total_bytes = 0usize;
    for item in &items {
        let Value::Str(s) = item else {
            return Err(format!(
                "fs.write_lines() expects string lines, got '{}'",
                item.type_name()
            ));
        };
        total_bytes = total_bytes.saturating_add(s.len().saturating_add(1));
        if total_bytes > MAX_FS_BYTES {
            return Err("fs.write_lines() exceeds 32 MiB limit".to_string());
        }
    }
    vm.charge_io(total_bytes)?;
    let file = File::create(&path_buf).map_err(|e| format!("fs.write_lines('{}'): {}", path, e))?;
    let mut writer = BufWriter::new(file);
    for (idx, item) in items.iter().enumerate() {
        if (idx & 255) == 0 {
            vm.check_timeout()?;
        }
        if let Value::Str(s) = item {
            writer
                .write_all(s.as_bytes())
                .and_then(|_| writer.write_all(b"\n"))
                .map_err(|e| format!("fs.write_lines('{}'): {}", path, e))?;
        }
    }
    writer
        .flush()
        .map_err(|e| format!("fs.write_lines('{}'): {}", path, e))?;
    Ok(Value::Unit)
}

fn fs_open_lines(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("fs.open_lines() expects 1 argument (path)".to_string());
    }
    let path = expect_str(&args[0], "open_lines")?;
    let path_buf = vm.resolve_read_path(path)?;
    let file = File::open(&path_buf).map_err(|e| format!("fs.open_lines('{}'): {}", path, e))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("fs.open_lines('{}'): {}", path, e))?;
    if !meta.is_file() {
        return Err("Only regular files can be opened for line streaming".to_string());
    }
    let (id, def) = FS_STREAMS.with(|reg| {
        let mut reg = reg.borrow_mut();
        let id = reg.next_id;
        reg.next_id += 1;
        reg.readers.insert(
            id,
            FsReaderHandle {
                reader: Some(BufReader::new(file)),
                bytes_read: 0,
                closed: false,
            },
        );
        (id, reg.reader_def.clone())
    });
    Ok(Value::StructInstance(Rc::new(StructInstance {
        def,
        fields: RefCell::new(vec![Value::Int(id as i64)]),
    })))
}

fn fs_reader_next(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("FsLineReader.next() expects no arguments".to_string());
    }
    let id = handle_id(&args[0], "FsLineReader", "next")?;
    let res = FS_STREAMS.with(|reg| {
        let mut reg = reg.borrow_mut();
        let Some(handle) = reg.readers.get_mut(&id) else {
            return Err("FsLineReader handle is closed".to_string());
        };
        if handle.closed {
            return Err("FsLineReader handle is closed".to_string());
        }
        let Some(reader) = handle.reader.as_mut() else {
            return Ok(None);
        };
        read_one_bounded_line(reader, &mut handle.bytes_read, "fs.open_lines")
    })?;
    match res {
        Some(line) => {
            vm.charge_io(line.len().max(1))?;
            vm.charge_alloc(line.len())?;
            Ok(Value::from_str(line))
        }
        None => Ok(Value::Unit),
    }
}

fn fs_reader_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("FsLineReader.close() expects no arguments".to_string());
    }
    let id = handle_id(&args[0], "FsLineReader", "close")?;
    FS_STREAMS.with(|reg| {
        let mut reg = reg.borrow_mut();
        if let Some(mut handle) = reg.readers.remove(&id) {
            handle.closed = true;
            handle.reader = None;
        }
    });
    Ok(Value::Unit)
}

fn fs_open_writer(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("fs.write_stream() expects 1 or 2 arguments (path, [append])".to_string());
    }
    let path = expect_str(&args[0], "write_stream")?;
    let append = if args.len() == 2 {
        match args[1] {
            Value::Bool(b) => b,
            ref other => {
                return Err(format!(
                    "fs.write_stream() append flag must be bool, got '{}'",
                    other.type_name()
                ));
            }
        }
    } else {
        false
    };
    let path_buf = vm.resolve_write_path(path)?;
    check_safe_write_target(vm, &path_buf, path)?;
    let initial_bytes = if append {
        fs::metadata(&path_buf)
            .map(|m| m.len() as usize)
            .unwrap_or(0)
    } else {
        0
    };
    if initial_bytes > MAX_FS_BYTES {
        return Err("fs.write_stream() target file exceeds 32 MiB limit".to_string());
    }
    let file = if append {
        OpenOptions::new().create(true).append(true).open(&path_buf)
    } else {
        File::create(&path_buf)
    }
    .map_err(|e| format!("fs.write_stream('{}'): {}", path, e))?;

    let (id, def) = FS_STREAMS.with(|reg| {
        let mut reg = reg.borrow_mut();
        let id = reg.next_id;
        reg.next_id += 1;
        reg.writers.insert(
            id,
            FsWriterHandle {
                writer: Some(BufWriter::new(file)),
                bytes_written: initial_bytes,
                closed: false,
            },
        );
        (id, reg.writer_def.clone())
    });
    Ok(Value::StructInstance(Rc::new(StructInstance {
        def,
        fields: RefCell::new(vec![Value::Int(id as i64)]),
    })))
}

fn fs_writer_write_impl(
    vm: &mut VM,
    args: &[Value],
    add_newline: bool,
    name: &str,
) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!(
            "FsStreamWriter.{}() expects 1 argument (text)",
            name
        ));
    }
    let id = handle_id(&args[0], "FsStreamWriter", name)?;
    let Value::Str(text) = &args[1] else {
        return Err(format!(
            "FsStreamWriter.{}() expects a string, got '{}'",
            name,
            args[1].type_name()
        ));
    };
    let add_len = text.len().saturating_add(usize::from(add_newline));
    vm.charge_io(add_len)?;
    FS_STREAMS.with(|reg| {
        let mut reg = reg.borrow_mut();
        let Some(handle) = reg.writers.get_mut(&id) else {
            return Err("FsStreamWriter handle is closed".to_string());
        };
        if handle.closed {
            return Err("FsStreamWriter handle is closed".to_string());
        }
        let next_total = handle.bytes_written.saturating_add(add_len);
        if next_total > MAX_FS_BYTES {
            return Err("FsStreamWriter exceeds 32 MiB limit".to_string());
        }
        let Some(writer) = handle.writer.as_mut() else {
            return Err("FsStreamWriter handle is closed".to_string());
        };
        writer
            .write_all(text.as_bytes())
            .map_err(|e| format!("FsStreamWriter.{}(): {}", name, e))?;
        if add_newline {
            writer
                .write_all(b"\n")
                .map_err(|e| format!("FsStreamWriter.{}(): {}", name, e))?;
        }
        handle.bytes_written = next_total;
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn fs_writer_write(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    fs_writer_write_impl(vm, args, false, "write")
}

fn fs_writer_write_line(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    fs_writer_write_impl(vm, args, true, "write_line")
}

fn fs_writer_flush(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("FsStreamWriter.flush() expects no arguments".to_string());
    }
    let id = handle_id(&args[0], "FsStreamWriter", "flush")?;
    FS_STREAMS.with(|reg| {
        let mut reg = reg.borrow_mut();
        let Some(handle) = reg.writers.get_mut(&id) else {
            return Err("FsStreamWriter handle is closed".to_string());
        };
        if handle.closed {
            return Err("FsStreamWriter handle is closed".to_string());
        }
        if let Some(ref mut w) = handle.writer {
            w.flush()
                .map_err(|e| format!("FsStreamWriter.flush(): {}", e))?;
        }
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn fs_writer_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("FsStreamWriter.close() expects no arguments".to_string());
    }
    let id = handle_id(&args[0], "FsStreamWriter", "close")?;
    let mut err = None;
    FS_STREAMS.with(|reg| {
        let mut reg = reg.borrow_mut();
        if let Some(mut handle) = reg.writers.remove(&id) {
            handle.closed = true;
            if let Some(ref mut w) = handle.writer {
                if let Err(e) = w.flush() {
                    err = Some(format!("FsStreamWriter.close(): {}", e));
                }
            }
            handle.writer = None;
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    Ok(Value::Unit)
}

fn fs_temp_file(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 1 {
        return Err("fs.temp_file() expects 0 or 1 argument ([prefix])".to_string());
    }
    let prefix = if args.len() == 1 {
        let raw = expect_str(&args[0], "temp_file")?;
        if raw.is_empty()
            || raw.len() > 48
            || !raw
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(
                "fs.temp_file() prefix must be 1..48 ASCII alphanumeric/'_'/'-' characters"
                    .to_string(),
            );
        }
        raw
    } else {
        "askane"
    };

    for _ in 0..32 {
        let seq = TEMP_FILE_SEQ.fetch_add(1, Ordering::Relaxed);
        let name = format!("_{}_{}_{}.tmp", prefix, std::process::id(), seq);
        let candidate = vm.resolve_write_path(&name)?;
        check_safe_write_target(vm, &candidate, &name)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => {
                let display = strip_verbatim(&candidate).into_owned();
                vm.charge_io(1)?;
                return Ok(Value::from_str(display));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("fs.temp_file(): {}", e)),
        }
    }
    Err("fs.temp_file(): failed to allocate unique temp file".to_string())
}

fn walk_collect(
    vm: &mut VM,
    dir: &std::path::Path,
    base: &std::path::Path,
    out: &mut Vec<Value>,
    depth: usize,
    max_depth: usize,
    max_entries: usize,
) -> Result<(), String> {
    if depth > max_depth || out.len() >= max_entries {
        return Ok(());
    }
    vm.check_timeout()?;
    vm.charge_work(1)?;
    let rd = fs::read_dir(dir).map_err(|e| format!("fs.walk('{}'): {}", dir.display(), e))?;
    let mut children: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).collect();
    children.sort();
    for path in children {
        if out.len() >= max_entries {
            break;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let check_target = strip_verbatim(&path);
        if vm.resolve_read_path(&check_target).is_err() {
            continue;
        }
        let rel = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let is_dir = meta.is_dir();
        let size = if meta.is_file() { meta.len() as i64 } else { 0 };
        vm.charge_alloc(rel.len() + name.len() + 128)?;
        let mut map = new_dict_with_capacity(4);
        map.insert("path".to_string(), Value::from_str(rel));
        map.insert("name".to_string(), Value::from_str(name));
        map.insert("is_dir".to_string(), Value::Bool(is_dir));
        map.insert("size".to_string(), Value::Int(size));
        out.push(Value::dict(map));
        if is_dir && depth < max_depth {
            walk_collect(vm, &path, base, out, depth + 1, max_depth, max_entries)?;
        }
    }
    Ok(())
}

fn fs_walk(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err(
            "fs.walk() expects 1 to 3 arguments (dir, [max_depth], [max_entries])".to_string(),
        );
    }
    let dir = expect_str(&args[0], "walk")?;
    let max_depth = if args.len() >= 2 {
        match &args[1] {
            Value::Int(n) if *n >= 0 => (*n as usize).min(MAX_DIR_DEPTH_HARD),
            other => {
                return Err(format!(
                    "fs.walk() max_depth must be non-negative int, got '{}'",
                    other.type_name()
                ));
            }
        }
    } else {
        16
    };
    let max_entries = if args.len() == 3 {
        match &args[2] {
            Value::Int(n) if *n > 0 => (*n as usize).min(MAX_WALK_ENTRIES_HARD),
            other => {
                return Err(format!(
                    "fs.walk() max_entries must be positive int, got '{}'",
                    other.type_name()
                ));
            }
        }
    } else {
        10_000
    };
    let dir_buf = vm.resolve_read_path(dir)?;
    let meta = fs::symlink_metadata(&dir_buf).map_err(|e| format!("fs.walk('{}'): {}", dir, e))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(format!(
            "fs.walk('{}'): path is not a regular directory",
            dir
        ));
    }
    let mut out = Vec::new();
    walk_collect(vm, &dir_buf, &dir_buf, &mut out, 0, max_depth, max_entries)?;
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn copy_dir_inner(
    vm: &mut VM,
    src: &std::path::Path,
    dst: &std::path::Path,
    depth: usize,
    max_depth: usize,
    files_copied: &mut usize,
    bytes_copied: &mut usize,
) -> Result<(), String> {
    if depth > max_depth {
        return Err(format!(
            "fs.copy_dir() maximum directory depth ({}) exceeded",
            max_depth
        ));
    }
    vm.check_timeout()?;
    vm.charge_work(1)?;
    if let Ok(meta) = fs::symlink_metadata(dst) {
        if meta.file_type().is_symlink() {
            return Err(format!(
                "fs.copy_dir(): refusing to traverse or overwrite symlink '{}'",
                dst.display()
            ));
        }
    }
    fs::create_dir_all(dst).map_err(|e| format!("fs.copy_dir(): {}", e))?;
    let rd = fs::read_dir(src).map_err(|e| format!("fs.copy_dir(): {}", e))?;
    let mut entries: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for entry_path in entries {
        let meta = match fs::symlink_metadata(&entry_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let Some(fname) = entry_path.file_name() else {
            continue;
        };
        let target = dst.join(fname);
        if meta.is_dir() {
            copy_dir_inner(
                vm,
                &entry_path,
                &target,
                depth + 1,
                max_depth,
                files_copied,
                bytes_copied,
            )?;
        } else if meta.is_file() {
            *files_copied += 1;
            if *files_copied > 10_000 {
                return Err("fs.copy_dir() file count limit (10000) exceeded".to_string());
            }
            let len = meta.len() as usize;
            *bytes_copied = bytes_copied.saturating_add(len);
            if *bytes_copied > MAX_FS_BYTES {
                return Err("fs.copy_dir() exceeds 32 MiB total size limit".to_string());
            }
            let raw_t = strip_verbatim(&target).into_owned();
            let safe_target = vm.resolve_write_path(&raw_t)?;
            check_safe_write_target(vm, &safe_target, &raw_t)?;
            vm.charge_io(len)?;
            fs::copy(&entry_path, &safe_target)
                .map_err(|e| format!("fs.copy_dir('{}'): {}", raw_t, e))?;
        }
    }
    Ok(())
}

fn fs_copy_dir(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("fs.copy_dir() expects 2 or 3 arguments (src, dst, [max_depth])".to_string());
    }
    let src = expect_str(&args[0], "copy_dir")?;
    let dst = expect_str(&args[1], "copy_dir")?;
    let max_depth = if args.len() == 3 {
        match &args[2] {
            Value::Int(n) if *n >= 0 => (*n as usize).min(MAX_DIR_DEPTH_HARD),
            other => {
                return Err(format!(
                    "fs.copy_dir() max_depth must be non-negative int, got '{}'",
                    other.type_name()
                ));
            }
        }
    } else {
        16
    };
    let src_buf = vm.resolve_read_path(src)?;
    let src_meta =
        fs::symlink_metadata(&src_buf).map_err(|e| format!("fs.copy_dir('{}'): {}", src, e))?;
    if src_meta.file_type().is_symlink() || !src_meta.is_dir() {
        return Err(format!(
            "fs.copy_dir('{}'): source is not a regular directory",
            src
        ));
    }
    let dst_buf = vm.resolve_write_path(dst)?;
    check_safe_write_target(vm, &dst_buf, dst)?;
    let canon_src = fs::canonicalize(&src_buf).unwrap_or_else(|_| src_buf.clone());
    if dst_buf.starts_with(&canon_src) {
        return Err("fs.copy_dir(): destination cannot be inside source directory".to_string());
    }
    let mut files_copied = 0usize;
    let mut bytes_copied = 0usize;
    copy_dir_inner(
        vm,
        &src_buf,
        &dst_buf,
        0,
        max_depth,
        &mut files_copied,
        &mut bytes_copied,
    )?;
    Ok(Value::Int(files_copied as i64))
}

fn remove_dir_no_symlink(vm: &mut VM, dir: &std::path::Path, depth: usize) -> Result<(), String> {
    if depth > MAX_DIR_DEPTH_HARD {
        return Err("fs.remove_dir() maximum recursion depth exceeded".to_string());
    }
    vm.check_timeout()?;
    vm.charge_work(1)?;
    let meta = fs::symlink_metadata(dir).map_err(|e| format!("fs.remove_dir(): {}", e))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "fs.remove_dir(): refusing to traverse symlink '{}'",
            dir.display()
        ));
    }
    if !meta.is_dir() {
        return Err(format!(
            "fs.remove_dir(): '{}' is not a directory",
            dir.display()
        ));
    }
    let rd = fs::read_dir(dir).map_err(|e| format!("fs.remove_dir(): {}", e))?;
    for entry in rd.flatten() {
        let p = entry.path();
        let em = fs::symlink_metadata(&p).map_err(|e| format!("fs.remove_dir(): {}", e))?;
        if em.file_type().is_symlink() {
            fs::remove_file(&p)
                .or_else(|_| fs::remove_dir(&p))
                .map_err(|e| format!("fs.remove_dir(): {}", e))?;
        } else if em.is_dir() {
            remove_dir_no_symlink(vm, &p, depth + 1)?;
        } else {
            fs::remove_file(&p).map_err(|e| format!("fs.remove_dir(): {}", e))?;
        }
    }
    fs::remove_dir(dir).map_err(|e| format!("fs.remove_dir(): {}", e))
}

fn fs_remove_dir(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("fs.remove_dir() expects 1 or 2 arguments (dir, [recursive])".to_string());
    }
    let dir = expect_str(&args[0], "remove_dir")?;
    let recursive = if args.len() == 2 {
        match &args[1] {
            Value::Bool(b) => *b,
            other => {
                return Err(format!(
                    "fs.remove_dir() recursive flag must be bool, got '{}'",
                    other.type_name()
                ));
            }
        }
    } else {
        false
    };
    let dir_buf = vm.resolve_write_link_or_path(dir)?;
    let meta =
        fs::symlink_metadata(&dir_buf).map_err(|e| format!("fs.remove_dir('{}'): {}", dir, e))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "fs.remove_dir('{}'): refusing to remove symlink as directory",
            dir
        ));
    }
    if !meta.is_dir() {
        return Err(format!("fs.remove_dir('{}'): path is not a directory", dir));
    }
    if recursive {
        remove_dir_no_symlink(vm, &dir_buf, 0)?;
    } else {
        fs::remove_dir(&dir_buf).map_err(|e| format!("fs.remove_dir('{}'): {}", dir, e))?;
    }
    Ok(Value::Unit)
}
