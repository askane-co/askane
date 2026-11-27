use crate::budget::BudgetCore;
use crate::scene::units::DEPTH_CLEAR_VALUE;

pub const MAX_SURFACE_DIM: i32 = 8192;
pub const MAX_USER_SURFACE_DIM: i32 = 4096;
pub const MAX_SURFACE_PIXELS: usize = 33_554_432;
pub const MAX_SURFACES: usize = 64;
pub const MAX_TARGET_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
    R8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl ClipRect {
    #[inline]
    pub fn full(w: i32, h: i32) -> Self {
        Self {
            x0: 0,
            y0: 0,
            x1: w.max(0),
            y1: h.max(0),
        }
    }

    #[inline]
    pub fn intersect(self, other: Self) -> Self {
        Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        }
    }

    #[inline]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}

/// Software depth buffer canonicalized to NDC `[0.0, 1.0]`, clear `1.0`, pass `frag_z < buf_z`.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct DepthBuffer {
    pub width: i32,
    pub height: i32,
    pub data: Vec<f32>,
    pub charged_bytes: u64,
}

#[allow(dead_code)]
impl DepthBuffer {
    pub fn new(width: i32, height: i32, budget: Option<&BudgetCore>) -> Result<Self, String> {
        let len = validate_surface_dims(width, height, MAX_SURFACE_DIM)?;
        let bytes = (len as u64).saturating_mul(std::mem::size_of::<f32>() as u64);
        if let Some(b) = budget {
            b.charge_alloc(bytes)?;
        }
        Ok(Self {
            width,
            height,
            data: vec![DEPTH_CLEAR_VALUE; len],
            charged_bytes: if budget.is_some() { bytes } else { 0 },
        })
    }

    #[inline]
    pub fn clear(&mut self) {
        self.data.fill(DEPTH_CLEAR_VALUE);
    }

    #[inline]
    pub fn peek(&self, x: i32, y: i32) -> Option<f32> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return None;
        }
        self.data.get((y as usize) * (self.width as usize) + (x as usize)).copied()
    }

    /// Tests NDC depth (`[0.0, 1.0]`, `<`) and updates buffer on pass.
    #[inline]
    pub fn test_and_set(&mut self, x: i32, y: i32, frag_z: f32) -> bool {
        if x < 0
            || y < 0
            || x >= self.width
            || y >= self.height
            || !frag_z.is_finite()
            || !(0.0..=1.0).contains(&frag_z)
        {
            return false;
        }
        let idx = (y as usize) * (self.width as usize) + (x as usize);
        if frag_z < self.data[idx] {
            self.data[idx] = frag_z;
            true
        } else {
            false
        }
    }

    pub fn refund_to(&mut self, budget: Option<&BudgetCore>) {
        let bytes = std::mem::take(&mut self.charged_bytes);
        if bytes > 0 {
            if let Some(b) = budget {
                b.refund_alloc(bytes);
            }
        }
    }
}

/// Canonical software pixel surface (`0xAARRGGBB` little-endian `Rgba8` or single-channel `R8`).
#[derive(Clone, Debug)]
pub struct Surface {
    pub width: i32,
    pub height: i32,
    pub format: PixelFormat,
    pub pixels: Vec<u32>,
    pub r8: Vec<u8>,
    pub clip: Option<ClipRect>,
    pub depth: Option<DepthBuffer>,
    pub charged_bytes: u64,
}

pub fn validate_surface_dims(width: i32, height: i32, max_dim: i32) -> Result<usize, String> {
    if width <= 0 || height <= 0 || width > max_dim || height > max_dim {
        return Err(format!(
            "Surface dimensions {}x{} out of 1..={} bounds",
            width, height, max_dim
        ));
    }
    let count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| "Surface pixel count overflow".to_string())?;
    if count > MAX_SURFACE_PIXELS {
        return Err(format!(
            "Surface pixel count {} exceeds limit {}",
            count, MAX_SURFACE_PIXELS
        ));
    }
    Ok(count)
}

