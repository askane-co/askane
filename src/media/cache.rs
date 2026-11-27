use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;

use crate::media::image::{
    ColorSpace, DecodeOpts, Filter, MediaImage, PixelFormat, SourceFormat, TextureData, WrapMode,
    decode_image_bytes_with_hint,
};
use crate::media::{
    DEFAULT_ASSET_CACHE_BYTES, MAX_ASSET_HANDLES, MAX_SOURCE_BYTES, MediaBudgetGuard, err_bad_opt,
    err_corrupt, err_limit, temp_budget_guard,
};
use crate::modules::archive::{ZipEntryMeta, parse_zip_cd, read_zip_entry_bytes};
use crate::value::Value;
use crate::vm::VM;

const MAX_BUNDLE_ENTRIES: usize = 8192;
const MAX_BUNDLE_TOTAL_UNCOMPRESSED: usize = 128 * 1024 * 1024;
const MAX_ASSET_CACHE_CONFIG_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct AssetManifestHint {
    pub format: Option<PixelFormat>,
    pub color_space: Option<ColorSpace>,
    pub mipmaps: Option<bool>,
    pub filter: Option<Filter>,
    pub wrap: Option<WrapMode>,
}

struct MountedBundle {
    id: i64,
    prefix: String,
    data: Rc<Vec<u8>>,
    by_name: HashMap<String, usize>,
    metas: Vec<ZipEntryMeta>,
    hints: HashMap<String, AssetManifestHint>,
    _budget: MediaBudgetGuard,
}

struct CacheEntry {
    image: Rc<MediaImage>,
    bytes: usize,
    last_used: u64,
}

struct AssetRegistry {
    max_bytes: usize,
    used_bytes: usize,
    tick: u64,
    next_bundle_id: i64,
    cache: HashMap<String, CacheEntry>,
    bundles: Vec<MountedBundle>,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl Default for AssetRegistry {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_ASSET_CACHE_BYTES,
            used_bytes: 0,
            tick: 0,
            next_bundle_id: 1,
            cache: HashMap::new(),
            bundles: Vec::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }
}

thread_local! {
    static ASSET_REG: RefCell<AssetRegistry> = RefCell::new(AssetRegistry::default());
}

pub fn reset_cache() {
    crate::media::anim::reset_anims();
    ASSET_REG.with(|reg| {
        *reg.borrow_mut() = AssetRegistry::default();
    });
}

pub fn normalize_vpath(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err(err_bad_opt("asset path cannot be empty"));
    }
    if raw.bytes().any(|b| b == 0) {
        return Err(err_bad_opt("asset path contains NUL byte"));
    }
    let replaced = raw.replace('\\', "/");
    if replaced.starts_with('/') || (replaced.len() >= 2 && replaced.as_bytes()[1] == b':') {
        return Err(err_bad_opt(format!(
            "asset path '{}' cannot be absolute or drive-prefixed",
            raw
        )));
    }
    let mut parts = Vec::new();
    for seg in replaced.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(err_bad_opt(format!(
                "asset path '{}' cannot contain '..'",
                raw
            )));
        }
        parts.push(seg);
    }
    if parts.is_empty() {
        return Err(err_bad_opt(format!("invalid empty asset path '{}'", raw)));
    }
    Ok(parts.join("/"))
}

fn normalize_mount_prefix(raw: Option<&str>) -> Result<String, String> {
    match raw {
        None | Some("") | Some(".") | Some("/") => Ok(String::new()),
        Some(p) => {
            let norm = normalize_vpath(p)?;
            Ok(format!("{}/", norm))
        }
    }
}

