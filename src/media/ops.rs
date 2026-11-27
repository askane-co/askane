use std::rc::Rc;
use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::color::{
    SRGB_TO_LINEAR_U16, linear8_to_srgb8, linear16_to_srgb8, premultiply_rgba8, srgb8_to_linear8,
};
use crate::media::image::{
    ColorSpace, MediaImage, MipChain, MipLevel, PixelFormat, SourceFormat,
};
use crate::media::{
    Digest, MAX_DIM, MAX_MEDIA_BYTES, MAX_MIP_LEVELS, MediaBudgetGuard, check_deadline,
    err_bad_opt, err_limit, temp_budget_guard, validate_dims,
};

#[inline]
fn luma_bt601(r: u8, g: u8, b: u8) -> u8 {
    ((77u32 * (r as u32) + 150u32 * (g as u32) + 29u32 * (b as u32) + 128) >> 8) as u8
}

pub fn convert_buffer(
    src: &[u8],
    width: u32,
    height: u32,
    from: PixelFormat,
    to: PixelFormat,
) -> Result<Vec<u8>, String> {
    let pixels = validate_dims(width, height, MAX_DIM)?;
    let expected = pixels * from.channels();
    if src.len() != expected {
        return Err(err_bad_opt("source pixel buffer length mismatch"));
    }
    if from == to {
        return Ok(src.to_vec());
    }
    let out_len = pixels
        .checked_mul(to.channels())
        .ok_or_else(|| err_limit("converted image byte size overflow"))?;
    let mut out = Vec::with_capacity(out_len);
    match (from, to) {
        (PixelFormat::R8, PixelFormat::Rg8) => {
            for &g in src {
                out.push(g);
                out.push(g);
            }
        }
        (PixelFormat::R8, PixelFormat::Rgb8) => {
            for &g in src {
                out.extend_from_slice(&[g, g, g]);
            }
        }
        (PixelFormat::R8, PixelFormat::Rgba8) => {
            for &g in src {
                out.extend_from_slice(&[g, g, g, 255]);
            }
        }
        (PixelFormat::Rg8, PixelFormat::R8) => {
            for px in src.chunks_exact(2) {
                out.push(px[0]);
            }
        }
        (PixelFormat::Rg8, PixelFormat::Rgb8) => {
            for px in src.chunks_exact(2) {
                out.extend_from_slice(&[px[0], px[1], 0]);
            }
        }
        (PixelFormat::Rg8, PixelFormat::Rgba8) => {
            for px in src.chunks_exact(2) {
                out.extend_from_slice(&[px[0], px[1], 0, 255]);
            }
        }
        (PixelFormat::Rgb8, PixelFormat::R8) => {
            for px in src.chunks_exact(3) {
                out.push(luma_bt601(px[0], px[1], px[2]));
            }
        }
        (PixelFormat::Rgb8, PixelFormat::Rg8) => {
            for px in src.chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1]]);
            }
        }
        (PixelFormat::Rgb8, PixelFormat::Rgba8) => {
            for px in src.chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        (PixelFormat::Rgba8, PixelFormat::R8) => {
            for px in src.chunks_exact(4) {
                out.push(luma_bt601(px[0], px[1], px[2]));
            }
        }
        (PixelFormat::Rgba8, PixelFormat::Rg8) => {
            for px in src.chunks_exact(4) {
                out.extend_from_slice(&[px[0], px[1]]);
            }
        }
        (PixelFormat::Rgba8, PixelFormat::Rgb8) => {
            for px in src.chunks_exact(4) {
                out.extend_from_slice(&[px[0], px[1], px[2]]);
            }
        }
        _ => unreachable!(),
    }
    Ok(out)
}

pub fn convert_format(
    src: &MediaImage,
    target: PixelFormat,
    budget_core: Option<&Arc<BudgetCore>>,
) -> Result<Rc<MediaImage>, String> {
    let buf = convert_buffer(&src.data, src.width, src.height, src.format, target)?;
    let digest = Digest::from_parts(&[b"op:convert|", &src.digest.0, &[target as u8]]);
    MediaImage::from_vec(
        src.width,
        src.height,
        target,
        src.color_space,
        SourceFormat::Derived,
        buf,
        Some(target.has_alpha()),
        Some(digest),
        budget_core,
    )
}

