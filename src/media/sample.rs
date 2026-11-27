use crate::media::color::{SRGB_TO_LINEAR_U16, linear16_to_srgb8};
use crate::media::err_bad_opt;
use crate::media::image::{ColorSpace, Filter, MediaImage, PixelFormat, TextureData, WrapMode};

#[inline]
pub fn wrap_coord(coord: i32, size: i32, mode: WrapMode) -> i32 {
    if size <= 1 {
        return 0;
    }
    match mode {
        WrapMode::Clamp => coord.clamp(0, size - 1),
        WrapMode::Repeat => coord.rem_euclid(size),
        WrapMode::Mirror => {
            let period = size * 2;
            let m = coord.rem_euclid(period);
            if m < size { m } else { period - 1 - m }
        }
    }
}

#[inline]
pub fn read_rgba_texel(
    data: &[u8],
    width: u32,
    height: u32,
    fmt: PixelFormat,
    x: i32,
    y: i32,
) -> [u8; 4] {
    let w = width as usize;
    let h = height as usize;
    let ux = (x.clamp(0, w as i32 - 1)) as usize;
    let uy = (y.clamp(0, h as i32 - 1)) as usize;
    let idx = (uy * w + ux) * fmt.channels();
    match fmt {
        PixelFormat::R8 => {
            let v = data[idx];
            [v, v, v, 255]
        }
        PixelFormat::Rg8 => [data[idx], data[idx + 1], 0, 255],
        PixelFormat::Rgb8 => [data[idx], data[idx + 1], data[idx + 2], 255],
        PixelFormat::Rgba8 => [data[idx], data[idx + 1], data[idx + 2], data[idx + 3]],
    }
}

pub fn sample_buffer_nearest(
    data: &[u8],
    width: u32,
    height: u32,
    fmt: PixelFormat,
    u_q16: i64,
    v_q16: i64,
    wrap_u: WrapMode,
    wrap_v: WrapMode,
) -> [u8; 4] {
    let tx = ((u_q16 * (width as i64)) >> 16) as i32;
    let ty = ((v_q16 * (height as i64)) >> 16) as i32;
    let cx = wrap_coord(tx, width as i32, wrap_u);
    let cy = wrap_coord(ty, height as i32, wrap_v);
    read_rgba_texel(data, width, height, fmt, cx, cy)
}

pub fn sample_buffer_bilinear(
    data: &[u8],
    width: u32,
    height: u32,
    fmt: PixelFormat,
    color_space: ColorSpace,
    u_q16: i64,
    v_q16: i64,
    wrap_u: WrapMode,
    wrap_v: WrapMode,
) -> [u8; 4] {
    let sw = width as i32;
    let sh = height as i32;
    let fx_q16 = u_q16 * (sw as i64) - 32768;
    let fy_q16 = v_q16 * (sh as i64) - 32768;

    let x0_raw = fx_q16.div_euclid(65536) as i32;
    let y0_raw = fy_q16.div_euclid(65536) as i32;
    let wx = fx_q16.rem_euclid(65536) as u64;
    let wy = fy_q16.rem_euclid(65536) as u64;
    let inv_wx = 65536 - wx;
    let inv_wy = 65536 - wy;

    let x0 = wrap_coord(x0_raw, sw, wrap_u);
    let x1 = wrap_coord(x0_raw + 1, sw, wrap_u);
    let y0 = wrap_coord(y0_raw, sh, wrap_v);
    let y1 = wrap_coord(y0_raw + 1, sh, wrap_v);

    let p00 = read_rgba_texel(data, width, height, fmt, x0, y0);
    let p10 = read_rgba_texel(data, width, height, fmt, x1, y0);
    let p01 = read_rgba_texel(data, width, height, fmt, x0, y1);
    let p11 = read_rgba_texel(data, width, height, fmt, x1, y1);

    let w00 = (inv_wx * inv_wy) >> 16;
    let w10 = (wx * inv_wy) >> 16;
    let w01 = (inv_wx * wy) >> 16;
    let w11 = 65536u64.saturating_sub(w00 + w10 + w01);

    if color_space == ColorSpace::Srgb {
        if fmt == PixelFormat::Rgba8 {
            let a00 = p00[3] as u64;
            let a10 = p10[3] as u64;
            let a01 = p01[3] as u64;
            let a11 = p11[3] as u64;
            let a_sum = a00 * w00 + a10 * w10 + a01 * w01 + a11 * w11;
            let a_out = ((a_sum + 32768) >> 16).min(255) as u8;
            if a_sum == 0 {
                return [0, 0, 0, 0];
            }
            let mut out = [0, 0, 0, a_out];
            for c in 0..3 {
                let acc = (SRGB_TO_LINEAR_U16[p00[c] as usize] as u64) * a00 * w00
                    + (SRGB_TO_LINEAR_U16[p10[c] as usize] as u64) * a10 * w10
                    + (SRGB_TO_LINEAR_U16[p01[c] as usize] as u64) * a01 * w01
                    + (SRGB_TO_LINEAR_U16[p11[c] as usize] as u64) * a11 * w11;
                let lin16 = ((acc + a_sum / 2) / a_sum).min(65535) as u16;
                out[c] = linear16_to_srgb8(lin16);
            }
            out
        } else {
            let mut out = [0, 0, 0, 255];
            for c in 0..3 {
                let acc = (SRGB_TO_LINEAR_U16[p00[c] as usize] as u64) * w00
                    + (SRGB_TO_LINEAR_U16[p10[c] as usize] as u64) * w10
                    + (SRGB_TO_LINEAR_U16[p01[c] as usize] as u64) * w01
                    + (SRGB_TO_LINEAR_U16[p11[c] as usize] as u64) * w11;
                let lin16 = ((acc + 32768) >> 16).min(65535) as u16;
                out[c] = linear16_to_srgb8(lin16);
            }
            out
        }
    } else {
        let mut out = [0u8; 4];
        for c in 0..4 {
            let acc = (p00[c] as u64) * w00
                + (p10[c] as u64) * w10
                + (p01[c] as u64) * w01
                + (p11[c] as u64) * w11;
            out[c] = ((acc + 32768) >> 16).min(255) as u8;
        }
        out
    }
}

