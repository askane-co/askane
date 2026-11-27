use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::formats::RawDecodedImage;
use crate::media::image::{PixelFormat, SourceFormat};
use crate::media::{
    check_deadline, err_corrupt, err_limit, err_unsupported, validate_dims, MAX_BMP_DIM,
    MAX_MEDIA_BYTES,
};

fn bitfield_extract(val: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let shifted = (val & mask) >> shift;
    let width = (mask >> shift).count_ones();
    if width == 0 {
        return 0;
    }
    let max_v = (1u32 << width.min(16)) - 1;
    ((shifted.min(max_v) * 255 + (max_v / 2)) / max_v).min(255) as u8
}

pub fn decode_bmp(
    data: &[u8],
    _budget: Option<&Arc<BudgetCore>>,
    deadline_check: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<RawDecodedImage, String> {
    if data.len() < 54 || &data[0..2] != b"BM" {
        return Err(err_corrupt("Invalid BMP signature"));
    }
    let offset = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
    let dib_size = u32::from_le_bytes([data[14], data[15], data[16], data[17]]) as usize;
    if dib_size < 40 || 14usize.saturating_add(dib_size) > data.len() {
        return Err(err_corrupt("Unsupported or truncated BMP DIB header"));
    }
    let width_i = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
    let raw_height_i = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
    let bpp = u16::from_le_bytes([data[28], data[29]]);
    let compression = u32::from_le_bytes([data[30], data[31], data[32], data[33]]);
    let clr_used = u32::from_le_bytes([data[46], data[47], data[48], data[49]]) as usize;

    if width_i <= 0 || raw_height_i == 0 {
        return Err(err_corrupt("BMP dimensions must be non-zero"));
    }
    let width = width_i as u32;
    let height = raw_height_i.unsigned_abs();
    let _pixels = validate_dims(width, height, MAX_BMP_DIM)?;
    let top_down = raw_height_i < 0;

    let valid_bpp = matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32);
    if !valid_bpp {
        return Err(err_unsupported(format!("Unsupported BMP bit depth {}", bpp)));
    }
    match compression {
        0 => {} // BI_RGB
        3 if matches!(bpp, 16 | 32) => {} // BI_BITFIELDS
        1 | 2 | 4 | 5 => {
            return Err(err_unsupported(format!(
                "Unsupported BMP compression {} (RLE/JPEG/PNG not supported)",
                compression
            )));
        }
        other => {
            return Err(err_unsupported(format!(
                "Unsupported BMP compression {}",
                other
            )));
        }
    }

    let mut masks = [0x00FF_0000u32, 0x0000_FF00u32, 0x0000_00FFu32, 0xFF00_0000u32];
    if compression == 3 {
        if dib_size >= 52 && 14 + 52 <= data.len() {
            masks[0] = u32::from_le_bytes([data[54], data[55], data[56], data[57]]);
            masks[1] = u32::from_le_bytes([data[58], data[59], data[60], data[61]]);
            masks[2] = u32::from_le_bytes([data[62], data[63], data[64], data[65]]);
            masks[3] = if dib_size >= 56 && 14 + 56 <= data.len() {
                u32::from_le_bytes([data[66], data[67], data[68], data[69]])
            } else {
                0
            };
        } else if 14 + dib_size + 12 <= data.len() {
            let m0 = 14 + dib_size;
            masks[0] = u32::from_le_bytes([data[m0], data[m0 + 1], data[m0 + 2], data[m0 + 3]]);
            masks[1] = u32::from_le_bytes([data[m0 + 4], data[m0 + 5], data[m0 + 6], data[m0 + 7]]);
            masks[2] = u32::from_le_bytes([data[m0 + 8], data[m0 + 9], data[m0 + 10], data[m0 + 11]]);
            masks[3] = 0;
        }
    } else if bpp == 16 {
        masks = [0x7C00, 0x03E0, 0x001F, 0];
    }

    let mut palette: Vec<[u8; 3]> = Vec::new();
    if bpp <= 8 {
        let max_entries = 1usize << (bpp as usize);
        let entry_count = if clr_used == 0 {
            max_entries
        } else {
            clr_used.min(max_entries)
        };
        let pal_start = 14usize
            .checked_add(dib_size)
            .ok_or_else(|| err_corrupt("BMP palette offset overflow"))?;
        let pal_end = pal_start
            .checked_add(entry_count * 4)
            .ok_or_else(|| err_corrupt("BMP palette size overflow"))?;
        if pal_end > data.len() || offset < pal_end {
            return Err(err_corrupt("BMP palette out of bounds"));
        }
        palette.reserve(entry_count);
        for i in 0..entry_count {
            let p = pal_start + i * 4;
            palette.push([data[p + 2], data[p + 1], data[p]]);
        }
    } else if offset < 14 + dib_size {
        return Err(err_corrupt("BMP pixel offset points inside DIB header"));
    }

    let w = width as usize;
    let h = height as usize;
    let row_bits = w
        .checked_mul(bpp as usize)
        .ok_or_else(|| err_limit("BMP row bits overflow"))?;
    let row_bytes = row_bits.div_ceil(8);
    let stride = (row_bytes + 3) & !3;
    let needed = offset
        .checked_add(
            stride
                .checked_mul(h)
                .ok_or_else(|| err_limit("BMP pixel data size overflow"))?,
        )
        .ok_or_else(|| err_corrupt("BMP offset overflow"))?;
    if needed > data.len() {
        return Err(err_corrupt("Truncated BMP pixel data"));
    }

    let has_alpha = compression == 3 && masks[3] != 0;
    let out_fmt = if has_alpha {
        PixelFormat::Rgba8
    } else {
        PixelFormat::Rgb8
    };
    let out_bpp = out_fmt.bpp();
    let total_bytes = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(out_bpp))
        .ok_or_else(|| err_limit("BMP output buffer overflow"))?;
    if total_bytes > MAX_MEDIA_BYTES {
        return Err(err_limit("BMP decoded image exceeds 64 MiB limit"));
    }

    let mut out = vec![0u8; total_bytes];
    for y in 0..h {
        if (y & 255) == 0 {
            check_deadline(deadline_check)?;
        }
        let src_y = if top_down { y } else { (h - 1) - y };
        let row_start = offset + src_y * stride;
        let row = &data[row_start..row_start + stride];
        let dst_row = y * w * out_bpp;
        for x in 0..w {
            let di = dst_row + x * out_bpp;
            match bpp {
                1 | 4 | 8 => {
                    let per_byte = 8 / (bpp as usize);
                    let byte = row[x / per_byte];
                    let idx = if bpp == 8 {
                        byte as usize
                    } else {
                        let bits = bpp as usize;
                        let shift = 8 - bits * (1 + (x % per_byte));
                        ((byte >> shift) & ((1u8 << bits) - 1)) as usize
                    };
                    let c = *palette
                        .get(idx)
                        .ok_or_else(|| err_corrupt("BMP palette index out of bounds"))?;
                    out[di] = c[0];
                    out[di + 1] = c[1];
                    out[di + 2] = c[2];
                }
                16 => {
                    let px = u16::from_le_bytes([row[x * 2], row[x * 2 + 1]]) as u32;
                    out[di] = bitfield_extract(px, masks[0]);
                    out[di + 1] = bitfield_extract(px, masks[1]);
                    out[di + 2] = bitfield_extract(px, masks[2]);
                    if out_fmt == PixelFormat::Rgba8 {
                        out[di + 3] = bitfield_extract(px, masks[3]);
                    }
                }
                24 => {
                    let p = x * 3;
                    out[di] = row[p + 2];
                    out[di + 1] = row[p + 1];
                    out[di + 2] = row[p];
                }
                32 => {
                    let p = x * 4;
                    if compression == 3 {
                        let px = u32::from_le_bytes([row[p], row[p + 1], row[p + 2], row[p + 3]]);
                        out[di] = bitfield_extract(px, masks[0]);
                        out[di + 1] = bitfield_extract(px, masks[1]);
                        out[di + 2] = bitfield_extract(px, masks[2]);
                        if out_fmt == PixelFormat::Rgba8 {
                            out[di + 3] = bitfield_extract(px, masks[3]);
                        }
                    } else {
                        out[di] = row[p + 2];
                        out[di + 1] = row[p + 1];
                        out[di + 2] = row[p];
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    Ok(RawDecodedImage {
        width,
        height,
        format: out_fmt,
        source: SourceFormat::Bmp,
        data: out,
        frames: None,
        delays_ms: Vec::new(),
        loop_count: 0,
    })
}

pub fn decode_bmp_argb(data: &[u8]) -> Result<(i32, i32, Vec<u32>), String> {
    let raw = decode_bmp(data, None, None)?;
    let n = (raw.width as usize) * (raw.height as usize);
    let mut out = Vec::with_capacity(n);
    match raw.format {
        PixelFormat::Rgb8 => {
            for p in raw.data.chunks_exact(3) {
                out.push(0xFF00_0000 | ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | (p[2] as u32));
            }
        }
        PixelFormat::Rgba8 => {
            for p in raw.data.chunks_exact(4) {
                out.push(
                    ((p[3] as u32) << 24)
                        | ((p[0] as u32) << 16)
                        | ((p[1] as u32) << 8)
                        | (p[2] as u32),
                );
            }
        }
        _ => unreachable!(),
    }
    Ok((raw.width as i32, raw.height as i32, out))
}
