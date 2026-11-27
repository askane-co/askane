use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::rc::Rc;

use crate::modules::compress::crc32_bytes;
use crate::modules::{NativeFnPtr, build_module};
use crate::value::{FastMap, StructDef, StructInstance, Value, new_dict_map, new_dict_with_capacity};
use crate::vm::VM;

const KV_MAGIC: &[u8; 8] = b"ASKV01\0\0";
const OP_SET: u8 = 1;
const OP_DEL: u8 = 2;
const MAX_KEY_BYTES: usize = 4096;
const MAX_VAL_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENCODE_DEPTH: usize = 64;

struct KvStore {
    path: PathBuf,
    file: Option<File>,
    map: BTreeMap<String, Value>,
    closed: bool,
}

struct KvRegistry {
    next_id: u64,
    stores: HashMap<u64, KvStore>,
    def: Rc<StructDef>,
}

thread_local! {
    static KV_STORES: RefCell<KvRegistry> = RefCell::new(KvRegistry {
        next_id: 1,
        stores: HashMap::new(),
        def: make_kv_struct_def(),
    });
}

fn make_kv_struct_def() -> Rc<StructDef> {
    let methods_spec: &[(&'static str, NativeFnPtr)] = &[
        ("get", kv_get),
        ("set", kv_set),
        ("delete", kv_delete),
        ("has", kv_has),
        ("keys", kv_keys),
        ("prefix", kv_prefix),
        ("batch", kv_batch),
        ("compact", kv_compact),
        ("len", kv_len),
        ("close", kv_close),
    ];
    let field_names = vec!["__id".to_string()];
    let mut field_index = FastMap::default();
    field_index.insert("__id".to_string(), 0);
    let mut methods = new_dict_with_capacity(methods_spec.len());
    for &(name, func) in methods_spec {
        methods.insert(name.to_string(), Value::native_fn(name, func));
    }
    Rc::new(StructDef {
        ic_id: crate::bytecode::next_ic_id(),
        name: "KvStore".to_string(),
        field_names,
        field_index,
        field_defaults: vec![None],
        methods,
    })
}

pub fn reset_state() {
    KV_STORES.with(|reg| {
        let mut reg = reg.borrow_mut();
        for (_, mut store) in reg.stores.drain() {
            if let Some(ref mut f) = store.file {
                let _ = f.flush();
            }
        }
    });
}

pub fn module() -> Value {
    build_module(
        "kv",
        &[
            ("open", kv_open),
            ("get", kv_get),
            ("set", kv_set),
            ("delete", kv_delete),
            ("has", kv_has),
            ("keys", kv_keys),
            ("prefix", kv_prefix),
            ("batch", kv_batch),
            ("compact", kv_compact),
            ("len", kv_len),
            ("close", kv_close),
        ],
        &[],
    )
}

fn expect_str<'a>(v: &'a Value, ctx: &str) -> Result<&'a str, String> {
    match v {
        Value::Str(s) => Ok(s.as_str()),
        other => Err(format!(
            "{} expects a string, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn extract_store_id(v: &Value, ctx: &str) -> Result<u64, String> {
    let Value::StructInstance(inst) = v else {
        return Err(format!(
            "{} expects a KvStore handle, got '{}'",
            ctx,
            v.type_name()
        ));
    };
    let expected_ic = KV_STORES.with(|reg| reg.borrow().def.ic_id);
    if inst.def.ic_id != expected_ic {
        return Err(format!("{} expects a KvStore handle", ctx));
    }
    let fields = inst.fields.borrow();
    match fields.first() {
        Some(Value::Int(id)) if *id > 0 => Ok(*id as u64),
        _ => Err(format!("{}: invalid KvStore handle", ctx)),
    }
}

fn encode_value(v: &Value, out: &mut Vec<u8>, depth: usize) -> Result<(), String> {
    if depth > MAX_ENCODE_DEPTH {
        return Err("kv: value nesting exceeds 64 levels".to_string());
    }
    match v {
        Value::Unit => out.push(0),
        Value::Bool(b) => {
            out.push(1);
            out.push(if *b { 1 } else { 0 });
        }
        Value::Int(n) => {
            out.push(2);
            out.extend_from_slice(&n.to_le_bytes());
        }
        Value::Float(f) => {
            if !f.is_finite() {
                return Err("kv: cannot store NaN or Infinity float".to_string());
            }
            out.push(3);
            out.extend_from_slice(&f.to_bits().to_le_bytes());
        }
        Value::Str(s) => {
            let len = u32::try_from(s.len()).map_err(|_| "kv: string too large".to_string())?;
            out.push(4);
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        Value::Bytes(b) => {
            let len = u32::try_from(b.len()).map_err(|_| "kv: bytes too large".to_string())?;
            out.push(5);
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(b);
        }
        Value::List(list) => {
            let items = list.borrow();
            let count = u32::try_from(items.len()).map_err(|_| "kv: list too large".to_string())?;
            out.push(6);
            out.extend_from_slice(&count.to_le_bytes());
            for item in items.iter() {
                encode_value(item, out, depth + 1)?;
            }
        }
        Value::Dict(dict) => {
            let map = dict.borrow();
            let count = u32::try_from(map.len()).map_err(|_| "kv: dict too large".to_string())?;
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push(7);
            out.extend_from_slice(&count.to_le_bytes());
            for (k, val) in entries {
                let klen =
                    u32::try_from(k.len()).map_err(|_| "kv: dict key too large".to_string())?;
                out.extend_from_slice(&klen.to_le_bytes());
                out.extend_from_slice(k.as_bytes());
                encode_value(val, out, depth + 1)?;
            }
        }
        other => {
            return Err(format!(
                "kv: unsupported value type '{}'",
                other.type_name()
            ));
        }
    }
    if out.len() > MAX_VAL_BYTES {
        return Err("kv: encoded value exceeds 4 MiB limit".to_string());
    }
    Ok(())
}

fn read_u32_le(data: &[u8], pos: &mut usize) -> Result<u32, String> {
    if *pos + 4 > data.len() {
        return Err("kv: truncated value length".to_string());
    }
    let val = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]);
    *pos += 4;
    Ok(val)
}

fn decode_value(data: &[u8], pos: &mut usize, depth: usize) -> Result<Value, String> {
    if depth > MAX_ENCODE_DEPTH {
        return Err("kv: stored value exceeds max depth".to_string());
    }
    if *pos >= data.len() {
        return Err("kv: truncated value tag".to_string());
    }
    let tag = data[*pos];
    *pos += 1;
    match tag {
        0 => Ok(Value::Unit),
        1 => {
            if *pos >= data.len() {
                return Err("kv: truncated bool".to_string());
            }
            let b = data[*pos] != 0;
            *pos += 1;
            Ok(Value::Bool(b))
        }
        2 => {
            if *pos + 8 > data.len() {
                return Err("kv: truncated i64".to_string());
            }
            let n = i64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
            *pos += 8;
            Ok(Value::Int(n))
        }
        3 => {
            if *pos + 8 > data.len() {
                return Err("kv: truncated f64".to_string());
            }
            let bits = u64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
            *pos += 8;
            Ok(Value::Float(f64::from_bits(bits)))
        }
        4 => {
            let len = read_u32_le(data, pos)? as usize;
            if *pos + len > data.len() {
                return Err("kv: truncated string".to_string());
            }
            let s = std::str::from_utf8(&data[*pos..*pos + len])
                .map_err(|_| "kv: invalid UTF-8 string in store".to_string())?
                .to_string();
            *pos += len;
            Ok(Value::from_str(s))
        }
        5 => {
            let len = read_u32_le(data, pos)? as usize;
            if *pos + len > data.len() {
                return Err("kv: truncated bytes".to_string());
            }
            let b = data[*pos..*pos + len].to_vec();
            *pos += len;
            Ok(Value::Bytes(Rc::new(b)))
        }
        6 => {
            let count = read_u32_le(data, pos)? as usize;
            if count > 1_000_000 {
                return Err("kv: list count exceeds limit".to_string());
            }
            let mut items = Vec::with_capacity(count.min(1024));
            for _ in 0..count {
                items.push(decode_value(data, pos, depth + 1)?);
            }
            Ok(Value::List(Rc::new(RefCell::new(items))))
        }
        7 => {
            let count = read_u32_le(data, pos)? as usize;
            if count > 1_000_000 {
                return Err("kv: dict count exceeds limit".to_string());
            }
            let mut map = new_dict_with_capacity(count.min(1024));
            for _ in 0..count {
                let klen = read_u32_le(data, pos)? as usize;
                if *pos + klen > data.len() {
                    return Err("kv: truncated dict key".to_string());
                }
                let k = std::str::from_utf8(&data[*pos..*pos + klen])
                    .map_err(|_| "kv: invalid UTF-8 dict key".to_string())?
                    .to_string();
                *pos += klen;
                let val = decode_value(data, pos, depth + 1)?;
                map.insert(k, val);
            }
            Ok(Value::dict(map))
        }
        _ => Err(format!("kv: unknown value tag {}", tag)),
    }
}

fn build_record_bytes(op: u8, key: &str, val_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if key.len() > MAX_KEY_BYTES {
        return Err("kv: key exceeds 4096 bytes".to_string());
    }
    let klen = key.len() as u32;
    let vlen = u32::try_from(val_bytes.len()).map_err(|_| "kv: value too large".to_string())?;
    let mut rec = Vec::with_capacity(1 + 4 + 4 + key.len() + val_bytes.len() + 4);
    rec.push(op);
    rec.extend_from_slice(&klen.to_le_bytes());
    rec.extend_from_slice(&vlen.to_le_bytes());
    rec.extend_from_slice(key.as_bytes());
    rec.extend_from_slice(val_bytes);
    let crc = crc32_bytes(&rec);
    rec.extend_from_slice(&crc.to_le_bytes());
    Ok(rec)
}

fn replay_kv_bytes(raw: &[u8]) -> Result<(BTreeMap<String, Value>, usize), String> {
    if raw.is_empty() {
        return Ok((BTreeMap::new(), 0));
    }
    if raw.len() < KV_MAGIC.len() || &raw[..8] != KV_MAGIC {
        return Err("kv.open(): invalid KV store header".to_string());
    }
    let mut map = BTreeMap::new();
    let mut pos = 8usize;
    while pos < raw.len() {
        let rec_start = pos;
        if pos + 9 > raw.len() {
            break; // Truncated tail record ignored cleanly
        }
        let op = raw[pos];
        let klen = u32::from_le_bytes(raw[pos + 1..pos + 5].try_into().unwrap()) as usize;
        let vlen = u32::from_le_bytes(raw[pos + 5..pos + 9].try_into().unwrap()) as usize;
        if klen > MAX_KEY_BYTES || vlen > MAX_VAL_BYTES {
            break;
        }
        let Some(rec_end) = pos
            .checked_add(9)
            .and_then(|n| n.checked_add(klen))
            .and_then(|n| n.checked_add(vlen))
            .and_then(|n| n.checked_add(4))
        else {
            break;
        };
        if rec_end > raw.len() {
            break;
        }
        let body = &raw[pos..rec_end - 4];
        let expected_crc = u32::from_le_bytes(raw[rec_end - 4..rec_end].try_into().unwrap());
        if crc32_bytes(body) != expected_crc {
            break;
        }
        let key_bytes = &raw[pos + 9..pos + 9 + klen];
        let val_bytes = &raw[pos + 9 + klen..pos + 9 + klen + vlen];
        let Ok(key) = std::str::from_utf8(key_bytes) else {
            break;
        };
        if op == OP_SET {
            let mut vpos = 0usize;
            let Ok(val) = decode_value(val_bytes, &mut vpos, 0) else {
                break;
            };
            if vpos != val_bytes.len() {
                break;
            }
            map.insert(key.to_string(), val);
        } else if op == OP_DEL {
            map.remove(key);
        } else {
            break;
        }
        pos = rec_end;
        let _ = rec_start;
    }
    Ok((map, pos))
}

fn kv_open(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("kv.open() expects 1 path argument".to_string());
    }
    let raw = expect_str(&args[0], "kv.open()")?;
    let path = vm.resolve_write_path(raw)?;
    crate::modules::fs::check_safe_write_target(vm, &path, raw)?;

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)
        .map_err(|e| format!("kv.open('{}') failed: {}", raw, e))?;

    let meta_len = file.metadata().map_err(|e| e.to_string())?.len();
    if meta_len > 64 * 1024 * 1024 {
        return Err("kv.open(): store file exceeds 64 MiB limit".to_string());
    }
    vm.charge_io(meta_len as usize)?;
    vm.charge_alloc(meta_len as usize)?;
    vm.charge_work((meta_len / 64) + 10)?;

    let mut raw_bytes = Vec::with_capacity(meta_len as usize);
    file.read_to_end(&mut raw_bytes)
        .map_err(|e| e.to_string())?;

    let (map, valid_end) = if raw_bytes.is_empty() {
        file.write_all(KV_MAGIC).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        (BTreeMap::new(), KV_MAGIC.len())
    } else {
        replay_kv_bytes(&raw_bytes)?
    };
    if valid_end < raw_bytes.len() {
        file.set_len(valid_end as u64).map_err(|e| e.to_string())?;
    }
    file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;

    let handle_val = KV_STORES.with(|reg| {
        let mut reg = reg.borrow_mut();
        let id = reg.next_id;
        reg.next_id += 1;
        reg.stores.insert(
            id,
            KvStore {
                path,
                file: Some(file),
                map,
                closed: false,
            },
        );
        Value::StructInstance(Rc::new(StructInstance {
            def: Rc::clone(&reg.def),
            fields: RefCell::new(vec![Value::Int(id as i64)]),
        }))
    });
    Ok(handle_val)
}

