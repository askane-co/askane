use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::formats::RawDecodedImage;
use crate::media::image::{PixelFormat, SourceFormat};
use crate::media::{check_deadline, err_corrupt, err_limit, err_unsupported, validate_dims, MAX_DIM, MAX_MEDIA_BYTES};

#[inline]
fn decode_rgb15(lo: u8, hi: u8) -> [u8; 3] {
    let v = u16::from_le_bytes([lo, hi]);
    let r5 = ((v >> 10) & 0x1F) as u32;
    let g5 = ((v >> 5) & 0x1F) as u32;
    let b5 = (v & 0x1F) as u32;
    [
        ((r5 * 255 + 15) / 31) as u8,
        ((g5 * 255 + 15) / 31) as u8,
        ((b5 * 255 + 15) / 31) as u8,
    ]
}

pub fn decode_tga(
    data: &[u8],
    _budget: Option<&Arc<BudgetCore>>,
    deadline_check: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<RawDecodedImage, String> {
    if data.len() < 18 {
        return Err(err_corrupt("TGA header too short"));
    }
    let id_length = data[0] as usize;
    let color_map_type = data[1];
    let image_type = data[2];
    let cmap_len = u16::from_le_bytes([data[5], data[6]]) as usize;
    let cmap_entry_bits = data[7] as usize;
    let width = u16::from_le_bytes([data[12], data[13]]) as u32;
    let height = u16::from_le_bytes([data[14], data[15]]) as u32;
    let bpp = data[16];
    let descriptor = data[17];

    if color_map_type != 0 && (image_type == 1 || image_type == 9) {
        return Err(err_unsupported("Colormapped TGA images are not supported"));
    }
    if color_map_type != 0 && color_map_type != 1 {
        return Err(err_corrupt("Invalid TGA color map type"));
    }
    if !matches!(image_type, 2 | 3 | 10 | 11) {
        return Err(err_unsupported(format!(
            "Unsupported TGA image type {}",
            image_type
        )));
    }
    let pixels = validate_dims(width, height, MAX_DIM)?;
    let is_gray = image_type == 3 || image_type == 11;
    let is_rle = image_type == 10 || image_type == 11;

    if is_gray {
        if bpp != 8 && bpp != 16 {
            return Err(err_unsupported(format!(
                "Unsupported grayscale TGA bpp {}",
                bpp
            )));
        }
    } else if !matches!(bpp, 15 | 16 | 24 | 32) {
        return Err(err_unsupported(format!(
            "Unsupported truecolor TGA bpp {}",
            bpp
        )));
    }

    let cmap_bytes = if color_map_type == 1 {
        cmap_len.checked_mul(cmap_entry_bits.div_ceil(8))
            .ok_or_else(|| err_limit("TGA color map size overflow"))?
    } else {
        0
    };
    let mut pos = 18usize
        .checked_add(id_length)
        .and_then(|p| p.checked_add(cmap_bytes))
        .ok_or_else(|| err_corrupt("TGA offset overflow"))?;
    if pos > data.len() {
        return Err(err_corrupt("Truncated TGA header/id section"));
    }

    let src_bpp = match bpp {
        8 => 1usize,
        15 | 16 => 2usize,
        24 => 3usize,
        32 => 4usize,
        _ => unreachable!(),
    };
    let out_fmt = if is_gray {
        if bpp == 16 {
            PixelFormat::Rgba8
        } else {
            PixelFormat::R8
        }
    } else if bpp == 32 || (bpp == 16 && (descriptor & 0x0F) > 0) {
        PixelFormat::Rgba8
    } else {
        PixelFormat::Rgb8
    };
    let out_bpp = out_fmt.bpp();
    let total_bytes = pixels
        .checked_mul(out_bpp)
        .ok_or_else(|| err_limit("TGA pixel buffer overflow"))?;
    if total_bytes > MAX_MEDIA_BYTES {
        return Err(err_limit("TGA image exceeds 64 MiB limit"));
    }

    let mut out = vec![0u8; total_bytes];
    let flip_h = (descriptor & 0x10) != 0;
    let top_to_bottom = (descriptor & 0x20) != 0;
    let w = width as usize;
    let h = height as usize;

    let write_px = |dst: &mut [u8], idx: usize, s: &[u8]| {
        let x = idx % w;
        let y = idx / w;
        let dx = if flip_h { (w - 1) - x } else { x };
        let dy = if top_to_bottom { y } else { (h - 1) - y };
        let di = (dy * w + dx) * out_bpp;
        match out_fmt {
            PixelFormat::R8 => {
                dst[di] = s[0];
            }
            PixelFormat::Rgb8 => {
                if src_bpp == 2 {
                    let c = decode_rgb15(s[0], s[1]);
                    dst[di..di + 3].copy_from_slice(&c);
                } else {
                    dst[di] = s[2];
                    dst[di + 1] = s[1];
                    dst[di + 2] = s[0];
                }
            }
            PixelFormat::Rgba8 => {
                if is_gray {
                    dst[di] = s[0];
                    dst[di + 1] = s[0];
                    dst[di + 2] = s[0];
                    dst[di + 3] = s[1];
                } else if src_bpp == 2 {
                    let c = decode_rgb15(s[0], s[1]);
                    let a = if (s[1] & 0x80) != 0 { 255 } else { 0 };
                    dst[di..di + 3].copy_from_slice(&c);
                    dst[di + 3] = a;
                } else {
                    dst[di] = s[2];
                    dst[di + 1] = s[1];
                    dst[di + 2] = s[0];
                    dst[di + 3] = if src_bpp == 4 { s[3] } else { 255 };
                }
            }
            PixelFormat::Rg8 => unreachable!(),
        }
    };

    if !is_rle {
        let needed = pixels
            .checked_mul(src_bpp)
            .ok_or_else(|| err_limit("TGA payload overflow"))?;
        if pos.checked_add(needed).is_none_or(|e| e > data.len()) {
            return Err(err_corrupt("Truncated TGA uncompressed pixel data"));
        }
        for i in 0..pixels {
            if (i & 65535) == 0 {
                check_deadline(deadline_check)?;
            }
            let p = pos + i * src_bpp;
            write_px(&mut out, i, &data[p..p + src_bpp]);
        }
    } else {
        let mut idx = 0usize;
        while idx < pixels {
            if (idx & 65535) == 0 {
                check_deadline(deadline_check)?;
            }
            if pos >= data.len() {
                return Err(err_corrupt("Truncated TGA RLE stream"));
            }
            let hdr = data[pos];
            pos += 1;
            let count = ((hdr & 0x7F) as usize) + 1;
            if idx + count > pixels {
                return Err(err_corrupt("TGA RLE packet overruns image pixel count"));
            }
            if (hdr & 0x80) != 0 {
                if pos + src_bpp > data.len() {
                    return Err(err_corrupt("Truncated TGA RLE run pixel"));
                }
                let px = &data[pos..pos + src_bpp];
                pos += src_bpp;
                for k in 0..count {
                    write_px(&mut out, idx + k, px);
                }
            } else {
                let raw_bytes = count * src_bpp;
                if pos + raw_bytes > data.len() {
                    return Err(err_corrupt("Truncated TGA RLE raw packet"));
                }
                for k in 0..count {
                    let p = pos + k * src_bpp;
                    write_px(&mut out, idx + k, &data[p..p + src_bpp]);
                }
                pos += raw_bytes;
            }
            idx += count;
        }
    }

    Ok(RawDecodedImage {
        width,
        height,
        format: out_fmt,
        source: SourceFormat::Tga,
        data: out,
        frames: None,
        delays_ms: Vec::new(),
        loop_count: 0,
    })
}
