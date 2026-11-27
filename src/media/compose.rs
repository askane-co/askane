use crate::media::color::SRGB_TO_LINEAR_U16;
use crate::media::image::{ColorSpace, Filter, MediaImage, PixelFormat, WrapMode};
use crate::media::sample::{read_rgba_texel, sample_buffer_bilinear, sample_buffer_nearest};
use crate::surface::ClipRect;

pub struct BlitTarget<'a> {
    pub pixels: &'a mut [u32],
    pub width: i32,
    pub height: i32,
    pub clip: Option<ClipRect>,
    pub is_offscreen: bool,
}

#[derive(Clone, Debug)]
pub struct BlitOpts {
    pub src_x: i32,
    pub src_y: i32,
    pub src_w: Option<i32>,
    pub src_h: Option<i32>,
    pub dst_x: f64,
    pub dst_y: f64,
    pub dst_w: Option<f64>,
    pub dst_h: Option<f64>,
    pub scale_x: f64,
    pub scale_y: f64,
    pub rotation_deg: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub flip_x: bool,
    pub flip_y: bool,
    pub tint: u32,
    pub opacity: u8,
    pub color_key: Option<u32>,
    pub filter: Filter,
}

impl Default for BlitOpts {
    fn default() -> Self {
        Self {
            src_x: 0,
            src_y: 0,
            src_w: None,
            src_h: None,
            dst_x: 0.0,
            dst_y: 0.0,
            dst_w: None,
            dst_h: None,
            scale_x: 1.0,
            scale_y: 1.0,
            rotation_deg: 0.0,
            origin_x: 0.0,
            origin_y: 0.0,
            flip_x: false,
            flip_y: false,
            tint: 0x00FF_FFFF,
            opacity: 255,
            color_key: None,
            filter: Filter::Nearest,
        }
    }
}

#[inline]
fn blend_over_argb(src_r: u32, src_g: u32, src_b: u32, src_a: u32, dst_px: u32) -> u32 {
    if src_a == 0 {
        return dst_px;
    }
    if src_a >= 255 {
        return 0xFF00_0000 | (src_r << 16) | (src_g << 8) | src_b;
    }
    let inv = 255 - src_a;
    let da = (dst_px >> 24) & 0xFF;
    let dr = (dst_px >> 16) & 0xFF;
    let dg = (dst_px >> 8) & 0xFF;
    let db = dst_px & 0xFF;
    let r = (src_r * src_a + dr * inv + 127) / 255;
    let g = (src_g * src_a + dg * inv + 127) / 255;
    let b = (src_b * src_a + db * inv + 127) / 255;
    let a = src_a + ((da * inv + 127) / 255);
    (a.min(255) << 24) | (r << 16) | (g << 8) | b
}

#[inline]
fn write_blitted_pixel(
    dst: &mut u32,
    rgba: [u8; 4],
    img_has_alpha: bool,
    is_offscreen: bool,
    tint: u32,
    opacity: u8,
    color_key: Option<u32>,
) {
    let orig_rgb = ((rgba[0] as u32) << 16) | ((rgba[1] as u32) << 8) | (rgba[2] as u32);
    if color_key == Some(orig_rgb) {
        return;
    }
    let mut r = rgba[0] as u32;
    let mut g = rgba[1] as u32;
    let mut b = rgba[2] as u32;
    let mut a = rgba[3] as u32;
    if tint != 0x00FF_FFFF {
        let tr = (tint >> 16) & 0xFF;
        let tg = (tint >> 8) & 0xFF;
        let tb = tint & 0xFF;
        r = (r * tr + 127) / 255;
        g = (g * tg + 127) / 255;
        b = (b * tb + 127) / 255;
    }
    if opacity < 255 {
        a = (a * (opacity as u32) + 127) / 255;
    }

    if !img_has_alpha && opacity == 255 {
        let rgb = (r << 16) | (g << 8) | b;
        *dst = if is_offscreen {
            0xFF00_0000 | rgb
        } else {
            rgb
        };
        return;
    }

    if a == 0 {
        return;
    }
    if a >= 255 {
        let rgb = (r << 16) | (g << 8) | b;
        *dst = if is_offscreen {
            0xFF00_0000 | rgb
        } else {
            rgb
        };
    } else if is_offscreen {
        *dst = blend_over_argb(r, g, b, a, *dst);
    } else {
        *dst = blend_over_argb(r, g, b, a, 0xFF00_0000 | *dst) & 0x00FF_FFFF;
    }
}

