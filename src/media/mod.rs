pub mod anim;
pub mod atlas;
pub mod cache;
pub mod color;
pub mod compose;
pub mod formats;
pub mod image;
pub mod ops;
pub mod sample;

use std::sync::Arc;

use crate::budget::{BudgetCore, TempCharge};

pub const MAX_DIM: u32 = 8192;
pub const MAX_BMP_DIM: u32 = 4096;
pub const MAX_PIXELS: usize = 33_554_432;
pub const MAX_MEDIA_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_GIF_FRAMES: usize = 4096;
pub const MAX_ATLAS_CELLS: usize = 4096;
pub const MAX_MIP_LEVELS: usize = 16;
pub const MAX_ASSET_HANDLES: usize = 4096;
pub const DEFAULT_ASSET_CACHE_BYTES: usize = 128 * 1024 * 1024;

#[inline]
pub fn err_limit(msg: impl AsRef<str>) -> String {
    format!("E_MEDIA_LIMIT: {}", msg.as_ref())
}

#[inline]
pub fn err_unsupported(msg: impl AsRef<str>) -> String {
    format!("E_MEDIA_UNSUPPORTED: {}", msg.as_ref())
}

#[inline]
pub fn err_corrupt(msg: impl AsRef<str>) -> String {
    format!("E_MEDIA_CORRUPT: {}", msg.as_ref())
}

#[inline]
pub fn err_bad_opt(msg: impl AsRef<str>) -> String {
    format!("E_MEDIA_BAD_OPT: {}", msg.as_ref())
}

#[allow(dead_code)]
#[inline]
pub fn err_nondet(msg: impl AsRef<str>) -> String {
    format!("E_MEDIA_NONDET: {}", msg.as_ref())
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    pub fn from_parts(parts: &[&[u8]]) -> Self {
        let mut total = 0usize;
        for p in parts {
            total = total.saturating_add(p.len());
        }
        let mut buf = Vec::with_capacity(total);
        for p in parts {
            buf.extend_from_slice(p);
        }
        Self(crate::modules::hash::sha256_bytes(&buf))
    }

    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(64);
        for b in self.0 {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0F) as usize] as char);
        }
        out
    }

    pub fn short_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(8);
        for b in &self.0[..4] {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0F) as usize] as char);
        }
        out
    }
}

impl std::fmt::Debug for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Digest({})", self.short_hex())
    }
}

#[derive(Debug)]
pub struct MediaBudgetGuard {
    core: Option<Arc<BudgetCore>>,
    bytes: u64,
}

impl MediaBudgetGuard {
    pub fn charge(core: Option<&Arc<BudgetCore>>, bytes: usize) -> Result<Self, String> {
        if bytes > MAX_MEDIA_BYTES {
            return Err(err_limit(format!(
                "media allocation {} bytes exceeds {} byte limit",
                bytes, MAX_MEDIA_BYTES
            )));
        }
        let b = bytes as u64;
        if let Some(c) = core {
            c.charge_alloc(b)?;
            Ok(Self {
                core: Some(Arc::clone(c)),
                bytes: b,
            })
        } else {
            Ok(Self {
                core: None,
                bytes: b,
            })
        }
    }

    #[inline]
    pub fn bytes(&self) -> usize {
        self.bytes as usize
    }
}

impl Drop for MediaBudgetGuard {
    fn drop(&mut self) {
        if self.bytes > 0 {
            if let Some(c) = &self.core {
                c.refund_alloc(self.bytes);
            }
            self.bytes = 0;
        }
    }
}

#[inline]
pub fn temp_budget_guard<'a>(
    core: Option<&'a Arc<BudgetCore>>,
    bytes: usize,
) -> Result<Option<TempCharge<'a>>, String> {
    if let Some(c) = core {
        Ok(Some(c.temp_charge(bytes as u64)?))
    } else {
        Ok(None)
    }
}

pub fn validate_dims(width: u32, height: u32, max_dim: u32) -> Result<usize, String> {
    if width == 0 || height == 0 || width > max_dim || height > max_dim {
        return Err(err_limit(format!(
            "image dimensions {}x{} out of 1..={} bounds",
            width, height, max_dim
        )));
    }
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| err_limit("image pixel count overflow"))?;
    if pixels > MAX_PIXELS {
        return Err(err_limit(format!(
            "image pixel count {} exceeds {} limit",
            pixels, MAX_PIXELS
        )));
    }
    Ok(pixels)
}

#[inline]
pub fn check_deadline(check: Option<&dyn Fn() -> Result<(), String>>) -> Result<(), String> {
    if let Some(f) = check {
        f()?;
    }
    Ok(())
}

pub fn reset_media_state() {
    cache::reset_cache();
}
