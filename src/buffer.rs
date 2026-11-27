use std::sync::Weak;

use crate::budget::BudgetCore;
use crate::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elem {
    U8,
    I32,
    U32,
    F32,
}

impl Elem {
    #[inline]
    pub const fn width(self) -> usize {
        match self {
            Elem::U8 => 1,
            Elem::I32 | Elem::U32 | Elem::F32 => 4,
        }
    }

    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Elem::U8 => "u8",
            Elem::I32 => "i32",
            Elem::U32 => "u32",
            Elem::F32 => "f32",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "u8" | "byte" => Ok(Elem::U8),
            "i32" | "int32" => Ok(Elem::I32),
            "u32" | "uint32" | "rgba" | "argb" => Ok(Elem::U32),
            "f32" | "float32" => Ok(Elem::F32),
            other => Err(format!(
                "Unknown buffer element type '{}' (expected 'u8', 'i32', 'u32', or 'f32')",
                other
            )),
        }
    }
}

/// Safe typed byte-backed storage shared across buffers, images, audio, and 3D mesh attributes.
pub struct BufData {
    pub elem: Elem,
    pub len: usize,
    pub raw: Vec<u8>,
    pub charged: usize,
    pub budget: Option<Weak<BudgetCore>>,
}

impl Clone for BufData {
    fn clone(&self) -> Self {
        Self {
            elem: self.elem,
            len: self.len,
            raw: self.raw.clone(),
            charged: 0,
            budget: None,
        }
    }
}

impl Drop for BufData {
    fn drop(&mut self) {
        if self.charged > 0 {
            if let Some(ref w) = self.budget {
                if let Some(core) = w.upgrade() {
                    core.refund_alloc(self.charged as u64);
                }
            }
        }
    }
}

impl PartialEq for BufData {
    fn eq(&self, other: &Self) -> bool {
        self.elem == other.elem && self.len == other.len && self.raw == other.raw
    }
}

impl BufData {
    pub const MAX_BYTE_LEN: usize = 64 * 1024 * 1024;

    pub fn byte_len_for(elem: Elem, len: usize) -> Result<usize, String> {
        let bytes = len
            .checked_mul(elem.width())
            .ok_or_else(|| "Buffer size overflow".to_string())?;
        if bytes > Self::MAX_BYTE_LEN {
            return Err(format!(
                "Buffer exceeds 64 MiB limit (requested {} bytes)",
                bytes
            ));
        }
        Ok(bytes)
    }

    pub fn new_zeroed(
        elem: Elem,
        len: usize,
        budget: Option<std::sync::Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        let byte_len = Self::byte_len_for(elem, len)?;
        if let Some(ref b) = budget {
            b.charge_alloc(byte_len as u64)?;
        }
        let raw = vec![0u8; byte_len];
        Ok(Self {
            elem,
            len,
            raw,
            charged: if budget.is_some() { byte_len } else { 0 },
            budget: budget.map(|b| std::sync::Arc::downgrade(&b)),
        })
    }

    pub fn from_raw(
        elem: Elem,
        raw: Vec<u8>,
        charged: usize,
        budget: Option<Weak<BudgetCore>>,
    ) -> Result<Self, String> {
        if raw.len() > Self::MAX_BYTE_LEN {
            return Err(format!(
                "Buffer exceeds 64 MiB limit (got {} bytes)",
                raw.len()
            ));
        }
        let w = elem.width();
        if raw.len() % w != 0 {
            return Err(format!(
                "Byte length {} is not divisible by '{}' element width {}",
                raw.len(),
                elem.name(),
                w
            ));
        }
        Ok(Self {
            elem,
            len: raw.len() / w,
            raw,
            charged,
            budget,
        })
    }

    #[inline]
    pub fn byte_len(&self) -> usize {
        self.raw.len()
    }

    pub fn resolve_index(&self, idx: i64, ctx: &str) -> Result<usize, String> {
        let actual = if idx < 0 {
            (self.len as i128) + (idx as i128)
        } else {
            idx as i128
        };
        if actual < 0 || actual >= self.len as i128 {
            return Err(format!(
                "{}: index {} out of bounds for buffer of length {}",
                ctx, idx, self.len
            ));
        }
        Ok(actual as usize)
    }

    #[allow(dead_code)]
    pub fn read_u32_at(&self, idx: usize) -> Option<u32> {
        let off = idx.checked_mul(4)?;
        let chunk: [u8; 4] = self.raw.get(off..off + 4)?.try_into().ok()?;
        Some(u32::from_le_bytes(chunk))
    }