fn with_store_mut<R>(
    id: u64,
    ctx: &str,
    f: impl FnOnce(&mut KvStore) -> Result<R, String>,
) -> Result<R, String> {
    KV_STORES.with(|reg| {
        let mut reg = reg.borrow_mut();
        let store = reg
            .stores
            .get_mut(&id)
            .ok_or_else(|| format!("{}: store handle is closed or invalid", ctx))?;
        if store.closed {
            return Err(format!("{}: store is closed", ctx));
        }
        f(store)
    })
}

fn kv_get(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("kv.get() expects 2 arguments (store, key)".to_string());
    }
    let id = extract_store_id(&args[0], "kv.get()")?;
    let key = expect_str(&args[1], "kv.get()")?;
    vm.charge_work(2)?;
    with_store_mut(id, "kv.get()", |store| {
        Ok(store.map.get(key).cloned().unwrap_or(Value::Unit))
    })
}

fn kv_has(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("kv.has() expects 2 arguments (store, key)".to_string());
    }
    let id = extract_store_id(&args[0], "kv.has()")?;
    let key = expect_str(&args[1], "kv.has()")?;
    vm.charge_work(1)?;
    with_store_mut(id, "kv.has()", |store| {
        Ok(Value::Bool(store.map.contains_key(key)))
    })
}