fn parse_manifest_dict(val: &Value) -> Result<HashMap<String, AssetManifestHint>, String> {
    let mut out = HashMap::new();
    let Value::Dict(root_rc) = val else {
        return Err(err_corrupt("bundle manifest.toml must parse to a table"));
    };
    let root = root_rc.borrow();
    let Some(Value::Dict(assets_rc)) = root.get("assets") else {
        return Ok(out);
    };
    let assets_map = assets_rc.borrow();
    for (asset_path, cfg_val) in assets_map.iter() {
        let norm_key = normalize_vpath(asset_path)?;
        let Value::Dict(cfg_rc) = cfg_val else {
            return Err(err_corrupt(format!(
                "manifest [assets.\"{}\"] must be a table",
                asset_path
            )));
        };
        let cfg = cfg_rc.borrow();
        let mut hint = AssetManifestHint::default();
        for (k, v) in cfg.iter() {
            match k.as_str() {
                "format" => {
                    let s = v.as_str().ok_or_else(|| {
                        err_corrupt("manifest 'format' must be a string")
                    })?;
                    hint.format = Some(PixelFormat::from_name(s)?);
                }
                "color_space" => {
                    let s = v.as_str().ok_or_else(|| {
                        err_corrupt("manifest 'color_space' must be a string")
                    })?;
                    hint.color_space = Some(ColorSpace::from_name(s)?);
                }
                "mipmaps" => {
                    let Value::Bool(b) = v else {
                        return Err(err_corrupt("manifest 'mipmaps' must be a bool"));
                    };
                    hint.mipmaps = Some(*b);
                }
                "filter" => {
                    let s = v.as_str().ok_or_else(|| {
                        err_corrupt("manifest 'filter' must be a string")
                    })?;
                    hint.filter = Some(Filter::from_name(s)?);
                }
                "wrap" => {
                    let s = v.as_str().ok_or_else(|| {
                        err_corrupt("manifest 'wrap' must be a string")
                    })?;
                    hint.wrap = Some(WrapMode::from_name(s)?);
                }
                _ => {
                    // Forward compatibility in manifests ignores unknown metadata keys safely.
                }
            }
        }
        out.insert(norm_key, hint);
    }
    Ok(out)
}

fn read_limited_disk_file(
    vm: &mut VM,
    resolved: &PathBuf,
    label: &str,
    permanent_alloc: bool,
) -> Result<Vec<u8>, String> {
    let file = File::open(resolved)
        .map_err(|e| format!("assets: failed to open '{}': {}", label, e))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("assets: failed to stat '{}': {}", label, e))?;
    if !meta.is_file() {
        return Err(err_bad_opt(format!(
            "assets: '{}' is not a regular file",
            label
        )));
    }
    if meta.len() > MAX_SOURCE_BYTES as u64 {
        return Err(err_limit(format!(
            "assets: '{}' ({} bytes) exceeds {} byte limit",
            label,
            meta.len(),
            MAX_SOURCE_BYTES
        )));
    }
    let expected = meta.len() as usize;
    vm.charge_io(expected)?;
    let _tmp_guard = if permanent_alloc {
        vm.charge_alloc(expected)?;
        None
    } else {
        temp_budget_guard(Some(vm.budget_core()), expected)?
    };
    let mut buf = Vec::with_capacity(expected);
    file.take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("assets: failed to read '{}': {}", label, e))?;
    if buf.len() > MAX_SOURCE_BYTES {
        return Err(err_limit(format!(
            "assets: '{}' exceeds {} byte limit",
            label, MAX_SOURCE_BYTES
        )));
    }
    Ok(buf)
}