pub fn to_linear(
    src: &MediaImage,
    budget_core: Option<&Arc<BudgetCore>>,
) -> Result<Rc<MediaImage>, String> {
    let mut out = src.data.to_vec();
    if src.color_space == ColorSpace::Srgb {
        match src.format {
            PixelFormat::R8 => {
                for b in &mut out {
                    *b = srgb8_to_linear8(*b);
                }
            }
            PixelFormat::Rg8 => {
                for px in out.chunks_exact_mut(2) {
                    px[0] = srgb8_to_linear8(px[0]);
                    px[1] = srgb8_to_linear8(px[1]);
                }
            }
            PixelFormat::Rgb8 => {
                for px in out.chunks_exact_mut(3) {
                    px[0] = srgb8_to_linear8(px[0]);
                    px[1] = srgb8_to_linear8(px[1]);
                    px[2] = srgb8_to_linear8(px[2]);
                }
            }
            PixelFormat::Rgba8 => {
                for px in out.chunks_exact_mut(4) {
                    px[0] = srgb8_to_linear8(px[0]);
                    px[1] = srgb8_to_linear8(px[1]);
                    px[2] = srgb8_to_linear8(px[2]);
                }
            }
        }
    }
    let digest = Digest::from_parts(&[b"op:to_linear|", &src.digest.0]);
    MediaImage::from_vec(
        src.width,
        src.height,
        src.format,
        ColorSpace::Linear,
        SourceFormat::Derived,
        out,
        Some(src.has_alpha),
        Some(digest),
        budget_core,
    )
}

pub fn to_srgb(
    src: &MediaImage,
    budget_core: Option<&Arc<BudgetCore>>,
) -> Result<Rc<MediaImage>, String> {
    let mut out = src.data.to_vec();
    if src.color_space == ColorSpace::Linear {
        match src.format {
            PixelFormat::R8 => {
                for b in &mut out {
                    *b = linear8_to_srgb8(*b);
                }
            }
            PixelFormat::Rg8 => {
                for px in out.chunks_exact_mut(2) {
                    px[0] = linear8_to_srgb8(px[0]);
                    px[1] = linear8_to_srgb8(px[1]);
                }
            }
            PixelFormat::Rgb8 => {
                for px in out.chunks_exact_mut(3) {
                    px[0] = linear8_to_srgb8(px[0]);
                    px[1] = linear8_to_srgb8(px[1]);
                    px[2] = linear8_to_srgb8(px[2]);
                }
            }
            PixelFormat::Rgba8 => {
                for px in out.chunks_exact_mut(4) {
                    px[0] = linear8_to_srgb8(px[0]);
                    px[1] = linear8_to_srgb8(px[1]);
                    px[2] = linear8_to_srgb8(px[2]);
                }
            }
        }
    }
    let digest = Digest::from_parts(&[b"op:to_srgb|", &src.digest.0]);
    MediaImage::from_vec(
        src.width,
        src.height,
        src.format,
        ColorSpace::Srgb,
        SourceFormat::Derived,
        out,
        Some(src.has_alpha),
        Some(digest),
        budget_core,
    )
}

pub fn premultiply(
    src: &MediaImage,
    budget_core: Option<&Arc<BudgetCore>>,
) -> Result<Rc<MediaImage>, String> {
    let out = match src.format {
        PixelFormat::Rgba8 => premultiply_rgba8(&src.data),
        _ => src.data.to_vec(),
    };
    let digest = Digest::from_parts(&[b"op:premultiply|", &src.digest.0]);
    MediaImage::from_vec(
        src.width,
        src.height,
        src.format,
        src.color_space,
        SourceFormat::Derived,
        out,
        Some(src.has_alpha),
        Some(digest),
        budget_core,
    )
}

