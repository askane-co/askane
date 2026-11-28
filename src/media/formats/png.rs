use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::formats::RawDecodedImage;
use crate::media::image::{PixelFormat, SourceFormat};
use crate::media::{
    check_deadline, err_corrupt, err_limit, err_unsupported, temp_budget_guard, validate_dims,
    MAX_DIM, MAX_MEDIA_BYTES,
};

const MAX_CHUNKS: usize = 100_000;
const MAX_IDAT_CHUNKS: usize = 65_536;
const MAX_INFLATE_TRANSIENT: usize = 128 * 1024 * 1024;

#[inline]
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i32;
    let b = b as i32;
    let c = c as i32;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

fn unfilter_pass_scanlines(
    raw: &[u8],
    row_bytes: usize,
    height: usize,
    bpp: usize,
) -> Result<Vec<u8>, String> {
    let stride = 1usize
        .checked_add(row_bytes)
        .ok_or_else(|| err_limit("PNG pass row overflow"))?;
    let expected = stride
        .checked_mul(height)
        .ok_or_else(|| err_limit("PNG pass buffer overflow"))?;
    if raw.len() != expected {
        return Err(err_corrupt(format!(
            "PNG decompressed pass size mismatch: expected {}, got {}",
            expected,
            raw.len()
        )));
    }
    let out_bytes = row_bytes
        .checked_mul(height)
        .ok_or_else(|| err_limit("PNG unfiltered buffer overflow"))?;
    let mut out = vec![0u8; out_bytes];
    for y in 0..height {
        let src_row = &raw[y * stride..(y + 1) * stride];
        let filter = src_row[0];
        let src = &src_row[1..];
        let dst_offset = y * row_bytes;
        for x in 0..row_bytes {
            let left = if x >= bpp {
                out[dst_offset + x - bpp]
            } else {
                0
            };
            let up = if y > 0 {
                out[(y - 1) * row_bytes + x]
            } else {
                0
            };
            let up_left = if y > 0 && x >= bpp {
                out[(y - 1) * row_bytes + x - bpp]
            } else {
                0
            };
            out[dst_offset + x] = match filter {
                0 => src[x],
                1 => src[x].wrapping_add(left),
                2 => src[x].wrapping_add(up),
                3 => src[x].wrapping_add((((left as u16) + (up as u16)) / 2) as u8),
                4 => src[x].wrapping_add(paeth(left, up, up_left)),
                other => {
                    return Err(err_unsupported(format!(
                        "Unsupported PNG scanline filter {}",
                        other
                    )));
                }
            };
        }
    }
    Ok(out)
}

#[inline]
fn unpack_sub_byte(row: &[u8], x: usize, bit_depth: u8) -> u8 {
    let bits = bit_depth as usize;
    let per_byte = 8 / bits;
    let byte = row[x / per_byte];
    let shift = 8 - bits * (1 + (x % per_byte));
    let mask = (1u8 << bits) - 1;
    (byte >> shift) & mask
}

#[inline]
fn sample_to_u8(row: &[u8], x: usize, channel: usize, channels: usize, bit_depth: u8) -> u8 {
    match bit_depth {
        16 => {
            let p = (x * channels + channel) * 2;
            let v = u16::from_be_bytes([row[p], row[p + 1]]);
            ((v as u32 + 128) >> 8).min(255) as u8
        }
        8 => row[x * channels + channel],
        d => {
            let s = unpack_sub_byte(row, x * channels + channel, d);
            let max_v = ((1u16 << d) - 1) as u16;
            ((s as u16 * 255 + (max_v / 2)) / max_v) as u8
        }
    }
}

#[inline]
fn sample_to_u16(row: &[u8], x: usize, channel: usize, channels: usize, bit_depth: u8) -> u16 {
    match bit_depth {
        16 => {
            let p = (x * channels + channel) * 2;
            u16::from_be_bytes([row[p], row[p + 1]])
        }
        8 => (row[x * channels + channel] as u16) * 257,
        d => {
            let s = unpack_sub_byte(row, x * channels + channel, d) as u32;
            let max_v = ((1u32 << d) - 1).max(1);
            ((s * 65535 + (max_v / 2)) / max_v) as u16
        }
    }
}

