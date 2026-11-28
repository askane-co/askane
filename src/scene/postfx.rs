use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::{MediaBudgetGuard, temp_budget_guard};
use crate::scene::asset::{AssetRef, read_asset_ref_bytes};
use crate::surface::ClipRect;
use crate::vm::VM;

pub const MAX_LUT_FILE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug)]
pub struct Lut3D {
    pub size: usize,
    pub table: Vec<[f32; 3]>,
    #[allow(dead_code)]
    pub budget: Option<MediaBudgetGuard>,
}

impl Lut3D {
    pub fn from_cube_str(
        src: &str,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        if src.len() > MAX_LUT_FILE_BYTES {
            return Err("E_LUT_INVALID: LUT file exceeds 2 MiB limit".to_string());
        }
        let mut size: Option<usize> = None;
        let mut domain_min = [0.0f32; 3];
        let mut domain_max = [1.0f32; 3];
        let mut entries: Vec<[f32; 3]> = Vec::new();

        for (ln_no, raw_line) in src.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with("TITLE") {
                continue;
            }
            if line.starts_with("LUT_1D_SIZE") {
                return Err("E_LUT_INVALID: only LUT_3D_SIZE is supported".to_string());
            }
            if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
                let n: usize = rest
                    .trim()
                    .parse()
                    .map_err(|_| format!("E_LUT_INVALID: invalid LUT_3D_SIZE at line {}", ln_no + 1))?;
                if n != 16 && n != 32 {
                    return Err(format!(
                        "E_LUT_INVALID: LUT_3D_SIZE {} unsupported (expected 16 or 32)",
                        n
                    ));
                }
                size = Some(n);
                entries.reserve(n * n * n);
                continue;
            }
            if let Some(rest) = line.strip_prefix("DOMAIN_MIN") {
                domain_min = parse_vec3_line(rest, ln_no + 1)?;
                continue;
            }
            if let Some(rest) = line.strip_prefix("DOMAIN_MAX") {
                domain_max = parse_vec3_line(rest, ln_no + 1)?;
                continue;
            }
            let rgb = parse_vec3_line(line, ln_no + 1)?;
            entries.push(rgb);
        }

        let n = size.ok_or_else(|| "E_LUT_INVALID: missing LUT_3D_SIZE header".to_string())?;
        let expected = n * n * n;
        if entries.len() != expected {
            return Err(format!(
                "E_LUT_INVALID: expected {} RGB entries for {}^3 LUT, got {}",
                expected,
                n,
                entries.len()
            ));
        }

        for i in 0..3 {
            let span = domain_max[i] - domain_min[i];
            if !span.is_finite() || span.abs() <= 1e-6 {
                return Err("E_LUT_INVALID: invalid DOMAIN_MIN/MAX range".to_string());
            }
        }
        // Normalize domain to [0, 1] once at load
        if domain_min != [0.0, 0.0, 0.0] || domain_max != [1.0, 1.0, 1.0] {
            for e in &mut entries {
                for ch in 0..3 {
                    e[ch] = ((e[ch] - domain_min[ch]) / (domain_max[ch] - domain_min[ch])).clamp(0.0, 1.0);
                }
            }
        }

        let bytes = entries.len() * 12;
        let guard = MediaBudgetGuard::charge(budget_core, bytes)?;
        Ok(Self {
            size: n,
            table: entries,
            budget: Some(guard),
        })
    }

    pub fn load_file(vm: &mut VM, raw_path: &str) -> Result<Self, String> {
        let aref = AssetRef::parse_allow_sandbox_root(raw_path, vm)?;
        let (bytes, _, _) = read_asset_ref_bytes(vm, &aref)?;
        if bytes.len() > MAX_LUT_FILE_BYTES {
            return Err("E_LUT_INVALID: LUT file exceeds 2 MiB limit".to_string());
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| "E_LUT_INVALID: LUT file must be valid UTF-8".to_string())?;
        Self::from_cube_str(text, Some(vm.budget_core()))
    }

    #[inline]
    pub fn sample_trilinear(&self, r: f32, g: f32, b: f32) -> [f32; 3] {
        let max_idx = (self.size - 1) as f32;
        let fr = r.clamp(0.0, 1.0) * max_idx;
        let fg = g.clamp(0.0, 1.0) * max_idx;
        let fb = b.clamp(0.0, 1.0) * max_idx;

        let r0 = (fr.floor() as usize).min(self.size - 1);
        let g0 = (fg.floor() as usize).min(self.size - 1);
        let b0 = (fb.floor() as usize).min(self.size - 1);
        let r1 = (r0 + 1).min(self.size - 1);
        let g1 = (g0 + 1).min(self.size - 1);
        let b1 = (b0 + 1).min(self.size - 1);

        let tr = fr - (r0 as f32);
        let tg = fg - (g0 as f32);
        let tb = fb - (b0 as f32);

        let idx = |ri: usize, gi: usize, bi: usize| -> [f32; 3] {
            self.table[(bi * self.size + gi) * self.size + ri]
        };

        let c000 = idx(r0, g0, b0);
        let c100 = idx(r1, g0, b0);
        let c010 = idx(r0, g1, b0);
        let c110 = idx(r1, g1, b0);
        let c001 = idx(r0, g0, b1);
        let c101 = idx(r1, g0, b1);
        let c011 = idx(r0, g1, b1);
        let c111 = idx(r1, g1, b1);

        let mut out = [0.0f32; 3];
        for ch in 0..3 {
            let x00 = c000[ch] + (c100[ch] - c000[ch]) * tr;
            let x10 = c010[ch] + (c110[ch] - c010[ch]) * tr;
            let x01 = c001[ch] + (c101[ch] - c001[ch]) * tr;
            let x11 = c011[ch] + (c111[ch] - c011[ch]) * tr;
            let y0 = x00 + (x10 - x00) * tg;
            let y1 = x01 + (x11 - x01) * tg;
            out[ch] = (y0 + (y1 - y0) * tb).clamp(0.0, 1.0);
        }
        out
    }

    pub fn apply_surface(&self, buf: &mut [u32], bw: i32, bh: i32, clip: ClipRect, mix: f32) {
        if bw <= 0 || bh <= 0 {
            return;
        }
        let m = mix.clamp(0.0, 1.0);
        let x0 = clip.x0.max(0);
        let y0 = clip.y0.max(0);
        let x1 = clip.x1.min(bw);
        let y1 = clip.y1.min(bh);
        let bw_u = bw as usize;
        for py in y0..y1 {
            let row = (py as usize) * bw_u;
            for px in x0..x1 {
                let dst = buf[row + px as usize];
                let r = ((dst >> 16) & 0xFF) as f32 / 255.0;
                let g = ((dst >> 8) & 0xFF) as f32 / 255.0;
                let b = (dst & 0xFF) as f32 / 255.0;
                let mapped = self.sample_trilinear(r, g, b);
                let or = ((r + (mapped[0] - r) * m) * 255.0).round().clamp(0.0, 255.0) as u32;
                let og = ((g + (mapped[1] - g) * m) * 255.0).round().clamp(0.0, 255.0) as u32;
                let ob = ((b + (mapped[2] - b) * m) * 255.0).round().clamp(0.0, 255.0) as u32;
                buf[row + px as usize] = (dst & 0xFF00_0000) | (or << 16) | (og << 8) | ob;
            }
        }
    }
}

