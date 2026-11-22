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
    if norm.contains(':') || norm.starts_with("//") {
        return Err(format!("Unsafe archive entry path '{}'", name));
    }
    let check_norm = norm.strip_suffix('/').unwrap_or(&norm);
    if check_norm.is_empty() {
        return Err(format!("Unsafe archive entry path '{}'", name));
    }
    for part in check_norm.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("Archive path traversal blocked: '{}'", name));
        }
    }
    Ok(norm)
}

fn build_zip_bytes(vm: &mut VM, entries: &HashMap<String, Value>) -> Result<Vec<u8>, String> {
    if entries.len() > u16::MAX as usize {
        return Err("archive.pack() entry count exceeds ZIP limit (65535)".to_string());
    }
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();
    let mut out = Vec::new();
    let mut central = Vec::new();

    for (idx, key) in keys.iter().enumerate() {
        if (idx & 63) == 0 {
            vm.check_timeout()?;
        }
        let name = validate_entry_name(key)?;
        let raw = entries[*key].as_bytes_cow("archive.pack")?;
        vm.charge_work((raw.len() as u64) / 32 + 1)?;
        let crc = crc32_bytes(&raw);
        let uncomp_len = u32::try_from(raw.len())
            .map_err(|_| "archive.pack() entry exceeds 4 GiB ZIP limit".to_string())?;
        let deflated = deflate_raw(&raw);
        let (method, payload): (u16, &[u8]) = if deflated.len() < raw.len() {
            (8, &deflated)
        } else {
            (0, &raw)
        };
        let comp_len = u32::try_from(payload.len())
            .map_err(|_| "archive.pack() compressed entry exceeds 4 GiB ZIP limit".to_string())?;
        let offset = u32::try_from(out.len())
            .map_err(|_| "archive.pack() archive offset exceeds 4 GiB ZIP limit".to_string())?;
        let name_bytes = name.as_bytes();
        let name_len = u16::try_from(name_bytes.len())
            .map_err(|_| "archive.pack() entry name exceeds 65535 bytes".to_string())?;

        let add_bytes = 30usize
            .saturating_add(46)
            .saturating_add(name_bytes.len().saturating_mul(2))
            .saturating_add(payload.len());
        if out
            .len()
            .saturating_add(central.len())
            .saturating_add(add_bytes)
            > DEFAULT_MAX_DECOMPRESS.saturating_mul(4)
        {
            return Err("archive.pack() exceeds maximum archive size limit".to_string());
        }
        vm.charge_alloc(add_bytes)?;

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

    let cd_offset = u32::try_from(out.len())
        .map_err(|_| "archive.pack() central directory offset exceeds 4 GiB limit".to_string())?;
    let cd_size = u32::try_from(central.len())
        .map_err(|_| "archive.pack() central directory exceeds 4 GiB limit".to_string())?;
    let count = u16::try_from(entries.len())
        .map_err(|_| "archive.pack() entry count exceeds ZIP limit".to_string())?;
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

fn parse_zip_cd(vm: &mut VM, data: &[u8]) -> Result<Vec<ZipEntryMeta>, String> {
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
    let cd_end = cd_off
        .checked_add(cd_size)
        .filter(|&end| end <= eocd)
        .ok_or_else(|| "Invalid ZIP central directory bounds".to_string())?;
    if total_entries > 20_000 {
        return Err("ZIP entry count exceeds 20,000 limit".to_string());
    }
    vm.charge_alloc(total_entries.saturating_mul(std::mem::size_of::<ZipEntryMeta>()))?;
    vm.charge_work((total_entries as u64) / 8 + 1)?;

    let mut pos = cd_off;
    let mut out = Vec::with_capacity(total_entries);
    for i in 0..total_entries {
        if (i & 1023) == 0 {
            vm.check_timeout()?;
        }
        if pos.checked_add(46).is_none_or(|p| p > cd_end)
            || data[pos..pos + 4] != [0x50, 0x4B, 0x01, 0x02]
        {
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
        let header_end = pos
            .checked_add(46)
            .and_then(|p| p.checked_add(name_len))
            .and_then(|p| p.checked_add(extra_len))
            .and_then(|p| p.checked_add(cmt_len))
            .filter(|&end| end <= cd_end)
            .ok_or_else(|| "ZIP central directory entry truncated".to_string())?;
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
    if lh.checked_add(30).is_none_or(|p| p > data.len())
        || data[lh..lh + 4] != [0x50, 0x4B, 0x03, 0x04]
    {
        return Err(format!("Corrupt local header for '{}'", meta.name));
    }
    let name_len = u16::from_le_bytes([data[lh + 26], data[lh + 27]]) as usize;
    let extra_len = u16::from_le_bytes([data[lh + 28], data[lh + 29]]) as usize;
    let payload_start = lh
        .checked_add(30)
        .and_then(|p| p.checked_add(name_len))
        .and_then(|p| p.checked_add(extra_len))
        .ok_or_else(|| format!("Corrupt local header bounds for '{}'", meta.name))?;
    let payload_end = payload_start
        .checked_add(meta.comp_size)
        .filter(|&end| end <= data.len())
        .ok_or_else(|| format!("Truncated payload for '{}'", meta.name))?;
    let payload = &data[payload_start..payload_end];
    let out = match meta.method {
        0 => {
            vm.charge_alloc(payload.len())?;
            payload.to_vec()
        }
        8 => inflate_raw(payload, max_entry.min(meta.uncomp_size), Some(vm))?,
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

fn read_archive_source<'a>(
    vm: &mut VM,
    arg: &'a Value,
) -> Result<std::borrow::Cow<'a, [u8]>, String> {
    match arg {
        Value::Bytes(b) => Ok(std::borrow::Cow::Borrowed(b.as_slice())),
        Value::Str(path) => {
            use std::io::Read;
            let resolved = vm.resolve_read_path(path)?;
            let file =
                fs::File::open(&resolved).map_err(|e| format!("archive read '{}': {}", path, e))?;
            let meta = file
                .metadata()
                .map_err(|e| format!("archive read '{}': {}", path, e))?;
            if !meta.is_file() {
                return Err(format!("archive read '{}': not a regular file", path));
            }
            if meta.len() > DEFAULT_MAX_DECOMPRESS as u64 {
                return Err(format!(
                    "archive read '{}': exceeds {} byte limit",
                    path, DEFAULT_MAX_DECOMPRESS
                ));
            }
            let expected = meta.len() as usize;
            vm.charge_io(expected)?;
            vm.charge_alloc(expected)?;
            let mut data = Vec::with_capacity(expected);
            file.take(DEFAULT_MAX_DECOMPRESS as u64 + 1)
                .read_to_end(&mut data)
                .map_err(|e| format!("archive read '{}': {}", path, e))?;
            if data.len() > DEFAULT_MAX_DECOMPRESS {
                return Err(format!(
                    "archive read '{}': exceeds {} byte limit",
                    path, DEFAULT_MAX_DECOMPRESS
                ));
            }
            Ok(std::borrow::Cow::Owned(data))
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
    crate::modules::fs::check_safe_write_target(vm, &resolved, path)?;
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
    let metas = parse_zip_cd(vm, &data)?;
    vm.charge_alloc(metas.len().saturating_mul(128))?;
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
    let metas = parse_zip_cd(vm, &data)?;
    let Some(meta) = metas.into_iter().find(|m| m.name == norm_target) else {
        return Err(format!("ZIP entry '{}' not found", norm_target));
    };
    let bytes = read_zip_entry_bytes(vm, &data, &meta, DEFAULT_MAX_DECOMPRESS)?;
    Ok(Value::Bytes(Rc::new(bytes)))
}

fn ensure_no_symlink_components(
    root: &std::path::Path,
    rel_name: &str,
) -> Result<std::path::PathBuf, String> {
    let mut cur = root.to_path_buf();
    for part in rel_name.split('/') {
        cur.push(part);
        if let Ok(meta) = fs::symlink_metadata(&cur) {
            if meta.file_type().is_symlink() {
                return Err(format!(
                    "Refusing to traverse or overwrite symlink '{}'",
                    rel_name
                ));
            }
        }
    }
    Ok(cur)
}

fn archive_extract(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("archive.extract() expects (zip_path_or_bytes, dest_dir)".to_string());
    }
    let Value::Str(dest_dir) = &args[1] else {
        return Err("archive.extract() dest_dir must be a string".to_string());
    };
    let dest_root = vm.resolve_write_path(dest_dir)?;
    if let Ok(meta) = fs::symlink_metadata(&dest_root) {
        if meta.file_type().is_symlink() {
            return Err(format!(
                "archive.extract() dest_dir '{}' cannot be a symlink",
                dest_dir
            ));
        }
    }
    fs::create_dir_all(&dest_root)
        .map_err(|e| format!("archive.extract() mkdir '{}': {}", dest_dir, e))?;
    let canon_dest_root = fs::canonicalize(&dest_root)
        .map_err(|e| format!("archive.extract() canonicalize '{}': {}", dest_dir, e))?;

    let data = read_archive_source(vm, &args[0])?;
    let metas = parse_zip_cd(vm, &data)?;
    let mut total_written = 0usize;
    let mut files_count = 0i64;

    for (idx, meta) in metas.into_iter().enumerate() {
        if (idx & 63) == 0 {
            vm.check_timeout()?;
        }
        if meta.name.ends_with('/') {
            continue;
        }
        let joined = PathBuf::from(dest_dir).join(&meta.name);
        let joined_str = joined.to_string_lossy().to_string();
        let target_path = vm.resolve_write_path(&joined_str)?;
        crate::modules::fs::check_safe_write_target(vm, &target_path, &meta.name)?;
        let checked_rel = ensure_no_symlink_components(&dest_root, &meta.name)?;
        if let Some(parent) = checked_rel.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            let canon_parent = fs::canonicalize(parent).map_err(|e| e.to_string())?;
            if !canon_parent.starts_with(&canon_dest_root) {
                return Err(format!(
                    "Archive extraction path '{}' escapes destination directory",
                    meta.name
                ));
            }
        }
        if let Ok(tmeta) = fs::symlink_metadata(&target_path) {
            if tmeta.file_type().is_symlink() {
                return Err(format!("Refusing to overwrite symlink '{}'", meta.name));
            }
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