fn kv_set(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("kv.set() expects 3 arguments (store, key, value)".to_string());
    }
    let id = extract_store_id(&args[0], "kv.set()")?;
    let key = expect_str(&args[1], "kv.set()")?;
    let mut val_buf = Vec::new();
    encode_value(&args[2], &mut val_buf, 0)?;
    let rec = build_record_bytes(OP_SET, key, &val_buf)?;
    vm.charge_io(rec.len())?;
    vm.charge_work((rec.len() as u64) / 64 + 4)?;

    let val_clone = args[2].clone();
    with_store_mut(id, "kv.set()", |store| {
        let f = store
            .file
            .as_mut()
            .ok_or_else(|| "kv.set(): store file closed".to_string())?;
        f.write_all(&rec).map_err(|e| e.to_string())?;
        f.flush().map_err(|e| e.to_string())?;
        store.map.insert(key.to_string(), val_clone);
        Ok(Value::Unit)
    })
}

fn kv_delete(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("kv.delete() expects 2 arguments (store, key)".to_string());
    }
    let id = extract_store_id(&args[0], "kv.delete()")?;
    let key = expect_str(&args[1], "kv.delete()")?;
    let rec = build_record_bytes(OP_DEL, key, &[])?;
    vm.charge_io(rec.len())?;
    vm.charge_work(4)?;
    with_store_mut(id, "kv.delete()", |store| {
        let existed = store.map.remove(key).is_some();
        if existed {
            let f = store
                .file
                .as_mut()
                .ok_or_else(|| "kv.delete(): store file closed".to_string())?;
            f.write_all(&rec).map_err(|e| e.to_string())?;
            f.flush().map_err(|e| e.to_string())?;
        }
        Ok(Value::Bool(existed))
    })
}

