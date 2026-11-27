use crate::camera::Camera2D;
use crate::layers::{BlendMode, LayerBank};
use crate::surface::ClipRect;

#[inline]
pub fn blend_normal(dst: u32, src: u32, preserve_dst_alpha: bool) -> u32 {
    let sa = (src >> 24) & 0xFF;
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;
    crate::surface::blend_straight_argb(dst, sr, sg, sb, sa, preserve_dst_alpha)
}

#[inline]
pub fn blend_add(dst: u32, src: u32, preserve_dst_alpha: bool) -> u32 {
    let sa = (src >> 24) & 0xFF;
    if sa == 0 {
        return dst;
    }
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;
    let da = (dst >> 24) & 0xFF;
    let ar = (sr * sa + 127) / 255;
    let ag = (sg * sa + 127) / 255;
    let ab = (sb * sa + 127) / 255;
    let out_r = (dr + ar).min(255);
    let out_g = (dg + ag).min(255);
    let out_b = (db + ab).min(255);
    let out_a = if preserve_dst_alpha { 0 } else { da.max(sa) };
    (out_a << 24) | (out_r << 16) | (out_g << 8) | out_b
}

#[inline]
pub fn blend_multiply(dst: u32, src: u32, preserve_dst_alpha: bool) -> u32 {
    let sa = (src >> 24) & 0xFF;
    if sa == 0 {
        return dst;
    }
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;
    let da = (dst >> 24) & 0xFF;
    let mr = (sr * dr + 127) / 255;
    let mg = (sg * dg + 127) / 255;
    let mb = (sb * db + 127) / 255;
    let inv = 255 - sa;
    let out_r = (mr * sa + dr * inv + 127) / 255;
    let out_g = (mg * sa + dg * inv + 127) / 255;
    let out_b = (mb * sa + db * inv + 127) / 255;
    let out_a = if preserve_dst_alpha {
        0
    } else {
        (sa * 255 + da * inv + 127) / 255
    };
    (out_a << 24) | (out_r << 16) | (out_g << 8) | out_b
}

#[inline]
fn apply_blend(mode: BlendMode, dst: u32, src: u32, preserve_dst_alpha: bool) -> u32 {
    match mode {
        BlendMode::Normal => blend_normal(dst, src, preserve_dst_alpha),
        BlendMode::Add => blend_add(dst, src, preserve_dst_alpha),
        BlendMode::Multiply => blend_multiply(dst, src, preserve_dst_alpha),
    }
}

/// Composites all visible alive layers sorted stably by `(z_order, slot)` onto `dst`.
/// Returns the total destination pixel count visited so callers can charge VM work budget.
pub fn compose_layers(
    dst: &mut [u32],
    dst_w: i32,
    dst_h: i32,
    dst_clip: ClipRect,
    dst_is_offscreen: bool,
    layers: &LayerBank,
    cam: &Camera2D,
) -> u64 {
    if dst_w <= 0 || dst_h <= 0 {
        return 0;
    }
    let mut order: Vec<(i32, usize)> = layers
        .slots
        .iter()
        .enumerate()
        .filter_map(|(idx, slot)| {
            slot.as_ref()
                .filter(|l| l.alive && l.visible)
                .map(|l| (l.z_order, idx))
        })
        .collect();
    order.sort_by_key(|&(z, idx)| (z, idx));

    let mut total_pixels: u64 = 0;
    let dst_w_u = dst_w as usize;

    for &(_, idx) in &order {
        let Some(layer) = layers.slots[idx].as_ref() else {
            continue;
        };
        let sw = layer.surface.width;
        let sh = layer.surface.height;
        if sw <= 0 || sh <= 0 {
            continue;
        }

        let (ox, oy) = if cam.enabled {
            (
                -(cam.x * layer.parallax_x).round() as i32 + cam.shake_offset_x.round() as i32,
                -(cam.y * layer.parallax_y).round() as i32 + cam.shake_offset_y.round() as i32,
            )
        } else {
            (cam.shake_offset_x.round() as i32, cam.shake_offset_y.round() as i32)
        };

        let x0 = ox.max(dst_clip.x0);
        let y0 = oy.max(dst_clip.y0);
        let x1 = ox.saturating_add(sw).min(dst_clip.x1);
        let y1 = oy.saturating_add(sh).min(dst_clip.y1);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }

        let span_w = (x1 - x0) as u64;
        let span_h = (y1 - y0) as u64;
        total_pixels = total_pixels.saturating_add(span_w.saturating_mul(span_h));

        let sw_u = sw as usize;
        let src = &layer.surface.pixels;
        let mode = layer.mode;
        for py in y0..y1 {
            let sy = (py - oy) as usize;
            let srow = sy * sw_u;
            let drow = (py as usize) * dst_w_u;
            for px in x0..x1 {
                let sx = (px - ox) as usize;
                let sc = src[srow + sx];
                if (sc >> 24) == 0 {
                    continue;
                }
                let d_idx = drow + px as usize;
                dst[d_idx] = apply_blend(mode, dst[d_idx], sc, !dst_is_offscreen);
            }
        }
    }

    total_pixels
}