pub fn mount_bundle_bytes(
    vm: &mut VM,
    zip_bytes: Vec<u8>,
    prefix_opt: Option<&str>,
) -> Result<i64, String> {
    if zip_bytes.len() > MAX_SOURCE_BYTES {
        return Err(err_limit(format!(
            "bundle size {} bytes exceeds {} byte limit",
            zip_bytes.len(),
            MAX_SOURCE_BYTES
        )));
    }
    let prefix = normalize_mount_prefix(prefix_opt)?;
    let metas = parse_zip_cd(vm, &zip_bytes)?;
    if metas.len() > MAX_BUNDLE_ENTRIES {
        return Err(err_limit(format!(
            "bundle entry count {} exceeds {} limit",
            metas.len(),
            MAX_BUNDLE_ENTRIES
        )));
    }

    let mut by_name = HashMap::with_capacity(metas.len());
    let mut total_uncomp = 0usize;
    let mut manifest_idx: Option<usize> = None;

    for (idx, m) in metas.iter().enumerate() {
        if m.name.ends_with('/') {
            continue;
        }
        let norm = normalize_vpath(&m.name)?;
        total_uncomp = total_uncomp
            .checked_add(m.uncomp_size)
            .ok_or_else(|| err_limit("bundle uncompressed size overflow"))?;
        if total_uncomp > MAX_BUNDLE_TOTAL_UNCOMPRESSED {
            return Err(err_limit(format!(
                "bundle total uncompressed size exceeds {} bytes",
                MAX_BUNDLE_TOTAL_UNCOMPRESSED
            )));
        }
        if by_name.insert(norm.clone(), idx).is_some() {
            return Err(err_corrupt(format!(
                "duplicate bundle entry '{}'",
                norm
            )));
        }
        if norm == "__askane__/manifest.toml" {
            manifest_idx = Some(idx);
        }
    }

    let mut hints = HashMap::new();
    if let Some(m_idx) = manifest_idx {
        let raw_manifest =
            read_zip_entry_bytes(vm, &zip_bytes, &metas[m_idx], 1024 * 1024)?;
        let text = std::str::from_utf8(&raw_manifest)
            .map_err(|_| err_corrupt("bundle manifest.toml is not valid UTF-8"))?;
        let parsed = crate::modules::toml::parse_toml_str(vm, text)?;
        hints = parse_manifest_dict(&parsed)?;
    }

    let bundle_guard = MediaBudgetGuard::charge(Some(vm.budget_core()), zip_bytes.len())?;
    let rc_bytes = Rc::new(zip_bytes);
    ASSET_REG.with(|reg| {
        let mut r = reg.borrow_mut();
        if r.bundles.len() >= 64 {
            return Err(err_limit("maximum mounted asset bundles (64) exceeded"));
        }
        let id = r.next_bundle_id;
        r.next_bundle_id += 1;
        r.bundles.push(MountedBundle {
            id,
            prefix,
            data: rc_bytes,
            by_name,
            metas,
            hints,
            _budget: bundle_guard,
        });
        Ok(id)
    })
}

pub fn mount_bundle_file(
    vm: &mut VM,
    path: &str,
    prefix_opt: Option<&str>,
) -> Result<i64, String> {
    let resolved = vm.resolve_read_path(path)?;
    let bytes = read_limited_disk_file(vm, &resolved, path, false)?;
    mount_bundle_bytes(vm, bytes, prefix_opt)
}

pub fn unmount_bundle(id_or_prefix: &Value) -> Result<bool, String> {
    ASSET_REG.with(|reg| {
        let mut r = reg.borrow_mut();
        let before = r.bundles.len();
        match id_or_prefix {
            Value::Int(id) => r.bundles.retain(|b| b.id != *id),
            Value::Str(p) => {
                let target_pref = normalize_mount_prefix(Some(p.as_str()))?;
                r.bundles.retain(|b| b.prefix != target_pref);
            }
            other => {
                return Err(err_bad_opt(format!(
                    "assets.unmount() expects bundle id or mount prefix, got '{}'",
                    other.type_name()
                )));
            }
        }
        Ok(r.bundles.len() != before)
    })
}

fn find_in_bundles(
    vpath: &str,
) -> Option<(Rc<Vec<u8>>, ZipEntryMeta, Option<AssetManifestHint>, String)> {
    ASSET_REG.with(|reg| {
        let r = reg.borrow();
        // Search latest mounted first so overlays work deterministically.
        for b in r.bundles.iter().rev() {
            let rel = if b.prefix.is_empty() {
                vpath
            } else if let Some(stripped) = vpath.strip_prefix(&b.prefix) {
                stripped
            } else {
                continue;
            };
            if let Some(&idx) = b.by_name.get(rel) {
                let meta = b.metas[idx].clone();
                let hint = b.hints.get(rel).cloned();
                let source_key = format!("bundle#{}:{}", b.id, rel);
                return Some((Rc::clone(&b.data), meta, hint, source_key));
            }
        }
        None
    })
}

fn read_asset_bytes_inner(
    vm: &mut VM,
    path: &str,
    permanent_alloc: bool,
) -> Result<(Vec<u8>, Option<AssetManifestHint>, String), String> {
    if let Ok(vpath) = normalize_vpath(path) {
        if let Some((zip_rc, meta, hint, source_key)) = find_in_bundles(&vpath) {
            let bytes = read_zip_entry_bytes(vm, &zip_rc, &meta, MAX_SOURCE_BYTES)?;
            return Ok((bytes, hint, source_key));
        }
    }
    let resolved = vm.resolve_read_path(path)?;
    let canon = std::fs::canonicalize(&resolved).unwrap_or_else(|_| resolved.clone());
    let source_key = format!("file:{}", canon.to_string_lossy());
    let bytes = read_limited_disk_file(vm, &resolved, path, permanent_alloc)?;
    Ok((bytes, None, source_key))
}