pub fn sample_image_nearest(
    img: &MediaImage,
    u_q16: i64,
    v_q16: i64,
    wrap_u: WrapMode,
    wrap_v: WrapMode,
) -> [u8; 4] {
    sample_buffer_nearest(
        &img.data,
        img.width,
        img.height,
        img.format,
        u_q16,
        v_q16,
        wrap_u,
        wrap_v,
    )
}

pub fn sample_texture_q16(
    tex: &TextureData,
    u_q16: i64,
    v_q16: i64,
    lod_q8: i32,
) -> [u8; 4] {
    let cs = if tex.srgb {
        ColorSpace::Srgb
    } else {
        ColorSpace::Linear
    };
    let img = &tex.image;
    match tex.filter {
        Filter::Nearest => sample_buffer_nearest(
            &img.data,
            img.width,
            img.height,
            img.format,
            u_q16,
            v_q16,
            tex.wrap_u,
            tex.wrap_v,
        ),
        Filter::Linear => sample_buffer_bilinear(
            &img.data,
            img.width,
            img.height,
            img.format,
            cs,
            u_q16,
            v_q16,
            tex.wrap_u,
            tex.wrap_v,
        ),
        Filter::MipNearest => {
            if let Some(chain) = img.mips.borrow().as_ref() {
                let max_l = (chain.levels.len().saturating_sub(1)) as i32;
                let idx = ((lod_q8 + 128) >> 8).clamp(0, max_l) as usize;
                if let Some((w, h, slice)) = chain.level_slice(idx) {
                    return sample_buffer_bilinear(
                        slice,
                        w,
                        h,
                        img.format,
                        cs,
                        u_q16,
                        v_q16,
                        tex.wrap_u,
                        tex.wrap_v,
                    );
                }
            }
            sample_buffer_bilinear(
                &img.data,
                img.width,
                img.height,
                img.format,
                cs,
                u_q16,
                v_q16,
                tex.wrap_u,
                tex.wrap_v,
            )
        }
        Filter::MipLinear => {
            if let Some(chain) = img.mips.borrow().as_ref() {
                let max_l = (chain.levels.len().saturating_sub(1)) as i32;
                let clamped = lod_q8.clamp(0, max_l << 8);
                let l0 = (clamped >> 8) as usize;
                let l1 = (l0 + 1).min(max_l as usize);
                let frac = (clamped & 0xFF) as u32;
                if let (Some((w0, h0, s0)), Some((w1, h1, s1))) =
                    (chain.level_slice(l0), chain.level_slice(l1))
                {
                    let c0 = sample_buffer_bilinear(
                        s0, w0, h0, img.format, cs, u_q16, v_q16, tex.wrap_u, tex.wrap_v,
                    );
                    if l0 == l1 || frac == 0 {
                        return c0;
                    }
                    let c1 = sample_buffer_bilinear(
                        s1, w1, h1, img.format, cs, u_q16, v_q16, tex.wrap_u, tex.wrap_v,
                    );
                    let inv = 256 - frac;
                    return [
                        (((c0[0] as u32) * inv + (c1[0] as u32) * frac + 128) >> 8) as u8,
                        (((c0[1] as u32) * inv + (c1[1] as u32) * frac + 128) >> 8) as u8,
                        (((c0[2] as u32) * inv + (c1[2] as u32) * frac + 128) >> 8) as u8,
                        (((c0[3] as u32) * inv + (c1[3] as u32) * frac + 128) >> 8) as u8,
                    ];
                }
            }
            sample_buffer_bilinear(
                &img.data,
                img.width,
                img.height,
                img.format,
                cs,
                u_q16,
                v_q16,
                tex.wrap_u,
                tex.wrap_v,
            )
        }
    }
}

pub fn sample_texture_uv(
    tex: &TextureData,
    u: f64,
    v: f64,
    lod: f64,
) -> Result<[u8; 4], String> {
    if !u.is_finite() || !v.is_finite() || !lod.is_finite() {
        return Err(err_bad_opt("texture sample UV and LOD must be finite"));
    }
    let u_q16 = (u.clamp(-4096.0, 4096.0) * 65536.0).round() as i64;
    let v_q16 = (v.clamp(-4096.0, 4096.0) * 65536.0).round() as i64;
    let lod_q8 = (lod.clamp(0.0, 16.0) * 256.0).round() as i32;
    Ok(sample_texture_q16(tex, u_q16, v_q16, lod_q8))
}