pub fn blit_image(dst: &mut BlitTarget<'_>, img: &MediaImage, opts: &BlitOpts) {
    if dst.width <= 0 || dst.height <= 0 || opts.opacity == 0 {
        return;
    }
    let clip = dst
        .clip
        .unwrap_or_else(|| ClipRect::full(dst.width, dst.height))
        .intersect(ClipRect::full(dst.width, dst.height));
    if clip.x0 >= clip.x1 || clip.y0 >= clip.y1 {
        return;
    }

    let sw = img.width as i32;
    let sh = img.height as i32;
    let sx0 = opts.src_x.max(0);
    let sy0 = opts.src_y.max(0);
    if sx0 >= sw || sy0 >= sh {
        return;
    }
    let sub_w = opts.src_w.unwrap_or(sw - sx0).min(sw - sx0);
    let sub_h = opts.src_h.unwrap_or(sh - sy0).min(sh - sy0);
    if sub_w <= 0 || sub_h <= 0 {
        return;
    }

    let target_w = opts
        .dst_w
        .unwrap_or_else(|| (sub_w as f64) * opts.scale_x.abs());
    let target_h = opts
        .dst_h
        .unwrap_or_else(|| (sub_h as f64) * opts.scale_y.abs());
    if !target_w.is_finite() || !target_h.is_finite() || target_w <= 0.0 || target_h <= 0.0 {
        return;
    }

    let eff_flip_x = opts.flip_x ^ (opts.scale_x < 0.0);
    let eff_flip_y = opts.flip_y ^ (opts.scale_y < 0.0);

    let is_fast_1to1 = opts.rotation_deg.abs() < 1e-6
        && !eff_flip_x
        && !eff_flip_y
        && opts.origin_x.abs() < 1e-6
        && opts.origin_y.abs() < 1e-6
        && (target_w - sub_w as f64).abs() < 1e-6
        && (target_h - sub_h as f64).abs() < 1e-6
        && opts.tint == 0x00FF_FFFF
        && opts.opacity == 255;

    let img_alpha = img.has_alpha || img.format == PixelFormat::Rgba8;
    let buf_w = dst.width as usize;

    if is_fast_1to1 {
        let dx = opts.dst_x.round() as i32;
        let dy = opts.dst_y.round() as i32;
        let x0 = dx.max(clip.x0);
        let y0 = dy.max(clip.y0);
        let x1 = dx.saturating_add(sub_w).min(clip.x1);
        let y1 = dy.saturating_add(sub_h).min(clip.y1);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for py in y0..y1 {
            let sy = sy0 + (py - dy);
            let drow = (py as usize) * buf_w;
            for px in x0..x1 {
                let sx = sx0 + (px - dx);
                let rgba = read_rgba_texel(&img.data, img.width, img.height, img.format, sx, sy);
                write_blitted_pixel(
                    &mut dst.pixels[drow + px as usize],
                    rgba,
                    img_alpha,
                    dst.is_offscreen,
                    0x00FF_FFFF,
                    255,
                    opts.color_key,
                );
            }
        }
        return;
    }

    let rad = opts.rotation_deg.to_radians();
    let cos_a = rad.cos();
    let sin_a = rad.sin();
    let ox = opts.origin_x * target_w;
    let oy = opts.origin_y * target_h;
    let px_origin = opts.dst_x;
    let py_origin = opts.dst_y;

    // Corners relative to pivot -> world bounding box.
    let corners = [
        (-ox, -oy),
        (target_w - ox, -oy),
        (-ox, target_h - oy),
        (target_w - ox, target_h - oy),
    ];
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (cx, cy) in corners {
        let wx = px_origin + cx * cos_a - cy * sin_a;
        let wy = py_origin + cx * sin_a + cy * cos_a;
        min_x = min_x.min(wx);
        max_x = max_x.max(wx);
        min_y = min_y.min(wy);
        max_y = max_y.max(wy);
    }

    let x0 = (min_x.floor() as i32).max(clip.x0);
    let y0 = (min_y.floor() as i32).max(clip.y0);
    let x1 = (max_x.ceil() as i32).saturating_add(1).min(clip.x1);
    let y1 = (max_y.ceil() as i32).saturating_add(1).min(clip.y1);
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let bilinear = matches!(
        opts.filter,
        Filter::Linear | Filter::MipNearest | Filter::MipLinear
    );
    let inv_w = (sub_w as f64) / target_w;
    let inv_h = (sub_h as f64) / target_h;
    let iw_f = img.width as f64;
    let ih_f = img.height as f64;

    for py in y0..y1 {
        let drow = (py as usize) * buf_w;
        let rel_y = (py as f64 + 0.5) - py_origin;
        for px in x0..x1 {
            let rel_x = (px as f64 + 0.5) - px_origin;
            let lx = rel_x * cos_a + rel_y * sin_a + ox;
            let ly = -rel_x * sin_a + rel_y * cos_a + oy;
            if lx < 0.0 || ly < 0.0 || lx >= target_w || ly >= target_h {
                continue;
            }
            let mut u_px = lx * inv_w;
            let mut v_px = ly * inv_h;
            if eff_flip_x {
                u_px = (sub_w as f64) - u_px;
            }
            if eff_flip_y {
                v_px = (sub_h as f64) - v_px;
            }
            let abs_x = (sx0 as f64) + u_px.clamp(0.0, (sub_w as f64) - 1e-4);
            let abs_y = (sy0 as f64) + v_px.clamp(0.0, (sub_h as f64) - 1e-4);

            let rgba = if bilinear {
                let u_q16 = ((abs_x / iw_f) * 65536.0).round() as i64;
                let v_q16 = ((abs_y / ih_f) * 65536.0).round() as i64;
                sample_buffer_bilinear(
                    &img.data,
                    img.width,
                    img.height,
                    img.format,
                    img.color_space,
                    u_q16,
                    v_q16,
                    WrapMode::Clamp,
                    WrapMode::Clamp,
                )
            } else {
                let u_q16 = ((abs_x / iw_f) * 65536.0).round() as i64;
                let v_q16 = ((abs_y / ih_f) * 65536.0).round() as i64;
                sample_buffer_nearest(
                    &img.data,
                    img.width,
                    img.height,
                    img.format,
                    u_q16,
                    v_q16,
                    WrapMode::Clamp,
                    WrapMode::Clamp,
                )
            };

            write_blitted_pixel(
                &mut dst.pixels[drow + px as usize],
                rgba,
                img_alpha,
                dst.is_offscreen,
                opts.tint,
                opts.opacity,
                opts.color_key,
            );
        }
    }
}

