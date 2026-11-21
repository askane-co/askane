use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use crate::modules::build_module;
use crate::modules::compress::{DEFAULT_MAX_DECOMPRESS, crc32_bytes, deflate_raw, inflate_raw};
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "archive",
        &[
            ("pack", archive_pack),
            ("create", archive_create),
            ("list", archive_list),
            ("read_entry", archive_read_entry),
            ("extract", archive_extract),
        ],
        &[],
    )
}

fn validate_entry_name(name: &str) -> Result<String, String> {
    if name.is_empty()
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.contains('\0')
        || name.chars().any(char::is_control)
    {
        return Err(format!("Unsafe archive entry path '{}'", name));
    }
    let norm = name.replace('\\', "/");
    if norm.len() >= 2 && norm.as_bytes()[1] == b':' {
        return Err(format!("Unsafe archive drive path '{}'", name));
    }
    for part in norm.split('/') {
        if part == ".." || part == "." && norm == "." {
            return Err(format!("Archive path traversal blocked: '{}'", name));
        }
    }
    Ok(norm)
}

fn build_zip_bytes(vm: &mut VM, entries: &HashMap<String, Value>) -> Result<Vec<u8>, String> {
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();
    let mut out = Vec::new();
    let mut central = Vec::new();

    for key in keys {
        let name = validate_entry_name(key)?;
        let raw = entries[key].as_bytes_cow("archive.pack")?;
        let crc = crc32_bytes(&raw);
        let uncomp_len = raw.len() as u32;
        let deflated = deflate_raw(&raw);
        let (method, payload): (u16, &[u8]) = if deflated.len() < raw.len() {
            (8, &deflated)
        } else {
            (0, &raw)
        };
        let comp_len = payload.len() as u32;
        let offset = out.len() as u32;
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() as u16;

        // Local file header
        out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&comp_len.to_le_bytes());
        out.extend_from_slice(&uncomp_len.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(payload);

        // Central directory header
        central.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&comp_len.to_le_bytes());
        central.extend_from_slice(&uncomp_len.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }

    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    let count = entries.len() as u16;
    out.extend_from_slice(&central);

    // EOCD
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());

    vm.charge_alloc(out.len())?;
    Ok(out)
}

struct ZipEntryMeta {
    name: String,
    method: u16,
    crc: u32,
    comp_size: usize,
    uncomp_size: usize,
    local_header_off: usize,
}

fn parse_zip_cd(data: &[u8]) -> Result<Vec<ZipEntryMeta>, String> {
    if data.len() < 22 {
        return Err("Not a valid ZIP archive (too short)".to_string());
    }
    let min_off = data.len().saturating_sub(65_557);
    let mut eocd_pos = None;
    for pos in (min_off..=data.len() - 22).rev() {
        if data[pos..pos + 4] == [0x50, 0x4B, 0x05, 0x06] {
            eocd_pos = Some(pos);
            break;
        }
    }
    let eocd = eocd_pos.ok_or_else(|| "ZIP end-of-central-directory not found".to_string())?;
    let total_entries = u16::from_le_bytes([data[eocd + 10], data[eocd + 11]]) as usize;
    let cd_size = u32::from_le_bytes(data[eocd + 12..eocd + 16].try_into().unwrap()) as usize;
    let cd_off = u32::from_le_bytes(data[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
    if cd_off.saturating_add(cd_size) > eocd {
        return Err("Invalid ZIP central directory bounds".to_string());
    }
    if total_entries > 20_000 {
        return Err("ZIP entry count exceeds 20,000 limit".to_string());
    }

    let mut pos = cd_off;
    let mut out = Vec::with_capacity(total_entries);
    for _ in 0..total_entries {
        if pos + 46 > cd_off + cd_size || data[pos..pos + 4] != [0x50, 0x4B, 0x01, 0x02] {
            return Err("Corrupt ZIP central directory entry".to_string());
        }
        let method = u16::from_le_bytes([data[pos + 10], data[pos + 11]]);
        let crc = u32::from_le_bytes(data[pos + 16..pos + 20].try_into().unwrap());
        let comp_size = u32::from_le_bytes(data[pos + 20..pos + 24].try_into().unwrap()) as usize;
        let uncomp_size = u32::from_le_bytes(data[pos + 24..pos + 28].try_into().unwrap()) as usize;
        let name_len = u16::from_le_bytes([data[pos + 28], data[pos + 29]]) as usize;
        let extra_len = u16::from_le_bytes([data[pos + 30], data[pos + 31]]) as usize;
        let cmt_len = u16::from_le_bytes([data[pos + 32], data[pos + 33]]) as usize;
        let local_header_off =
            u32::from_le_bytes(data[pos + 42..pos + 46].try_into().unwrap()) as usize;
        let header_end = pos + 46 + name_len + extra_len + cmt_len;
        if header_end > cd_off + cd_size {
            return Err("ZIP central directory entry truncated".to_string());
        }
        let raw_name = std::str::from_utf8(&data[pos + 46..pos + 46 + name_len])
            .map_err(|_| "ZIP entry filename is not valid UTF-8".to_string())?;
        let name = validate_entry_name(raw_name)?;
        out.push(ZipEntryMeta {
            name,
            method,
            crc,
            comp_size,
            uncomp_size,
            local_header_off,
        });
        pos = header_end;
    }
    Ok(out)
}

fn read_zip_entry_bytes(
    vm: &mut VM,
    data: &[u8],
    meta: &ZipEntryMeta,
    max_entry: usize,
) -> Result<Vec<u8>, String> {
    if meta.uncomp_size > max_entry {
        return Err(format!(
            "ZIP entry '{}' exceeds max size ({} bytes)",
            meta.name, max_entry
        ));
    }
    let lh = meta.local_header_off;
    if lh + 30 > data.len() || data[lh..lh + 4] != [0x50, 0x4B, 0x03, 0x04] {
        return Err(format!("Corrupt local header for '{}'", meta.name));
    }
    let name_len = u16::from_le_bytes([data[lh + 26], data[lh + 27]]) as usize;
    let extra_len = u16::from_le_bytes([data[lh + 28], data[lh + 29]]) as usize;
    let payload_start = lh + 30 + name_len + extra_len;
    let payload_end = payload_start.saturating_add(meta.comp_size);
    if payload_end > data.len() {
        return Err(format!("Truncated payload for '{}'", meta.name));
    }
    let payload = &data[payload_start..payload_end];
    let out = match meta.method {
        0 => {
            vm.charge_alloc(payload.len())?;
            payload.to_vec()
        }
        8 => inflate_raw(payload, max_entry.min(meta.uncomp_size.max(4096)), Some(vm))?,
        other => {
            return Err(format!(
                "Unsupported ZIP compression method {} for '{}'",
                other, meta.name
            ));
        }
    };
    if out.len() != meta.uncomp_size || crc32_bytes(&out) != meta.crc {
        return Err(format!("ZIP CRC/size mismatch for '{}'", meta.name));
    }
    Ok(out)
}

fn read_archive_source(vm: &mut VM, arg: &Value) -> Result<Vec<u8>, String> {
    match arg {
        Value::Bytes(b) => Ok((**b).clone()),
        Value::Str(path) => {
            let resolved = vm.resolve_read_path(path)?;
            let data =
                fs::read(&resolved).map_err(|e| format!("archive read '{}': {}", path, e))?;
            vm.charge_io(data.len())?;
            Ok(data)
        }
        other => Err(format!(
            "archive expects path string or bytes, got '{}'",
            other.type_name()
        )),
    }
}

fn archive_pack(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("archive.pack() expects 1 dict argument ({ name: bytes/str })".to_string());
    }
    let Value::Dict(dict_rc) = &args[0] else {
        return Err("archive.pack() expects a dict".to_string());
    };
    let map = dict_rc.borrow().clone();
    let bytes = build_zip_bytes(vm, &map)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}

fn archive_create(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("archive.create() expects (zip_path, entries_dict)".to_string());
    }
    let Value::Str(path) = &args[0] else {
        return Err("archive.create() path must be a string".to_string());
    };
    let Value::Dict(dict_rc) = &args[1] else {
        return Err("archive.create() second argument must be a dict".to_string());
    };
    let resolved = vm.resolve_write_path(path)?;
    let map = dict_rc.borrow().clone();
    let bytes = build_zip_bytes(vm, &map)?;
    vm.charge_io(bytes.len())?;
    fs::write(&resolved, &bytes).map_err(|e| format!("archive.create '{}': {}", path, e))?;
    Ok(Value::Int(bytes.len() as i64))
}