struct PreparedPng {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    channels: usize,
    bpp_filter: usize,
    palette: Vec<[u8; 3]>,
    trns: Vec<u8>,
    passes: Vec<(usize, usize, usize, usize, usize, usize)>,
    raw: Vec<u8>,
}

fn prepare_png<'a>(
    data: &[u8],
    strict_crc: bool,
    budget: Option<&'a Arc<BudgetCore>>,
    deadline_check: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<(PreparedPng, Option<crate::media::TempCharge<'a>>), String> {
    const SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if data.len() < 8 || &data[..8] != SIG {
        return Err(err_corrupt("Invalid PNG signature"));
    }

    let mut pos = 8usize;
    let mut chunk_count = 0usize;
    let mut idat_count = 0usize;
    let mut saw_ihdr = false;
    let mut saw_iend = false;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut bit_depth = 0u8;
    let mut color_type = 0u8;
    let mut interlace = 0u8;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat = Vec::new();

    while pos + 12 <= data.len() {
        chunk_count += 1;
        if chunk_count > MAX_CHUNKS {
            return Err(err_limit("PNG chunk count exceeds 100000 limit"));
        }
        if (chunk_count & 255) == 0 {
            check_deadline(deadline_check)?;
        }

        let len =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let ctype = &data[pos + 4..pos + 8];
        let body_start = pos + 8;
        let body_end = body_start
            .checked_add(len)
            .ok_or_else(|| err_corrupt("PNG chunk length overflow"))?;
        let crc_end = body_end
            .checked_add(4)
            .ok_or_else(|| err_corrupt("PNG chunk CRC overflow"))?;
        if crc_end > data.len() {
            return Err(err_corrupt("Truncated PNG chunk"));
        }
        let body = &data[body_start..body_end];
        if strict_crc {
            let expected_crc = u32::from_be_bytes([
                data[body_end],
                data[body_end + 1],
                data[body_end + 2],
                data[body_end + 3],
            ]);
            let actual_crc = crate::modules::compress::crc32_bytes(&data[pos + 4..body_end]);
            if expected_crc != actual_crc {
                return Err(err_corrupt("Corrupt PNG chunk CRC"));
            }
        }

        match ctype {
            b"IHDR" => {
                if saw_ihdr || body.len() != 13 {
                    return Err(err_corrupt("Invalid PNG IHDR chunk"));
                }
                saw_ihdr = true;
                width = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                height = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
                bit_depth = body[8];
                color_type = body[9];
                if body[10] != 0 || body[11] != 0 {
                    return Err(err_unsupported("Unsupported PNG compression or filter method"));
                }
                interlace = body[12];
                if interlace != 0 && interlace != 1 {
                    return Err(err_unsupported(format!(
                        "Unsupported PNG interlace method {}",
                        interlace
                    )));
                }
                let _ = validate_dims(width, height, MAX_DIM)?;
            }
            b"PLTE" => {
                if !saw_ihdr || body.is_empty() || body.len() % 3 != 0 || body.len() > 256 * 3 {
                    return Err(err_corrupt("Invalid PNG PLTE chunk"));
                }
                palette.clear();
                for c in body.chunks_exact(3) {
                    palette.push([c[0], c[1], c[2]]);
                }
            }
            b"tRNS" => {
                if !saw_ihdr || body.len() > 1024 {
                    return Err(err_corrupt("Invalid PNG tRNS chunk"));
                }
                trns = body.to_vec();
            }
            b"IDAT" => {
                if !saw_ihdr {
                    return Err(err_corrupt("PNG IDAT before IHDR"));
                }
                idat_count += 1;
                if idat_count > MAX_IDAT_CHUNKS {
                    return Err(err_limit("PNG IDAT chunk count exceeds 65536 limit"));
                }
                if idat.len().saturating_add(body.len()) > MAX_MEDIA_BYTES {
                    return Err(err_limit("PNG compressed IDAT exceeds 64 MiB limit"));
                }
                idat.extend_from_slice(body);
            }
            b"IEND" => {
                saw_iend = true;
                break;
            }
            other => {
                if (other[0] & 0x20) == 0 {
                    return Err(err_unsupported(format!(
                        "Unsupported critical PNG chunk {:?}",
                        String::from_utf8_lossy(other)
                    )));
                }
            }
        }
        pos = crc_end;
    }

    if !saw_ihdr || !saw_iend || idat.is_empty() {
        return Err(err_corrupt("Incomplete PNG stream"));
    }
    if color_type == 3 && palette.is_empty() {
        return Err(err_corrupt("PNG indexed image missing PLTE chunk"));
    }

    let channels = match color_type {
        0 => 1usize,
        2 => 3usize,
        3 => 1usize,
        4 => 2usize,
        6 => 4usize,
        other => {
            return Err(err_unsupported(format!(
                "Unsupported PNG color type {}",
                other
            )));
        }
    };
    let valid_depth = match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(bit_depth, 8 | 16),
        _ => false,
    };
    if !valid_depth {
        return Err(err_unsupported(format!(
            "Unsupported PNG bit depth {} for color type {}",
            bit_depth, color_type
        )));
    }

    let w = width as usize;
    let h = height as usize;
    let bits_per_pixel = channels * (bit_depth as usize);
    let bpp_filter = bits_per_pixel.div_ceil(8).max(1);
    let max_out_bytes = w
        .checked_mul(h)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| err_limit("PNG output size overflow"))?;
    if max_out_bytes > MAX_MEDIA_BYTES {
        return Err(err_limit("Decoded PNG exceeds 64 MiB limit"));
    }

    const ADAM7: [(usize, usize, usize, usize); 7] = [
        (0, 8, 0, 8),
        (4, 8, 0, 8),
        (0, 4, 4, 8),
        (2, 4, 0, 4),
        (0, 2, 2, 4),
        (1, 2, 0, 2),
        (0, 1, 1, 2),
    ];
    let passes: Vec<(usize, usize, usize, usize, usize, usize)> = if interlace == 0 {
        let row_bytes = (w * bits_per_pixel).div_ceil(8);
        vec![(0, 1, 0, 1, w, row_bytes)]
    } else {
        let mut p = Vec::new();
        for &(x0, dx, y0, dy) in &ADAM7 {
            if w > x0 && h > y0 {
                let pw = (w - x0).div_ceil(dx);
                let row_bytes = (pw * bits_per_pixel).div_ceil(8);
                p.push((x0, dx, y0, dy, pw, row_bytes));
            }
        }
        p
    };

    let mut expected_raw = 0usize;
    for &(_, _, y0, dy, _, row_bytes) in &passes {
        let ph = (h - y0).div_ceil(dy);
        let pass_raw = (1usize + row_bytes)
            .checked_mul(ph)
            .ok_or_else(|| err_limit("PNG pass raw buffer overflow"))?;
        expected_raw = expected_raw
            .checked_add(pass_raw)
            .ok_or_else(|| err_limit("PNG total raw buffer overflow"))?;
    }
    let raw_cap = (max_out_bytes.saturating_mul(2).saturating_add(65_536)).min(MAX_INFLATE_TRANSIENT);
    if expected_raw > raw_cap {
        return Err(err_limit(format!(
            "PNG raw scanline stream {} bytes exceeds {} cap",
            expected_raw, raw_cap
        )));
    }

    let inflate_guard = temp_budget_guard(budget, expected_raw)?;
    let raw = crate::modules::compress::inflate_zlib(&idat, expected_raw, None)
        .map_err(|e| err_corrupt(format!("PNG inflate failed: {}", e)))?;
    if raw.len() != expected_raw {
        return Err(err_corrupt(format!(
            "PNG raw size mismatch: expected {}, got {}",
            expected_raw,
            raw.len()
        )));
    }

    Ok((
        PreparedPng {
            width,
            height,
            bit_depth,
            color_type,
            channels,
            bpp_filter,
            palette,
            trns,
            passes,
            raw,
        },
        inflate_guard,
    ))
}