impl Surface {
    pub fn new_rgba8(
        width: i32,
        height: i32,
        clear_color: u32,
        budget: Option<&BudgetCore>,
    ) -> Result<Self, String> {
        debug_assert!(cfg!(target_endian = "little"));
        let count = validate_surface_dims(width, height, MAX_SURFACE_DIM)?;
        let bytes = (count as u64).saturating_mul(std::mem::size_of::<u32>() as u64);
        if let Some(b) = budget {
            b.charge_alloc(bytes)?;
        }
        Ok(Self {
            width,
            height,
            format: PixelFormat::Rgba8,
            pixels: vec![clear_color; count],
            r8: Vec::new(),
            clip: None,
            depth: None,
            charged_bytes: if budget.is_some() { bytes } else { 0 },
        })
    }

    pub fn new_r8(
        width: i32,
        height: i32,
        clear_val: u8,
        budget: Option<&BudgetCore>,
    ) -> Result<Self, String> {
        let count = validate_surface_dims(width, height, MAX_SURFACE_DIM)?;
        let bytes = count as u64;
        if let Some(b) = budget {
            b.charge_alloc(bytes)?;
        }
        Ok(Self {
            width,
            height,
            format: PixelFormat::R8,
            pixels: Vec::new(),
            r8: vec![clear_val; count],
            clip: None,
            depth: None,
            charged_bytes: if budget.is_some() { bytes } else { 0 },
        })
    }

    pub fn from_rgba_pixels(
        width: i32,
        height: i32,
        pixels: Vec<u32>,
        budget: Option<&BudgetCore>,
    ) -> Result<Self, String> {
        let count = validate_surface_dims(width, height, MAX_SURFACE_DIM)?;
        if pixels.len() != count {
            return Err("Surface pixel buffer length mismatch".to_string());
        }
        let bytes = (count as u64).saturating_mul(std::mem::size_of::<u32>() as u64);
        if let Some(b) = budget {
            b.charge_alloc(bytes)?;
        }
        Ok(Self {
            width,
            height,
            format: PixelFormat::Rgba8,
            pixels,
            r8: Vec::new(),
            clip: None,
            depth: None,
            charged_bytes: if budget.is_some() { bytes } else { 0 },
        })
    }

    #[allow(dead_code)]
    pub fn attach_depth(&mut self, budget: Option<&BudgetCore>) -> Result<(), String> {
        if self.depth.is_some() {
            return Ok(());
        }
        self.depth = Some(DepthBuffer::new(self.width, self.height, budget)?);
        Ok(())
    }

    pub fn refund_to(&mut self, budget: Option<&BudgetCore>) {
        if let Some(mut d) = self.depth.take() {
            d.refund_to(budget);
        }
        let bytes = std::mem::take(&mut self.charged_bytes);
        if bytes > 0 {
            if let Some(b) = budget {
                b.refund_alloc(bytes);
            }
        }
    }

    #[inline]
    pub fn effective_clip(&self) -> ClipRect {
        let bounds = ClipRect::full(self.width, self.height);
        match self.clip {
            Some(c) => bounds.intersect(c),
            None => bounds,
        }
    }

    #[allow(dead_code)]
    #[inline]
    pub fn peek_pixel(&self, x: i32, y: i32) -> Option<u32> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return None;
        }
        let idx = (y as usize) * (self.width as usize) + (x as usize);
        match self.format {
            PixelFormat::Rgba8 => self.pixels.get(idx).copied(),
            PixelFormat::R8 => self.r8.get(idx).map(|&v| v as u32),
        }
    }

    #[inline]
    pub fn clear_rgba(&mut self, color: u32) {
        if self.format != PixelFormat::Rgba8 {
            return;
        }
        if let Some(clip) = self.clip {
            let c = ClipRect::full(self.width, self.height).intersect(clip);
            fill_rect_clipped(&mut self.pixels, self.width, c, c.x0, c.y0, c.x1 - c.x0, c.y1 - c.y0, color);
        } else {
            self.pixels.fill(color);
        }
        if let Some(d) = self.depth.as_mut() {
            d.clear();
        }
    }
}

/// Deterministic 64-bit FNV-1a digest (`16`-char hex) for golden-frame assertions.
pub fn hash_rgba_buffer(width: i32, height: i32, pixels: &[u32]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mix = |h: &mut u64, byte: u8| {
        *h ^= byte as u64;
        *h = h.wrapping_mul(0x0100_0000_01b3);
    };
    for &b in &width.to_le_bytes() {
        mix(&mut h, b);
    }
    for &b in &height.to_le_bytes() {
        mix(&mut h, b);
    }
    for &px in pixels {
        for &b in &px.to_le_bytes() {
            mix(&mut h, b);
        }
    }
    format!("{:016x}", h)
}