fn kv_keys(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("kv.keys() expects 1 argument (store)".to_string());
    }
    let id = extract_store_id(&args[0], "kv.keys()")?;
    with_store_mut(id, "kv.keys()", |store| {
        vm.charge_alloc(
            store
                .map
                .len()
                .saturating_mul(std::mem::size_of::<Value>() + 24),
        )?;
        let keys: Vec<Value> = store.map.keys().cloned().map(Value::from_str).collect();
        Ok(Value::List(Rc::new(RefCell::new(keys))))
    })
}

fn kv_prefix(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("kv.prefix() expects 2 arguments (store, prefix)".to_string());
    }
    let id = extract_store_id(&args[0], "kv.prefix()")?;
    let pfx = expect_str(&args[1], "kv.prefix()")?.to_string();
    with_store_mut(id, "kv.prefix()", |store| {
        let mut out = new_dict_map();
        for (k, v) in store.map.range(pfx.clone()..) {
            if !k.starts_with(&pfx) {
                break;
            }
            out.insert(k.clone(), v.clone());
        }
        vm.charge_alloc(
            out.len()
                .saturating_mul(std::mem::size_of::<Value>() * 2 + 24),
        )?;
        Ok(Value::dict(out))
    })
}

fn kv_batch(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("kv.batch() expects 2 arguments (store, ops_dict_or_list)".to_string());
    }
    let id = extract_store_id(&args[0], "kv.batch()")?;
    let mut encoded_ops: Vec<(bool, String, Value, Vec<u8>)> = Vec::new();
    let mut payload = Vec::new();

    match &args[1] {
        Value::Dict(d) => {
            let map = d.borrow();
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (k, v) in entries {
                if matches!(v, Value::Unit) {
                    let rec = build_record_bytes(OP_DEL, k, &[])?;
                    payload.extend_from_slice(&rec);
                    encoded_ops.push((false, k.clone(), Value::Unit, Vec::new()));
                } else {
                    let mut vbuf = Vec::new();
                    encode_value(v, &mut vbuf, 0)?;
                    let rec = build_record_bytes(OP_SET, k, &vbuf)?;
                    payload.extend_from_slice(&rec);
                    encoded_ops.push((true, k.clone(), v.clone(), vbuf));
                }
            }
        }
        Value::List(list) => {
            let items = list.borrow();
            for item in items.iter() {
                let Value::List(pair) = item else {
                    return Err("kv.batch() list items must be [key, value] or [key]".to_string());
                };
                let p = pair.borrow();
                if p.is_empty() || p.len() > 2 {
                    return Err("kv.batch() list items must be [key, value] or [key]".to_string());
                }
                let k = expect_str(&p[0], "kv.batch()")?.to_string();
                if p.len() == 1 || matches!(p[1], Value::Unit) {
                    let rec = build_record_bytes(OP_DEL, &k, &[])?;
                    payload.extend_from_slice(&rec);
                    encoded_ops.push((false, k, Value::Unit, Vec::new()));
                } else {
                    let mut vbuf = Vec::new();
                    encode_value(&p[1], &mut vbuf, 0)?;
                    let rec = build_record_bytes(OP_SET, &k, &vbuf)?;
                    payload.extend_from_slice(&rec);
                    encoded_ops.push((true, k, p[1].clone(), vbuf));
                }
            }
        }
        other => {
            return Err(format!(
                "kv.batch() expects dict or list of pairs, got '{}'",
                other.type_name()
            ));
        }
    }

    vm.charge_io(payload.len())?;
    vm.charge_work((payload.len() as u64) / 32 + 4)?;
    let count = encoded_ops.len();
    with_store_mut(id, "kv.batch()", |store| {
        if !payload.is_empty() {
            let f = store
                .file
                .as_mut()
                .ok_or_else(|| "kv.batch(): store file closed".to_string())?;
            f.write_all(&payload).map_err(|e| e.to_string())?;
            f.flush().map_err(|e| e.to_string())?;
        }
        for (is_set, k, v, _) in encoded_ops {
            if is_set {
                store.map.insert(k, v);
            } else {
                store.map.remove(&k);
            }
        }
        Ok(Value::Int(count as i64))
    })
}