pub fn crop(
    src: &MediaImage,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    budget_core: Option<&Arc<BudgetCore>>,
) -> Result<Rc<MediaImage>, String> {
    if w <= 0 || h <= 0 || x < 0 || y < 0 {
        return Err(err_bad_opt("crop rectangle requires non-negative origin and positive size"));
    }
    let end_x = x.checked_add(w).ok_or_else(|| err_bad_opt("crop width overflow"))?;
    let end_y = y.checked_add(h).ok_or_else(|| err_bad_opt("crop height overflow"))?;
    if end_x > src.width as i64 || end_y > src.height as i64 {
        return Err(err_bad_opt(format!(
            "crop rect ({},{},{}x{}) exceeds source image bounds {}x{}",
            x, y, w, h, src.width, src.height
        )));
    }
    let uw = w as u32;
    let uh = h as u32;
    let ux = x as usize;
    let uy = y as usize;
    let bpp = src.format.channels();
    let out_bytes = src.format.byte_len(uw, uh)?;
    let _tmp = temp_budget_guard(budget_core, out_bytes)?;
    let mut out = vec![0u8; out_bytes];
    let src_stride = (src.width as usize) * bpp;
    let dst_stride = (uw as usize) * bpp;
    for row in 0..(uh as usize) {
        let s_off = (uy + row) * src_stride + ux * bpp;
        let d_off = row * dst_stride;
        out[d_off..d_off + dst_stride].copy_from_slice(&src.data[s_off..s_off + dst_stride]);
    }
    drop(_tmp);
    let digest = Digest::from_parts(&[
        b"op:crop|",
        &src.digest.0,
        &uw.to_le_bytes(),
        &uh.to_le_bytes(),
        &(ux as u32).to_le_bytes(),
        &(uy as u32).to_le_bytes(),
    ]);
    MediaImage::from_vec(
        uw,
        uh,
        src.format,
        src.color_space,
        SourceFormat::Derived,
        out,
        Some(src.has_alpha),
        Some(digest),
        budget_core,
    )
}

pub fn flip(
    src: &MediaImage,
    horizontal: bool,
    vertical: bool,
    budget_core: Option<&Arc<BudgetCore>>,
) -> Result<Rc<MediaImage>, String> {
    let bpp = src.format.channels();
    let w = src.width as usize;
    let h = src.height as usize;
    let out_bytes = src.format.byte_len(src.width, src.height)?;
    let _tmp = temp_budget_guard(budget_core, out_bytes)?;
    let mut out = vec![0u8; out_bytes];
    for dy in 0..h {
        let sy = if vertical { h - 1 - dy } else { dy };
        let srow = sy * w * bpp;
        let drow = dy * w * bpp;
        if !horizontal {
            out[drow..drow + w * bpp].copy_from_slice(&src.data[srow..srow + w * bpp]);
        } else {
            for dx in 0..w {
                let sx = w - 1 - dx;
                out[drow + dx * bpp..drow + (dx + 1) * bpp]
                    .copy_from_slice(&src.data[srow + sx * bpp..srow + (sx + 1) * bpp]);
            }
        }
    }
    drop(_tmp);
    let digest = Digest::from_parts(&[
        b"op:flip|",
        &src.digest.0,
        &[u8::from(horizontal), u8::from(vertical)],
    ]);
    MediaImage::from_vec(
        src.width,
        src.height,
        src.format,
        src.color_space,
        SourceFormat::Derived,
        out,
        Some(src.has_alpha),
        Some(digest),
        budget_core,
    )
}