#[inline]
pub fn effective_clip(w: i32, h: i32, clip: Option<ClipRect>) -> ClipRect {
    let full = ClipRect::full(w, h);
    match clip {
        Some(c) => full.intersect(c),
        None => full,
    }
}

#[inline]
pub fn blend_straight_argb(
    dst: u32,
    sr: u32,
    sg: u32,
    sb: u32,
    sa: u32,
    preserve_dst_alpha: bool,
) -> u32 {
    if sa == 0 {
        return dst;
    }
    if sa >= 255 {
        return if preserve_dst_alpha {
            (sr << 16) | (sg << 8) | sb
        } else {
            0xFF00_0000 | (sr << 16) | (sg << 8) | sb
        };
    }
    let inv = 255 - sa;
    let da = (dst >> 24) & 0xFF;
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;
    if preserve_dst_alpha {
        let r = (sr * sa + dr * inv + 127) / 255;
        let g = (sg * sa + dg * inv + 127) / 255;
        let b = (sb * sa + db * inv + 127) / 255;
        return (r << 16) | (g << 8) | b;
    }
    if da == 0 {
        return (sa << 24) | (sr << 16) | (sg << 8) | sb;
    }
    if da == 255 {
        let r = (sr * sa + dr * inv + 127) / 255;
        let g = (sg * sa + dg * inv + 127) / 255;
        let b = (sb * sa + db * inv + 127) / 255;
        return 0xFF00_0000 | (r << 16) | (g << 8) | b;
    }
    let w_src = sa * 255;
    let w_dst = da * inv;
    let denom = w_src + w_dst;
    let out_a = (denom + 127) / 255;
    let half = denom / 2;
    let out_r = ((sr * w_src + dr * w_dst + half) / denom).min(255);
    let out_g = ((sg * w_src + dg * w_dst + half) / denom).min(255);
    let out_b = ((sb * w_src + db * w_dst + half) / denom).min(255);
    (out_a << 24) | (out_r << 16) | (out_g << 8) | out_b
}

#[inline]
pub fn blend_argb_over(dst: u32, src: u32) -> u32 {
    let sa = (src >> 24) & 0xFF;
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;
    blend_straight_argb(dst, sr, sg, sb, sa, false)
}

#[inline]
pub fn blend_rgb_alpha_on(dst: u32, rgb: u32, alpha: u32, preserve_dst_alpha: bool) -> u32 {
    let sr = (rgb >> 16) & 0xFF;
    let sg = (rgb >> 8) & 0xFF;
    let sb = rgb & 0xFF;
    blend_straight_argb(dst, sr, sg, sb, alpha, preserve_dst_alpha)
}

pub fn fill_rect_clipped(
    buf: &mut [u32],
    bw: i32,
    clip: ClipRect,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: u32,
) {
    if bw <= 0 || w <= 0 || h <= 0 {
        return;
    }
    let x0 = x.max(clip.x0);
    let y0 = y.max(clip.y0);
    let x1 = x.saturating_add(w).min(clip.x1);
    let y1 = y.saturating_add(h).min(clip.y1);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let bw_usize = bw as usize;
    for py in y0..y1 {
        let row = (py as usize) * bw_usize;
        for px in x0..x1 {
            buf[row + px as usize] = color;
        }
    }
}

pub fn fill_rect_alpha_clipped(
    buf: &mut [u32],
    bw: i32,
    clip: ClipRect,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    rgb: u32,
    alpha: u32,
    preserve_dst_alpha: bool,
) {
    if bw <= 0 || w <= 0 || h <= 0 || alpha == 0 {
        return;
    }
    if alpha >= 255 {
        let color = if preserve_dst_alpha {
            rgb & 0x00FF_FFFF
        } else {
            0xFF00_0000 | (rgb & 0x00FF_FFFF)
        };
        fill_rect_clipped(buf, bw, clip, x, y, w, h, color);
        return;
    }
    let x0 = x.max(clip.x0);
    let y0 = y.max(clip.y0);
    let x1 = x.saturating_add(w).min(clip.x1);
    let y1 = y.saturating_add(h).min(clip.y1);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let bw_usize = bw as usize;
    for py in y0..y1 {
        let row = (py as usize) * bw_usize;
        for px in x0..x1 {
            let slot = &mut buf[row + px as usize];
            *slot = blend_rgb_alpha_on(*slot, rgb, alpha, preserve_dst_alpha);
        }
    }
}