fn parse_vec3_line(line: &str, line_no: usize) -> Result<[f32; 3], String> {
    let mut parts = line.split_whitespace();
    let mut out = [0.0f32; 3];
    for ch in &mut out {
        let tok = parts
            .next()
            .ok_or_else(|| format!("E_LUT_INVALID: expected 3 floats at line {}", line_no))?;
        let val: f32 = tok
            .parse()
            .map_err(|_| format!("E_LUT_INVALID: invalid float '{}' at line {}", tok, line_no))?;
        if !val.is_finite() {
            return Err(format!("E_LUT_INVALID: non-finite LUT value at line {}", line_no));
        }
        *ch = val;
    }
    if parts.next().is_some() {
        return Err(format!("E_LUT_INVALID: extra tokens at line {}", line_no));
    }
    Ok(out)
}

pub fn apply_bloom(
    buf: &mut [u32],
    bw: i32,
    bh: i32,
    clip: ClipRect,
    threshold: f32,
    intensity: f32,
    radius: i32,
    budget_core: Option<&Arc<BudgetCore>>,
) -> Result<(), String> {
    if bw <= 0 || bh <= 0 || intensity <= 0.0 {
        return Ok(());
    }
    let x0 = clip.x0.max(0) as usize;
    let y0 = clip.y0.max(0) as usize;
    let x1 = clip.x1.min(bw) as usize;
    let y1 = clip.y1.min(bh) as usize;
    if x0 >= x1 || y0 >= y1 {
        return Ok(());
    }
    let cw = x1 - x0;
    let ch = y1 - y0;
    let rad = radius.clamp(1, 16) as isize;
    let thresh = (threshold.clamp(0.0, 1.0) * 255.0) as u32;
    let inten = intensity.clamp(0.0, 8.0);
    let bw_u = bw as usize;

    let work_bytes = cw.saturating_mul(ch).saturating_mul(12);
    let _guard = temp_budget_guard(budget_core, work_bytes)?;
    let mut bright = vec![0u32; cw * ch];
    let mut tmp = vec![0u32; cw * ch];

    for ry in 0..ch {
        let src_row = (y0 + ry) * bw_u;
        let dst_row = ry * cw;
        for rx in 0..cw {
            let p = buf[src_row + x0 + rx];
            let r = (p >> 16) & 0xFF;
            let g = (p >> 8) & 0xFF;
            let b = p & 0xFF;
            let lum = (77 * r + 150 * g + 29 * b) >> 8;
            if lum > thresh {
                let scale = (lum - thresh) * 255 / (255 - thresh).max(1);
                let br = (r * scale) / 255;
                let bg = (g * scale) / 255;
                let bb = (b * scale) / 255;
                bright[dst_row + rx] = (br << 16) | (bg << 8) | bb;
            }
        }
    }

    // Horizontal separable box blur
    for ry in 0..ch {
        let row = ry * cw;
        for rx in 0..cw {
            let mut sr = 0u32;
            let mut sg = 0u32;
            let mut sb = 0u32;
            let mut cnt = 0u32;
            let start = (rx as isize - rad).max(0) as usize;
            let end = (rx as isize + rad).min(cw as isize - 1) as usize;
            for kx in start..=end {
                let p = bright[row + kx];
                sr += (p >> 16) & 0xFF;
                sg += (p >> 8) & 0xFF;
                sb += p & 0xFF;
                cnt += 1;
            }
            tmp[row + rx] = ((sr / cnt) << 16) | ((sg / cnt) << 8) | (sb / cnt);
        }
    }

    // Vertical separable box blur + additive composite
    for ry in 0..ch {
        let start = (ry as isize - rad).max(0) as usize;
        let end = (ry as isize + rad).min(ch as isize - 1) as usize;
        let dst_row = (y0 + ry) * bw_u;
        for rx in 0..cw {
            let mut sr = 0u32;
            let mut sg = 0u32;
            let mut sb = 0u32;
            let mut cnt = 0u32;
            for ky in start..=end {
                let p = tmp[ky * cw + rx];
                sr += (p >> 16) & 0xFF;
                sg += (p >> 8) & 0xFF;
                sb += p & 0xFF;
                cnt += 1;
            }
            let gr = ((sr as f32 / cnt as f32) * inten).round() as u32;
            let gg = ((sg as f32 / cnt as f32) * inten).round() as u32;
            let gb = ((sb as f32 / cnt as f32) * inten).round() as u32;
            let slot = &mut buf[dst_row + x0 + rx];
            let dst = *slot;
            let r = (((dst >> 16) & 0xFF) + gr).min(255);
            let g = (((dst >> 8) & 0xFF) + gg).min(255);
            let b = ((dst & 0xFF) + gb).min(255);
            *slot = (dst & 0xFF00_0000) | (r << 16) | (g << 8) | b;
        }
    }
    Ok(())
}