    #[allow(dead_code)]
    pub fn write_u32_at(&mut self, idx: usize, v: u32) -> bool {
        let Some(off) = idx.checked_mul(4) else {
            return false;
        };
        let Some(dst) = self.raw.get_mut(off..off + 4) else {
            return false;
        };
        dst.copy_from_slice(&v.to_le_bytes());
        true
    }

    pub fn get_value(&self, idx: usize) -> Result<Value, String> {
        if idx >= self.len {
            return Err(format!(
                "Buffer index {} out of bounds for length {}",
                idx, self.len
            ));
        }
        Ok(match self.elem {
            Elem::U8 => Value::Int(self.raw[idx] as i64),
            Elem::I32 => {
                let off = idx * 4;
                let chunk: [u8; 4] = self.raw[off..off + 4].try_into().unwrap();
                Value::Int(i32::from_le_bytes(chunk) as i64)
            }
            Elem::U32 => {
                let off = idx * 4;
                let chunk: [u8; 4] = self.raw[off..off + 4].try_into().unwrap();
                Value::Int(u32::from_le_bytes(chunk) as i64)
            }
            Elem::F32 => {
                let off = idx * 4;
                let chunk: [u8; 4] = self.raw[off..off + 4].try_into().unwrap();
                Value::Float(f32::from_le_bytes(chunk) as f64)
            }
        })
    }