pub fn decode_png(
    data: &[u8],
    strict_crc: bool,
    budget: Option<&Arc<BudgetCore>>,
    deadline_check: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<RawDecodedImage, String> {
    let (prep, _inflate_guard) = prepare_png(data, strict_crc, budget, deadline_check)?;
    let w = prep.width as usize;
    let h = prep.height as usize;
    let out_format = match prep.color_type {
        0 if prep.trns.is_empty() => PixelFormat::R8,
        2 if prep.trns.is_empty() => PixelFormat::Rgb8,
        _ => PixelFormat::Rgba8,
    };
    let decoded_bytes = w
        .checked_mul(h)
        .and_then(|px| px.checked_mul(out_format.bpp()))
        .ok_or_else(|| err_limit("PNG output size overflow"))?;
    if decoded_bytes > MAX_MEDIA_BYTES {
        return Err(err_limit("Decoded PNG exceeds 64 MiB limit"));
    }

    let trns_gray: Option<u8> = if prep.color_type == 0 && prep.trns.len() >= 2 {
        let v = u16::from_be_bytes([prep.trns[0], prep.trns[1]]);
        Some(if prep.bit_depth == 16 {
            ((v as u32 + 128) >> 8).min(255) as u8
        } else {
            let max_v = ((1u16 << prep.bit_depth) - 1).max(1);
            ((v.min(max_v) * 255 + (max_v / 2)) / max_v) as u8
        })
    } else {
        None
    };
    let trns_rgb: Option<[u8; 3]> = if prep.color_type == 2 && prep.trns.len() >= 6 {
        let cvt = |hi: u8, lo: u8| {
            let v = u16::from_be_bytes([hi, lo]);
            if prep.bit_depth == 16 {
                ((v as u32 + 128) >> 8).min(255) as u8
            } else {
                lo
            }
        };
        Some([
            cvt(prep.trns[0], prep.trns[1]),
            cvt(prep.trns[2], prep.trns[3]),
            cvt(prep.trns[4], prep.trns[5]),
        ])
    } else {
        None
    };

    let mut out = vec![0u8; decoded_bytes];
    let out_bpp = out_format.bpp();
    let mut raw_offset = 0usize;

    for &(x0, dx, y0, dy, pw, row_bytes) in &prep.passes {
        let ph = (h - y0).div_ceil(dy);
        let pass_len = (1 + row_bytes) * ph;
        let unfiltered = unfilter_pass_scanlines(
            &prep.raw[raw_offset..raw_offset + pass_len],
            row_bytes,
            ph,
            prep.bpp_filter,
        )?;
        raw_offset += pass_len;

        for py in 0..ph {
            if (py & 255) == 0 {
                check_deadline(deadline_check)?;
            }
            let row = &unfiltered[py * row_bytes..(py + 1) * row_bytes];
            let dst_y = y0 + py * dy;
            for px in 0..pw {
                let dst_x = x0 + px * dx;
                let di = (dst_y * w + dst_x) * out_bpp;
                match prep.color_type {
                    0 => {
                        let gval = sample_to_u8(row, px, 0, 1, prep.bit_depth);
                        if out_format == PixelFormat::R8 {
                            out[di] = gval;
                        } else {
                            let a = if trns_gray == Some(gval) { 0 } else { 255 };
                            out[di] = gval;
                            out[di + 1] = gval;
                            out[di + 2] = gval;
                            out[di + 3] = a;
                        }
                    }
                    2 => {
                        let r = sample_to_u8(row, px, 0, 3, prep.bit_depth);
                        let g = sample_to_u8(row, px, 1, 3, prep.bit_depth);
                        let b = sample_to_u8(row, px, 2, 3, prep.bit_depth);
                        out[di] = r;
                        out[di + 1] = g;
                        out[di + 2] = b;
                        if out_format == PixelFormat::Rgba8 {
                            out[di + 3] = if trns_rgb == Some([r, g, b]) { 0 } else { 255 };
                        }
                    }
                    3 => {
                        let idx = if prep.bit_depth == 8 {
                            row[px] as usize
                        } else {
                            unpack_sub_byte(row, px, prep.bit_depth) as usize
                        };
                        let c = *prep
                            .palette
                            .get(idx)
                            .ok_or_else(|| err_corrupt("PNG palette index out of bounds"))?;
                        let a = *prep.trns.get(idx).unwrap_or(&255);
                        out[di] = c[0];
                        out[di + 1] = c[1];
                        out[di + 2] = c[2];
                        out[di + 3] = a;
                    }
                    4 => {
                        let gval = sample_to_u8(row, px, 0, 2, prep.bit_depth);
                        let a = sample_to_u8(row, px, 1, 2, prep.bit_depth);
                        out[di] = gval;
                        out[di + 1] = gval;
                        out[di + 2] = gval;
                        out[di + 3] = a;
                    }
                    6 => {
                        out[di] = sample_to_u8(row, px, 0, 4, prep.bit_depth);
                        out[di + 1] = sample_to_u8(row, px, 1, 4, prep.bit_depth);
                        out[di + 2] = sample_to_u8(row, px, 2, 4, prep.bit_depth);
                        out[di + 3] = sample_to_u8(row, px, 3, 4, prep.bit_depth);
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    Ok(RawDecodedImage {
        width: prep.width,
        height: prep.height,
        format: out_format,
        source: SourceFormat::Png,
        data: out,
        frames: None,
        delays_ms: Vec::new(),
        loop_count: 0,
    })
}

/// Decodes a PNG image to exact 16-bit grayscale height samples `[0..=65535]`.
pub fn decode_png_gray_u16(
    data: &[u8],
    strict_crc: bool,
    budget: Option<&Arc<BudgetCore>>,
    deadline_check: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<(u32, u32, Vec<u16>), String> {
    let (prep, _inflate_guard) = prepare_png(data, strict_crc, budget, deadline_check)?;
    let w = prep.width as usize;
    let h = prep.height as usize;
    let px_count = w
        .checked_mul(h)
        .ok_or_else(|| err_limit("PNG u16 output size overflow"))?;
    let mut out = vec![0u16; px_count];
    let mut raw_offset = 0usize;

    for &(x0, dx, y0, dy, pw, row_bytes) in &prep.passes {
        let ph = (h - y0).div_ceil(dy);
        let pass_len = (1 + row_bytes) * ph;
        let unfiltered = unfilter_pass_scanlines(
            &prep.raw[raw_offset..raw_offset + pass_len],
            row_bytes,
            ph,
            prep.bpp_filter,
        )?;
        raw_offset += pass_len;

        for py in 0..ph {
            if (py & 255) == 0 {
                check_deadline(deadline_check)?;
            }
            let row = &unfiltered[py * row_bytes..(py + 1) * row_bytes];
            let dst_y = y0 + py * dy;
            for px in 0..pw {
                let dst_x = x0 + px * dx;
                let di = dst_y * w + dst_x;
                out[di] = match prep.color_type {
                    0 | 4 => sample_to_u16(row, px, 0, prep.channels, prep.bit_depth),
                    2 | 6 => {
                        let r = sample_to_u16(row, px, 0, prep.channels, prep.bit_depth) as u32;
                        let g = sample_to_u16(row, px, 1, prep.channels, prep.bit_depth) as u32;
                        let b = sample_to_u16(row, px, 2, prep.channels, prep.bit_depth) as u32;
                        ((r * 77 + g * 150 + b * 29) >> 8) as u16
                    }
                    3 => {
                        let idx = if prep.bit_depth == 8 {
                            row[px] as usize
                        } else {
                            unpack_sub_byte(row, px, prep.bit_depth) as usize
                        };
                        let c = *prep
                            .palette
                            .get(idx)
                            .ok_or_else(|| err_corrupt("PNG palette index out of bounds"))?;
                        let luma = ((c[0] as u32) * 77 + (c[1] as u32) * 150 + (c[2] as u32) * 29) >> 8;
                        (luma * 257) as u16
                    }
                    _ => unreachable!(),
                };
            }
        }
    }
    Ok((prep.width, prep.height, out))
}

pub fn decode_png_argb(data: &[u8]) -> Result<(i32, i32, Vec<u32>), String> {
    let raw = decode_png(data, true, None, None)?;
    let px = (raw.width as usize) * (raw.height as usize);
    let mut out = Vec::with_capacity(px);
    match raw.format {
        PixelFormat::R8 => {
            for &g in &raw.data {
                let g32 = g as u32;
                out.push(0xFF00_0000 | (g32 << 16) | (g32 << 8) | g32);
            }
        }
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
        PixelFormat::Rg8 => {
            for p in raw.data.chunks_exact(2) {
                let g32 = p[0] as u32;
                out.push(((p[1] as u32) << 24) | (g32 << 16) | (g32 << 8) | g32);
            }
        }
    }
    Ok((raw.width as i32, raw.height as i32, out))
}