pub fn apply_vignette(
    buf: &mut [u32],
    bw: i32,
    bh: i32,
    clip: ClipRect,
    strength: f32,
    radius: f32,
) {
    if bw <= 0 || bh <= 0 {
        return;
    }
    let st = strength.clamp(0.0, 1.0);
    let rad = radius.clamp(0.1, 2.0);
    let x0 = clip.x0.max(0);
    let y0 = clip.y0.max(0);
    let x1 = clip.x1.min(bw);
    let y1 = clip.y1.min(bh);
    let half_w = (bw as f32) * 0.5;
    let half_h = (bh as f32) * 0.5;
    let bw_u = bw as usize;

    for py in y0..y1 {
        let ny = ((py as f32) - half_h) / half_h.max(1.0);
        let row = (py as usize) * bw_u;
        for px in x0..x1 {
            let nx = ((px as f32) - half_w) / half_w.max(1.0);
            let d = (nx * nx + ny * ny).sqrt();
            let t = ((d - rad * 0.5) / (1.42 - rad * 0.5).max(0.1)).clamp(0.0, 1.0);
            let atten = 1.0 - st * t * t * (3.0 - 2.0 * t);
            let dst = buf[row + px as usize];
            let r = (((dst >> 16) & 0xFF) as f32 * atten).round().clamp(0.0, 255.0) as u32;
            let g = (((dst >> 8) & 0xFF) as f32 * atten).round().clamp(0.0, 255.0) as u32;
            let b = ((dst & 0xFF) as f32 * atten).round().clamp(0.0, 255.0) as u32;
            buf[row + px as usize] = (dst & 0xFF00_0000) | (r << 16) | (g << 8) | b;
        }
    }
}

