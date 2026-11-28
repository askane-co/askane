#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::bytecode;
use crate::compiler::Compiler;
use crate::lexer::Lexer;
use crate::modules::{compress, hash};
use crate::parser::Parser;

pub const PAK_MAGIC: &[u8; 10] = b"ASKANE-PAK";
pub const PAK_VERSION: u16 = 1;
pub const PAK_HEADER_SIZE: usize = 80;
pub const PAK_TRAILER_MAGIC: &[u8; 16] = b"ASKANE-PAK-EOF\x01\0";
pub const PAK_TRAILER_SIZE: usize = 64;

pub const DEFAULT_MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TOTAL_UNCOMPRESSED: u64 = 512 * 1024 * 1024;
pub const MAX_PAK_ENTRIES: usize = 65_536;
pub const PACKED_ASKC_HASH: [u8; 32] = *b"ASKANE-PACKED-BYTECODE-V020\0\0\0\0\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleMode {
    Hermetic,
    Overlay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyMode {
    Full,
    Lazy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PakEntryKind {
    Asset = 0,
    Source = 1,
    Bytecode = 2,
    Entrypoint = 3,
}

impl PakEntryKind {
    fn from_u8(b: u8) -> Result<Self, String> {
        match b {
            0 => Ok(Self::Asset),
            1 => Ok(Self::Source),
            2 => Ok(Self::Bytecode),
            3 => Ok(Self::Entrypoint),
            other => Err(format!("E_BUNDLE_CORRUPT: invalid entry kind {}", other)),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Asset => "asset",
            Self::Source => "source",
            Self::Bytecode => "bytecode",
            Self::Entrypoint => "entry",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PakMethod {
    Stored = 0,
    Deflate = 1,
}

impl PakMethod {
    fn from_u8(b: u8) -> Result<Self, String> {
        match b {
            0 => Ok(Self::Stored),
            1 => Ok(Self::Deflate),
            other => Err(format!(
                "E_BUNDLE_CORRUPT: unsupported compression method {}",
                other
            )),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Stored => "stored",
            Self::Deflate => "deflate",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PakEntryMeta {
    pub name: String,
    pub kind: PakEntryKind,
    pub method: PakMethod,
    pub data_offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub sha256: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct PakArchive {
    pub data: Rc<Vec<u8>>,
    #[allow(dead_code)]
    pub flags: u32,
    pub entry_script: Option<String>,
    pub entries: Vec<PakEntryMeta>,
    pub by_lower_key: HashMap<String, usize>,
}

#[derive(Clone, Debug)]
pub enum VfsRead {
    PakBytes(Vec<u8>),
    DiskPath(PathBuf),
}

/// Pure-Rust Unicode NFC normalization for bundle entry keys and logical paths
/// (never apply to raw OS filesystem paths on macOS HFS+/APFS).
pub fn normalize_nfc(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    let mut nfd = Vec::with_capacity(s.len());
    for ch in s.chars() {
        decompose_char(ch, &mut nfd);
    }
    canonical_order(&mut nfd);
    compose_chars(&nfd)
}

fn combining_class(c: char) -> u8 {
    match c as u32 {
        0x0300..=0x0314 | 0x033D..=0x0344 | 0x0346 | 0x034A..=0x034C | 0x0350..=0x0352 => 230,
        0x0316..=0x0319 | 0x031C..=0x0326 | 0x0329..=0x0333 | 0x0339..=0x033C | 0x0347..=0x0349 => 220,
        0x0327 | 0x0328 => 202,
        0x031B => 216,
        0x0334..=0x0338 => 1,
        _ => 0,
    }
}

fn decompose_pair(c: char) -> Option<(char, char)> {
    Some(match c {
        '\u{00C0}' => ('A', '\u{0300}'),
        '\u{00C1}' => ('A', '\u{0301}'),
        '\u{00C2}' => ('A', '\u{0302}'),
        '\u{00C3}' => ('A', '\u{0303}'),
        '\u{00C4}' => ('A', '\u{0308}'),
        '\u{00C5}' => ('A', '\u{030A}'),
        '\u{00C7}' => ('C', '\u{0327}'),
        '\u{00C8}' => ('E', '\u{0300}'),
        '\u{00C9}' => ('E', '\u{0301}'),
        '\u{00CA}' => ('E', '\u{0302}'),
        '\u{00CB}' => ('E', '\u{0308}'),
        '\u{00CC}' => ('I', '\u{0300}'),
        '\u{00CD}' => ('I', '\u{0301}'),
        '\u{00CE}' => ('I', '\u{0302}'),
        '\u{00CF}' => ('I', '\u{0308}'),
        '\u{00D1}' => ('N', '\u{0303}'),
        '\u{00D2}' => ('O', '\u{0300}'),
        '\u{00D3}' => ('O', '\u{0301}'),
        '\u{00D4}' => ('O', '\u{0302}'),
        '\u{00D5}' => ('O', '\u{0303}'),
        '\u{00D6}' => ('O', '\u{0308}'),
        '\u{00D9}' => ('U', '\u{0300}'),
        '\u{00DA}' => ('U', '\u{0301}'),
        '\u{00DB}' => ('U', '\u{0302}'),
        '\u{00DC}' => ('U', '\u{0308}'),
        '\u{00DD}' => ('Y', '\u{0301}'),
        '\u{00E0}' => ('a', '\u{0300}'),
        '\u{00E1}' => ('a', '\u{0301}'),
        '\u{00E2}' => ('a', '\u{0302}'),
        '\u{00E3}' => ('a', '\u{0303}'),
        '\u{00E4}' => ('a', '\u{0308}'),
        '\u{00E5}' => ('a', '\u{030A}'),
        '\u{00E7}' => ('c', '\u{0327}'),
        '\u{00E8}' => ('e', '\u{0300}'),
        '\u{00E9}' => ('e', '\u{0301}'),
        '\u{00EA}' => ('e', '\u{0302}'),
        '\u{00EB}' => ('e', '\u{0308}'),
        '\u{00EC}' => ('i', '\u{0300}'),
        '\u{00ED}' => ('i', '\u{0301}'),
        '\u{00EE}' => ('i', '\u{0302}'),
        '\u{00EF}' => ('i', '\u{0308}'),
        '\u{00F1}' => ('n', '\u{0303}'),
        '\u{00F2}' => ('o', '\u{0300}'),
        '\u{00F3}' => ('o', '\u{0301}'),
        '\u{00F4}' => ('o', '\u{0302}'),
        '\u{00F5}' => ('o', '\u{0303}'),
        '\u{00F6}' => ('o', '\u{0308}'),
        '\u{00F9}' => ('u', '\u{0300}'),
        '\u{00FA}' => ('u', '\u{0301}'),
        '\u{00FB}' => ('u', '\u{0302}'),
        '\u{00FC}' => ('u', '\u{0308}'),
        '\u{00FD}' => ('y', '\u{0301}'),
        '\u{00FF}' => ('y', '\u{0308}'),
        '\u{0401}' => ('\u{0415}', '\u{0308}'), // Ё
        '\u{0451}' => ('\u{0435}', '\u{0308}'), // ё
        '\u{0419}' => ('\u{0418}', '\u{0306}'), // Й
        '\u{0439}' => ('\u{0438}', '\u{0306}'), // й
        _ => return None,
    })
}

fn compose_pair(a: char, b: char) -> Option<char> {
    let u = a as u32;
    let v = b as u32;
    if (0x1100..=0x1112).contains(&u) && (0x1161..=0x1175).contains(&v) {
        let s = 0xAC00 + ((u - 0x1100) * 21 + (v - 0x1161)) * 28;
        return char::from_u32(s);
    }
    if (0xAC00..=0xD7A3).contains(&u)
        && (u - 0xAC00).is_multiple_of(28)
        && (0x11A8..=0x11C2).contains(&v)
    {
        return char::from_u32(u + (v - 0x11A7));
    }
    Some(match (a, b) {
        ('A', '\u{0300}') => '\u{00C0}',
        ('A', '\u{0301}') => '\u{00C1}',
        ('A', '\u{0302}') => '\u{00C2}',
        ('A', '\u{0303}') => '\u{00C3}',
        ('A', '\u{0308}') => '\u{00C4}',
        ('A', '\u{030A}') => '\u{00C5}',
        ('C', '\u{0327}') => '\u{00C7}',
        ('E', '\u{0300}') => '\u{00C8}',
        ('E', '\u{0301}') => '\u{00C9}',
        ('E', '\u{0302}') => '\u{00CA}',
        ('E', '\u{0308}') => '\u{00CB}',
        ('I', '\u{0300}') => '\u{00CC}',
        ('I', '\u{0301}') => '\u{00CD}',
        ('I', '\u{0302}') => '\u{00CE}',
        ('I', '\u{0308}') => '\u{00CF}',
        ('N', '\u{0303}') => '\u{00D1}',
        ('O', '\u{0300}') => '\u{00D2}',
        ('O', '\u{0301}') => '\u{00D3}',
        ('O', '\u{0302}') => '\u{00D4}',
        ('O', '\u{0303}') => '\u{00D5}',
        ('O', '\u{0308}') => '\u{00D6}',
        ('U', '\u{0300}') => '\u{00D9}',
        ('U', '\u{0301}') => '\u{00DA}',
        ('U', '\u{0302}') => '\u{00DB}',
        ('U', '\u{0308}') => '\u{00DC}',
        ('Y', '\u{0301}') => '\u{00DD}',
        ('a', '\u{0300}') => '\u{00E0}',
        ('a', '\u{0301}') => '\u{00E1}',
        ('a', '\u{0302}') => '\u{00E2}',
        ('a', '\u{0303}') => '\u{00E3}',
        ('a', '\u{0308}') => '\u{00E4}',
        ('a', '\u{030A}') => '\u{00E5}',
        ('c', '\u{0327}') => '\u{00E7}',
        ('e', '\u{0300}') => '\u{00E8}',
        ('e', '\u{0301}') => '\u{00E9}',
        ('e', '\u{0302}') => '\u{00EA}',
        ('e', '\u{0308}') => '\u{00EB}',
        ('i', '\u{0300}') => '\u{00EC}',
        ('i', '\u{0301}') => '\u{00ED}',
        ('i', '\u{0302}') => '\u{00EE}',
        ('i', '\u{0308}') => '\u{00EF}',
        ('n', '\u{0303}') => '\u{00F1}',
        ('o', '\u{0300}') => '\u{00F2}',
        ('o', '\u{0301}') => '\u{00F3}',
        ('o', '\u{0302}') => '\u{00F4}',
        ('o', '\u{0303}') => '\u{00F5}',
        ('o', '\u{0308}') => '\u{00F6}',
        ('u', '\u{0300}') => '\u{00F9}',
        ('u', '\u{0301}') => '\u{00FA}',
        ('u', '\u{0302}') => '\u{00FB}',
        ('u', '\u{0308}') => '\u{00FC}',
        ('y', '\u{0301}') => '\u{00FD}',
        ('y', '\u{0308}') => '\u{00FF}',
        ('\u{0415}', '\u{0308}') => '\u{0401}',
        ('\u{0435}', '\u{0308}') => '\u{0451}',
        ('\u{0418}', '\u{0306}') => '\u{0419}',
        ('\u{0438}', '\u{0306}') => '\u{0439}',
        _ => return None,
    })
}

fn decompose_char(c: char, out: &mut Vec<char>) {
    let u = c as u32;
    if (0xAC00..=0xD7A3).contains(&u) {
        let s_idx = u - 0xAC00;
        let l = 0x1100 + s_idx / 588;
        let v = 0x1161 + (s_idx % 588) / 28;
        let t = s_idx % 28;
        out.push(char::from_u32(l).unwrap());
        out.push(char::from_u32(v).unwrap());
        if t > 0 {
            out.push(char::from_u32(0x11A7 + t).unwrap());
        }
        return;
    }
    if let Some((a, b)) = decompose_pair(c) {
        decompose_char(a, out);
        out.push(b);
    } else {
        out.push(c);
    }
}

fn canonical_order(chars: &mut [char]) {
    let n = chars.len();
    for i in 1..n {
        let mut j = i;
        while j > 0 {
            let cc_prev = combining_class(chars[j - 1]);
            let cc_cur = combining_class(chars[j]);
            if cc_cur != 0 && cc_prev > cc_cur {
                chars.swap(j - 1, j);
                j -= 1;
            } else {
                break;
            }
        }
    }
}

fn compose_chars(chars: &[char]) -> String {
    if chars.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(chars.len());
    let mut starter = chars[0];
    let mut has_starter = combining_class(starter) == 0;
    let mut prev_cc: u8 = if has_starter {
        0
    } else {
        out.push(starter);
        255
    };
    let mut blocked_tail: Vec<char> = Vec::new();

    for &ch in &chars[1..] {
        let cc = combining_class(ch);
        if has_starter && (blocked_tail.is_empty() || prev_cc < cc) {
            if let Some(comp) = compose_pair(starter, ch) {
                starter = comp;
                continue;
            }
        }
        if cc == 0 {
            if has_starter {
                out.push(starter);
                for tc in blocked_tail.drain(..) {
                    out.push(tc);
                }
            }
            starter = ch;
            has_starter = true;
            prev_cc = 0;
        } else {
            blocked_tail.push(ch);
            prev_cc = prev_cc.max(cc);
        }
    }
    if has_starter {
        out.push(starter);
        for tc in blocked_tail {
            out.push(tc);
        }
    }
    out
}

pub fn is_windows_reserved_stem(stem: &str) -> bool {
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

pub fn validate_pak_entry_name(raw: &str) -> Result<String, String> {
    if raw.is_empty() || raw.len() > 512 {
        return Err("E_BUNDLE_CORRUPT: empty or oversized bundle entry path".to_string());
    }
    if raw.contains('\0') || raw.chars().any(|c| c < ' ' || c == '\x7F') {
        return Err(format!(
            "E_BUNDLE_CORRUPT: control character in bundle path {:?}",
            raw
        ));
    }
    if raw.contains('\\') || raw.starts_with('/') || raw.ends_with('/') {
        return Err(format!(
            "E_BUNDLE_CORRUPT: invalid slash in bundle entry path {:?}",
            raw
        ));
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(format!(
            "E_BUNDLE_CORRUPT: drive prefix in bundle path {:?}",
            raw
        ));
    }
    let segs: Vec<&str> = raw.split('/').collect();
    if segs.len() > 32 {
        return Err(format!(
            "E_BUNDLE_CORRUPT: bundle entry depth > 32 at {:?}",
            raw
        ));
    }
    for seg in &segs {
        if seg.is_empty() || *seg == "." || *seg == ".." {
            return Err(format!(
                "E_BUNDLE_CORRUPT: invalid segment '{}' in bundle path {:?}",
                seg, raw
            ));
        }
        if seg.ends_with('.') || seg.ends_with(' ') {
            return Err(format!(
                "E_BUNDLE_CORRUPT: trailing dot/space segment '{}' in bundle path {:?}",
                seg, raw
            ));
        }
        if seg
            .chars()
            .any(|c| matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        {
            return Err(format!(
                "E_BUNDLE_CORRUPT: forbidden character in bundle path {:?}",
                raw
            ));
        }
        let stem = seg.split('.').next().unwrap_or(seg);
        if is_windows_reserved_stem(stem) {
            return Err(format!(
                "E_BUNDLE_CORRUPT: Windows reserved name '{}' in bundle path {:?}",
                seg, raw
            ));
        }
    }
    Ok(normalize_nfc(raw))
}

pub fn logical_normalize_rel(raw: &str) -> Result<String, String> {
    if raw.is_empty() || raw.len() > 1024 || raw.contains('\0') {
        return Err(format!("Invalid bundle relative path {:?}", raw));
    }
    if raw.starts_with('/') || raw.starts_with('\\') {
        return Err(format!(
            "Absolute path {:?} is not allowed inside bundle VFS",
            raw
        ));
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(format!("Drive path {:?} is not allowed inside bundle VFS", raw));
    }
    let unified = raw.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for seg in unified.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(format!(
                        "Bundle path traversal ('..') outside root in {:?}",
                        raw
                    ));
                }
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    validate_pak_entry_name(&joined).map_err(|_| format!("Invalid bundle path {:?}", raw))
}

#[derive(Clone, Debug)]
pub struct BuildPakOptions {
    pub pack_askc: bool,
    pub max_entry_bytes: usize,
    pub compress: bool,
    pub entry_override: Option<String>,
}

impl Default for BuildPakOptions {
    fn default() -> Self {
        Self {
            pack_askc: false,
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            compress: true,
            entry_override: None,
        }
    }
}

struct PendingEntry {
    name: String,
    kind: PakEntryKind,
    raw_bytes: Vec<u8>,
}

fn collect_dir_entries(
    root: &Path,
    cur: &Path,
    entry_rel: &str,
    opts: &BuildPakOptions,
    seen_lower: &mut HashMap<String, String>,
    out: &mut BTreeMap<String, PendingEntry>,
    depth: usize,
) -> Result<(), String> {
    if depth > 32 {
        return Err(format!(
            "bundle directory depth exceeded 32 at '{}'",
            cur.display()
        ));
    }
    let mut children: Vec<PathBuf> = fs::read_dir(cur)
        .map_err(|e| format!("bundle read_dir '{}': {}", cur.display(), e))?
        .filter_map(|e| e.ok().map(|x| x.path()))
        .collect();
    children.sort();

    for p in children {
        let meta = fs::symlink_metadata(&p)
            .map_err(|e| format!("bundle stat '{}': {}", p.display(), e))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let fname = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if matches!(fname.as_str(), ".git" | ".askane_cache" | "target") {
            continue;
        }
        if fname.ends_with(".pak") {
            continue;
        }
        if meta.is_dir() {
            collect_dir_entries(root, &p, entry_rel, opts, seen_lower, out, depth + 1)?;
            continue;
        }
        if !meta.is_file() {
            continue;
        }

        let rel = p
            .strip_prefix(root)
            .map_err(|e| format!("strip_prefix '{}': {}", p.display(), e))?;
        let raw_rel: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        let rel_joined = raw_rel.join("/");
        let mut norm_name = validate_pak_entry_name(&rel_joined)?;

        if meta.len() > opts.max_entry_bytes as u64 {
            return Err(format!(
                "bundle entry '{}' ({} bytes) exceeds --max-entry-mb limit ({} bytes)",
                norm_name,
                meta.len(),
                opts.max_entry_bytes
            ));
        }

        let mut bytes =
            fs::read(&p).map_err(|e| format!("bundle read '{}': {}", p.display(), e))?;
        let is_entry_src = norm_name.eq_ignore_ascii_case(entry_rel);
        let mut kind = if is_entry_src {
            PakEntryKind::Entrypoint
        } else if norm_name.ends_with(".ask") {
            PakEntryKind::Source
        } else if norm_name.ends_with(".askc") {
            PakEntryKind::Bytecode
        } else {
            PakEntryKind::Asset
        };

        if opts.pack_askc && norm_name.ends_with(".ask") {
            let src = String::from_utf8(bytes.clone()).map_err(|_| {
                format!(
                    "bundle --pack-askc: '{}' is not valid UTF-8 source",
                    norm_name
                )
            })?;
            let is_module = !is_entry_src;
            let tokens = Lexer::new(&src).tokenize()?;
            let program = Parser::new(tokens).parse_program()?;
            let proto = if is_module {
                let mname = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("mod");
                Compiler::compile_module(&program, mname)?
            } else {
                Compiler::compile(&program)?
            };
            bytes = bytecode::encode_cached_proto(&proto, &PACKED_ASKC_HASH, is_module, false)?;
            norm_name = format!("{}c", norm_name);
            kind = if is_entry_src {
                PakEntryKind::Entrypoint
            } else {
                PakEntryKind::Bytecode
            };
        }

        let lower = norm_name.to_lowercase();
        if let Some(prev) = seen_lower.insert(lower, norm_name.clone()) {
            return Err(format!(
                "E_BUNDLE_CORRUPT: case-insensitive collision between '{}' and '{}'",
                prev, norm_name
            ));
        }
        out.insert(
            norm_name.clone(),
            PendingEntry {
                name: norm_name,
                kind,
                raw_bytes: bytes,
            },
        );
    }
    Ok(())
}

pub fn build_pak_from_target(
    target: &Path,
    opts: &BuildPakOptions,
) -> Result<(Vec<u8>, Vec<PakEntryMeta>), String> {
    let (root, default_entry) = if target.is_dir() {
        let mut entry = "main.ask".to_string();
        let toml_path = target.join("askane.toml");
        if toml_path.is_file() {
            if let Ok(text) = fs::read_to_string(&toml_path) {
                for line in text.lines() {
                    let trimmed = line.trim();
                    if let Some(rest) = trimmed.strip_prefix("entry") {
                        if let Some((_, val)) = rest.split_once('=') {
                            let clean = val.trim().trim_matches('"').trim_matches('\'');
                            if !clean.is_empty() {
                                entry = clean.replace('\\', "/");
                            }
                        }
                    }
                }
            }
        }
        (target.to_path_buf(), entry)
    } else if target.is_file() {
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let fname = target
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "Invalid entry filename".to_string())?;
        (parent.to_path_buf(), fname.to_string())
    } else {
        return Err(format!("Bundle target '{}' does not exist", target.display()));
    };

    let entry_rel = opts
        .entry_override
        .clone()
        .unwrap_or(default_entry)
        .replace('\\', "/");

    let mut seen_lower = HashMap::new();
    let mut pending = BTreeMap::new();
    collect_dir_entries(
        &root,
        &root,
        &entry_rel,
        opts,
        &mut seen_lower,
        &mut pending,
        0,
    )?;

    if pending.is_empty() {
        return Err("No files found to package into ASKANE-PAK".to_string());
    }
    if pending.len() > MAX_PAK_ENTRIES {
        return Err(format!(
            "Bundle entry count {} exceeds {} limit",
            pending.len(),
            MAX_PAK_ENTRIES
        ));
    }

    let entries: Vec<PendingEntry> = pending.into_values().collect();
    encode_pak_entries(&entries, opts)
}

#[cfg(test)]
pub fn build_pak_from_memory_entries(
    items: &[(&str, PakEntryKind, &[u8])],
    opts: &BuildPakOptions,
) -> Result<Vec<u8>, String> {
    let mut seen_lower = HashMap::new();
    let mut pending = BTreeMap::new();
    for &(raw_name, kind, bytes) in items {
        let name = validate_pak_entry_name(raw_name)?;
        if bytes.len() > opts.max_entry_bytes {
            return Err(format!(
                "Entry '{}' ({} bytes) exceeds limit ({} bytes)",
                name,
                bytes.len(),
                opts.max_entry_bytes
            ));
        }
        let lower = name.to_lowercase();
        if let Some(prev) = seen_lower.insert(lower, name.clone()) {
            return Err(format!(
                "E_BUNDLE_CORRUPT: case-insensitive collision '{}' vs '{}'",
                prev, name
            ));
        }
        pending.insert(
            name.clone(),
            PendingEntry {
                name,
                kind,
                raw_bytes: bytes.to_vec(),
            },
        );
    }
    let entries: Vec<PendingEntry> = pending.into_values().collect();
    let (bytes, _) = encode_pak_entries(&entries, opts)?;
    Ok(bytes)
}

fn encode_pak_entries(
    entries: &[PendingEntry],
    opts: &BuildPakOptions,
) -> Result<(Vec<u8>, Vec<PakEntryMeta>), String> {
    let mut total_uncompressed: u64 = 0;
    for e in entries {
        total_uncompressed = total_uncompressed.saturating_add(e.raw_bytes.len() as u64);
        if total_uncompressed > MAX_TOTAL_UNCOMPRESSED {
            return Err(format!(
                "Bundle total uncompressed payload exceeds {} MiB limit",
                MAX_TOTAL_UNCOMPRESSED / (1024 * 1024)
            ));
        }
    }

    let mut data_region: Vec<u8> = Vec::new();
    let mut metas: Vec<PakEntryMeta> = Vec::with_capacity(entries.len());
    let mut has_askc = false;
    let mut has_entry = false;

    for e in entries {
        if e.kind == PakEntryKind::Bytecode {
            has_askc = true;
        }
        if e.kind == PakEntryKind::Entrypoint {
            has_entry = true;
        }
        let sha = hash::sha256_bytes(&e.raw_bytes);
        let (method, payload) = if opts.compress && e.raw_bytes.len() >= 64 {
            let def = compress::deflate_raw(&e.raw_bytes);
            if def.len() < e.raw_bytes.len() {
                (PakMethod::Deflate, def)
            } else {
                (PakMethod::Stored, e.raw_bytes.clone())
            }
        } else {
            (PakMethod::Stored, e.raw_bytes.clone())
        };
        let rel_off = PAK_HEADER_SIZE as u64 + data_region.len() as u64;
        data_region.extend_from_slice(&payload);
        metas.push(PakEntryMeta {
            name: e.name.clone(),
            kind: e.kind,
            method,
            data_offset: rel_off,
            compressed_size: payload.len() as u64,
            uncompressed_size: e.raw_bytes.len() as u64,
            sha256: sha,
        });
    }

    let mut toc_bytes: Vec<u8> = Vec::new();
    for m in &metas {
        let nb = m.name.as_bytes();
        toc_bytes.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        toc_bytes.extend_from_slice(nb);
        toc_bytes.push(m.method as u8);
        toc_bytes.push(m.kind as u8);
        toc_bytes.extend_from_slice(&0u16.to_le_bytes());
        toc_bytes.extend_from_slice(&m.data_offset.to_le_bytes());
        toc_bytes.extend_from_slice(&m.compressed_size.to_le_bytes());
        toc_bytes.extend_from_slice(&m.uncompressed_size.to_le_bytes());
        toc_bytes.extend_from_slice(&m.sha256);
    }

    let flags: u32 = u32::from(has_entry) | (u32::from(has_askc) << 1);
    let toc_offset = PAK_HEADER_SIZE as u64 + data_region.len() as u64;
    let toc_size = toc_bytes.len() as u64;
    let data_size = data_region.len() as u64;
    let toc_sha256 = hash::sha256_bytes(&toc_bytes);

    let mut body = Vec::with_capacity(PAK_HEADER_SIZE + data_region.len() + toc_bytes.len());
    body.extend_from_slice(PAK_MAGIC);
    body.extend_from_slice(&PAK_VERSION.to_le_bytes());
    body.extend_from_slice(&flags.to_le_bytes());
    body.extend_from_slice(&(metas.len() as u64).to_le_bytes());
    body.extend_from_slice(&toc_offset.to_le_bytes());
    body.extend_from_slice(&toc_size.to_le_bytes());
    body.extend_from_slice(&data_size.to_le_bytes());
    body.extend_from_slice(&toc_sha256);
    debug_assert_eq!(body.len(), PAK_HEADER_SIZE);
    body.extend_from_slice(&data_region);
    body.extend_from_slice(&toc_bytes);

    let full = append_eof_trailer(body, 0)?;
    Ok((full, metas))
}

fn append_eof_trailer(mut container_prefix: Vec<u8>, pak_offset: u64) -> Result<Vec<u8>, String> {
    let total_len = container_prefix.len() as u64;
    if total_len < pak_offset + PAK_HEADER_SIZE as u64 {
        return Err("E_BUNDLE_CORRUPT: container smaller than header".to_string());
    }
    let pak_size = total_len - pak_offset;
    let pak_sha = hash::sha256_bytes(&container_prefix[pak_offset as usize..]);
    container_prefix.extend_from_slice(&pak_offset.to_le_bytes());
    container_prefix.extend_from_slice(&pak_size.to_le_bytes());
    container_prefix.extend_from_slice(&pak_sha);
    container_prefix.extend_from_slice(PAK_TRAILER_MAGIC);
    Ok(container_prefix)
}

pub fn find_trailer_slice(file_bytes: &[u8]) -> Option<(u64, u64, [u8; 32])> {
    if file_bytes.len() < PAK_TRAILER_SIZE {
        return None;
    }
    let t = &file_bytes[file_bytes.len() - PAK_TRAILER_SIZE..];
    if &t[48..64] != PAK_TRAILER_MAGIC {
        return None;
    }
    let pak_offset = u64::from_le_bytes(t[0..8].try_into().ok()?);
    let pak_size = u64::from_le_bytes(t[8..16].try_into().ok()?);
    let mut sha = [0u8; 32];
    sha.copy_from_slice(&t[16..48]);
    Some((pak_offset, pak_size, sha))
}

pub fn parse_from_file(
    path: &Path,
    verify: VerifyMode,
    max_entry_bytes: usize,
) -> Result<PakArchive, String> {
    let mut file =
        File::open(path).map_err(|e| format!("Failed to open bundle '{}': {}", path.display(), e))?;
    let file_len = file
        .metadata()
        .map_err(|e| format!("Failed to stat bundle '{}': {}", path.display(), e))?
        .len();
    if file_len < (PAK_HEADER_SIZE + PAK_TRAILER_SIZE) as u64 {
        return Err("E_BUNDLE_CORRUPT: bundle file too small".to_string());
    }
    file.seek(SeekFrom::End(-(PAK_TRAILER_SIZE as i64)))
        .map_err(|e| format!("E_BUNDLE_CORRUPT: seek trailer failed: {}", e))?;
    let mut trailer = [0u8; PAK_TRAILER_SIZE];
    file.read_exact(&mut trailer)
        .map_err(|e| format!("E_BUNDLE_CORRUPT: read trailer failed: {}", e))?;
    if &trailer[48..64] != PAK_TRAILER_MAGIC {
        return Err("E_BUNDLE_CORRUPT: missing ASKANE-PAK EOF trailer".to_string());
    }
    let pak_offset = u64::from_le_bytes(trailer[0..8].try_into().unwrap());
    let pak_size = u64::from_le_bytes(trailer[8..16].try_into().unwrap());
    let mut expected_sha = [0u8; 32];
    expected_sha.copy_from_slice(&trailer[16..48]);

    if pak_size < PAK_HEADER_SIZE as u64
        || pak_size > MAX_TOTAL_UNCOMPRESSED + 64 * 1024 * 1024
        || pak_offset
            .checked_add(pak_size)
            .and_then(|s| s.checked_add(PAK_TRAILER_SIZE as u64))
            != Some(file_len)
    {
        return Err("E_BUNDLE_CORRUPT: trailer bounds mismatch".to_string());
    }

    file.seek(SeekFrom::Start(pak_offset))
        .map_err(|e| format!("E_BUNDLE_CORRUPT: seek pak start failed: {}", e))?;
    let mut pak_body = vec![0u8; pak_size as usize];
    file.read_exact(&mut pak_body)
        .map_err(|e| format!("E_BUNDLE_CORRUPT: truncated pak body: {}", e))?;
    if hash::sha256_bytes(&pak_body) != expected_sha {
        return Err("E_BUNDLE_CORRUPT: container SHA-256 checksum mismatch".to_string());
    }
    parse_pak_body(pak_body, verify, max_entry_bytes)
}

#[cfg(test)]
pub fn parse_pak_bytes(
    bytes: &[u8],
    verify: VerifyMode,
    max_entry_bytes: usize,
) -> Result<PakArchive, String> {
    let Some((pak_offset, pak_size, expected_sha)) = find_trailer_slice(bytes) else {
        return Err("E_BUNDLE_CORRUPT: missing ASKANE-PAK EOF trailer".to_string());
    };
    if pak_size < PAK_HEADER_SIZE as u64
        || pak_offset
            .checked_add(pak_size)
            .and_then(|s| s.checked_add(PAK_TRAILER_SIZE as u64))
            != Some(bytes.len() as u64)
    {
        return Err("E_BUNDLE_CORRUPT: invalid trailer offset/size".to_string());
    }
    let slice = &bytes[pak_offset as usize..(pak_offset + pak_size) as usize];
    if hash::sha256_bytes(slice) != expected_sha {
        return Err("E_BUNDLE_CORRUPT: container SHA-256 checksum mismatch".to_string());
    }
    parse_pak_body(slice.to_vec(), verify, max_entry_bytes)
}

fn parse_pak_body(
    pak_body: Vec<u8>,
    verify: VerifyMode,
    max_entry_bytes: usize,
) -> Result<PakArchive, String> {
    if pak_body.len() < PAK_HEADER_SIZE || &pak_body[0..10] != PAK_MAGIC {
        return Err("E_BUNDLE_CORRUPT: invalid ASKANE-PAK magic header".to_string());
    }
    let version = u16::from_le_bytes(pak_body[10..12].try_into().unwrap());
    if version != PAK_VERSION {
        return Err(format!(
            "E_BUNDLE_CORRUPT: unsupported format version {}",
            version
        ));
    }
    let flags = u32::from_le_bytes(pak_body[12..16].try_into().unwrap());
    let entry_count = u64::from_le_bytes(pak_body[16..24].try_into().unwrap());
    let toc_offset = u64::from_le_bytes(pak_body[24..32].try_into().unwrap());
    let toc_size = u64::from_le_bytes(pak_body[32..40].try_into().unwrap());
    let data_size = u64::from_le_bytes(pak_body[40..48].try_into().unwrap());
    let expected_toc_sha: &[u8] = &pak_body[48..80];

    if entry_count > MAX_PAK_ENTRIES as u64 {
        return Err("E_BUNDLE_CORRUPT: entry count exceeds limit".to_string());
    }
    if toc_offset != PAK_HEADER_SIZE as u64 + data_size
        || toc_offset.checked_add(toc_size) != Some(pak_body.len() as u64)
    {
        return Err("E_BUNDLE_CORRUPT: malformed TOC offset or size".to_string());
    }
    let toc = &pak_body[toc_offset as usize..(toc_offset + toc_size) as usize];
    if &hash::sha256_bytes(toc)[..] != expected_toc_sha {
        return Err("E_BUNDLE_CORRUPT: TOC SHA-256 checksum mismatch".to_string());
    }

    let mut pos = 0usize;
    let mut entries = Vec::with_capacity(entry_count as usize);
    let mut by_lower_key = HashMap::with_capacity(entry_count as usize);
    let mut entry_script: Option<String> = None;
    let mut total_uncompressed: u64 = 0;

    for idx in 0..entry_count as usize {
        if pos + 2 > toc.len() {
            return Err("E_BUNDLE_CORRUPT: truncated TOC entry name length".to_string());
        }
        let nlen = u16::from_le_bytes([toc[pos], toc[pos + 1]]) as usize;
        pos += 2;
        if nlen == 0 || pos + nlen + 60 > toc.len() {
            return Err("E_BUNDLE_CORRUPT: truncated TOC record".to_string());
        }
        let raw_name = std::str::from_utf8(&toc[pos..pos + nlen])
            .map_err(|_| "E_BUNDLE_CORRUPT: invalid UTF-8 entry name".to_string())?;
        pos += nlen;
        let name = validate_pak_entry_name(raw_name)?;

        let method = PakMethod::from_u8(toc[pos])?;
        let kind = PakEntryKind::from_u8(toc[pos + 1])?;
        // [pos+2..pos+4] reserved flags
        let data_offset = u64::from_le_bytes(toc[pos + 4..pos + 12].try_into().unwrap());
        let compressed_size = u64::from_le_bytes(toc[pos + 12..pos + 20].try_into().unwrap());
        let uncompressed_size = u64::from_le_bytes(toc[pos + 20..pos + 28].try_into().unwrap());
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(&toc[pos + 28..pos + 60]);
        pos += 60;

        if uncompressed_size > max_entry_bytes as u64 {
            return Err(format!(
                "E_BUNDLE_CORRUPT: entry '{}' size {} exceeds limit {}",
                name, uncompressed_size, max_entry_bytes
            ));
        }
        total_uncompressed = total_uncompressed
            .checked_add(uncompressed_size)
            .ok_or_else(|| "E_BUNDLE_CORRUPT: size overflow".to_string())?;
        if total_uncompressed > MAX_TOTAL_UNCOMPRESSED {
            return Err("E_BUNDLE_CORRUPT: cumulative bundle size exceeds 512 MiB limit".to_string());
        }
        if data_offset < PAK_HEADER_SIZE as u64
            || data_offset
                .checked_add(compressed_size)
                .is_none_or(|end| end > toc_offset)
        {
            return Err(format!(
                "E_BUNDLE_CORRUPT: entry '{}' data range out of bounds",
                name
            ));
        }

        let lower = name.to_lowercase();
        if by_lower_key.insert(lower, idx).is_some() {
            return Err(format!(
                "E_BUNDLE_CORRUPT: duplicate or case-conflicting entry '{}'",
                name
            ));
        }
        if kind == PakEntryKind::Entrypoint && entry_script.is_none() {
            entry_script = Some(name.clone());
        }
        entries.push(PakEntryMeta {
            name,
            kind,
            method,
            data_offset,
            compressed_size,
            uncompressed_size,
            sha256,
        });
    }
    if pos != toc.len() {
        return Err("E_BUNDLE_CORRUPT: trailing bytes in TOC".to_string());
    }

    let archive = PakArchive {
        data: Rc::new(pak_body),
        flags,
        entry_script,
        entries,
        by_lower_key,
    };

    if verify == VerifyMode::Full {
        for m in &archive.entries {
            let _ = archive.read_entry_meta(m)?;
        }
    }

    Ok(archive)
}

impl PakArchive {
    pub fn find(&self, logical_rel: &str) -> Option<&PakEntryMeta> {
        let lower = normalize_nfc(logical_rel).to_lowercase();
        self.by_lower_key.get(&lower).map(|&i| &self.entries[i])
    }

    pub fn read_entry_meta(&self, meta: &PakEntryMeta) -> Result<Vec<u8>, String> {
        let start = meta.data_offset as usize;
        let end = start + meta.compressed_size as usize;
        let raw_slice = &self.data[start..end];
        let out = match meta.method {
            PakMethod::Stored => {
                if raw_slice.len() as u64 != meta.uncompressed_size {
                    return Err(format!(
                        "E_BUNDLE_CORRUPT: stored size mismatch on '{}'",
                        meta.name
                    ));
                }
                raw_slice.to_vec()
            }
            PakMethod::Deflate => {
                let dec = compress::inflate_raw(
                    raw_slice,
                    meta.uncompressed_size as usize,
                    None,
                )
                .map_err(|e| format!("E_BUNDLE_CORRUPT: inflate '{}' failed: {}", meta.name, e))?;
                if dec.len() as u64 != meta.uncompressed_size {
                    return Err(format!(
                        "E_BUNDLE_CORRUPT: inflated size mismatch on '{}'",
                        meta.name
                    ));
                }
                dec
            }
        };
        if hash::sha256_bytes(&out) != meta.sha256 {
            return Err(format!(
                "E_BUNDLE_CORRUPT: SHA-256 checksum mismatch on '{}'",
                meta.name
            ));
        }
        Ok(out)
    }

    pub fn read_logical(&self, raw_rel: &str) -> Result<Option<Vec<u8>>, String> {
        let Ok(norm) = logical_normalize_rel(raw_rel) else {
            return Ok(None);
        };
        match self.find(&norm) {
            Some(meta) => Ok(Some(self.read_entry_meta(meta)?)),
            None => Ok(None),
        }
    }

    pub fn has_logical(&self, raw_rel: &str) -> bool {
        logical_normalize_rel(raw_rel)
            .ok()
            .and_then(|n| self.find(&n))
            .is_some()
    }

    pub fn is_dir(&self, raw_rel: &str) -> bool {
        let clean = match raw_rel.trim() {
            "" | "." | "./" => String::new(),
            other => match logical_normalize_rel(other) {
                Ok(n) => format!("{}/", n.to_lowercase()),
                Err(_) => return false,
            },
        };
        if clean.is_empty() {
            return !self.entries.is_empty();
        }
        self.entries
            .iter()
            .any(|e| e.name.to_lowercase().starts_with(&clean))
    }

    pub fn list_dir(&self, raw_rel: &str) -> Vec<String> {
        let prefix = match raw_rel.trim() {
            "" | "." | "./" => String::new(),
            other => match logical_normalize_rel(other) {
                Ok(n) => format!("{}/", n.to_lowercase()),
                Err(_) => return Vec::new(),
            },
        };
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for e in &self.entries {
            let low = e.name.to_lowercase();
            let rest_start = if prefix.is_empty() {
                0
            } else if low.starts_with(&prefix) {
                prefix.len()
            } else {
                continue;
            };
            let rest = &e.name[rest_start..];
            let child = rest.split('/').next().unwrap_or("");
            if !child.is_empty() && seen.insert(child.to_lowercase()) {
                out.push(child.to_string());
            }
        }
        out.sort();
        out
    }
}

pub fn embed_pak_into_executable(
    out_exe: &Path,
    pak_with_trailer: &[u8],
) -> Result<(), String> {
    if pak_with_trailer.len() < PAK_HEADER_SIZE + PAK_TRAILER_SIZE {
        return Err("E_BUNDLE_CORRUPT: pak buffer too small to embed".to_string());
    }
    let pak_body = &pak_with_trailer[..pak_with_trailer.len() - PAK_TRAILER_SIZE];
    let host_exe = env::current_exe().map_err(|e| format!("current_exe failed: {}", e))?;
    let host_bytes =
        fs::read(&host_exe).map_err(|e| format!("Failed to read host binary '{}': {}", host_exe.display(), e))?;
    let clean_host_len = match find_trailer_slice(&host_bytes) {
        Some((off, _, _)) => off as usize,
        None => host_bytes.len(),
    };
    let clean_host = &host_bytes[..clean_host_len];
    let pak_offset = clean_host.len() as u64;
    let mut combined = Vec::with_capacity(clean_host.len() + pak_body.len() + PAK_TRAILER_SIZE);
    combined.extend_from_slice(clean_host);
    combined.extend_from_slice(pak_body);
    let final_bytes = append_eof_trailer(combined, pak_offset)?;

    if let Some(parent) = out_exe.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent dir '{}': {}", parent.display(), e))?;
    }
    let mut f = File::create(out_exe)
        .map_err(|e| format!("Failed to create embedded binary '{}': {}", out_exe.display(), e))?;
    f.write_all(&final_bytes)
        .map_err(|e| format!("Failed to write embedded binary '{}': {}", out_exe.display(), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mut perms) = f.metadata().map(|m| m.permissions()) {
            perms.set_mode(0o755);
            let _ = fs::set_permissions(out_exe, perms);
        }
    }
    Ok(())
}

pub fn try_load_embedded_from_current_exe(verify: VerifyMode) -> Result<Option<PakArchive>, String> {
    let exe = match env::current_exe() {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    let mut f = match File::open(&exe) {
        Ok(f) => f,
        Err(_) => return Ok(None),
    };
    let len = match f.metadata().map(|m| m.len()) {
        Ok(l) => l,
        Err(_) => return Ok(None),
    };
    if len < (PAK_HEADER_SIZE + PAK_TRAILER_SIZE) as u64 {
        return Ok(None);
    }
    if f.seek(SeekFrom::End(-(PAK_TRAILER_SIZE as i64))).is_err() {
        return Ok(None);
    }
    let mut trailer = [0u8; PAK_TRAILER_SIZE];
    if f.read_exact(&mut trailer).is_err() {
        return Ok(None);
    }
    if &trailer[48..64] != PAK_TRAILER_MAGIC {
        return Ok(None);
    }
    Ok(Some(parse_from_file(&exe, verify, DEFAULT_MAX_ENTRY_BYTES)?))
}