pub fn blit_nine_slice(
    dst: &mut BlitTarget<'_>,
    img: &MediaImage,
    dx: i32,
    dy: i32,
    dw: i32,
    dh: i32,
    slice: [i32; 4],
) {
    let sw = img.width as i32;
    let sh = img.height as i32;
    if sw <= 0 || sh <= 0 || dw <= 0 || dh <= 0 {
        return;
    }
    let sl = slice[0].clamp(0, sw / 2);
    let sr = slice[2].clamp(0, sw - sl);
    let st = slice[1].clamp(0, sh / 2);
    let sb = slice[3].clamp(0, sh - st);

    let dl = sl.min(dw);
    let dr = sr.min((dw - dl).max(0));
    let dt = st.min(dh);
    let db = sb.min((dh - dt).max(0));

    let sx_bounds = [0, sl, sw - sr, sw];
    let sy_bounds = [0, st, sh - sb, sh];
    let dx_bounds = [dx, dx + dl, dx + dw - dr, dx + dw];
    let dy_bounds = [dy, dy + dt, dy + dh - db, dy + dh];

    let _unused = (SRGB_TO_LINEAR_U16[0], ColorSpace::Srgb);
    for ry in 0..3 {
        let sy0 = sy_bounds[ry];
        let sub_h = sy_bounds[ry + 1] - sy0;
        let dest_y = dy_bounds[ry];
        let dest_h = dy_bounds[ry + 1] - dest_y;
        if sub_h <= 0 || dest_h <= 0 {
            continue;
        }
        for rx in 0..3 {
            let sx0 = sx_bounds[rx];
            let sub_w = sx_bounds[rx + 1] - sx0;
            let dest_x = dx_bounds[rx];
            let dest_w = dx_bounds[rx + 1] - dest_x;
            if sub_w <= 0 || dest_w <= 0 {
                continue;
            }
            let opts = BlitOpts {
                src_x: sx0,
                src_y: sy0,
                src_w: Some(sub_w),
                src_h: Some(sub_h),
                dst_x: dest_x as f64,
                dst_y: dest_y as f64,
                dst_w: Some(dest_w as f64),
                dst_h: Some(dest_h as f64),
                filter: Filter::Nearest,
                ..BlitOpts::default()
            };
            blit_image(dst, img, &opts);
        }
    }
}