fn archive_list(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("archive.list() expects 1 argument (zip_path or bytes)".to_string());
    }
    let data = read_archive_source(vm, &args[0])?;
    let metas = parse_zip_cd(&data)?;
    let out: Vec<Value> = metas
        .into_iter()
        .map(|m| {
            let mut map = HashMap::new();
            map.insert("name".to_string(), Value::Str(m.name));
            map.insert("size".to_string(), Value::Int(m.uncomp_size as i64));
            map.insert(
                "compressed_size".to_string(),
                Value::Int(m.comp_size as i64),
            );
            map.insert("method".to_string(), Value::Int(m.method as i64));
            Value::Dict(Rc::new(RefCell::new(map)))
        })
        .collect();
    vm.charge_alloc(out.len().saturating_mul(64))?;
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn archive_read_entry(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("archive.read_entry() expects (zip_path_or_bytes, entry_name)".to_string());
    }
    let Value::Str(target) = &args[1] else {
        return Err("archive.read_entry() entry_name must be a string".to_string());
    };
    let norm_target = validate_entry_name(target)?;
    let data = read_archive_source(vm, &args[0])?;
    let metas = parse_zip_cd(&data)?;
    let Some(meta) = metas.into_iter().find(|m| m.name == norm_target) else {
        return Err(format!("ZIP entry '{}' not found", norm_target));
    };
    let bytes = read_zip_entry_bytes(vm, &data, &meta, DEFAULT_MAX_DECOMPRESS)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}

fn archive_extract(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("archive.extract() expects (zip_path_or_bytes, dest_dir)".to_string());
    }
    let Value::Str(dest_dir) = &args[1] else {
        return Err("archive.extract() dest_dir must be a string".to_string());
    };
    let dest_root = vm.resolve_write_path(dest_dir)?;
    fs::create_dir_all(&dest_root)
        .map_err(|e| format!("archive.extract() mkdir '{}': {}", dest_dir, e))?;

    let data = read_archive_source(vm, &args[0])?;
    let metas = parse_zip_cd(&data)?;
    let mut total_written = 0usize;
    let mut files_count = 0i64;

    for meta in metas {
        if meta.name.ends_with('/') {
            continue;
        }
        let joined = PathBuf::from(dest_dir).join(&meta.name);
        let joined_str = joined.to_string_lossy().to_string();
        let target_path = vm.resolve_write_path(&joined_str)?;
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = read_zip_entry_bytes(
            vm,
            &data,
            &meta,
            DEFAULT_MAX_DECOMPRESS.saturating_sub(total_written),
        )?;
        total_written = total_written.saturating_add(bytes.len());
        vm.charge_io(bytes.len())?;
        fs::write(&target_path, &bytes)
            .map_err(|e| format!("archive.extract() write '{}': {}", meta.name, e))?;
        files_count += 1;
    }
    Ok(Value::Int(files_count))
}