pub fn apply_scanlines(
    buf: &mut [u32],
    bw: i32,
    bh: i32,
    clip: ClipRect,
    opacity: f32,
    step: i32,
) {
    if bw <= 0 || bh <= 0 {
        return;
    }
    let op = opacity.clamp(0.0, 1.0);
    let step = step.clamp(2, 16);
    let factor = 1.0 - op;
    let x0 = clip.x0.max(0);
    let y0 = clip.y0.max(0);
    let x1 = clip.x1.min(bw);
    let y1 = clip.y1.min(bh);
    let bw_u = bw as usize;

    for py in y0..y1 {
        if py % step != 0 {
            continue;
        }
        let row = (py as usize) * bw_u;
        for px in x0..x1 {
            let dst = buf[row + px as usize];
            let r = (((dst >> 16) & 0xFF) as f32 * factor).round().clamp(0.0, 255.0) as u32;
            let g = (((dst >> 8) & 0xFF) as f32 * factor).round().clamp(0.0, 255.0) as u32;
            let b = ((dst & 0xFF) as f32 * factor).round().clamp(0.0, 255.0) as u32;
            buf[row + px as usize] = (dst & 0xFF00_0000) | (r << 16) | (g << 8) | b;
        }
    }
}

/// Deterministic uncompressed 24-bit bottom-up BMP encoder (4-byte aligned rows, zero timestamps).
pub fn encode_bmp24(buf: &[u32], bw: i32, bh: i32) -> Result<Vec<u8>, String> {
    if bw <= 0 || bh <= 0 || bw > 8192 || bh > 8192 {
        return Err("E_MEDIA_LIMIT: snapshot BMP dimensions out of 1..=8192".to_string());
    }
    let w = bw as usize;
    let h = bh as usize;
    if buf.len() != w * h {
        return Err("E_MEDIA_CORRUPT: snapshot pixel buffer length mismatch".to_string());
    }
    let row_bytes = w * 3;
    let row_stride = (row_bytes + 3) & !3;
    let pad = row_stride - row_bytes;
    let img_bytes = row_stride * h;
    let file_size = 54 + img_bytes;
    let mut out = Vec::with_capacity(file_size);

    // BITMAPFILEHEADER (14 bytes)
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());

    // BITMAPINFOHEADER (40 bytes)
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(bw as i32).to_le_bytes());
    out.extend_from_slice(&(bh as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(img_bytes as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for y in (0..h).rev() {
        let row = &buf[y * w..(y + 1) * w];
        for &px in row {
            out.push((px & 0xFF) as u8);
            out.push(((px >> 8) & 0xFF) as u8);
            out.push(((px >> 16) & 0xFF) as u8);
        }
        for _ in 0..pad {
            out.push(0);
        }
    }
    Ok(out)
}