fn kv_compact(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("kv.compact() expects 1 argument (store)".to_string());
    }
    let id = extract_store_id(&args[0], "kv.compact()")?;
    with_store_mut(id, "kv.compact()", |store| {
        let mut snapshot = Vec::from(*KV_MAGIC);
        for (k, v) in &store.map {
            let mut vbuf = Vec::new();
            encode_value(v, &mut vbuf, 0)?;
            let rec = build_record_bytes(OP_SET, k, &vbuf)?;
            snapshot.extend_from_slice(&rec);
        }
        vm.charge_io(snapshot.len())?;
        vm.charge_work((snapshot.len() as u64) / 32 + 8)?;

        let tmp_path = store.path.with_extension("kvtmp");
        let mut tmp_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)
            .map_err(|e| e.to_string())?;
        tmp_file.write_all(&snapshot).map_err(|e| e.to_string())?;
        tmp_file.sync_all().map_err(|e| e.to_string())?;
        drop(tmp_file);

        drop(store.file.take());
        #[cfg(windows)]
        let _ = fs::remove_file(&store.path);
        fs::rename(&tmp_path, &store.path).map_err(|e| e.to_string())?;
        let mut new_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&store.path)
            .map_err(|e| e.to_string())?;
        new_file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
        store.file = Some(new_file);
        Ok(Value::Int(snapshot.len() as i64))
    })
}

fn kv_len(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("kv.len() expects 1 argument (store)".to_string());
    }
    let id = extract_store_id(&args[0], "kv.len()")?;
    with_store_mut(id, "kv.len()", |store| {
        Ok(Value::Int(store.map.len() as i64))
    })
}

fn kv_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("kv.close() expects 1 argument (store)".to_string());
    }
    let id = extract_store_id(&args[0], "kv.close()")?;
    KV_STORES.with(|reg| {
        let mut reg = reg.borrow_mut();
        if let Some(mut store) = reg.stores.remove(&id) {
            store.closed = true;
            if let Some(ref mut f) = store.file {
                let _ = f.flush();
            }
            drop(store.file.take());
        }
    });
    Ok(Value::Unit)
}