pub fn read_asset_bytes(
    vm: &mut VM,
    path: &str,
) -> Result<(Vec<u8>, Option<AssetManifestHint>, String), String> {
    read_asset_bytes_inner(vm, path, true)
}

fn evict_lru_to_fit(reg: &mut AssetRegistry, incoming_bytes: usize) -> Result<(), String> {
    if incoming_bytes > reg.max_bytes && reg.max_bytes > 0 {
        return Err(err_limit(format!(
            "asset size {} bytes exceeds asset cache budget {} bytes",
            incoming_bytes, reg.max_bytes
        )));
    }
    while reg.used_bytes.saturating_add(incoming_bytes) > reg.max_bytes {
        let candidate = reg
            .cache
            .iter()
            .filter(|(_, e)| Rc::strong_count(&e.image) == 1)
            .min_by(|(ka, ea), (kb, eb)| {
                ea.last_used.cmp(&eb.last_used).then_with(|| ka.cmp(kb))
            })
            .map(|(k, _)| k.clone());
        let Some(evict_key) = candidate else {
            break;
        };
        if let Some(removed) = reg.cache.remove(&evict_key) {
            reg.used_bytes = reg.used_bytes.saturating_sub(removed.bytes);
            reg.evictions += 1;
        }
    }
    Ok(())
}

pub fn run_cache_gc() -> usize {
    ASSET_REG.with(|reg| {
        let mut r = reg.borrow_mut();
        let evict_keys: Vec<String> = r
            .cache
            .iter()
            .filter(|(_, e)| Rc::strong_count(&e.image) == 1)
            .map(|(k, _)| k.clone())
            .collect();
        let count = evict_keys.len();
        for k in evict_keys {
            if let Some(removed) = r.cache.remove(&k) {
                r.used_bytes = r.used_bytes.saturating_sub(removed.bytes);
                r.evictions += 1;
            }
        }
        count
    })
}

pub fn set_cache_budget(max_bytes: usize) -> Result<(), String> {
    if max_bytes > MAX_ASSET_CACHE_CONFIG_BYTES {
        return Err(err_limit(format!(
            "asset cache budget {} exceeds {} byte cap",
            max_bytes, MAX_ASSET_CACHE_CONFIG_BYTES
        )));
    }
    ASSET_REG.with(|reg| {
        let mut r = reg.borrow_mut();
        r.max_bytes = max_bytes;
        evict_lru_to_fit(&mut r, 0)
    })
}

pub fn cache_stats() -> (usize, usize, usize, u64, u64, u64, usize) {
    ASSET_REG.with(|reg| {
        let r = reg.borrow();
        (
            r.used_bytes,
            r.max_bytes,
            r.cache.len(),
            r.hits,
            r.misses,
            r.evictions,
            r.bundles.len(),
        )
    })
}

pub fn bundle_list_entries() -> Vec<String> {
    ASSET_REG.with(|reg| {
        let r = reg.borrow();
        let mut out = Vec::new();
        for b in &r.bundles {
            for k in b.by_name.keys() {
                out.push(format!("{}{}", b.prefix, k));
            }
        }
        out.sort();
        out.dedup();
        out
    })
}

fn cache_key_for(source_key: &str, opts: &DecodeOpts) -> String {
    format!(
        "{}|f={}|c={}|m={}|crc={}",
        source_key,
        opts.format.map(|f| f.name()).unwrap_or("auto"),
        opts.color_space.map(|c| c.name()).unwrap_or("auto"),
        opts.mipmaps.map(|b| if b { "1" } else { "0" }).unwrap_or("auto"),
        if opts.strict_crc { "1" } else { "0" }
    )
}

fn ext_hint(path: &str) -> Option<SourceFormat> {
    let clean = path.split(['/', '\\']).last().unwrap_or(path);
    let (_, ext) = clean.rsplit_once('.')?;
    SourceFormat::from_ext(ext)
}