pub fn rotate(
    src: &MediaImage,
    degrees: i64,
    budget_core: Option<&Arc<BudgetCore>>,
) -> Result<Rc<MediaImage>, String> {
    let norm = degrees.rem_euclid(360);
    if !matches!(norm, 0 | 90 | 180 | 270) {
        return Err(err_bad_opt(format!(
            "rotate() only accepts 0, 90, 180, or 270 degrees, got {}",
            degrees
        )));
    }
    let sw = src.width as usize;
    let sh = src.height as usize;
    let bpp = src.format.channels();
    let (dw, dh) = if norm == 90 || norm == 270 {
        (src.height, src.width)
    } else {
        (src.width, src.height)
    };
    let out_bytes = src.format.byte_len(dw, dh)?;
    let _tmp = temp_budget_guard(budget_core, out_bytes)?;
    let mut out = vec![0u8; out_bytes];
    let duw = dw as usize;
    for sy in 0..sh {
        for sx in 0..sw {
            let (dx, dy) = match norm {
                0 => (sx, sy),
                90 => (sh - 1 - sy, sx),
                180 => (sw - 1 - sx, sh - 1 - sy),
                270 => (sy, sw - 1 - sx),
                _ => unreachable!(),
            };
            let si = (sy * sw + sx) * bpp;
            let di = (dy * duw + dx) * bpp;
            out[di..di + bpp].copy_from_slice(&src.data[si..si + bpp]);
        }
    }
    drop(_tmp);
    let digest = Digest::from_parts(&[b"op:rotate|", &src.digest.0, &(norm as u32).to_le_bytes()]);
    MediaImage::from_vec(
        dw,
        dh,
        src.format,
        src.color_space,
        SourceFormat::Derived,
        out,
        Some(src.has_alpha),
        Some(digest),
        budget_core,
    )
}

#[inline]
fn linear_u16_channel(px: &[u8], ch: usize, srgb: bool, is_alpha: bool) -> u32 {
    let v = px[ch];
    if !srgb || is_alpha {
        (v as u32) * 257
    } else {
        SRGB_TO_LINEAR_U16[v as usize] as u32
    }
}