#[inline]
pub fn put_pixel_clipped(buf: &mut [u32], bw: i32, clip: ClipRect, x: i32, y: i32, color: u32) {
    if clip.contains(x, y) {
        buf[(y as usize) * (bw as usize) + (x as usize)] = color;
    }
}

pub fn fill_circle_clipped(
    buf: &mut [u32],
    bw: i32,
    clip: ClipRect,
    cx: i32,
    cy: i32,
    radius: i32,
    rgb: u32,
    alpha: u32,
    preserve_dst_alpha: bool,
) {
    if bw <= 0 || radius <= 0 || alpha == 0 {
        return;
    }
    let y0 = (cy - radius).max(clip.y0);
    let y1 = (cy + radius).min(clip.y1 - 1);
    let x0 = (cx - radius).max(clip.x0);
    let x1 = (cx + radius).min(clip.x1 - 1);
    if x0 > x1 || y0 > y1 {
        return;
    }
    let r2 = (radius as i64) * (radius as i64);
    let bw_usize = bw as usize;
    let solid = if preserve_dst_alpha {
        rgb & 0x00FF_FFFF
    } else {
        0xFF00_0000 | (rgb & 0x00FF_FFFF)
    };
    for py in y0..=y1 {
        let dy = (py - cy) as i64;
        let row = (py as usize) * bw_usize;
        for px in x0..=x1 {
            let dx = (px - cx) as i64;
            if dx * dx + dy * dy <= r2 {
                if alpha >= 255 {
                    buf[row + px as usize] = solid;
                } else {
                    let slot = &mut buf[row + px as usize];
                    *slot = blend_rgb_alpha_on(*slot, rgb, alpha, preserve_dst_alpha);
                }
            }
        }
    }
}

pub fn stroke_line_clipped(
    buf: &mut [u32],
    bw: i32,
    clip: ClipRect,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut steps = 0usize;
    while steps < 32768 {
        put_pixel_clipped(buf, bw, clip, x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
        steps += 1;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn blit_rgba_surface(
    dst: &mut [u32],
    dst_w: i32,
    dst_clip: ClipRect,
    preserve_dst_alpha: bool,
    src: &[u32],
    src_w: i32,
    src_h: i32,
    dx: i32,
    dy: i32,
    scale_x: f64,
    scale_y: f64,
    key: Option<u32>,
    global_alpha: u8,
) {
    if dst_w <= 0 || src_w <= 0 || src_h <= 0 || global_alpha == 0 {
        return;
    }
    let dw = ((src_w as f64) * scale_x).round().max(1.0) as i32;
    let dh = ((src_h as f64) * scale_y).round().max(1.0) as i32;
    let x0 = dx.max(dst_clip.x0);
    let y0 = dy.max(dst_clip.y0);
    let x1 = dx.saturating_add(dw).min(dst_clip.x1);
    let y1 = dy.saturating_add(dh).min(dst_clip.y1);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let dst_w_u = dst_w as usize;
    let src_w_u = src_w as usize;
    for py in y0..y1 {
        let sy = if dw == src_w && dh == src_h {
            py - dy
        } else {
            (((py - dy) as i64 * src_h as i64) / dh as i64).clamp(0, src_h as i64 - 1) as i32
        };
        let srow = (sy as usize) * src_w_u;
        let drow = (py as usize) * dst_w_u;
        for px in x0..x1 {
            let sx = if dw == src_w && dh == src_h {
                px - dx
            } else {
                (((px - dx) as i64 * src_w as i64) / dw as i64).clamp(0, src_w as i64 - 1) as i32
            };
            let sc = src[srow + sx as usize];
            if let Some(k) = key {
                if (sc & 0x00FF_FFFF) == (k & 0x00FF_FFFF) {
                    continue;
                }
            }
            let sa = (sc >> 24) & 0xFF;
            let eff_a = if global_alpha == 255 {
                sa
            } else {
                (sa * (global_alpha as u32) + 127) / 255
            };
            let d_idx = drow + px as usize;
            dst[d_idx] = blend_straight_argb(
                dst[d_idx],
                (sc >> 16) & 0xFF,
                (sc >> 8) & 0xFF,
                sc & 0xFF,
                eff_a,
                preserve_dst_alpha,
            );
        }
    }
}