    pub fn set_value(&mut self, idx: usize, val: &Value) -> Result<(), String> {
        if idx >= self.len {
            return Err(format!(
                "Buffer index {} out of bounds for length {}",
                idx, self.len
            ));
        }
        match self.elem {
            Elem::U8 => {
                let byte = value_to_u8(val)?;
                self.raw[idx] = byte;
            }
            Elem::I32 => {
                let v = value_to_i32(val)?;
                let off = idx * 4;
                self.raw[off..off + 4].copy_from_slice(&v.to_le_bytes());
            }
            Elem::U32 => {
                let v = value_to_u32(val)?;
                let off = idx * 4;
                self.raw[off..off + 4].copy_from_slice(&v.to_le_bytes());
            }
            Elem::F32 => {
                let v = value_to_f32(val)?;
                let off = idx * 4;
                self.raw[off..off + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        Ok(())
    }

    pub fn fill_value(&mut self, val: &Value) -> Result<(), String> {
        match self.elem {
            Elem::U8 => {
                let b = value_to_u8(val)?;
                self.raw.fill(b);
            }
            Elem::I32 => {
                let bytes = value_to_i32(val)?.to_le_bytes();
                for chunk in self.raw.chunks_exact_mut(4) {
                    chunk.copy_from_slice(&bytes);
                }
            }
            Elem::U32 => {
                let bytes = value_to_u32(val)?.to_le_bytes();
                for chunk in self.raw.chunks_exact_mut(4) {
                    chunk.copy_from_slice(&bytes);
                }
            }
            Elem::F32 => {
                let bytes = value_to_f32(val)?.to_le_bytes();
                for chunk in self.raw.chunks_exact_mut(4) {
                    chunk.copy_from_slice(&bytes);
                }
            }
        }
        Ok(())
    }

    pub fn fill_rgba(&mut self, rgba: u32) -> Result<(), String> {
        let bytes = rgba.to_le_bytes();
        match self.elem {
            Elem::U32 => {
                for chunk in self.raw.chunks_exact_mut(4) {
                    chunk.copy_from_slice(&bytes);
                }
            }
            Elem::U8 => {
                if self.raw.len() % 4 != 0 {
                    return Err(
                        "buffer.fill_rgba() on 'u8' buffer requires byte length divisible by 4"
                            .to_string(),
                    );
                }
                for chunk in self.raw.chunks_exact_mut(4) {
                    chunk.copy_from_slice(&bytes);
                }
            }
            other => {
                return Err(format!(
                    "buffer.fill_rgba() supports 'u32' or 4-byte-aligned 'u8', got '{}'",
                    other.name()
                ));
            }
        }
        Ok(())
    }

    pub fn copy_range_from(
        &mut self,
        dst_elem_off: usize,
        src_elem: Elem,
        src_raw: &[u8],
        src_elem_off: usize,
        count: usize,
    ) -> Result<(), String> {
        if self.elem != src_elem {
            return Err(format!(
                "Buffer element type mismatch: '{}' vs '{}'",
                self.elem.name(),
                src_elem.name()
            ));
        }
        let w = self.elem.width();
        let src_len = src_raw.len() / w;
        if dst_elem_off.saturating_add(count) > self.len {
            return Err("buffer.copy_from(): destination range out of bounds".to_string());
        }
        if src_elem_off.saturating_add(count) > src_len {
            return Err("buffer.copy_from(): source range out of bounds".to_string());
        }
        let dst_b = dst_elem_off * w;
        let src_b = src_elem_off * w;
        let n_bytes = count * w;
        self.raw[dst_b..dst_b + n_bytes].copy_from_slice(&src_raw[src_b..src_b + n_bytes]);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn blit_2d(
        &mut self,
        dst_w: usize,
        dst_h: usize,
        dst_x: i64,
        dst_y: i64,
        src_elem: Elem,
        src_raw: &[u8],
        src_w: usize,
        src_h: usize,
        src_x: i64,
        src_y: i64,
        copy_w: i64,
        copy_h: i64,
    ) -> Result<(), String> {
        if self.elem != src_elem {
            return Err(format!(
                "buffer.blit(): element type mismatch '{}' vs '{}'",
                self.elem.name(),
                src_elem.name()
            ));
        }
        let w_bytes = self.elem.width();
        let dst_needed = dst_w
            .checked_mul(dst_h)
            .ok_or_else(|| "buffer.blit(): destination dimensions overflow".to_string())?;
        let src_needed = src_w
            .checked_mul(src_h)
            .ok_or_else(|| "buffer.blit(): source dimensions overflow".to_string())?;
        if dst_needed > self.len {
            return Err("buffer.blit(): destination dimensions exceed buffer length".to_string());
        }
        if src_needed > src_raw.len() / w_bytes {
            return Err("buffer.blit(): source dimensions exceed buffer length".to_string());
        }
        if copy_w <= 0 || copy_h <= 0 {
            return Ok(());
        }

        let mut sx0 = src_x;
        let mut sy0 = src_y;
        let mut dx0 = dst_x;
        let mut dy0 = dst_y;
        let mut cw = copy_w;
        let mut ch = copy_h;

        if sx0 < 0 {
            let cut = -sx0;
            sx0 = 0;
            dx0 = dx0.saturating_add(cut);
            cw -= cut;
        }
        if sy0 < 0 {
            let cut = -sy0;
            sy0 = 0;
            dy0 = dy0.saturating_add(cut);
            ch -= cut;
        }
        if dx0 < 0 {
            let cut = -dx0;
            dx0 = 0;
            sx0 = sx0.saturating_add(cut);
            cw -= cut;
        }
        if dy0 < 0 {
            let cut = -dy0;
            dy0 = 0;
            sy0 = sy0.saturating_add(cut);
            ch -= cut;
        }
        if cw <= 0 || ch <= 0 {
            return Ok(());
        }
        let max_sw = (src_w as i64).saturating_sub(sx0);
        let max_dw = (dst_w as i64).saturating_sub(dx0);
        let max_sh = (src_h as i64).saturating_sub(sy0);
        let max_dh = (dst_h as i64).saturating_sub(dy0);
        cw = cw.min(max_sw).min(max_dw);
        ch = ch.min(max_sh).min(max_dh);
        if cw <= 0 || ch <= 0 {
            return Ok(());
        }

        let row_bytes = (cw as usize) * w_bytes;
        for row in 0..(ch as usize) {
            let s_off = ((sy0 as usize + row) * src_w + (sx0 as usize)) * w_bytes;
            let d_off = ((dy0 as usize + row) * dst_w + (dx0 as usize)) * w_bytes;
            self.raw[d_off..d_off + row_bytes]
                .copy_from_slice(&src_raw[s_off..s_off + row_bytes]);
        }
        Ok(())
    }
}

fn value_to_u8(val: &Value) -> Result<u8, String> {
    match val {
        Value::Int(i) if (0..=255).contains(i) => Ok(*i as u8),
        Value::Bool(b) => Ok(if *b { 1 } else { 0 }),
        Value::Int(i) => Err(format!("Value {} out of range for u8 (0..=255)", i)),
        other => Err(format!("Expected u8 integer 0..=255, got '{}'", other.type_name())),
    }
}

fn value_to_i32(val: &Value) -> Result<i32, String> {
    match val {
        Value::Int(i) => {
            i32::try_from(*i).map_err(|_| format!("Integer {} out of range for i32", i))
        }
        other => Err(format!("Expected i32 integer, got '{}'", other.type_name())),
    }
}

fn value_to_u32(val: &Value) -> Result<u32, String> {
    match val {
        Value::Int(i) => {
            u32::try_from(*i).map_err(|_| format!("Integer {} out of range for u32", i))
        }
        other => Err(format!("Expected u32 integer, got '{}'", other.type_name())),
    }
}

fn value_to_f32(val: &Value) -> Result<f32, String> {
    match val {
        Value::Float(f) => Ok(*f as f32),
        Value::Int(i) => Ok(*i as f32),
        other => Err(format!("Expected numeric value for f32, got '{}'", other.type_name())),
    }
}
