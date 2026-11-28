use std::fs;
use std::path::PathBuf;

use crate::media::{err_corrupt, err_limit};
use crate::modules::hash::sha256_bytes;
use crate::scene::asset::MAX_ASSET_BYTES;
use crate::vm::VM;

pub const ASKAS_MAGIC: &[u8; 6] = b"ASKAS\0";
pub const ASKAS_SCHEMA: u16 = 1;
pub const ASKAS_HEADER_LEN: usize = 46; // 6 magic + 2 schema + 2 parser_id + 4 len + 32 sha256

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParserId {
    Mesh = 2,
    Tiled = 3,
    Terrain = 4,
}

fn cache_dir_for_vm(vm: &VM) -> PathBuf {
    vm.sandbox_root().join("__askcache__").join("assets")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = sha256_bytes(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

pub fn askas_cache_path(vm: &VM, source_key_bytes: &[u8], parser_id: ParserId) -> PathBuf {
    let hash = sha256_hex(source_key_bytes);
    cache_dir_for_vm(vm).join(format!("{}_p{}.askas", hash, parser_id as u16))
}

pub fn encode_askas(parser_id: ParserId, payload: &[u8]) -> Result<Vec<u8>, String> {
    if payload.len() > MAX_ASSET_BYTES {
        return Err(err_limit("ASKAS cache payload exceeds 256 MiB limit"));
    }
    let digest = sha256_bytes(payload);
    let mut out = Vec::with_capacity(ASKAS_HEADER_LEN + payload.len());
    out.extend_from_slice(ASKAS_MAGIC);
    out.extend_from_slice(&ASKAS_SCHEMA.to_le_bytes());
    out.extend_from_slice(&(parser_id as u16).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&digest);
    out.extend_from_slice(payload);
    Ok(out)
}

pub fn decode_and_verify_askas(raw: &[u8], expected_parser: ParserId) -> Result<&[u8], String> {
    if raw.len() < ASKAS_HEADER_LEN {
        return Err(err_corrupt("corrupt ASKAS cache file (too short)"));
    }
    if &raw[0..6] != ASKAS_MAGIC {
        return Err(err_corrupt("corrupt ASKAS magic header"));
    }
    let schema = u16::from_le_bytes([raw[6], raw[7]]);
    if schema != ASKAS_SCHEMA {
        return Err(err_corrupt(format!(
            "unsupported ASKAS schema version {}",
            schema
        )));
    }
    let pid = u16::from_le_bytes([raw[8], raw[9]]);
    if pid != (expected_parser as u16) {
        return Err(err_corrupt("ASKAS parser_id mismatch"));
    }
    let len = u32::from_le_bytes([raw[10], raw[11], raw[12], raw[13]]) as usize;
    if len > MAX_ASSET_BYTES || ASKAS_HEADER_LEN + len != raw.len() {
        return Err(err_corrupt("corrupt ASKAS payload length"));
    }
    let payload = &raw[ASKAS_HEADER_LEN..ASKAS_HEADER_LEN + len];
    let expected_hash: [u8; 32] = raw[14..46].try_into().unwrap();
    let actual_hash = sha256_bytes(payload);
    // Constant-time checksum compare
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= expected_hash[i] ^ actual_hash[i];
    }
    if diff != 0 {
        return Err(err_corrupt("ASKAS SHA-256 integrity verification failed"));
    }
    Ok(payload)
}

pub fn try_load_askas<T>(
    vm: &VM,
    source_key_bytes: &[u8],
    parser_id: ParserId,
    deserialize_and_verify: impl FnOnce(&[u8]) -> Result<T, String>,
) -> Option<T> {
    if !vm.use_bytecode_cache() {
        return None;
    }
    let path = askas_cache_path(vm, source_key_bytes, parser_id);
    let raw = fs::read(&path).ok()?;
    let payload = decode_and_verify_askas(&raw, parser_id).ok()?;
    deserialize_and_verify(payload).ok()
}

pub fn try_store_askas(vm: &VM, source_key_bytes: &[u8], parser_id: ParserId, payload: &[u8]) {
    if !vm.use_bytecode_cache() {
        return;
    }
    let cache_probe = vm.sandbox_root().join("assets");
    if !vm.can_write_cache_for(&cache_probe) {
        return;
    }
    let path = askas_cache_path(vm, source_key_bytes, parser_id);
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let Ok(blob) = encode_askas(parser_id, payload) else {
        return;
    };
    let tmp = path.with_extension("askas.tmp");
    if fs::write(&tmp, &blob).is_ok() {
        if fs::rename(&tmp, &path).is_err() {
            let _ = fs::write(&path, &blob);
        }
        let _ = fs::remove_file(&tmp);
    }
}

pub struct BinWriter {
    pub buf: Vec<u8>,
}

impl BinWriter {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(256),
        }
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_str(&mut self, s: &str) {
        self.write_u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
    }
}

pub struct BinReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> BinReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub fn read_u8(&mut self) -> Result<u8, String> {
        let b = *self
            .bytes
            .get(self.pos)
            .ok_or_else(|| err_corrupt("ASKAS unexpected EOF (u8)"))?;
        self.pos += 1;
        Ok(b)
    }

    pub fn read_u32(&mut self) -> Result<u32, String> {
        if self.pos + 4 > self.bytes.len() {
            return Err(err_corrupt("ASKAS unexpected EOF (u32)"));
        }
        let v = u32::from_le_bytes(self.bytes[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    pub fn read_f32(&mut self) -> Result<f32, String> {
        let bits = self.read_u32()?;
        Ok(f32::from_bits(bits))
    }

    pub fn read_str(&mut self) -> Result<String, String> {
        let len = self.read_u32()? as usize;
        if len > 1024 * 1024 || self.pos + len > self.bytes.len() {
            return Err(err_corrupt("ASKAS string length out of bounds"));
        }
        let s = std::str::from_utf8(&self.bytes[self.pos..self.pos + len])
            .map_err(|_| err_corrupt("ASKAS invalid UTF-8 string"))?
            .to_string();
        self.pos += len;
        Ok(s)
    }
}
