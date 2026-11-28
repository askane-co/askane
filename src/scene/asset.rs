use crate::media::cache::{AssetManifestHint, normalize_vpath, read_asset_bytes_temp};
use crate::media::{err_bad_opt, err_limit};
use crate::vm::VM;

pub const MAX_ASSET_PATH_BYTES: usize = 4096;
pub const MAX_ASSET_SEGMENTS: usize = 32;
pub const MAX_ASSET_BYTES: usize = 256 * 1024 * 1024;
#[allow(dead_code)]
pub const MAX_ASSET_HANDLES: usize = 4096;

/// Validated sandbox-safe relative or asset reference path. Inner string is private so all paths pass `AssetRef::parse`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetRef(String);

impl AssetRef {
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() || raw.len() > MAX_ASSET_PATH_BYTES {
            return Err(err_bad_opt(format!(
                "asset reference length {} must be in 1..={}",
                raw.len(),
                MAX_ASSET_PATH_BYTES
            )));
        }
        if raw.bytes().any(|b| b == 0) {
            return Err(err_bad_opt("asset reference cannot contain NUL byte"));
        }
        let norm = normalize_vpath(raw)?;
        let seg_count = norm.split('/').count();
        if seg_count > MAX_ASSET_SEGMENTS {
            return Err(err_bad_opt(format!(
                "asset reference '{}' exceeds {} path segments",
                raw, MAX_ASSET_SEGMENTS
            )));
        }
        Ok(Self(norm))
    }

    pub fn parse_allow_sandbox_root(raw: &str, vm: &VM) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.len() > MAX_ASSET_PATH_BYTES {
            return Err(err_bad_opt("asset path length invalid"));
        }
        if trimmed.bytes().any(|b| b == 0) {
            return Err(err_bad_opt("asset path cannot contain NUL byte"));
        }
        if let Ok(ar) = Self::parse(trimmed) {
            return Ok(ar);
        }
        // Only fall back to explicit `--allow-read` / sandbox resolution if allowed by VM permissions.
        let _ = vm.resolve_read_path(trimmed)?;
        Ok(Self(trimmed.replace('\\', "/")))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Resolves a relative child URI against the parent asset path while blocking traversal (`..`), NUL, and absolute paths.
    pub fn join_relative(&self, child: &str) -> Result<Self, String> {
        let child_trim = child.trim();
        if child_trim.is_empty() || child_trim.len() > MAX_ASSET_PATH_BYTES {
            return Err(err_bad_opt("relative asset URI length invalid"));
        }
        if child_trim.bytes().any(|b| b == 0) {
            return Err(err_bad_opt("relative asset URI contains NUL byte"));
        }
        let clean_child = child_trim.replace('\\', "/");
        if clean_child.starts_with('/')
            || clean_child.starts_with("\\\\")
            || (clean_child.len() >= 2 && clean_child.as_bytes()[1] == b':')
            || clean_child.contains("://")
        {
            return Err(err_bad_opt(format!(
                "external or absolute child asset URI '{}' is blocked",
                child
            )));
        }
        let parent_dir = match self.0.rfind('/') {
            Some(idx) => &self.0[..idx],
            None => "",
        };
        let mut stack: Vec<&str> = if parent_dir.is_empty() {
            Vec::new()
        } else {
            parent_dir.split('/').collect()
        };
        for seg in clean_child.split('/') {
            if seg.is_empty() || seg == "." {
                continue;
            }
            if seg == ".." {
                if stack.is_empty() {
                    return Err(err_bad_opt(format!(
                        "asset path traversal '{}' escapes root",
                        child
                    )));
                }
                stack.pop();
            } else {
                stack.push(seg);
            }
        }
        if stack.is_empty() || stack.len() > MAX_ASSET_SEGMENTS {
            return Err(err_bad_opt(format!(
                "invalid relative asset path resolution for '{}'",
                child
            )));
        }
        Ok(Self(stack.join("/")))
    }
}

pub fn read_asset_ref_bytes(
    vm: &mut VM,
    asset: &AssetRef,
) -> Result<(Vec<u8>, Option<AssetManifestHint>, String), String> {
    let (bytes, hint, key) = read_asset_bytes_temp(vm, asset.as_str())?;
    if bytes.len() > MAX_ASSET_BYTES {
        return Err(err_limit(format!(
            "asset '{}' ({} bytes) exceeds 256 MiB limit",
            asset.as_str(),
            bytes.len()
        )));
    }
    Ok((bytes, hint, key))
}

pub fn read_path_checked_temp(
    vm: &mut VM,
    raw_path: &str,
) -> Result<(AssetRef, Vec<u8>, Option<AssetManifestHint>, String), String> {
    let aref = AssetRef::parse_allow_sandbox_root(raw_path, vm)?;
    let (bytes, hint, key) = read_asset_ref_bytes(vm, &aref)?;
    Ok((aref, bytes, hint, key))
}