pub fn resize(
    src: &MediaImage,
    new_w: u32,
    new_h: u32,
    method: &str,
    budget_core: Option<&Arc<BudgetCore>>,
    deadline: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<Rc<MediaImage>, String> {
    let norm = method.trim().to_ascii_lowercase();
    if !matches!(norm.as_str(), "nearest" | "box" | "bilinear") {
        return Err(err_bad_opt(format!(
            "resize() filter must be 'nearest', 'box', or 'bilinear', got '{}'",
            method
        )));
    }
    let out_bytes = src.format.byte_len(new_w, new_h)?;
    let _tmp = temp_budget_guard(budget_core, out_bytes)?;
    let bpp = src.format.channels();
    let sw = src.width as usize;
    let sh = src.height as usize;
    let dw = new_w as usize;
    let dh = new_h as usize;
    let mut out = vec![0u8; out_bytes];
    let srgb = src.color_space == ColorSpace::Srgb;
    let has_alpha = src.format == PixelFormat::Rgba8;

    match norm.as_str() {
        "nearest" => {
            for dy in 0..dh {
                if (dy & 63) == 0 {
                    check_deadline(deadline)?;
                }
                let sy = (dy * sh) / dh;
                let srow = sy * sw * bpp;
                let drow = dy * dw * bpp;
                for dx in 0..dw {
                    let sx = (dx * sw) / dw;
                    out[drow + dx * bpp..drow + (dx + 1) * bpp]
                        .copy_from_slice(&src.data[srow + sx * bpp..srow + (sx + 1) * bpp]);
                }
            }
        }
        "box" => {
            for dy in 0..dh {
                if (dy & 15) == 0 {
                    check_deadline(deadline)?;
                }
                let y0 = (dy * sh) / dh;
                let y1 = (((dy + 1) * sh) / dh).max(y0 + 1).min(sh);
                for dx in 0..dw {
                    let x0 = (dx * sw) / dw;
                    let x1 = (((dx + 1) * sw) / dw).max(x0 + 1).min(sw);
                    let count = ((y1 - y0) * (x1 - x0)) as u64;
                    let di = (dy * dw + dx) * bpp;
                    if srgb && has_alpha {
                        let mut sum_rgb = [0u64; 3];
                        let mut sum_a = 0u64;
                        for sy in y0..y1 {
                            for sx in x0..x1 {
                                let si = (sy * sw + sx) * 4;
                                let a = src.data[si + 3] as u64;
                                sum_a += a;
                                for c in 0..3 {
                                    sum_rgb[c] += (SRGB_TO_LINEAR_U16[src.data[si + c] as usize] as u64) * a;
                                }
                            }
                        }
                        let avg_a = ((sum_a + count / 2) / count) as u8;
                        out[di + 3] = avg_a;
                        if sum_a == 0 {
                            out[di] = 0;
                            out[di + 1] = 0;
                            out[di + 2] = 0;
                        } else {
                            for c in 0..3 {
                                let lin16 = ((sum_rgb[c] + sum_a / 2) / sum_a).min(65535) as u16;
                                out[di + c] = linear16_to_srgb8(lin16);
                            }
                        }
                    } else if srgb {
                        let mut sum = [0u64; 4];
                        for sy in y0..y1 {
                            for sx in x0..x1 {
                                let si = (sy * sw + sx) * bpp;
                                for c in 0..bpp {
                                    sum[c] += SRGB_TO_LINEAR_U16[src.data[si + c] as usize] as u64;
                                }
                            }
                        }
                        for c in 0..bpp {
                            let lin16 = ((sum[c] + count / 2) / count).min(65535) as u16;
                            out[di + c] = linear16_to_srgb8(lin16);
                        }
                    } else {
                        let mut sum = [0u64; 4];
                        for sy in y0..y1 {
                            for sx in x0..x1 {
                                let si = (sy * sw + sx) * bpp;
                                for c in 0..bpp {
                                    sum[c] += src.data[si + c] as u64;
                                }
                            }
                        }
                        for c in 0..bpp {
                            out[di + c] = ((sum[c] + count / 2) / count).min(255) as u8;
                        }
                    }
                }
            }
        }
        "bilinear" => {
            // 16.16 fixed point pixel centers.
            let x_scale = ((sw as u64) << 16) / (dw as u64);
            let y_scale = ((sh as u64) << 16) / (dh as u64);
            let max_sx = sw - 1;
            let max_sy = sh - 1;
            for dy in 0..dh {
                if (dy & 15) == 0 {
                    check_deadline(deadline)?;
                }
                let src_y_q16 = (dy as i64) * (y_scale as i64) + ((y_scale as i64) >> 1) - 32768;
                let clamped_y = src_y_q16.clamp(0, (max_sy as i64) << 16);
                let y0 = (clamped_y >> 16) as usize;
                let y1 = (y0 + 1).min(max_sy);
                let wy = (clamped_y & 0xFFFF) as u64;
                let inv_wy = 65536 - wy;

                for dx in 0..dw {
                    let src_x_q16 =
                        (dx as i64) * (x_scale as i64) + ((x_scale as i64) >> 1) - 32768;
                    let clamped_x = src_x_q16.clamp(0, (max_sx as i64) << 16);
                    let x0 = (clamped_x >> 16) as usize;
                    let x1 = (x0 + 1).min(max_sx);
                    let wx = (clamped_x & 0xFFFF) as u64;
                    let inv_wx = 65536 - wx;

                    let w00 = (inv_wx * inv_wy) >> 16;
                    let w10 = (wx * inv_wy) >> 16;
                    let w01 = (inv_wx * wy) >> 16;
                    let w11 = 65536u64.saturating_sub(w00 + w10 + w01);

                    let p00 = &src.data[(y0 * sw + x0) * bpp..][..bpp];
                    let p10 = &src.data[(y0 * sw + x1) * bpp..][..bpp];
                    let p01 = &src.data[(y1 * sw + x0) * bpp..][..bpp];
                    let p11 = &src.data[(y1 * sw + x1) * bpp..][..bpp];
                    let di = (dy * dw + dx) * bpp;

                    if srgb && has_alpha {
                        let a00 = p00[3] as u64;
                        let a10 = p10[3] as u64;
                        let a01 = p01[3] as u64;
                        let a11 = p11[3] as u64;
                        let a_sum = a00 * w00 + a10 * w10 + a01 * w01 + a11 * w11;
                        let a_out = ((a_sum + 32768) >> 16).min(255) as u8;
                        out[di + 3] = a_out;
                        if a_sum == 0 {
                            out[di] = 0;
                            out[di + 1] = 0;
                            out[di + 2] = 0;
                        } else {
                            for c in 0..3 {
                                let acc = (SRGB_TO_LINEAR_U16[p00[c] as usize] as u64) * a00 * w00
                                    + (SRGB_TO_LINEAR_U16[p10[c] as usize] as u64) * a10 * w10
                                    + (SRGB_TO_LINEAR_U16[p01[c] as usize] as u64) * a01 * w01
                                    + (SRGB_TO_LINEAR_U16[p11[c] as usize] as u64) * a11 * w11;
                                let lin16 = ((acc + a_sum / 2) / a_sum).min(65535) as u16;
                                out[di + c] = linear16_to_srgb8(lin16);
                            }
                        }
                    } else if srgb {
                        for c in 0..bpp {
                            let is_a = has_alpha && c == 3;
                            let v00 = linear_u16_channel(p00, c, true, is_a) as u64;
                            let v10 = linear_u16_channel(p10, c, true, is_a) as u64;
                            let v01 = linear_u16_channel(p01, c, true, is_a) as u64;
                            let v11 = linear_u16_channel(p11, c, true, is_a) as u64;
                            let acc = v00 * w00 + v10 * w10 + v01 * w01 + v11 * w11;
                            let lin16 = ((acc + 32768) >> 16).min(65535) as u16;
                            out[di + c] = if is_a {
                                ((lin16 as u32 + 128) / 257).min(255) as u8
                            } else {
                                linear16_to_srgb8(lin16)
                            };
                        }
                    } else {
                        for c in 0..bpp {
                            let acc = (p00[c] as u64) * w00
                                + (p10[c] as u64) * w10
                                + (p01[c] as u64) * w01
                                + (p11[c] as u64) * w11;
                            out[di + c] = ((acc + 32768) >> 16).min(255) as u8;
                        }
                    }
                }
            }
        }
        _ => unreachable!(),
    }

    drop(_tmp);
    let digest = Digest::from_parts(&[
        b"op:resize|",
        &src.digest.0,
        &new_w.to_le_bytes(),
        &new_h.to_le_bytes(),
        norm.as_bytes(),
    ]);
    MediaImage::from_vec(
        new_w,
        new_h,
        src.format,
        src.color_space,
        SourceFormat::Derived,
        out,
        Some(src.has_alpha),
        Some(digest),
        budget_core,
    )
}

pub fn generate_mipmaps(
    src: &MediaImage,
    budget_core: Option<&Arc<BudgetCore>>,
    deadline: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<Rc<MipChain>, String> {
    let bpp = src.format.channels();
    let srgb = src.color_space == ColorSpace::Srgb;
    let has_alpha = src.format == PixelFormat::Rgba8;

    let mut dims: Vec<(u32, u32)> = Vec::new();
    let mut cw = src.width;
    let mut ch = src.height;
    let mut total_bytes = 0usize;
    loop {
        let lvl_bytes = (cw as usize)
            .checked_mul(ch as usize)
            .and_then(|p| p.checked_mul(bpp))
            .ok_or_else(|| err_limit("mipmap size overflow"))?;
        total_bytes = total_bytes
            .checked_add(lvl_bytes)
            .ok_or_else(|| err_limit("mipmap chain byte size overflow"))?;
        if total_bytes > MAX_MEDIA_BYTES {
            return Err(err_limit(format!(
                "mipmap chain {} bytes exceeds {} byte limit",
                total_bytes, MAX_MEDIA_BYTES
            )));
        }
        dims.push((cw, ch));
        if (cw == 1 && ch == 1) || dims.len() >= MAX_MIP_LEVELS {
            break;
        }
        cw = (cw / 2).max(1);
        ch = (ch / 2).max(1);
    }

    let guard = MediaBudgetGuard::charge(budget_core, total_bytes)?;
    let mut packed = Vec::with_capacity(total_bytes);
    let mut levels = Vec::with_capacity(dims.len());

    let l0_len = src.data.len();
    packed.extend_from_slice(&src.data);
    levels.push(MipLevel {
        width: src.width,
        height: src.height,
        offset: 0,
        len: l0_len,
    });

    for idx in 1..dims.len() {
        check_deadline(deadline)?;
        let prev_lvl = &levels[idx - 1];
        let pw = prev_lvl.width as usize;
        let ph = prev_lvl.height as usize;
        let prev_slice = &packed[prev_lvl.offset..prev_lvl.offset + prev_lvl.len];
        let (nw_u32, nh_u32) = dims[idx];
        let nw = nw_u32 as usize;
        let nh = nh_u32 as usize;
        let dst_offset = packed.len();
        let dst_len = nw * nh * bpp;
        let mut next_buf = vec![0u8; dst_len];

        for dy in 0..nh {
            let sy0 = dy * 2;
            let sy1 = (sy0 + 2).min(ph);
            for dx in 0..nw {
                let sx0 = dx * 2;
                let sx1 = (sx0 + 2).min(pw);
                let count = ((sy1 - sy0) * (sx1 - sx0)) as u64;
                let di = (dy * nw + dx) * bpp;

                if srgb && has_alpha {
                    let mut sum_rgb = [0u64; 3];
                    let mut sum_a = 0u64;
                    for sy in sy0..sy1 {
                        for sx in sx0..sx1 {
                            let si = (sy * pw + sx) * 4;
                            let a = prev_slice[si + 3] as u64;
                            sum_a += a;
                            for c in 0..3 {
                                sum_rgb[c] +=
                                    (SRGB_TO_LINEAR_U16[prev_slice[si + c] as usize] as u64) * a;
                            }
                        }
                    }
                    let avg_a = ((sum_a + count / 2) / count).min(255) as u8;
                    next_buf[di + 3] = avg_a;
                    if sum_a == 0 {
                        next_buf[di] = 0;
                        next_buf[di + 1] = 0;
                        next_buf[di + 2] = 0;
                    } else {
                        for c in 0..3 {
                            let lin16 = ((sum_rgb[c] + sum_a / 2) / sum_a).min(65535) as u16;
                            next_buf[di + c] = linear16_to_srgb8(lin16);
                        }
                    }
                } else if srgb {
                    let mut sum = [0u64; 4];
                    for sy in sy0..sy1 {
                        for sx in sx0..sx1 {
                            let si = (sy * pw + sx) * bpp;
                            for c in 0..bpp {
                                sum[c] += SRGB_TO_LINEAR_U16[prev_slice[si + c] as usize] as u64;
                            }
                        }
                    }
                    for c in 0..bpp {
                        let lin16 = ((sum[c] + count / 2) / count).min(65535) as u16;
                        next_buf[di + c] = linear16_to_srgb8(lin16);
                    }
                } else {
                    let mut sum = [0u64; 4];
                    for sy in sy0..sy1 {
                        for sx in sx0..sx1 {
                            let si = (sy * pw + sx) * bpp;
                            for c in 0..bpp {
                                sum[c] += prev_slice[si + c] as u64;
                            }
                        }
                    }
                    for c in 0..bpp {
                        next_buf[di + c] = ((sum[c] + count / 2) / count).min(255) as u8;
                    }
                }
            }
        }
        packed.extend_from_slice(&next_buf);
        levels.push(MipLevel {
            width: nw_u32,
            height: nh_u32,
            offset: dst_offset,
            len: dst_len,
        });
    }

    Ok(Rc::new(MipChain {
        levels,
        data: Rc::from(packed),
        _budget: guard,
    }))
}

#[inline]
fn isqrt_u128(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let shift = (128 - n.leading_zeros() + 1) & !1;
    let mut bit = 1u128 << (shift.saturating_sub(2));
    let mut rem = n;
    let mut res = 0u128;
    while bit > rem {
        bit >>= 2;
    }
    while bit != 0 {
        if rem >= res + bit {
            rem -= res + bit;
            res = (res >> 1) + bit;
        } else {
            res >>= 1;
        }
        bit >>= 2;
    }
    res
}

/// Pure integer Sobel heightmap -> Rg8 tangent-space normal (Linear).
pub fn height_to_normal(
    src: &MediaImage,
    strength: f64,
    budget_core: Option<&Arc<BudgetCore>>,
) -> Result<Rc<MediaImage>, String> {
    if !strength.is_finite() || !(0.0..=64.0).contains(&strength) {
        return Err(err_bad_opt(
            "height_to_normal strength must be finite and in 0.0..=64.0",
        ));
    }
    let w = src.width as usize;
    let h = src.height as usize;
    let r8_buf: std::borrow::Cow<'_, [u8]> = if src.format == PixelFormat::R8 {
        std::borrow::Cow::Borrowed(src.data.as_ref())
    } else {
        std::borrow::Cow::Owned(convert_buffer(
            &src.data,
            src.width,
            src.height,
            src.format,
            PixelFormat::R8,
        )?)
    };

    let strength_q8 = (strength * 256.0).round() as i64;
    let out_bytes = PixelFormat::Rg8.byte_len(src.width, src.height)?;
    let _tmp = temp_budget_guard(budget_core, out_bytes)?;
    let mut out = vec![0u8; out_bytes];

    let sample_h = |ix: isize, iy: isize| -> i64 {
        let cx = ix.clamp(0, w as isize - 1) as usize;
        let cy = iy.clamp(0, h as isize - 1) as usize;
        r8_buf[cy * w + cx] as i64
    };

    // Sobel denominator baseline: 255 * 4 = 1020 scaled by Q8 => nz base = 261120.
    const NZ_Q8: i128 = 255 * 256 * 2;
    for y in 0..h {
        let iy = y as isize;
        for x in 0..w {
            let ix = x as isize;
            let tl = sample_h(ix - 1, iy - 1);
            let t = sample_h(ix, iy - 1);
            let tr = sample_h(ix + 1, iy - 1);
            let l = sample_h(ix - 1, iy);
            let r = sample_h(ix + 1, iy);
            let bl = sample_h(ix - 1, iy + 1);
            let b = sample_h(ix, iy + 1);
            let br = sample_h(ix + 1, iy + 1);

            let gx = (tr + 2 * r + br) - (tl + 2 * l + bl);
            let gy = (bl + 2 * b + br) - (tl + 2 * t + tr);
            let nx_q = -(gx as i128) * (strength_q8 as i128);
            let ny_q = -(gy as i128) * (strength_q8 as i128);
            let len_sq = (nx_q * nx_q + ny_q * ny_q + NZ_Q8 * NZ_Q8) as u128;
            let len = isqrt_u128(len_sq).max(1) as i128;

            let enc_x = 128 + ((nx_q * 127 + if nx_q >= 0 { len / 2 } else { -(len / 2) }) / len);
            let enc_y = 128 + ((ny_q * 127 + if ny_q >= 0 { len / 2 } else { -(len / 2) }) / len);

            let di = (y * w + x) * 2;
            out[di] = enc_x.clamp(0, 255) as u8;
            out[di + 1] = enc_y.clamp(0, 255) as u8;
        }
    }

    drop(_tmp);
    let digest = Digest::from_parts(&[
        b"op:height_to_normal|",
        &src.digest.0,
        &(strength_q8 as i32).to_le_bytes(),
    ]);
    MediaImage::from_vec(
        src.width,
        src.height,
        PixelFormat::Rg8,
        ColorSpace::Linear,
        SourceFormat::Derived,
        out,
        Some(false),
        Some(digest),
        budget_core,
    )
}