pub fn load_image_cached(
    vm: &mut VM,
    path: &str,
    explicit_opts: &DecodeOpts,
    bypass_cache: bool,
) -> Result<(Rc<MediaImage>, Option<AssetManifestHint>), String> {
    let (bytes, hint, source_key) = read_asset_bytes_inner(vm, path, false)?;

    let mut effective_opts = explicit_opts.clone();
    if let Some(ref h) = hint {
        if effective_opts.format.is_none() {
            effective_opts.format = h.format;
        }
        if effective_opts.color_space.is_none() {
            effective_opts.color_space = h.color_space;
        }
        if effective_opts.mipmaps.is_none() {
            effective_opts.mipmaps = h.mipmaps;
        }
    }

    let ckey = cache_key_for(&source_key, &effective_opts);
    if !bypass_cache {
        let hit = ASSET_REG.with(|reg| {
            let mut r = reg.borrow_mut();
            r.tick += 1;
            let tick = r.tick;
            let found = if let Some(entry) = r.cache.get_mut(&ckey) {
                entry.last_used = tick;
                Some(Rc::clone(&entry.image))
            } else {
                None
            };
            if found.is_some() {
                r.hits += 1;
            }
            found
        });
        if let Some(img) = hit {
            return Ok((img, hint));
        }
    }

    let timeout_check = || vm.check_timeout();
    let img = decode_image_bytes_with_hint(
        &bytes,
        ext_hint(path),
        &effective_opts,
        Some(vm.budget_core()),
        Some(&timeout_check),
    )?;

    if !bypass_cache {
        let img_bytes = img.byte_len();
        ASSET_REG.with(|reg| {
            let mut r = reg.borrow_mut();
            r.misses += 1;
            if r.cache.len() >= MAX_ASSET_HANDLES {
                let evict_keys: Vec<String> = r
                    .cache
                    .iter()
                    .filter(|(_, e)| Rc::strong_count(&e.image) == 1)
                    .map(|(k, _)| k.clone())
                    .collect();
                for k in evict_keys {
                    if let Some(removed) = r.cache.remove(&k) {
                        r.used_bytes = r.used_bytes.saturating_sub(removed.bytes);
                        r.evictions += 1;
                    }
                }
            }
            if r.max_bytes > 0 && img_bytes <= r.max_bytes && r.cache.len() < MAX_ASSET_HANDLES {
                let _ = evict_lru_to_fit(&mut r, img_bytes);
                if r.used_bytes.saturating_add(img_bytes) <= r.max_bytes {
                    r.tick += 1;
                    let tick = r.tick;
                    r.used_bytes += img_bytes;
                    r.cache.insert(
                        ckey,
                        CacheEntry {
                            image: Rc::clone(&img),
                            bytes: img_bytes,
                            last_used: tick,
                        },
                    );
                }
            }
        });
    }
    Ok((img, hint))
}

pub fn load_texture_cached(
    vm: &mut VM,
    path: &str,
    explicit_opts: &DecodeOpts,
    wrap_u_override: Option<WrapMode>,
    wrap_v_override: Option<WrapMode>,
    filter_override: Option<Filter>,
    srgb_override: Option<bool>,
) -> Result<Rc<TextureData>, String> {
    let (img, hint) = load_image_cached(vm, path, explicit_opts, false)?;
    let wrap_u = wrap_u_override
        .or_else(|| hint.as_ref().and_then(|h| h.wrap))
        .unwrap_or(WrapMode::Clamp);
    let wrap_v = wrap_v_override
        .or_else(|| hint.as_ref().and_then(|h| h.wrap))
        .unwrap_or(WrapMode::Clamp);
    let filter = filter_override
        .or_else(|| hint.as_ref().and_then(|h| h.filter))
        .unwrap_or(Filter::Nearest);
    let srgb = srgb_override.unwrap_or(img.color_space == ColorSpace::Srgb);
    let timeout_check = || vm.check_timeout();
    TextureData::new(
        img,
        wrap_u,
        wrap_v,
        filter,
        srgb,
        Some(vm.budget_core()),
        Some(&timeout_check),
    )
}
